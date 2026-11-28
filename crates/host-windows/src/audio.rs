use crate::lan::audio_wire::{AudioOutput, AudioSession, DisconnectPolicy};
use std::{
    io,
    time::{Duration, Instant},
};

const RECONNECT_GRACE: Duration = Duration::from_secs(18);
const QUALITY_HANDOFF_GRACE: Duration = Duration::from_secs(35);
const DEVICE_RETRY_INTERVAL: Duration = Duration::from_secs(2);

trait Mixer {
    type Endpoint: Eq;
    fn default_endpoint(&self) -> io::Result<Self::Endpoint>;
    fn muted(&self, endpoint: &Self::Endpoint) -> io::Result<bool>;
    fn set(&mut self, endpoint: &Self::Endpoint, volume: Option<u8>, muted: bool)
        -> io::Result<()>;
}

type SampleKey = ([u8; 32], u64, Instant);

struct PlaybackPolicy<M: Mixer> {
    mixer: M,
    // Remember only endpoints that were audible when this client requested silence.
    silenced: Vec<(M::Endpoint, Option<Instant>)>,
    finished: Option<SampleKey>,
    finish_retry: Option<(SampleKey, Instant)>,
}

impl<M: Mixer> PlaybackPolicy<M> {
    fn new(mixer: M) -> Self {
        Self {
            mixer,
            silenced: Vec::new(),
            finished: None,
            finish_retry: None,
        }
    }

    #[cfg(test)]
    fn update(
        &mut self,
        session: Option<AudioSession>,
        now: Instant,
        stopping: bool,
    ) -> io::Result<()> {
        self.update_with_media(session, now, stopping, || false)
    }

    fn update_with_media(
        &mut self,
        session: Option<AudioSession>,
        now: Instant,
        stopping: bool,
        media_active: impl FnOnce() -> bool,
    ) -> io::Result<()> {
        if stopping {
            // The host is exiting. Try pending device operations one last time.
            self.finish_retry = None;
            for (_, retry_at) in &mut self.silenced {
                *retry_at = None;
            }
        }
        let Some(session) = session else {
            self.finish_retry = None;
            return self.release_except(None, now);
        };
        let key = (session.owner, session.id, session.last_seen);
        let grace = if session.handoff {
            QUALITY_HANDOFF_GRACE
        } else {
            RECONNECT_GRACE
        };
        if self.finished == Some(key) {
            return self.release_except(None, now);
        }
        if stopping
            || session.ended
            || (now.saturating_duration_since(session.last_seen) >= grace
                && (session.handoff || !media_active()))
        {
            // A new owner or quality restart may have inherited our temporary mute. Only
            // the latest authenticated session gets to choose the final speaker state.
            if self
                .finish_retry
                .is_some_and(|(failed, retry_at)| failed == key && now < retry_at)
            {
                return Ok(());
            }
            if let Err(error) = self.finish(session) {
                self.finish_retry = Some((key, now + DEVICE_RETRY_INTERVAL));
                return Err(error);
            }
            self.finished = Some(key);
            self.finish_retry = None;
            return self.release_except(None, now);
        }

        self.finished = None;
        self.finish_retry = None;
        if session.sample.output != AudioOutput::Controller {
            return self.release_except(None, now);
        }
        self.silence_default(now)
    }

    fn silence_default(&mut self, now: Instant) -> io::Result<()> {
        let current = self.mixer.default_endpoint()?;
        let was_muted = self.mixer.muted(&current)?;
        if !was_muted {
            self.mixer.set(&current, None, true)?;
        }
        let cleanup = self.release_except(Some(&current), now);
        if !was_muted && !self.silenced.iter().any(|(id, _)| *id == current) {
            self.silenced.push((current, None));
        }
        cleanup
    }

