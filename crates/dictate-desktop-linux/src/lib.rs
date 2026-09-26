#![cfg(target_os = "linux")]

mod audio_ducking;
mod clipboard;
mod focus;
mod global_shortcuts;
mod input;

use std::ffi::OsStr;
use std::ffi::OsString;
use std::io;

pub use audio_ducking::AudioDucker;
pub use audio_ducking::AudioDuckingError;
pub use audio_ducking::DuckGuard;
use dictate_desktop::ClipboardFailure;
use dictate_desktop::ClipboardPasteBackend;
use dictate_desktop::ClipboardSink;
use dictate_desktop::DeliveryClipboardFailureKind;
use dictate_desktop::DeliveryReport;
use dictate_desktop::DeliveryTarget;
use dictate_desktop::DirectTyper;
use dictate_desktop::FocusObservation;
use dictate_desktop::FocusSnapshot;
use dictate_desktop::InputSynthesisOutcome;
use dictate_desktop::InsertionText;
pub use global_shortcuts::PushToTalkError;
pub use global_shortcuts::PushToTalkEvent;
pub use global_shortcuts::PushToTalkShortcut;
pub use global_shortcuts::listen_push_to_talk;
use wl_clipboard_rs::copy;

const TEXT_MIME: &str = "text/plain;charset=utf-8";

#[must_use]
pub fn observe() -> FocusObservation {
    let environment = SessionEnvironment::read();
    if environment.is_niri() {
        focus::observe(environment.niri_socket.as_deref())
    } else {
        FocusObservation::UnsupportedSession
    }
}

#[must_use]
pub fn snapshot() -> FocusSnapshot {
    FocusSnapshot::from_observation(&observe())
}

#[must_use = "delivery may fail; handle the DeliveryReport"]
pub fn deliver(target: DeliveryTarget, text: &str) -> DeliveryReport {
    let mut insertion = ClipboardPasteBackend::new(
        clipboard::PlatformClipboard::default(),
        input::ClipboardPasteChordBackend,
        DirectInputBackend,
    );
    let mut clipboard = PlatformClipboardSink;
    dictate_desktop::deliver(target, text, &mut insertion, &mut clipboard)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct DirectInputBackend;

impl DirectTyper for DirectInputBackend {
    fn type_text(&mut self, text: InsertionText<'_>) -> InputSynthesisOutcome {
        input::type_text(text)
    }
}

struct PlatformClipboardSink;

impl ClipboardSink for PlatformClipboardSink {
    fn copy(&mut self, text: &str) -> Result<(), ClipboardFailure> {
        let mut options = copy::Options::new();
        options.clipboard(copy::ClipboardType::Regular);
        options
            .copy(
                copy::Source::Bytes(text.as_bytes().to_vec().into_boxed_slice()),
                copy::MimeType::Specific(TEXT_MIME.to_owned()),
            )
            .map_err(|error| ClipboardFailure::new(copy_failure_kind(&error)))
    }
}

fn copy_failure_kind(error: &copy::Error) -> DeliveryClipboardFailureKind {
    match error {
        copy::Error::NoSeats | copy::Error::SeatNotFound => {
            DeliveryClipboardFailureKind::Unavailable
        }
        copy::Error::SocketOpenError(error) => DeliveryClipboardFailureKind::Io {
            operation: "opening the clipboard socket",
            kind: error.kind(),
        },
        copy::Error::WaylandConnection(_) => DeliveryClipboardFailureKind::Connection,
        copy::Error::WaylandCommunication(_) => DeliveryClipboardFailureKind::Communication,
        copy::Error::MissingProtocol { name, version } => {
            DeliveryClipboardFailureKind::MissingCapability {
                name: (*name).to_owned(),
                version: *version,
            }
        }
        copy::Error::PrimarySelectionUnsupported => DeliveryClipboardFailureKind::Unsupported,
        copy::Error::TempCopy(error) => {
            DeliveryClipboardFailureKind::TemporaryStorage(source_creation_error_kind(error))
        }
        copy::Error::TempFileRemove(error) | copy::Error::TempDirRemove(error) => {
            DeliveryClipboardFailureKind::TemporaryStorage(error.kind())
        }
        copy::Error::Paste(
            copy::DataSourceError::FileOpen(error) | copy::DataSourceError::Copy(error),
        ) => DeliveryClipboardFailureKind::DataTransfer(error.kind()),
    }
}

fn source_creation_error_kind(error: &copy::SourceCreationError) -> io::ErrorKind {
    match error {
        copy::SourceCreationError::TempDirCreate(error)
        | copy::SourceCreationError::TempFileCreate(error)
        | copy::SourceCreationError::DataCopy(error)
        | copy::SourceCreationError::TempFileWrite(error)
        | copy::SourceCreationError::TempFileOpen(error)
        | copy::SourceCreationError::TempFileMetadata(error)
        | copy::SourceCreationError::TempFileSeek(error)
        | copy::SourceCreationError::TempFileRead(error)
        | copy::SourceCreationError::TempFileTruncate(error) => error.kind(),
    }
}

struct SessionEnvironment {
    current_desktop: Option<OsString>,
    niri_socket: Option<OsString>,
}

impl SessionEnvironment {
    fn read() -> Self {
        Self {
            current_desktop: std::env::var_os("XDG_CURRENT_DESKTOP"),
            niri_socket: std::env::var_os("NIRI_SOCKET"),
        }
    }

    fn is_niri(&self) -> bool {
        self.niri_socket.is_some()
            || self
                .current_desktop
                .as_deref()
                .is_some_and(|desktop| desktop_name_matches(desktop, "niri"))
    }

    #[cfg(test)]
    fn for_test(current_desktop: Option<&str>, niri_socket: Option<&str>) -> Self {
        Self {
            current_desktop: current_desktop.map(OsString::from),
            niri_socket: niri_socket.map(OsString::from),
        }
    }
}

fn desktop_name_matches(desktop: &OsStr, expected: &str) -> bool {
    desktop
        .to_string_lossy()
        .split([':', ';'])
        .any(|name| name.trim().eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_detection_accepts_niri_socket_or_desktop_name() {
        assert!(SessionEnvironment::for_test(None, Some("socket")).is_niri());
        assert!(SessionEnvironment::for_test(Some("GNOME:niri"), None).is_niri());
        assert!(SessionEnvironment::for_test(Some("NIRI"), None).is_niri());
        assert!(!SessionEnvironment::for_test(Some("sway"), None).is_niri());
    }
}
