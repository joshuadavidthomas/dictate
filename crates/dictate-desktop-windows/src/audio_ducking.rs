//! Core Audio endpoint-volume ducking for Windows.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::ptr;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::IMMDevice;
use windows::Win32::Media::Audio::IMMDeviceEnumerator;
use windows::Win32::Media::Audio::MMDeviceEnumerator;
use windows::Win32::Media::Audio::eConsole;
use windows::Win32::Media::Audio::eRender;
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::Com::COINIT_MULTITHREADED;
use windows::Win32::System::Com::CoCreateInstance;
use windows::Win32::System::Com::CoInitializeEx;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Com::CoUninitialize;
use windows::core::HSTRING;

const VOLUME_TOLERANCE: f32 = 0.001;

/// Controls temporary attenuation of the current default render endpoint.
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
        with_com(|| restore(&self.state_file, &state, RestoreReason::CrashRecovery))
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
        with_com(|| self.duck_default_output(fraction))
    }

    fn duck_default_output(&self, fraction: f64) -> Result<DuckGuard, AudioDuckingError> {
        let (device, volume) = default_output()?;
        let original = master_volume(&volume)?;
        #[allow(clippy::cast_possible_truncation)]
        let ducked = original * (1.0 - fraction) as f32;
        let state = DuckState {
            device,
            original,
            ducked,
        };

        save_state(&self.state_file, &state)?;
        if let Err(update_error) = set_master_volume(&volume, ducked) {
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

        let applied = master_volume(&volume)?;
        if !volumes_match(applied, ducked) {
            restore(&self.state_file, &state, RestoreReason::UncertainDuck)?;
            return Err(AudioDuckingError::VolumeUpdateNotApplied {
                device: state.device,
                expected: ducked,
                actual: applied,
            });
        }

        eprintln!(
            "system audio ducked by {:.0}% on audio endpoint {}",
            fraction * 100.0,
            state.device
        );
        Ok(DuckGuard {
            state_file: self.state_file.clone(),
            state: Some(state),
        })
    }
}

/// Restores a ducked output endpoint when its recording span ends.
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
        if let Err(error) = with_com(|| restore(&self.state_file, &state, RestoreReason::GuardDrop))
        {
            eprintln!("system audio restore failed; crash recovery record retained: {error}");
        }
    }
}

#[derive(Debug, Error)]
pub enum AudioDuckingError {
    #[error("duck fraction {fraction} is outside 0.0..=1.0")]
    InvalidFraction { fraction: f64 },
    #[error("Windows audio {operation} failed: {source}")]
    Platform {
        operation: &'static str,
        #[source]
        source: windows::core::Error,
    },
    #[error(
        "Windows volume update was not applied to endpoint {device}: expected {expected}, got {actual}"
    )]
    VolumeUpdateNotApplied {
        device: String,
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
    device: String,
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
    let volume = endpoint_volume(&device_by_id(&state.device)?)?;
    let current = master_volume(&volume)?;
    match restore_decision(state, current, reason) {
        RestoreDecision::AlreadyOriginal => {
            remove_state(state_file)?;
            eprintln!(
                "system audio duck request failed before audio endpoint {} changed",
                state.device
            );
        }
        RestoreDecision::PreserveCurrent => {
            remove_state(state_file)?;
            eprintln!(
                "system audio restore skipped because audio endpoint {} changed while ducked",
                state.device
            );
        }
        RestoreDecision::RestoreOriginal => {
            set_master_volume(&volume, state.original)?;
            remove_state(state_file)?;
            match reason {
                RestoreReason::CrashRecovery => eprintln!(
                    "system audio restored after interrupted recording on audio endpoint {}",
                    state.device
                ),
                RestoreReason::GuardDrop => {
                    eprintln!("system audio restored on audio endpoint {}", state.device);
                }
                RestoreReason::UncertainDuck => eprintln!(
                    "system audio restored after an uncertain duck request on audio endpoint {}",
                    state.device
                ),
            }
        }
    }
    Ok(())
}

