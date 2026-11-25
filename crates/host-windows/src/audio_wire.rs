use std::collections::HashMap;
use std::time::{Duration, Instant};

const OWNER_GRACE: Duration = Duration::from_secs(5);

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

#[derive(PartialEq, Eq)]
pub(super) enum Phase {
    Live,
    End,
    Handoff,
}

pub(super) struct Report {
    id: u64,
    seq: u64,
    phase: Phase,
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
        if id == 0 || seq == 0 || volume > 100 || fields.next().is_some() {
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

// Each paired installation issues strictly increasing session IDs. Remember displaced
// sessions beyond any in-flight TLS request so late reports cannot mute another user.
#[derive(Default)]
pub(super) struct Streams {
    pub(super) heartbeat: Option<([u8; 32], Instant)>,
    pub(super) audio: Option<AudioSession>,
    sequence: u64,
    recent: HashMap<[u8; 32], (u64, Instant)>,
}

impl Streams {
    pub(super) fn legacy(&mut self, owner: [u8; 32], now: Instant) {
        // A v1 beat has no session ID. Even the same certificate may have a delayed
        // beat from an earlier stream. Keep a recent final sample long enough for
        // the mixer to apply it on its next poll.
        if self
            .audio
            .is_some_and(|s| now.duration_since(s.last_seen) < OWNER_GRACE)
        {
            return;
        }
        self.heartbeat = Some((owner, now));
        self.audio = None;
    }

    pub(super) fn update(&mut self, owner: [u8; 32], report: Report, now: Instant) -> bool {
        self.recent
            .retain(|_, (_, seen)| now.duration_since(*seen) < Duration::from_secs(90));
        let same = self
            .audio
            .is_some_and(|s| s.owner == owner && s.id == report.id);
        if same {
            let current = self.audio.unwrap();
            if report.seq <= self.sequence || current.ended || current.handoff {
                return false;
            }
        } else {
            if self
                .recent
                .get(&owner)
                .is_some_and(|(id, _)| report.id <= *id)
            {
                return false;
            }
            // A final packet from an unobserved, very short session is useful only when
            // there is no other live owner. It must never supersede a different session.
            if self.audio.is_some_and(|s| {
                s.owner != owner
                    && !s.ended
                    && (report.phase != Phase::Live
                        || now.duration_since(s.last_seen) < OWNER_GRACE)
            }) {
                return false;
            }
            if self.audio.is_none()
                && self
                    .heartbeat
                    .is_some_and(|(id, seen)| id != owner && now.duration_since(seen) < OWNER_GRACE)
            {
                return false;
            }
        }
        if report.phase == Phase::Live {
            self.heartbeat = Some((owner, now));
        }
        self.sequence = report.seq;
        self.recent.insert(owner, (report.id, now));
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
        assert!(sessions.audio.is_none());
        assert!(!sessions.update(b, report("2 3 end controller mute 12 0").unwrap(), now));
    }
}
