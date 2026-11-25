use crate::lan::audio_wire::{AudioOutput, AudioSession, DisconnectPolicy};
use std::{
    io,
    time::{Duration, Instant},
};

const RECONNECT_GRACE: Duration = Duration::from_secs(18);
const QUALITY_HANDOFF_GRACE: Duration = Duration::from_secs(35);
const DEVICE_RETRY: Duration = Duration::from_secs(10);

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
    retry: Option<(SampleKey, Instant)>,
}

impl<M: Mixer> PlaybackPolicy<M> {
    fn new(mixer: M) -> Self {
        Self {
            mixer,
            silenced: Vec::new(),
            finished: None,
            retry: None,
        }
    }

    fn update(
        &mut self,
        session: Option<AudioSession>,
        now: Instant,
        stopping: bool,
    ) -> io::Result<()> {
        let Some(session) = session else {
            self.finished = None;
            self.retry = None;
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
        if stopping || session.ended || now.saturating_duration_since(session.last_seen) >= grace {
            // A new owner or quality restart may have inherited our temporary mute. Only
            // the latest authenticated session gets to choose the final speaker state.
            let result = self.finish(session);
            if result.is_ok() {
                self.finished = Some(key);
                self.retry = None;
                return self.release_except(None, now);
            }
            if self.retry.is_none_or(|(failed, _)| failed != key) {
                self.retry = Some((key, now));
            } else if self
                .retry
                .is_some_and(|(_, since)| now.duration_since(since) >= DEVICE_RETRY)
            {
                self.finished = Some(key);
                // An unavailable endpoint cannot safely be unmuted on a later, unrelated stream.
                self.silenced.clear();
            }
            return result;
        }

        self.finished = None;
        self.retry = None;
        if session.sample.output != AudioOutput::Controller {
            return self.release_except(None, now);
        }
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
        self.silenced.retain_mut(|(id, first_failure)| {
            if Some(&*id) == keep {
                *first_failure = None;
                return true;
            }
            match self.mixer.set(id, None, false) {
                Ok(()) => false,
                Err(error) => {
                    failure = Some(error);
                    now.duration_since(*first_failure.get_or_insert(now)) < DEVICE_RETRY
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
        ui: &'a Sender<Event>,
        policy: PlaybackPolicy<CoreAudio>,
        error_reported: bool,
    }

    impl<'a> Playback<'a> {
        pub fn new(nearby: &'a Nearby, ui: &'a Sender<Event>) -> Self {
            Self {
                nearby,
                ui,
                policy: PlaybackPolicy::new(CoreAudio),
                error_reported: false,
            }
        }

        fn update(&mut self, stopping: bool) {
            let result = self
                .policy
                .update(self.nearby.audio_session(), Instant::now(), stopping);
            if let Err(e) = &result {
                if !self.error_reported {
                    let _ = self.ui.send(Event::Status(format!(
                        "无法设置电脑播放设备的音量或静音状态：{e}"
                    )));
                }
            }
            self.error_reported = result.is_err();
        }

        pub fn poll(&mut self) {
            self.update(false);
        }
    }

    impl Drop for Playback<'_> {
        fn drop(&mut self) {
            self.update(true);
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
            .update(Some(session), now + Duration::from_secs(1), false)
            .unwrap();
        assert_eq!(policy.mixer.outputs, [(75, false), (33, false)]);

        session.id += 1;
        session.ended = false;
        session.sample.disconnect = DisconnectPolicy::Mute;
        policy.update(Some(session), now, true).unwrap();
        assert_eq!(policy.mixer.outputs[1], (33, true));
    }
}