/// Runs `operation` with COM initialized on the current thread.
///
/// Threads that already initialized COM in another apartment keep it; Core Audio's endpoint
/// interfaces work from either apartment.
fn with_com<T>(
    operation: impl FnOnce() -> Result<T, AudioDuckingError>,
) -> Result<T, AudioDuckingError> {
    // SAFETY: COM initialization is balanced below whenever this call succeeds.
    let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if status == RPC_E_CHANGED_MODE {
        return operation();
    }
    status.ok().map_err(|source| AudioDuckingError::Platform {
        operation: "initializing COM",
        source,
    })?;
    let result = operation();
    // SAFETY: This balances the successful CoInitializeEx call above on the same thread.
    unsafe { CoUninitialize() };
    result
}

fn device_enumerator() -> Result<IMMDeviceEnumerator, AudioDuckingError> {
    // SAFETY: COM is initialized by `with_com` for the duration of every caller.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|source| {
        AudioDuckingError::Platform {
            operation: "creating the audio device enumerator",
            source,
        }
    })
}

fn default_output() -> Result<(String, IAudioEndpointVolume), AudioDuckingError> {
    let enumerator = device_enumerator()?;
    // SAFETY: `enumerator` is a live COM interface on this thread.
    let device =
        unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.map_err(|source| {
            AudioDuckingError::Platform {
                operation: "reading the default output endpoint",
                source,
            }
        })?;
    Ok((device_id(&device)?, endpoint_volume(&device)?))
}

fn device_by_id(id: &str) -> Result<IMMDevice, AudioDuckingError> {
    let enumerator = device_enumerator()?;
    let id = HSTRING::from(id);
    // SAFETY: `enumerator` is a live COM interface and `id` is a NUL-terminated UTF-16 string.
    unsafe { enumerator.GetDevice(&id) }.map_err(|source| AudioDuckingError::Platform {
        operation: "opening the ducked output endpoint",
        source,
    })
}

fn device_id(device: &IMMDevice) -> Result<String, AudioDuckingError> {
    // SAFETY: `device` is a live COM interface; the returned string is freed below.
    let id = unsafe { device.GetId() }.map_err(|source| AudioDuckingError::Platform {
        operation: "reading the output endpoint identifier",
        source,
    })?;
    // SAFETY: `GetId` returned a NUL-terminated UTF-16 string allocated by COM.
    let value = unsafe { id.to_string() };
    // SAFETY: The identifier was allocated with CoTaskMemAlloc and is no longer used.
    unsafe { CoTaskMemFree(Some(id.0.cast_const().cast())) };
    value.map_err(|source| AudioDuckingError::Platform {
        operation: "decoding the output endpoint identifier",
        source: windows::core::Error::new(windows::core::HRESULT(-1), source.to_string()),
    })
}

fn endpoint_volume(device: &IMMDevice) -> Result<IAudioEndpointVolume, AudioDuckingError> {
    // SAFETY: `device` is a live COM interface and no activation parameters are needed.
    unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|source| AudioDuckingError::Platform {
        operation: "activating endpoint volume control",
        source,
    })
}

fn master_volume(volume: &IAudioEndpointVolume) -> Result<f32, AudioDuckingError> {
    // SAFETY: `volume` is a live COM interface on this thread.
    unsafe { volume.GetMasterVolumeLevelScalar() }.map_err(|source| AudioDuckingError::Platform {
        operation: "reading output volume",
        source,
    })
}

fn set_master_volume(volume: &IAudioEndpointVolume, level: f32) -> Result<(), AudioDuckingError> {
    // SAFETY: `volume` is a live COM interface; a null event context is permitted.
    unsafe { volume.SetMasterVolumeLevelScalar(level, ptr::null()) }.map_err(|source| {
        AudioDuckingError::Platform {
            operation: "setting output volume",
            source,
        }
    })
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
                "dictate-windows-audio-duck-test-{}-{id}",
                std::process::id()
            ))
            .join("audio-duck.json")
    }

    fn state() -> DuckState {
        DuckState {
            device: "{0.0.0.00000000}.{test-endpoint}".to_owned(),
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
