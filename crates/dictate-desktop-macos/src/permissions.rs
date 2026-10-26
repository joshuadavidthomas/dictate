use std::cell::RefCell;
use std::fmt;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::AVAuthorizationStatus;
use objc2_av_foundation::AVCaptureDevice;
use objc2_av_foundation::AVMediaTypeAudio;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

const MICROPHONE_REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
/// Long enough that a 120 ms poll asks a fresh process twice a second at most.
const FRESH_STATUS_TTL: Duration = Duration::from_millis(500);

thread_local! {
    /// The last answer from a fresh copy of this process, with when it arrived.
    static SETTINGS_STATUS: RefCell<Option<(Instant, Option<PermissionStatus>)>> =
        const { RefCell::new(None) };
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDCheckAccess(request: u32) -> u32;
}

const IOHID_REQUEST_LISTEN_EVENT: u32 = 1;
const IOHID_ACCESS_GRANTED: u32 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Ready,
    NeedsRequest,
    NeedsSettings,
    /// Turned on in System Settings after this process started; macOS
    /// keeps a process's first answer, so it applies after a restart.
    NeedsRestart,
}

impl fmt::Display for PermissionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready => formatter.write_str("ready"),
            Self::NeedsRequest => formatter.write_str("needs request"),
            Self::NeedsSettings => formatter.write_str("needs System Settings"),
            Self::NeedsRestart => formatter.write_str("granted; restart to use it"),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
    let local = local_permission_status()?;
    // Nothing is missing from this process's point of view, so there is no
    // switch to second-guess: skip the process a fresh answer would cost.
    if local.ready() {
        return Ok(local);
    }
    let now = settings_status();
    Ok(PermissionStatus {
        microphone: local.microphone,
        input_monitoring: grant_state(
            local.input_monitoring == PermissionState::Ready,
            now.map_or(local.input_monitoring == PermissionState::Ready, |now| {
                now.input_monitoring == PermissionState::Ready
            }),
        ),
        accessibility: grant_state(
            local.accessibility == PermissionState::Ready,
            now.map_or(local.accessibility == PermissionState::Ready, |now| {
                now.accessibility == PermissionState::Ready
            }),
        ),
    })
}

/// What this process may use, on its own. The preflights answer from the state
/// the process started in, so a switch turned on since then is invisible here;
/// `permission_status` is the one that pairs this with what Settings shows.
pub fn local_permission_status() -> Result<PermissionStatus, PermissionStatusError> {
    Ok(PermissionStatus {
        microphone: microphone_state()?,
        input_monitoring: grant_state(
            listen_event_access(),
            granted(PermissionKind::InputMonitoring),
        ),
        accessibility: grant_state(post_event_access(), granted(PermissionKind::Accessibility)),
    })
}

/// The most recent fresh answer, kept briefly: the guide polls every 120 ms and
/// each answer costs a process.
fn settings_status() -> Option<PermissionStatus> {
    SETTINGS_STATUS.with_borrow_mut(|cached| {
        let fresh = cached
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < FRESH_STATUS_TTL);
        if !fresh {
            *cached = Some((Instant::now(), ask_a_fresh_process()));
        }
        cached.as_ref().and_then(|(_, status)| *status)
    })
}

/// What System Settings shows for this app now, which can be ahead of what a
/// long-running process may use.
///
/// macOS keeps a process's first preflight answer for its lifetime, so the
/// daemon cannot see a switch the user flips after launch. A second copy of
/// this executable inherits the app's TCC identity but starts with an empty
/// cache, so it answers what the switch says.
fn ask_a_fresh_process() -> Option<PermissionStatus> {
    let executable = std::env::current_exe().ok()?;
    let output = Command::new(executable)
        .args(["permissions", "--local", "--json"])
        .output()
        .ok()?;
    serde_json::from_slice(&output.stdout).ok()
}

/// Whether System Settings shows `kind` turned on for this app, even when this
/// process may not use it until it restarts.
pub(crate) fn granted_in_settings(kind: PermissionKind) -> bool {
    let now = settings_status();
    match kind {
        PermissionKind::Microphone => {
            microphone_state().is_ok_and(|state| state == PermissionState::Ready)
        }
        PermissionKind::InputMonitoring => now.map_or_else(
            || granted(PermissionKind::InputMonitoring),
            |now| now.input_monitoring == PermissionState::Ready,
        ),
        PermissionKind::Accessibility => now.map_or_else(
            || granted(PermissionKind::Accessibility),
            |now| now.accessibility == PermissionState::Ready,
        ),
    }
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

/// Asks for `kind`: the microphone prompt while it has never been
/// answered (answered on another thread, so this returns at once), and
/// otherwise System Settings opened at its list with the floating guide
/// beside it.
pub fn request_permission(kind: PermissionKind) -> Result<(), PermissionStatusError> {
    if kind == PermissionKind::Microphone && microphone_state()? == PermissionState::NeedsRequest {
        std::thread::spawn(|| match request_microphone_permission() {
            Ok(state) => eprintln!("microphone permission request finished: {state}"),
            Err(error) => eprintln!("microphone permission request failed: {error}"),
        });
        return Ok(());
    }
    open_permission_settings(kind)?;
    crate::permission_guide::show(kind);
    Ok(())
}

/// Whether System Settings shows `kind` turned on for this app now, which
/// can be ahead of what this process may use.
pub(crate) fn granted(kind: PermissionKind) -> bool {
    match kind {
        PermissionKind::Microphone => {
            microphone_state().is_ok_and(|state| state == PermissionState::Ready)
        }
        // SAFETY: These checks have no parameters beyond a constant and no ownership effects.
        PermissionKind::InputMonitoring => unsafe {
            IOHIDCheckAccess(IOHID_REQUEST_LISTEN_EVENT) == IOHID_ACCESS_GRANTED
        },
        // SAFETY: As above.
        PermissionKind::Accessibility => unsafe { AXIsProcessTrusted() },
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

/// `usable` is what this process may use, fixed at its first check;
/// `granted` is what System Settings shows now.
const fn grant_state(usable: bool, granted: bool) -> PermissionState {
    match (usable, granted) {
        (true, _) => PermissionState::Ready,
        (false, true) => PermissionState::NeedsRestart,
        (false, false) => PermissionState::NeedsSettings,
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
    fn a_switch_turned_on_after_launch_needs_a_restart() {
        assert_eq!(grant_state(true, false), PermissionState::Ready);
        assert_eq!(grant_state(false, true), PermissionState::NeedsRestart);
        assert_eq!(grant_state(false, false), PermissionState::NeedsSettings);
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
