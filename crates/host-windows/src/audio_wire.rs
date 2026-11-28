use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const OWNER_GRACE: Duration = Duration::from_secs(5);
const PREPARE_TIMEOUT: Duration = Duration::from_secs(18);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioOutput {
    Controller,
    Host,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectPolicy {
    Sync,
    Mute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioSample {
    pub output: AudioOutput,
    pub disconnect: DisconnectPolicy,
    pub volume: u8,
    pub muted: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct AudioSession {
    pub owner: [u8; 32],
    pub id: u64,
    pub last_seen: Instant,
    pub ended: bool,
    pub handoff: bool,
    pub sample: AudioSample,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Prepare,
    Cancel,
    Live,
    End,
    Handoff,
}

pub(super) struct Report {
    id: u64,
    seq: u64,
    pub(super) phase: Phase,
    sample: AudioSample,
}

impl Report {
    pub(super) fn parse(line: &[u8]) -> Option<Self> {
        let line = std::str::from_utf8(line).ok()?.strip_suffix('\n')?;
        let mut fields = line.strip_prefix("FLASHLINK_STREAM/2 ")?.split(' ');
        let number = |s: &str| {
            if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
                s.parse::<u64>().ok()
            } else {
                None
            }
        };
        let id = number(fields.next()?)?;
        let seq = number(fields.next()?)?;
        let phase = match fields.next()? {
            "prepare" => Phase::Prepare,
            "cancel" => Phase::Cancel,
            "live" => Phase::Live,
            "end" => Phase::End,
            "handoff" => Phase::Handoff,
            _ => return None,
        };
        let output = match fields.next()? {
            "controller" => AudioOutput::Controller,
            "host" => AudioOutput::Host,
            "both" => AudioOutput::Both,
            _ => return None,
        };
        let disconnect = match fields.next()? {
            "sync" => DisconnectPolicy::Sync,
            "mute" => DisconnectPolicy::Mute,
            _ => return None,
        };
        let volume = number(fields.next()?)?;
        let muted = match fields.next()? {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        if id == 0
            || seq == 0
            || volume > 100
            || fields.next().is_some()
            || (phase == Phase::Prepare && output != AudioOutput::Controller)
        {
            return None;
        }
        Some(Self {
            id,
            seq,
            phase,
            sample: AudioSample {
                output,
                disconnect,
                volume: volume as u8,
                muted,
            },
        })
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum PreparedMedia {
    #[default]
    Waiting,
    AwaitIdle,
    Idle,
    Started,
    Stopped,
}

struct Preparation {
    session: AudioSession,
    seq: u64,
    media: PreparedMedia,
    reply: Option<Sender<bool>>,
}

pub(crate) enum PlaybackTarget {
    Prepare,
    Session(Option<AudioSession>),
}

struct LegacyBeats {
    owner: [u8; 32],
    first: Instant,
    last: Instant,
    active: bool,
}

#[derive(Default)]
pub(super) struct Streams {
    pub(super) heartbeat: Option<([u8; 32], Instant)>,
    pub(super) audio: Option<AudioSession>,
    sequence: u64,
    preparing: Option<Preparation>,
    legacy_beat: Option<LegacyBeats>,
    // Retain the highest session ID beyond any in-flight TLS request.
    recent: HashMap<[u8; 32], (u64, Instant)>,
}

impl Streams {
    pub(super) fn legacy(&mut self, owner: [u8; 32], now: Instant) {
        // A v1 heartbeat carries no audio policy and cannot finalize a v2 session.
        if self
            .preparing
            .as_ref()
            .is_some_and(|p| now.duration_since(p.session.last_seen) < PREPARE_TIMEOUT)
            || self
                .audio
                .is_some_and(|s| !s.ended && now.duration_since(s.last_seen) < OWNER_GRACE)
        {
            return;
        }
        self.heartbeat = Some((owner, now));
        if let Some(l) = &mut self.legacy_beat {
            if l.owner == owner && now.duration_since(l.last) < OWNER_GRACE {
                l.last = now;
                return;
            }
        }
        self.legacy_beat = Some(LegacyBeats {
            owner,
            first: now,
            last: now,
            active: false,
        });
    }

    fn legacy_owns(&self, other: [u8; 32], now: Instant) -> bool {
        self.legacy_beat.as_ref().is_some_and(|l| {
            l.active && l.owner != other && now.duration_since(l.last) < OWNER_GRACE
        })
    }

    fn new_session_allowed(&self, owner: [u8; 32], report: &Report, now: Instant) -> bool {
        if self
            .recent
            .get(&owner)
            .is_some_and(|(id, _)| report.id <= *id)
        {
            return false;
        }
        !self.audio.is_some_and(|s| {
            s.owner != owner && !s.ended && now.duration_since(s.last_seen) < OWNER_GRACE
        }) && !self.legacy_owns(owner, now)
    }

    // Keep the previous session intact until media connects. Cancel or expiry can
    // then undo just our temporary mute and continue its original disconnect policy.
    pub(super) fn prepare(
        &mut self,
        owner: [u8; 32],
        report: Report,
        now: Instant,
    ) -> Option<Receiver<bool>> {
        let previous = self.preparing.as_ref();
        let same = previous.is_some_and(|p| (p.session.owner, p.session.id) == (owner, report.id));
        let allowed = match previous {
            Some(p) if same => report.seq >= p.seq,
            Some(p) if p.session.owner != owner => false,
            _ => self.new_session_allowed(owner, &report, now),
        };
        if !allowed {
            return None;
        }
        let (reply, result) = mpsc::channel();
        self.preparing = Some(Preparation {
            session: AudioSession {
                owner,
                id: report.id,
                last_seen: now,
                ended: false,
                handoff: false,
                sample: report.sample,
            },
            seq: report.seq,
            media: previous
                .filter(|_| same)
                .map_or(PreparedMedia::Waiting, |p| p.media),
            reply: Some(reply),
        });
        self.recent.insert(owner, (report.id, now));
        Some(result)
    }

    pub(super) fn apply(
        &mut self,
        now: Instant,
        stopping: bool,
        media_active: impl Fn() -> bool,
        apply: impl FnOnce(PlaybackTarget) -> io::Result<()>,
    ) -> io::Result<()> {
        self.recent.retain(|owner, (_, seen)| {
            now.duration_since(*seen) < Duration::from_secs(90)
                || self.audio.is_some_and(|s| s.owner == *owner && !s.ended)
                || self
                    .preparing
                    .as_ref()
                    .is_some_and(|p| p.session.owner == *owner)
        });
        if let Some(l) = &mut self.legacy_beat {
            // Old clients send every 1.8 s. A delayed packet, or its immediate duplicate,
            // cannot assume control over the outgoing stream's audio.
            if !l.active
                && now.duration_since(l.last) < OWNER_GRACE
                && l.last.duration_since(l.first) >= Duration::from_secs(1)
                && media_active()
            {
                l.active = true;
                if self
                    .audio
                    .is_some_and(|s| s.owner != l.owner && !s.ended && s.last_seen <= l.first)
                {
                    let retired = self.audio.take().unwrap();
                    if let Some((_, seen)) = self.recent.get_mut(&retired.owner) {
                        *seen = now;
                    }
                }
                if self
                    .preparing
                    .as_ref()
                    .is_some_and(|p| p.session.owner != l.owner && p.session.last_seen <= l.first)
                {
                    self.preparing = None;
                }
            }
        }
        if let Some(p) = &mut self.preparing {
            let active = media_active();
            let expired = now.duration_since(p.session.last_seen) >= PREPARE_TIMEOUT;
            if p.media == PreparedMedia::AwaitIdle && !active {
                p.media = PreparedMedia::Idle;
            }
            if p.media == PreparedMedia::Idle && active && !expired {
                p.media = PreparedMedia::Started;
            }
            if p.media == PreparedMedia::Started && !active {
                p.media = PreparedMedia::Stopped;
            }
            // A confirmed idle -> active edge ties this preparation to a media session.
            // Hold its mute while that session is running; an explicit cancel still rolls back.
            if stopping || (expired && p.media != PreparedMedia::Started) {
                if matches!(p.media, PreparedMedia::Started | PreparedMedia::Stopped) {
                    self.sequence = p.seq;
                    p.session.ended = p.media == PreparedMedia::Stopped;
                    self.audio = Some(p.session);
                }
                if let Some((_, seen)) = self.recent.get_mut(&p.session.owner) {
                    *seen = now;
                }
                self.preparing = None;
            } else {
                let result = apply(PlaybackTarget::Prepare);
                if let Some(reply) = p.reply.take() {
                    if reply.send(result.is_ok()).is_ok()
                        && result.is_ok()
                        && p.media == PreparedMedia::Waiting
                    {
                        p.media = if active {
                            PreparedMedia::AwaitIdle
                        } else {
                            PreparedMedia::Idle
                        };
                    }
                }
                return result;
            }
        }
        apply(PlaybackTarget::Session(self.audio))
    }

    pub(super) fn update(&mut self, owner: [u8; 32], report: Report, now: Instant) -> bool {
        let prepared = self
            .preparing
            .as_ref()
            .is_some_and(|p| (p.session.owner, p.session.id) == (owner, report.id));
        if prepared && self.preparing.as_ref().is_some_and(|p| report.seq <= p.seq) {
            return false;
        }
        if report.phase == Phase::Cancel {
            if prepared {
                self.preparing = None;
            }
            // The cancel can arrive before its prepare on a separate TLS connection.
            if self
                .recent
                .get(&owner)
                .is_none_or(|(id, _)| report.id >= *id)
            {
                self.recent.insert(owner, (report.id, now));
            }
            return true;
        }
        if self.legacy_owns(owner, now) {
            return false;
        }
        let same = self
            .audio
            .is_some_and(|s| s.owner == owner && s.id == report.id);
        if same {
            let current = self.audio.unwrap();
            if report.seq == self.sequence {
                return report.sample == current.sample
                    && current.ended == (report.phase == Phase::End)
                    && current.handoff == (report.phase == Phase::Handoff);
            }
            if report.seq < self.sequence || current.ended || current.handoff {
                return false;
            }
        } else {
            if !prepared
                && (!self.new_session_allowed(owner, &report, now)
                    || self
                        .preparing
                        .as_ref()
                        .is_some_and(|p| p.session.owner != owner))
            {
                return false;
            }
            // A final packet from an unobserved, very short session is useful only when
            // there is no other live owner. It must never supersede a different session.
            if !prepared
                && report.phase != Phase::Live
                && self.audio.is_some_and(|s| s.owner != owner && !s.ended)
            {
                return false;
            }
            self.preparing = None;
        }
        if report.phase == Phase::Live {
            self.heartbeat = Some((owner, now));
        }
        self.sequence = report.seq;
        self.legacy_beat = None;
        self.recent
            .entry(owner)
            .and_modify(|latest| {
                if report.id >= latest.0 {
                    *latest = (report.id, now);
                }
            })
            .or_insert((report.id, now));
        self.audio = Some(AudioSession {
            owner,
            id: report.id,
            last_seen: now,
            ended: report.phase == Phase::End,
            handoff: report.phase == Phase::Handoff,
            sample: report.sample,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(fields: &str) -> Option<Report> {
        Report::parse(format!("FLASHLINK_STREAM/2 {fields}\n").as_bytes())
    }

    #[test]
    fn parses_absolute_volume_and_mute_independently() {
        let sample = report("7 12 live both sync 67 1").unwrap().sample;
        assert_eq!(sample.volume, 67);
        assert!(sample.muted);
        assert_eq!(sample.output, AudioOutput::Both);
        assert_eq!(sample.disconnect, DisconnectPolicy::Sync);
        assert_eq!(report("7 13 end host mute 0 0").unwrap().sample.volume, 0);
        for invalid in [
            "7 1 live pc sync 80 0",
            "7 0 end host mute 80 0",
            "7 2 live both sync 101 0",
            "7 1 live host sync -1 0",
            "7 2 live both sync 100 true",
            "7 2 live both sync 0 0 extra",
            "7 2 live both something 10 0",
            "7 2 prepare both sync 35 0",
        ] {
            assert!(report(invalid).is_none());
        }
    }

    #[test]
    fn retired_sessions_and_old_disconnects_cannot_replace_the_current_device() {
        let mut sessions = Streams::default();
        let now = Instant::now();
        let a = [1; 32];
        let b = [2; 32];
        assert!(sessions.update(a, report("5 1 live both sync 45 0").unwrap(), now));
        assert!(!sessions.update(b, report("2 1 live host mute 10 0").unwrap(), now));
        assert!(sessions.update(a, report("5 3 handoff both sync 49 0").unwrap(), now));
        assert!(sessions.audio.unwrap().handoff);
        assert!(sessions.update(a, report("6 1 live both sync 55 0").unwrap(), now));
        assert!(!sessions.update(a, report("5 4 end both mute 0 1").unwrap(), now));
        assert!(sessions.update(a, report("6 3 end both sync 42 0").unwrap(), now));
        assert!(!sessions.update(a, report("6 2 live both sync 55 0").unwrap(), now));
        sessions.legacy(a, now);
        assert!(sessions.audio.unwrap().ended);
        assert_eq!(sessions.audio.unwrap().sample.volume, 42);
        assert!(sessions.update(b, report("2 2 live controller mute 12 0").unwrap(), now));
        sessions.legacy(b, now);
        assert_eq!(sessions.audio.unwrap().owner, b);
        sessions.legacy(a, now);
        assert_eq!(sessions.audio.unwrap().owner, b);
        assert_eq!(sessions.heartbeat.unwrap().0, b);
        let now = now + OWNER_GRACE;
        sessions.legacy(a, now);
        assert_eq!(sessions.heartbeat.unwrap().0, a);
        assert_eq!(
            sessions.audio.unwrap().sample.disconnect,
            DisconnectPolicy::Mute
        );
        assert_eq!(sessions.audio.unwrap().last_seen, now - OWNER_GRACE);
        assert!(sessions.update(b, report("2 3 end controller mute 12 0").unwrap(), now));
    }

    #[test]
    fn prepare_waits_for_the_mixer_and_only_a_newer_report_can_start_media() {
        let mut streams = Streams::default();
        let now = Instant::now();
        let owner = [7; 32];
        let prepare = || report("2 1 prepare controller sync 65 0").unwrap();
        let ack = streams.prepare(owner, prepare(), now).unwrap();
        assert!(matches!(ack.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let failure = streams.apply(
            now,
            false,
            || false,
            |_| Err(io::Error::other("no default device")),
        );
        assert!(failure.is_err());
        assert!(!ack.recv().unwrap());
        let ack = streams.prepare(owner, prepare(), now).unwrap();
        streams
            .apply(
                now,
                false,
                || false,
                |t| {
                    assert!(matches!(t, PlaybackTarget::Prepare));
                    Ok(())
                },
            )
            .unwrap();
        assert!(ack.recv().unwrap());
        assert!(streams.heartbeat.is_none());
        assert!(streams.prepare([8; 32], prepare(), now).is_none());
        assert!(!streams.update(owner, report("2 1 live controller sync 65 0").unwrap(), now));
        assert!(streams.update(owner, report("2 2 live controller sync 65 0").unwrap(), now));
        assert!(streams.prepare(owner, prepare(), now).is_none());
        let end = || report("2 3 end controller mute 65 0").unwrap();
        assert!(streams.update(owner, end(), now));
        assert!(streams.update(owner, end(), now + OWNER_GRACE));
        assert_eq!(streams.audio.unwrap().last_seen, now);
    }

    #[test]
    fn cancelled_or_expired_prepare_restores_previous_session_and_rejects_late_packets() {
        let now = Instant::now();
        let owner = [3; 32];
        for cancel in [false, true] {
            let mut streams = Streams::default();
            assert!(streams.update(owner, report("4 1 end host sync 57 0").unwrap(), now));
            let prepare = || report("5 1 prepare controller mute 13 0").unwrap();
            let _ack = streams.prepare(owner, prepare(), now).unwrap();
            let next = if cancel {
                assert!(streams.update(
                    owner,
                    report("5 2 cancel controller mute 13 0").unwrap(),
                    now
                ));
                now
            } else {
                now + PREPARE_TIMEOUT
            };
            streams
                .apply(
                    next,
                    false,
                    || false,
                    |target| {
                        let PlaybackTarget::Session(Some(s)) = target else {
                            panic!("expected previous session")
                        };
                        assert!(s.ended);
                        assert_eq!((s.id, s.sample.volume), (4, 57));
                        Ok(())
                    },
                )
                .unwrap();
            assert!(streams.prepare(owner, prepare(), next).is_none());
            assert!(!streams.update(
                owner,
                report("5 3 live controller mute 13 0").unwrap(),
                next
            ));
        }
    }

    fn tick(streams: &mut Streams, now: Instant, media: bool) -> Option<AudioSession> {
        let mut selected = None;
        streams
            .apply(
                now,
                false,
                || media,
                |target| {
                    if let PlaybackTarget::Session(session) = target {
                        selected = session;
                    }
                    Ok(())
                },
            )
            .unwrap();
        selected
    }

    #[test]
    fn late_first_live_can_resume_the_prepared_stream_after_control_loss() {
        let now = Instant::now();
        let owner = [1; 32];
        for terminal in ["live", "cancel", "media stopped"] {
            let mut s = Streams::default();
            let ack = s
                .prepare(
                    owner,
                    report("6 1 prepare controller mute 19 0").unwrap(),
                    now,
                )
                .unwrap();
            tick(&mut s, now, false);
            assert!(ack.recv().unwrap());
            // Observe the new media context after mute was acknowledged.
            tick(&mut s, now + Duration::from_millis(250), true);
            let late = now + Duration::from_secs(100);
            tick(&mut s, late, true);
            assert!(s.preparing.is_some());
            match terminal {
                "live" => {
                    assert!(s.update(
                        owner,
                        report("6 2 live controller sync 35 0").unwrap(),
                        late
                    ));
                    assert_eq!(tick(&mut s, late, true).unwrap().sample.volume, 35);
                }
                "cancel" => {
                    assert!(s.update(
                        owner,
                        report("6 2 cancel controller mute 19 0").unwrap(),
                        late
                    ));
                    assert!(tick(&mut s, late, true).is_none());
                    assert!(s.preparing.is_none());
                    assert!(!s.update(
                        owner,
                        report("6 3 live controller sync 35 0").unwrap(),
                        late
                    ));
                }
                _ => {
                    let session = tick(&mut s, late, false).unwrap();
                    assert_eq!(
                        (session.id, session.sample.disconnect, session.last_seen),
                        (6, DisconnectPolicy::Mute, now)
                    );
                }
            }
        }
    }

    #[test]
    fn short_media_session_without_live_applies_its_choice_after_grace() {
        let now = Instant::now();
        let owner = [1; 32];
        let mut s = Streams::default();
        assert!(s.update(owner, report("4 2 end host sync 70 0").unwrap(), now));
        let ack = s
            .prepare(
                owner,
                report("5 1 prepare controller mute 19 0").unwrap(),
                now,
            )
            .unwrap();
        tick(&mut s, now, false);
        assert!(ack.recv().unwrap());
        tick(&mut s, now + Duration::from_millis(300), true);
        tick(&mut s, now + Duration::from_secs(4), false);
        assert!(s.preparing.is_some());
        // Another transport may already be running by the time this grace period expires.
        let session = tick(&mut s, now + PREPARE_TIMEOUT, true).unwrap();
        assert!(session.ended);
        assert_eq!(
            (session.id, session.sample.disconnect, session.last_seen),
            (5, DisconnectPolicy::Mute, now)
        );
        assert!(s.preparing.is_none());
    }

    #[test]
    fn already_active_or_failed_preparation_never_claims_another_media_context() {
        let now = Instant::now();
        for (already_active, acknowledged) in [(true, true), (false, false)] {
            let mut s = Streams::default();
            let ack = s
                .prepare(
                    [1; 32],
                    report("2 1 prepare controller mute 30 0").unwrap(),
                    now,
                )
                .unwrap();
            let result = s.apply(
                now,
                false,
                || already_active,
                |_| {
                    if acknowledged {
                        Ok(())
                    } else {
                        Err(io::Error::other("mute failed"))
                    }
                },
            );
            assert_eq!(result.is_ok(), acknowledged);
            assert_eq!(ack.recv().unwrap(), acknowledged);
            tick(&mut s, now + Duration::from_secs(1), true);
            assert!(tick(&mut s, now + PREPARE_TIMEOUT, true).is_none());
            assert!(s.preparing.is_none());
        }
    }

    #[test]
    fn legacy_needs_sustained_beats_and_media_to_retire_an_unfinished_owner() {
        let now = Instant::now();
        let old = [1; 32];
        let next = [2; 32];
        for explicitly_ended in [false, true] {
            let mut s = Streams::default();
            assert!(s.update(old, report("6 1 live controller mute 60 0").unwrap(), now));
            if explicitly_ended {
                assert!(s.update(old, report("6 2 end controller mute 60 0").unwrap(), now));
            }
            let first = now + OWNER_GRACE;
            s.legacy(next, first);
            s.legacy(next, first);
            assert_eq!(tick(&mut s, first, true).unwrap().id, 6);
            let second = first + Duration::from_millis(1800);
            s.legacy(next, second);
            assert_eq!(tick(&mut s, second, false).unwrap().id, 6);
            let last = tick(&mut s, second, true);
            assert_eq!(
                last.map(|session| session.ended),
                explicitly_ended.then_some(true)
            );
            assert!(!s.update(
                old,
                report("6 3 live controller mute 60 0").unwrap(),
                second
            ));
            // An old final report cannot reappear after legacy stops sending heartbeats.
            assert!(!s.update(
                old,
                report("6 4 end controller mute 60 0").unwrap(),
                second + OWNER_GRACE
            ));
        }
    }
}
