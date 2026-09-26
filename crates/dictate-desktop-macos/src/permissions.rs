use std::fmt;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::AVAuthorizationStatus;
use objc2_av_foundation::AVCaptureDevice;
use objc2_av_foundation::AVMediaTypeAudio;
use serde::Serialize;
use thiserror::Error;

const MICROPHONE_REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Ready,
    NeedsRequest,
    NeedsSettings,
}

impl fmt::Display for PermissionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready => formatter.write_str("ready"),
            Self::NeedsRequest => formatter.write_str("needs request"),
            Self::NeedsSettings => formatter.write_str("needs System Settings"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionKind {
    Microphone,
    InputMonitoring,
    Accessibility,
}

impl PermissionKind {
    fn settings_uri(self) -> &'static str {
        match self {
            Self::Microphone => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            Self::InputMonitoring => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
            }
            Self::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct PermissionStatus {
    pub microphone: PermissionState,
    pub input_monitoring: PermissionState,
    pub accessibility: PermissionState,
}

impl PermissionStatus {
    #[must_use]
    pub const fn ready(self) -> bool {
        matches!(self.microphone, PermissionState::Ready)
            && matches!(self.input_monitoring, PermissionState::Ready)
            && matches!(self.accessibility, PermissionState::Ready)
    }
}

#[derive(Debug, Error)]
pub enum PermissionStatusError {
    #[error("AVFoundation did not expose the audio media type")]
    MissingAudioMediaType,
    #[error("the microphone permission request did not finish within 120 seconds")]
    MicrophoneRequestTimedOut,
    #[error("the microphone permission request callback disconnected")]
    MicrophoneRequestDisconnected,
    #[error("could not open macOS {permission} settings: {source}")]
    OpenSettings {
        permission: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("macOS could not open {permission} settings")]
    OpenSettingsRejected { permission: &'static str },
}

pub fn permission_status() -> Result<PermissionStatus, PermissionStatusError> {
    Ok(PermissionStatus {
        microphone: microphone_state()?,
        input_monitoring: event_permission_state(listen_event_access()),
        accessibility: event_permission_state(post_event_access()),
    })
}

pub fn request_microphone_permission() -> Result<PermissionState, PermissionStatusError> {
    let media_type = audio_media_type()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let handler = RcBlock::new(move |granted: Bool| {
        let _ignored = sender.send(granted.as_bool());
    });
    // SAFETY: The media type is AVFoundation's process-lifetime audio constant and the retained
    // block remains alive while this function waits for the asynchronous callback.
    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &handler);
    }
    match receiver.recv_timeout(MICROPHONE_REQUEST_TIMEOUT) {
        Ok(true) => Ok(PermissionState::Ready),
        Ok(false) => Ok(PermissionState::NeedsSettings),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err(PermissionStatusError::MicrophoneRequestTimedOut)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(PermissionStatusError::MicrophoneRequestDisconnected)
        }
    }
}

pub fn open_permission_settings(kind: PermissionKind) -> Result<(), PermissionStatusError> {
    let permission = match kind {
        PermissionKind::Microphone => "Microphone",
        PermissionKind::InputMonitoring => "Input Monitoring",
        PermissionKind::Accessibility => "Accessibility",
    };
    let status = Command::new("/usr/bin/open")
        .arg(kind.settings_uri())
        .status()
        .map_err(|source| PermissionStatusError::OpenSettings { permission, source })?;
    if status.success() {
        Ok(())
    } else {
        Err(PermissionStatusError::OpenSettingsRejected { permission })
    }
}

fn microphone_state() -> Result<PermissionState, PermissionStatusError> {
    let media_type = audio_media_type()?;
    // SAFETY: The media type is AVFoundation's process-lifetime audio constant.
    let status: AVAuthorizationStatus =
        unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    Ok(microphone_authorization_state(status.0))
}

fn audio_media_type() -> Result<&'static objc2_foundation::NSString, PermissionStatusError> {
    // SAFETY: This reads AVFoundation's immutable, process-lifetime media-type constant.
    unsafe { AVMediaTypeAudio.ok_or(PermissionStatusError::MissingAudioMediaType) }
}

const fn microphone_authorization_state(status: isize) -> PermissionState {
    match status {
        0 => PermissionState::NeedsRequest,
        3 => PermissionState::Ready,
        _ => PermissionState::NeedsSettings,
    }
}

const fn event_permission_state(granted: bool) -> PermissionState {
    if granted {
        PermissionState::Ready
    } else {
        PermissionState::NeedsSettings
    }
}

fn listen_event_access() -> bool {
    // SAFETY: This preflight function has no parameters or ownership effects.
    unsafe { CGPreflightListenEventAccess() }
}

fn post_event_access() -> bool {
    // SAFETY: This preflight function has no parameters or ownership effects.
    unsafe { CGPreflightPostEventAccess() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microphone_authorization_states_are_exhaustive() {
        assert_eq!(
            microphone_authorization_state(0),
            PermissionState::NeedsRequest
        );
        assert_eq!(microphone_authorization_state(3), PermissionState::Ready);
        for denied_or_restricted in [1, 2, 4, isize::MAX] {
            assert_eq!(
                microphone_authorization_state(denied_or_restricted),
                PermissionState::NeedsSettings
            );
        }
    }

    #[test]
    fn setup_requires_every_permission() {
        let ready = PermissionStatus {
            microphone: PermissionState::Ready,
            input_monitoring: PermissionState::Ready,
            accessibility: PermissionState::Ready,
        };
        assert!(ready.ready());
        assert!(
            !PermissionStatus {
                microphone: PermissionState::NeedsRequest,
                ..ready
            }
            .ready()
        );
        assert!(
            !PermissionStatus {
                input_monitoring: PermissionState::NeedsSettings,
                ..ready
            }
            .ready()
        );
        assert!(
            !PermissionStatus {
                accessibility: PermissionState::NeedsSettings,
                ..ready
            }
            .ready()
        );
    }
}