    fn finish(&mut self, session: AudioSession) -> io::Result<()> {
        let current = self.mixer.default_endpoint()?;
        let sample = session.sample;
        let (level, muted) = match sample.disconnect {
            DisconnectPolicy::Mute => (None, true),
            DisconnectPolicy::Sync => (Some(sample.volume), sample.muted || sample.volume == 0),
        };
        self.mixer.set(&current, level, muted)?;
        // The end policy replaces our temporary mute on this endpoint in one operation.
        self.silenced.retain(|(id, _)| *id != current);
        Ok(())
    }

    fn release_except(&mut self, keep: Option<&M::Endpoint>, now: Instant) -> io::Result<()> {
        let mut failure = None;
        self.silenced.retain_mut(|(id, retry_at)| {
            if Some(&*id) == keep {
                *retry_at = None;
                return true;
            }
            if retry_at.is_some_and(|deadline| now < deadline) {
                return true;
            }
            match self.mixer.set(id, None, false) {
                Ok(()) => false,
                Err(error) => {
                    failure = Some(error);
                    *retry_at = Some(now + DEVICE_RETRY_INTERVAL);
                    true
                }
            }
        });
        failure.map_or(Ok(()), Err)
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::{lan::Nearby, service::Event};
    use std::{ffi::c_void, ptr, slice, sync::mpsc::Sender};

    extern "C" {
        fn flashlink_audio_default(id: *mut *mut u16, length: *mut usize) -> i32;
        fn flashlink_audio_get_mute(id: *const u16, muted: *mut i32) -> i32;
        fn flashlink_audio_set(id: *const u16, percent: i32, muted: i32) -> i32;
        fn flashlink_media_active(engine_pid: u32, audio_port: u16) -> i32;
        fn flashlink_media_idle(engine_pid: u32, audio_port: u16) -> i32;
    }

    fn checked(hr: i32) -> io::Result<()> {
        if hr >= 0 {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "Core Audio HRESULT 0x{:08X}",
                hr as u32
            )))
        }
    }

    struct CoreAudio;
    impl Mixer for CoreAudio {
        type Endpoint = Vec<u16>;
        fn default_endpoint(&self) -> io::Result<Self::Endpoint> {
            let mut id = ptr::null_mut();
            let mut len = 0;
            unsafe {
                checked(flashlink_audio_default(&mut id, &mut len))?;
                let owned = slice::from_raw_parts(id, len).to_vec();
                windows_sys::Win32::System::Com::CoTaskMemFree(id.cast::<c_void>());
                Ok(owned)
            }
        }
        fn muted(&self, endpoint: &Self::Endpoint) -> io::Result<bool> {
            let mut muted = 0;
            checked(unsafe { flashlink_audio_get_mute(endpoint.as_ptr(), &mut muted) })?;
            Ok(muted != 0)
        }
        fn set(
            &mut self,
            endpoint: &Self::Endpoint,
            volume: Option<u8>,
            muted: bool,
        ) -> io::Result<()> {
            checked(unsafe {
                flashlink_audio_set(
                    endpoint.as_ptr(),
                    volume.map_or(-1, i32::from),
                    i32::from(muted),
                )
            })
        }
    }

    pub struct Playback<'a> {
        nearby: &'a Nearby,
        engine_pid: u32,
        audio_port: u16,
        ui: &'a Sender<Event>,
        policy: PlaybackPolicy<CoreAudio>,
        error_reported: bool,
    }

    impl<'a> Playback<'a> {
        pub fn new(nearby: &'a Nearby, ui: &'a Sender<Event>, engine_pid: u32, base: u16) -> Self {
            Self {
                nearby,
                engine_pid,
                audio_port: base + 11,
                ui,
                policy: PlaybackPolicy::new(CoreAudio),
                error_reported: false,
            }
        }

        fn update(&mut self, stopping: bool) {
            use crate::lan::audio_wire::PlaybackTarget;
            let now = Instant::now();
            let pid = self.engine_pid;
            let port = self.audio_port;
            let snapshot = std::cell::OnceCell::new();
            let media_active =
                || *snapshot.get_or_init(|| unsafe { flashlink_media_active(pid, port) != 0 });
            let result =
                self.nearby
                    .apply_audio(now, stopping, media_active, |target| match target {
                        PlaybackTarget::Prepare => self.policy.silence_default(now),
                        PlaybackTarget::Session(session) => {
                            self.policy
                                .update_with_media(session, now, stopping, media_active)
                        }
                    });
            if let Err(e) = &result {
                if !self.error_reported {
                    let _ = self.ui.send(Event::Status(format!(
                        "无法设置电脑播放设备的音量或静音状态：{e}"
                    )));
                }
            }
            // A skipped retry has no new error, but the previous failure is still pending.
            self.error_reported = result.is_err()
                || self.policy.finish_retry.is_some()
                || self
                    .policy
                    .silenced
                    .iter()
                    .any(|(_, retry_at)| retry_at.is_some());
        }

        pub fn poll(&mut self) {
            self.update(false);
        }

        pub fn media_idle(&self) -> bool {
            unsafe { flashlink_media_idle(self.engine_pid, self.audio_port) != 0 }
        }
    }

    impl Drop for Playback<'_> {
        fn drop(&mut self) {
            self.update(true);
        }
    }

    #[test]
    fn media_socket_must_belong_to_the_current_engine() {
        let socket = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let port = socket.local_addr().unwrap().port();
        let pid = std::process::id();
        unsafe {
            assert_ne!(flashlink_media_active(pid, port), 0);
            assert_eq!(flashlink_media_active(0, port), 0);
            drop(socket);
            assert_eq!(flashlink_media_active(pid, port), 0);
        }
    }
}

