use std::cell::RefCell;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::FlagSet as ContextFlagSet;
use pulse::context::State as ContextState;
use pulse::mainloop::standard::IterateResult;
use pulse::mainloop::standard::Mainloop;
use pulse::volume::ChannelVolumes;
use pulse::volume::Volume;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

const VOLUME_TOLERANCE: u32 = 64;

/// Controls temporary attenuation of the PulseAudio-compatible default sink.
#[derive(Clone, Debug)]
pub struct AudioDucker {
    state_file: PathBuf,
}

impl AudioDucker {
    #[must_use]
    pub fn new(state_file: PathBuf) -> Self {
        Self { state_file }
    }

    /// Restores audio left ducked by a daemon that exited without unwinding its guard.
    pub fn recover(&self) -> Result<(), AudioDuckingError> {
        let Some(state) = load_state(&self.state_file)? else {
            return Ok(());
        };

        restore(&self.state_file, &state, RestoreReason::CrashRecovery)
    }

    /// Attenuates the current default sink until the returned guard is dropped.
    pub fn duck(&self, fraction: f64) -> Result<DuckGuard, AudioDuckingError> {
        if !(0.0..=1.0).contains(&fraction) {
            return Err(AudioDuckingError::InvalidFraction { fraction });
        }
        if fraction == 0.0 {
            return Ok(DuckGuard {
                state_file: self.state_file.clone(),
                state: None,
            });
        }

        // A prior failed restore must be settled before its recovery record can be replaced.
        self.recover()?;

        let mut pulse = PulseConnection::connect()?;
        let sink = pulse.default_sink()?;
        let original = pulse.sink_volume(&sink)?;
        let ducked = original.ducked(fraction);
        let state = DuckState {
            sink,
            original,
            ducked: ducked.clone(),
        };

        // The recovery record reaches disk before the external state changes. An update whose
        // callback is lost is reconciled immediately; if that fails, the guard and state file
        // keep recovery armed without guessing whether Pulse applied the request.
        save_state(&self.state_file, &state)?;
        match pulse.set_sink_volume(&state.sink, &ducked) {
            Ok(VolumeUpdate::Applied) => {
                eprintln!(
                    "system audio ducked by {:.0}% on sink {}",
                    fraction * 100.0,
                    state.sink
                );
            }
            Ok(VolumeUpdate::Rejected) => {
                remove_state(&self.state_file)?;
                return Err(AudioDuckingError::SinkVolumeUpdateRejected { sink: state.sink });
            }
            Err(update_error) => {
                drop(pulse);
                if let Err(recovery_error) =
                    restore(&self.state_file, &state, RestoreReason::UncertainDuck)
                {
                    eprintln!(
                        "system audio duck request outcome is uncertain; recovery remains armed until recording ends: {update_error}; immediate recovery failed: {recovery_error}"
                    );
                    return Ok(DuckGuard {
                        state_file: self.state_file.clone(),
                        state: Some(state),
                    });
                }
                return Err(update_error);
            }
        }

        Ok(DuckGuard {
            state_file: self.state_file.clone(),
            state: Some(state),
        })
    }
}

/// Restores a ducked sink when its recording span ends.
#[derive(Debug)]
#[must_use = "dropping the guard restores system audio"]
pub struct DuckGuard {
    state_file: PathBuf,
    state: Option<DuckState>,
}

impl Drop for DuckGuard {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        if let Err(error) = restore(&self.state_file, &state, RestoreReason::GuardDrop) {
            eprintln!("system audio restore failed; crash recovery record retained: {error}");
        }
    }
}

