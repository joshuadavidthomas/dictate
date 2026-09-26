//! `CoreAudio` system-output ducking for macOS.

use std::ffi::c_void;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::ptr;
use std::ptr::NonNull;

use objc2_core_audio::AudioObjectGetPropertyData;
use objc2_core_audio::AudioObjectPropertyAddress;
use objc2_core_audio::AudioObjectSetPropertyData;
use objc2_core_audio::kAudioHardwarePropertyDefaultOutputDevice;
use objc2_core_audio::kAudioObjectPropertyElementMain;
use objc2_core_audio::kAudioObjectPropertyScopeGlobal;
use objc2_core_audio::kAudioObjectPropertyScopeOutput;
use objc2_core_audio::kAudioObjectSystemObject;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

const VIRTUAL_MAIN_VOLUME: u32 = u32::from_be_bytes(*b"vmvc");
const VOLUME_TOLERANCE: f32 = 0.001;
const F32_BYTE_SIZE: u32 = 4;

/// Controls temporary attenuation of the current `CoreAudio` default output device.
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

    /// Attenuates the current default output until the returned guard is dropped.
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

        self.recover()?;
        let device = default_output_device()?;
        let original = output_volume(device)?;
        #[allow(clippy::cast_possible_truncation)]
        let ducked = original * (1.0 - fraction) as f32;
        let state = DuckState {
            device,
            original,
            ducked,
        };

        save_state(&self.state_file, &state)?;
        if let Err(update_error) = set_output_volume(device, ducked) {
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

        let applied = output_volume(device)?;
        if !volumes_match(applied, ducked) {
            restore(&self.state_file, &state, RestoreReason::UncertainDuck)?;
            return Err(AudioDuckingError::VolumeUpdateNotApplied {
                device,
                expected: ducked,
                actual: applied,
            });
        }

        eprintln!(
            "system audio ducked by {:.0}% on CoreAudio device {device}",
            fraction * 100.0
        );
        Ok(DuckGuard {
            state_file: self.state_file.clone(),
            state: Some(state),
        })
    }
}

/// Restores a ducked output device when its recording span ends.
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

#[derive(Debug, Error)]
pub enum AudioDuckingError {
    #[error("duck fraction {fraction} is outside 0.0..=1.0")]
    InvalidFraction { fraction: f64 },
    #[error("CoreAudio {operation} failed with status {status}")]
    CoreAudio {
        operation: &'static str,
        status: i32,
    },
    #[error("CoreAudio did not report a default output device")]
    MissingDefaultOutput,
    #[error(
        "CoreAudio volume update was not applied to device {device}: expected {expected}, got {actual}"
    )]
    VolumeUpdateNotApplied {
        device: u32,
        expected: f32,
        actual: f32,
    },
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DuckState {
    device: u32,
    original: f32,
    ducked: f32,
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

fn restore_decision(state: &DuckState, current: f32, reason: RestoreReason) -> RestoreDecision {
    if reason == RestoreReason::UncertainDuck && volumes_match(current, state.original) {
        RestoreDecision::AlreadyOriginal
    } else if volumes_match(current, state.ducked) {
        RestoreDecision::RestoreOriginal
    } else {
        RestoreDecision::PreserveCurrent
    }
}

fn restore(
    state_file: &Path,
    state: &DuckState,
    reason: RestoreReason,
) -> Result<(), AudioDuckingError> {
    let current = output_volume(state.device)?;
    match restore_decision(state, current, reason) {
        RestoreDecision::AlreadyOriginal => {
            remove_state(state_file)?;
            eprintln!(
                "system audio duck request failed before CoreAudio device {} changed",
                state.device
            );
        }
        RestoreDecision::PreserveCurrent => {
            remove_state(state_file)?;
            eprintln!(
                "system audio restore skipped because CoreAudio device {} changed while ducked",
                state.device
            );
        }
        RestoreDecision::RestoreOriginal => {
            set_output_volume(state.device, state.original)?;
            remove_state(state_file)?;
            match reason {
                RestoreReason::CrashRecovery => eprintln!(
                    "system audio restored after interrupted recording on CoreAudio device {}",
                    state.device
                ),
                RestoreReason::GuardDrop => {
                    eprintln!("system audio restored on CoreAudio device {}", state.device);
                }
                RestoreReason::UncertainDuck => eprintln!(
                    "system audio restored after an uncertain duck request on CoreAudio device {}",
                    state.device
                ),
            }
        }
    }
    Ok(())
}

fn default_output_device() -> Result<u32, AudioDuckingError> {
    let mut address = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDefaultOutputDevice,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut data_size = u32::BITS / 8;
    let mut device = 0_u32;
    // SAFETY: All pointers reference initialized, correctly sized stack values.
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as u32,
            NonNull::from(&mut address),
            0,
            ptr::null(),
            NonNull::from(&mut data_size),
            NonNull::from(&mut device).cast::<c_void>(),
        )
    };
    core_audio_status("reading the default output device", status)?;
    if device == 0 {
        Err(AudioDuckingError::MissingDefaultOutput)
    } else {
        Ok(device)
    }
}