#[cfg(windows)]
pub use windows::Playback;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lan::audio_wire::AudioSample;

    struct Speakers {
        default: usize,
        outputs: [(u8, bool); 2],
        writes: Vec<(usize, Option<u8>, bool)>,
        unavailable: bool,
        missing_endpoint: Option<usize>,
    }

    impl Mixer for Speakers {
        type Endpoint = usize;
        fn default_endpoint(&self) -> io::Result<usize> {
            if self.unavailable {
                Err(io::Error::other("unplugged"))
            } else {
                Ok(self.default)
            }
        }
        fn muted(&self, id: &usize) -> io::Result<bool> {
            Ok(self.outputs[*id].1)
        }
        fn set(&mut self, id: &usize, level: Option<u8>, muted: bool) -> io::Result<()> {
            self.writes.push((*id, level, muted));
            if self.missing_endpoint == Some(*id) {
                return Err(io::Error::other("endpoint unplugged"));
            }
            if let Some(level) = level {
                self.outputs[*id].0 = level;
            }
            self.outputs[*id].1 = muted;
            Ok(())
        }
    }

    fn setup(now: Instant) -> (PlaybackPolicy<Speakers>, AudioSession) {
        let mixer = Speakers {
            default: 0,
            outputs: [(75, false), (42, false)],
            writes: vec![],
            unavailable: false,
            missing_endpoint: None,
        };
        let session = AudioSession {
            owner: [1; 32],
            id: 1,
            last_seen: now,
            ended: false,
            handoff: false,
            sample: AudioSample {
                output: AudioOutput::Controller,
                disconnect: DisconnectPolicy::Sync,
                volume: 33,
                muted: false,
            },
        };
        (PlaybackPolicy::new(mixer), session)
    }

    #[test]
    fn route_changes_release_only_mutes_created_by_flashlink() {
        let now = Instant::now();
        let (mut policy, mut session) = setup(now);
        policy.update(Some(session), now, false).unwrap();
        assert_eq!(policy.mixer.outputs[0], (75, true));
        policy.mixer.default = 1;
        policy.update(Some(session), now, false).unwrap();
        assert_eq!(policy.mixer.outputs, [(75, false), (42, true)]);
        session.sample.output = AudioOutput::Both;
        policy.update(Some(session), now, false).unwrap();
        assert_eq!(policy.mixer.outputs[1], (42, false));
        policy.mixer.outputs[1].1 = true;
        session.sample.output = AudioOutput::Controller;
        policy.update(Some(session), now, false).unwrap();
        session.sample.output = AudioOutput::Host;
        policy.update(Some(session), now, false).unwrap();
        assert_eq!(policy.mixer.outputs[1], (42, true));
        policy.update(None, now, false).unwrap();
        assert_eq!(policy.mixer.outputs[1], (42, true));
    }

    #[test]
    fn exiting_applies_last_volume_or_mute_without_an_intermediate_unmute() {
        let now = Instant::now();
        for (disconnect, volume, muted, output) in [
            (DisconnectPolicy::Mute, 33, false, (75, true)),
            (DisconnectPolicy::Sync, 33, false, (33, false)),
            (DisconnectPolicy::Sync, 33, true, (33, true)),
            (DisconnectPolicy::Sync, 0, false, (0, true)),
        ] {
            let (mut policy, mut session) = setup(now);
            policy.update(Some(session), now, false).unwrap();
            session.sample.disconnect = disconnect;
            session.sample.volume = volume;
            session.sample.muted = muted;
            session.ended = true;
            policy.update(Some(session), now, false).unwrap();
            assert_eq!(policy.mixer.outputs[0], output);
            assert_eq!(policy.mixer.writes.len(), 2);
            // A repeat or normal application shutdown must not override manual changes.
            policy.mixer.outputs[0] = (80, false);
            policy.update(Some(session), now, true).unwrap();
            assert_eq!(policy.mixer.outputs[0], (80, false));
        }
    }

    #[test]
    fn reconnect_and_quality_change_keep_the_live_route_until_their_deadlines() {
        let now = Instant::now();
        for (handoff, grace) in [(false, RECONNECT_GRACE), (true, QUALITY_HANDOFF_GRACE)] {
            let (mut policy, mut session) = setup(now);
            session.handoff = handoff;
            policy.update(Some(session), now, false).unwrap();
            policy
                .update(Some(session), now + grace - Duration::from_secs(1), false)
                .unwrap();
            assert_eq!(policy.mixer.writes.len(), 1);
            policy.update(Some(session), now + grace, false).unwrap();
            assert_eq!(policy.mixer.outputs[0], (33, false));
            session.id += 1;
            session.last_seen = now + grace;
            session.handoff = false;
            policy.update(Some(session), now + grace, false).unwrap();
            assert_eq!(policy.mixer.outputs[0], (33, true));
            policy.update(None, now + grace, false).unwrap();
            assert_eq!(policy.mixer.outputs[0], (33, false));
        }
    }

    #[test]
    fn a_stalled_heartbeat_does_not_end_an_active_media_stream() {
        let now = Instant::now();
        let (mut policy, session) = setup(now);
        policy
            .update_with_media(Some(session), now, false, || panic!("fresh heartbeat"))
            .unwrap();
        let later = now + RECONNECT_GRACE + Duration::from_secs(60);
        policy
            .update_with_media(Some(session), later, false, || true)
            .unwrap();
        assert_eq!(policy.mixer.outputs[0], (75, true));
        assert_eq!(policy.mixer.writes.len(), 1);
        policy
            .update_with_media(Some(session), later, false, || false)
            .unwrap();
        assert_eq!(policy.mixer.outputs[0], (33, false));
        policy
            .update_with_media(Some(session), later, false, || panic!("already finished"))
            .unwrap();
        assert_eq!(policy.mixer.writes.len(), 2);
    }

    #[test]
    fn cancelled_prepare_releases_only_its_mute_and_never_replays_an_old_finish() {
        let now = Instant::now();
        let (mut policy, mut session) = setup(now);
        session.ended = true;
        policy.update(Some(session), now, false).unwrap();
        policy.mixer.outputs[0] = (60, false);
        policy.silence_default(now).unwrap();
        assert_eq!(policy.mixer.outputs[0], (60, true));
        policy.update(None, now, false).unwrap();
        policy.update(Some(session), now, false).unwrap();
        assert_eq!(policy.mixer.outputs[0], (60, false));
    }

    #[test]
    fn disconnect_policy_survives_a_long_outage_without_retrying_every_poll() {
        for (choice, expected) in [
            (DisconnectPolicy::Mute, (42, true)),
            (DisconnectPolicy::Sync, (33, false)),
        ] {
            let start = Instant::now();
            let (mut policy, mut session) = setup(start);
            session.sample.disconnect = choice;
            session.ended = true;
            policy.mixer.default = 1;
            policy.mixer.missing_endpoint = Some(1);
            assert!(policy.update(Some(session), start, false).is_err());
            let later = start + Duration::from_secs(30);
            assert!(policy.update(Some(session), later, false).is_err());
            policy.mixer.missing_endpoint = None;
            policy
                .update(Some(session), later + Duration::from_millis(650), false)
                .unwrap();
            assert_eq!(policy.mixer.writes.len(), 2);
            policy
                .update(Some(session), later + DEVICE_RETRY_INTERVAL, false)
                .unwrap();
            assert_eq!(policy.mixer.outputs[1], expected);
            policy.mixer.outputs[1] = (90, false);
            policy
                .update(Some(session), later + Duration::from_secs(40), false)
                .unwrap();
            assert_eq!(policy.mixer.outputs[1], (90, false));
        }
    }

    #[test]
    fn a_new_live_session_supersedes_an_unapplied_final_volume() {
        let start = Instant::now();
        let (mut policy, mut session) = setup(start);
        session.ended = true;
        policy.mixer.unavailable = true;
        assert!(policy.update(Some(session), start, false).is_err());
        policy.mixer.unavailable = false;
        session.id += 1;
        session.ended = false;
        session.sample.output = AudioOutput::Both;
        let later = start + Duration::from_secs(30);
        session.last_seen = later;
        policy.update(Some(session), later, false).unwrap();
        assert!(policy.mixer.writes.is_empty());
        session.ended = true;
        session.sample.volume = 65;
        policy.update(Some(session), later, false).unwrap();
        assert_eq!(policy.mixer.outputs[0], (65, false));
    }

    #[test]
    fn unplugged_speaker_releases_its_temporary_mute_when_it_returns() {
        let start = Instant::now();
        let (mut policy, mut session) = setup(start);
        policy.update(Some(session), start, false).unwrap();
        policy.mixer.default = 1;
        policy.mixer.missing_endpoint = Some(0);
        assert!(policy.update(Some(session), start, false).is_err());
        session.ended = true;
        session.sample.disconnect = DisconnectPolicy::Mute;
        let later = start + Duration::from_secs(30);
        assert!(policy.update(Some(session), later, false).is_err());
        let writes = policy.mixer.writes.len();
        policy.mixer.missing_endpoint = None;
        policy
            .update(Some(session), later + Duration::from_millis(650), false)
            .unwrap();
        assert_eq!(policy.mixer.writes.len(), writes);
        policy
            .update(Some(session), later + DEVICE_RETRY_INTERVAL, false)
            .unwrap();
        assert_eq!(policy.mixer.outputs, [(75, false), (42, true)]);
    }

    #[test]
    fn final_state_retries_device_switch_and_graceful_host_exit() {
        let now = Instant::now();
        let (mut policy, mut session) = setup(now);
        session.sample.output = AudioOutput::Host;
        policy.update(Some(session), now, false).unwrap();
        assert!(policy.mixer.writes.is_empty());
        session.ended = true;
        policy.mixer.unavailable = true;
        assert!(policy.update(Some(session), now, false).is_err());
        policy.mixer.unavailable = false;
        policy.mixer.default = 1;
        policy
            .update(Some(session), now + DEVICE_RETRY_INTERVAL, false)
            .unwrap();
        assert_eq!(policy.mixer.outputs, [(75, false), (33, false)]);

        session.id += 1;
        session.ended = false;
        session.sample.disconnect = DisconnectPolicy::Mute;
        policy.update(Some(session), now, true).unwrap();
        assert_eq!(policy.mixer.outputs[1], (33, true));
    }
}