/// A typed audio-duck operation failure with `PulseAudio` implementation details kept private.
#[derive(Debug, Error)]
pub enum AudioDuckingError {
    #[error("duck fraction {fraction} is outside 0.0..=1.0")]
    InvalidFraction { fraction: f64 },
    #[error("could not allocate a PulseAudio main loop")]
    CreateMainloop,
    #[error("could not create a PulseAudio client context")]
    CreateContext,
    #[error("PulseAudio {operation} failed: {message}")]
    Pulse {
        operation: &'static str,
        message: String,
    },
    #[error("PulseAudio connection entered {state} state")]
    ConnectionState { state: String },
    #[error("PulseAudio server did not report a default sink")]
    MissingDefaultSink,
    #[error("PulseAudio sink {sink:?} did not return volume information")]
    MissingSink { sink: String },
    #[error("PulseAudio rejected the volume update for sink {sink:?}")]
    SinkVolumeUpdateRejected { sink: String },
    #[error("sink name contains a null byte")]
    InvalidSinkName,
    #[error("stored volume has {channels} channels; at most {maximum} are supported")]
    InvalidChannelCount { channels: usize, maximum: u8 },
    #[error("stored volume channel {channel} has invalid value {value}")]
    InvalidVolume { channel: usize, value: u32 },
    #[error("could not {operation} audio duck state file {path}: {source}")]
    StateIo {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse audio duck state file {path}: {source}")]
    StateJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DuckState {
    sink: String,
    original: SinkVolume,
    ducked: SinkVolume,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SinkVolume(Vec<u32>);

impl SinkVolume {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn ducked(&self, fraction: f64) -> Self {
        let retained = 1.0 - fraction;
        Self(
            self.0
                .iter()
                .map(|volume| (f64::from(*volume) * retained).round() as u32)
                .collect(),
        )
    }

    fn matches_with_tolerance(&self, other: &Self, tolerance: u32) -> bool {
        self.0.len() == other.0.len()
            && self
                .0
                .iter()
                .zip(&other.0)
                .all(|(left, right)| left.abs_diff(*right) <= tolerance)
    }

    fn to_channel_volumes(&self) -> Result<ChannelVolumes, AudioDuckingError> {
        let channels = u8::try_from(self.0.len()).map_err(|_error| {
            AudioDuckingError::InvalidChannelCount {
                channels: self.0.len(),
                maximum: ChannelVolumes::CHANNELS_MAX,
            }
        })?;
        if channels > ChannelVolumes::CHANNELS_MAX {
            return Err(AudioDuckingError::InvalidChannelCount {
                channels: self.0.len(),
                maximum: ChannelVolumes::CHANNELS_MAX,
            });
        }

        let mut volumes = ChannelVolumes::default();
        volumes.set_len(channels);
        for (channel, (target, value)) in volumes.get_mut().iter_mut().zip(&self.0).enumerate() {
            let volume = Volume(*value);
            if !volume.is_valid() {
                return Err(AudioDuckingError::InvalidVolume {
                    channel,
                    value: *value,
                });
            }
            *target = volume;
        }
        Ok(volumes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestoreReason {
    CrashRecovery,
    GuardDrop,
    UncertainDuck,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestoreDecision {
    RestoreOriginal,
    AlreadyOriginal,
    PreserveCurrent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VolumeUpdate {
    Applied,
    Rejected,
}

fn restore_decision(
    state: &DuckState,
    current: &SinkVolume,
    reason: RestoreReason,
) -> RestoreDecision {
    if current.matches_with_tolerance(&state.ducked, VOLUME_TOLERANCE) {
        RestoreDecision::RestoreOriginal
    } else if reason == RestoreReason::UncertainDuck
        && current.matches_with_tolerance(&state.original, VOLUME_TOLERANCE)
    {
        RestoreDecision::AlreadyOriginal
    } else {
        RestoreDecision::PreserveCurrent
    }
}

fn restore(
    state_file: &Path,
    state: &DuckState,
    reason: RestoreReason,
) -> Result<(), AudioDuckingError> {
    let mut pulse = PulseConnection::connect()?;
    let current = pulse.sink_volume(&state.sink)?;

    match restore_decision(state, &current, reason) {
        RestoreDecision::AlreadyOriginal => {
            remove_state(state_file)?;
            eprintln!(
                "system audio duck request failed before sink {} changed",
                state.sink
            );
            Ok(())
        }
        RestoreDecision::PreserveCurrent => {
            remove_state(state_file)?;
            eprintln!(
                "system audio restore skipped because sink {} changed while ducked",
                state.sink
            );
            Ok(())
        }
        RestoreDecision::RestoreOriginal => {
            match pulse.set_sink_volume(&state.sink, &state.original)? {
                VolumeUpdate::Applied => {}
                VolumeUpdate::Rejected => {
                    return Err(AudioDuckingError::SinkVolumeUpdateRejected {
                        sink: state.sink.clone(),
                    });
                }
            }
            remove_state(state_file)?;
            match reason {
                RestoreReason::CrashRecovery => {
                    eprintln!(
                        "system audio restored after interrupted recording on sink {}",
                        state.sink
                    );
                }
                RestoreReason::GuardDrop => {
                    eprintln!("system audio restored on sink {}", state.sink);
                }
                RestoreReason::UncertainDuck => {
                    eprintln!(
                        "system audio restored after an uncertain duck request on sink {}",
                        state.sink
                    );
                }
            }
            Ok(())
        }
    }
}

fn load_state(path: &Path) -> Result<Option<DuckState>, AudioDuckingError> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(AudioDuckingError::StateIo {
                operation: "read",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    serde_json::from_slice(&contents)
        .map(Some)
        .map_err(|source| AudioDuckingError::StateJson {
            path: path.to_path_buf(),
            source,
        })
}

fn save_state(path: &Path, state: &DuckState) -> Result<(), AudioDuckingError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| AudioDuckingError::StateIo {
            operation: "create parent directory for",
            path: path.to_path_buf(),
            source,
        })?;
    }
    let contents = serde_json::to_vec(state).map_err(|source| AudioDuckingError::StateJson {
        path: path.to_path_buf(),
        source,
    })?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, contents).map_err(|source| AudioDuckingError::StateIo {
        operation: "write temporary",
        path: temporary.clone(),
        source,
    })?;
    fs::rename(&temporary, path).map_err(|source| AudioDuckingError::StateIo {
        operation: "replace",
        path: path.to_path_buf(),
        source,
    })
}

fn remove_state(path: &Path) -> Result<(), AudioDuckingError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(AudioDuckingError::StateIo {
            operation: "remove",
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[derive(Default)]
struct SinkQuery {
    volume: Option<SinkVolume>,
    failed: bool,
    done: bool,
}

struct PulseConnection {
    // Pulse contexts must be dropped before the main loop that owns their I/O events.
    context: pulse::context::Context,
    mainloop: Mainloop,
}

impl PulseConnection {
    fn connect() -> Result<Self, AudioDuckingError> {
        let mut mainloop = Mainloop::new().ok_or(AudioDuckingError::CreateMainloop)?;
        let mut context = pulse::context::Context::new(&mainloop, "Dictate audio ducking")
            .ok_or(AudioDuckingError::CreateContext)?;
        context
            .connect(None, ContextFlagSet::NOAUTOSPAWN, None)
            .map_err(|error| pulse_error("connection", error))?;

        loop {
            match context.get_state() {
                ContextState::Ready => break,
                ContextState::Failed | ContextState::Terminated => {
                    return Err(AudioDuckingError::ConnectionState {
                        state: format!("{:?}", context.get_state()),
                    });
                }
                ContextState::Unconnected
                | ContextState::Connecting
                | ContextState::Authorizing
                | ContextState::SettingName => iterate(&mut mainloop)?,
            }
        }

        Ok(Self { context, mainloop })
    }

    fn default_sink(&mut self) -> Result<String, AudioDuckingError> {
        let result = Rc::new(RefCell::new(None));
        let callback_result = Rc::clone(&result);
        let _operation = self.context.introspect().get_server_info(move |info| {
            *callback_result.borrow_mut() = Some(
                info.default_sink_name
                    .as_ref()
                    .map(ToString::to_string)
                    .ok_or(AudioDuckingError::MissingDefaultSink),
            );
        });
        self.wait_until(|| result.borrow().is_some())?;
        result
            .borrow_mut()
            .take()
            .ok_or(AudioDuckingError::MissingDefaultSink)?
    }

    fn sink_volume(&mut self, sink: &str) -> Result<SinkVolume, AudioDuckingError> {
        validate_sink_name(sink)?;
        let query = Rc::new(RefCell::new(SinkQuery::default()));
        let callback_query = Rc::clone(&query);
        let _operation = self
            .context
            .introspect()
            .get_sink_info_by_name(sink, move |result| {
                let mut query = callback_query.borrow_mut();
                match result {
                    ListResult::Item(info) => {
                        query.volume = Some(SinkVolume(
                            info.volume.get().iter().map(|volume| volume.0).collect(),
                        ));
                    }
                    ListResult::End => query.done = true,
                    ListResult::Error => {
                        query.failed = true;
                        query.done = true;
                    }
                }
            });
        self.wait_until(|| query.borrow().done)?;

        let mut query = query.borrow_mut();
        if query.failed {
            return Err(AudioDuckingError::Pulse {
                operation: "sink volume query",
                message: "server rejected the request".to_owned(),
            });
        }
        query
            .volume
            .take()
            .ok_or_else(|| AudioDuckingError::MissingSink {
                sink: sink.to_owned(),
            })
    }

    fn set_sink_volume(
        &mut self,
        sink: &str,
        volume: &SinkVolume,
    ) -> Result<VolumeUpdate, AudioDuckingError> {
        validate_sink_name(sink)?;
        let volume = volume.to_channel_volumes()?;
        let result = Rc::new(RefCell::new(None));
        let callback_result = Rc::clone(&result);
        let mut introspector = self.context.introspect();
        let _operation = introspector.set_sink_volume_by_name(
            sink,
            &volume,
            Some(Box::new(move |success| {
                *callback_result.borrow_mut() = Some(success);
            })),
        );
        self.wait_until(|| result.borrow().is_some())?;
        if result.borrow().as_ref() == Some(&true) {
            Ok(VolumeUpdate::Applied)
        } else {
            Ok(VolumeUpdate::Rejected)
        }
    }

    fn wait_until(&mut self, finished: impl Fn() -> bool) -> Result<(), AudioDuckingError> {
        while !finished() {
            iterate(&mut self.mainloop)?;
            match self.context.get_state() {
                ContextState::Failed | ContextState::Terminated => {
                    return Err(AudioDuckingError::ConnectionState {
                        state: format!("{:?}", self.context.get_state()),
                    });
                }
                ContextState::Ready
                | ContextState::Unconnected
                | ContextState::Connecting
                | ContextState::Authorizing
                | ContextState::SettingName => {}
            }
        }
        Ok(())
    }
}

fn iterate(mainloop: &mut Mainloop) -> Result<(), AudioDuckingError> {
    match mainloop.iterate(true) {
        IterateResult::Success(_) => Ok(()),
        IterateResult::Quit(result) => Err(AudioDuckingError::Pulse {
            operation: "main loop",
            message: format!("quit with {result:?}"),
        }),
        IterateResult::Err(error) => Err(pulse_error("main loop iteration", error)),
    }
}

fn validate_sink_name(sink: &str) -> Result<(), AudioDuckingError> {
    if sink.contains('\0') {
        Err(AudioDuckingError::InvalidSinkName)
    } else {
        Ok(())
    }
}

fn pulse_error(operation: &'static str, error: pulse::error::PAErr) -> AudioDuckingError {
    AudioDuckingError::Pulse {
        operation,
        message: error
            .to_string()
            .unwrap_or_else(|| format!("PulseAudio error {error:?}")),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;

    static STATE_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn state_test_path() -> PathBuf {
        let id = STATE_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "dictate-audio-duck-test-{}-{id}",
                std::process::id()
            ))
            .join("audio-duck.json")
    }

    #[test]
    fn duck_volume_policy_reduces_each_channel_by_the_fraction() {
        let original = SinkVolume(vec![65_536, 32_768, 0]);

        assert_eq!(original.ducked(0.2), SinkVolume(vec![52_429, 26_214, 0]));
        assert_eq!(original.ducked(1.0), SinkVolume(vec![0, 0, 0]));
    }

    #[test]
    fn restore_policy_allows_quantization_tolerance() {
        let ducked = SinkVolume(vec![52_429, 52_429]);

        assert!(ducked.matches_with_tolerance(&SinkVolume(vec![52_365, 52_493]), 64));
        assert!(!ducked.matches_with_tolerance(&SinkVolume(vec![52_364, 52_429]), 64));
        assert!(!ducked.matches_with_tolerance(&SinkVolume(vec![52_429]), 64));
    }

    #[test]
    fn uncertain_duck_recovery_keeps_every_external_volume_state_safe() {
        let state = DuckState {
            sink: "test-sink".to_owned(),
            original: SinkVolume(vec![65_536, 65_536]),
            ducked: SinkVolume(vec![52_429, 52_429]),
        };

        assert_eq!(
            restore_decision(
                &state,
                &SinkVolume(vec![65_536, 65_536]),
                RestoreReason::UncertainDuck
            ),
            RestoreDecision::AlreadyOriginal
        );
        assert_eq!(
            restore_decision(
                &state,
                &SinkVolume(vec![52_429, 52_429]),
                RestoreReason::UncertainDuck
            ),
            RestoreDecision::RestoreOriginal
        );
        assert_eq!(
            restore_decision(
                &state,
                &SinkVolume(vec![60_000, 60_000]),
                RestoreReason::UncertainDuck
            ),
            RestoreDecision::PreserveCurrent
        );
    }

    #[test]
    fn ordinary_restore_never_treats_the_original_volume_as_an_uncertain_update() {
        let state = DuckState {
            sink: "test-sink".to_owned(),
            original: SinkVolume(vec![65_536]),
            ducked: SinkVolume(vec![52_429]),
        };

        assert_eq!(
            restore_decision(&state, &state.original, RestoreReason::CrashRecovery),
            RestoreDecision::PreserveCurrent
        );
        assert_eq!(
            restore_decision(&state, &state.ducked, RestoreReason::GuardDrop),
            RestoreDecision::RestoreOriginal
        );
    }

    #[test]
    fn audio_duck_state_file_round_trips() {
        let path = state_test_path();
        let state = DuckState {
            sink: "test-sink".to_owned(),
            original: SinkVolume(vec![65_536, 60_000]),
            ducked: SinkVolume(vec![52_429, 48_000]),
        };

        save_state(&path, &state).expect("state should save");
        assert_eq!(load_state(&path).expect("state should load"), Some(state));
        remove_state(&path).expect("state should be removed");
        assert_eq!(load_state(&path).expect("missing state should load"), None);
        fs::remove_dir_all(path.parent().expect("test state should have a parent"))
            .expect("test state directory should be removed");
    }
}