fn output_volume(device: u32) -> Result<f32, AudioDuckingError> {
    let mut address = volume_address();
    let mut data_size = F32_BYTE_SIZE;
    let mut volume = 0.0_f32;
    // SAFETY: All pointers reference initialized, correctly sized stack values.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&mut address),
            0,
            ptr::null(),
            NonNull::from(&mut data_size),
            NonNull::from(&mut volume).cast::<c_void>(),
        )
    };
    core_audio_status("reading output volume", status)?;
    Ok(volume)
}

fn set_output_volume(device: u32, volume: f32) -> Result<(), AudioDuckingError> {
    let mut address = volume_address();
    let mut volume = volume;
    // SAFETY: All pointers reference initialized, correctly sized stack values.
    let status = unsafe {
        AudioObjectSetPropertyData(
            device,
            NonNull::from(&mut address),
            0,
            ptr::null(),
            F32_BYTE_SIZE,
            NonNull::from(&mut volume).cast::<c_void>(),
        )
    };
    core_audio_status("setting output volume", status)
}

fn volume_address() -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: VIRTUAL_MAIN_VOLUME,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn core_audio_status(operation: &'static str, status: i32) -> Result<(), AudioDuckingError> {
    if status == 0 {
        Ok(())
    } else {
        Err(AudioDuckingError::CoreAudio { operation, status })
    }
}

fn volumes_match(left: f32, right: f32) -> bool {
    (left - right).abs() <= VOLUME_TOLERANCE
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
                "dictate-macos-audio-duck-test-{}-{id}",
                std::process::id()
            ))
            .join("audio-duck.json")
    }

    fn state() -> DuckState {
        DuckState {
            device: 42,
            original: 0.75,
            ducked: 0.6,
        }
    }

    #[test]
    fn restore_policy_restores_only_the_owned_volume() {
        let state = state();
        assert_eq!(
            restore_decision(&state, 0.6, RestoreReason::GuardDrop),
            RestoreDecision::RestoreOriginal
        );
        assert_eq!(
            restore_decision(&state, 0.4, RestoreReason::GuardDrop),
            RestoreDecision::PreserveCurrent
        );
        assert_eq!(
            restore_decision(&state, 0.75, RestoreReason::UncertainDuck),
            RestoreDecision::AlreadyOriginal
        );
    }

    #[test]
    fn restore_policy_allows_core_audio_quantization() {
        let state = state();
        assert_eq!(
            restore_decision(&state, 0.6009, RestoreReason::CrashRecovery),
            RestoreDecision::RestoreOriginal
        );
        assert_eq!(
            restore_decision(&state, 0.6011, RestoreReason::CrashRecovery),
            RestoreDecision::PreserveCurrent
        );
    }

    #[test]
    fn audio_duck_state_file_round_trips() {
        let path = state_test_path();
        let state = state();
        save_state(&path, &state).expect("state should save");
        assert_eq!(load_state(&path).expect("state should load"), Some(state));
        remove_state(&path).expect("state should be removed");
        assert_eq!(load_state(&path).expect("missing state should load"), None);
        fs::remove_dir_all(path.parent().expect("test state should have a parent"))
            .expect("test state directory should be removed");
    }
}
