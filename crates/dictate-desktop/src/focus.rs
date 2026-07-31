mod backends;

use std::ffi::OsStr;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::time::Duration;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;

const WINDOW_LABEL_LIMIT: usize = 160;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FocusObservation {
    Focused(FocusedWindow),
    NoFocusedWindow { source: FocusSource },
    UnsupportedSession,
    ProbeFailed(FocusProbeFailure),
}

impl fmt::Display for FocusObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Focused(window) => write!(formatter, "{window}"),
            Self::NoFocusedWindow { source } => {
                write!(formatter, "{source} reported no focused window")
            }
            Self::UnsupportedSession => {
                formatter.write_str("unavailable (no supported compositor session detected)")
            }
            Self::ProbeFailed(failure) => write!(formatter, "unavailable ({failure})"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FocusSnapshot {
    Focused(FocusedWindow),
    NoFocusedWindow { source: FocusSource },
    Unavailable,
}

impl FocusSnapshot {
    pub(crate) fn from_observation(observation: &FocusObservation) -> Self {
        match observation {
            FocusObservation::Focused(window) => Self::Focused(window.clone()),
            FocusObservation::NoFocusedWindow { source } => {
                Self::NoFocusedWindow { source: *source }
            }
            FocusObservation::UnsupportedSession | FocusObservation::ProbeFailed(_) => {
                Self::Unavailable
            }
        }
    }
}

impl fmt::Display for FocusSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Focused(window) => write!(formatter, "{window}"),
            Self::NoFocusedWindow { source } => {
                write!(formatter, "{source} reported no focused window")
            }
            Self::Unavailable => formatter.write_str("focus unavailable"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FocusedWindow {
    identity: FocusTargetIdentity,
    app_id: Option<WindowAppId>,
    title: Option<WindowTitle>,
}

impl FocusedWindow {
    pub(super) fn niri(
        instance: NiriInstanceId,
        window_id: u64,
        app_id: Option<&str>,
        title: Option<&str>,
    ) -> Self {
        Self {
            identity: FocusTargetIdentity::Niri {
                instance,
                window_id: NiriWindowId(window_id),
            },
            app_id: app_id.and_then(WindowAppId::new),
            title: title.and_then(WindowTitle::new),
        }
    }

    #[must_use]
    pub fn same_target(&self, other: &Self) -> bool {
        self.identity == other.identity
    }

    #[cfg(test)]
    pub(crate) fn test_niri(instance: u64, window_id: u64, app_id: &str, title: &str) -> Self {
        Self::niri(
            NiriInstanceId {
                compositor_pid: 1,
                start_time_ticks: instance,
            },
            window_id,
            Some(app_id),
            Some(title),
        )
    }

    fn source(&self) -> FocusSource {
        match self.identity {
            FocusTargetIdentity::Niri { .. } => FocusSource::Niri,
        }
    }
}

impl fmt::Display for FocusedWindow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.app_id, &self.title) {
            (Some(app_id), Some(title)) => {
                write!(
                    formatter,
                    "{app_id} — {title} (reported by {})",
                    self.source()
                )
            }
            (Some(app_id), None) => write!(formatter, "{app_id} (reported by {})", self.source()),
            (None, Some(title)) => write!(formatter, "{title} (reported by {})", self.source()),
            (None, None) => write!(formatter, "unnamed window (reported by {})", self.source()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum FocusTargetIdentity {
    Niri {
        instance: NiriInstanceId,
        window_id: NiriWindowId,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct NiriInstanceId {
    compositor_pid: i32,
    start_time_ticks: u64,
}

impl NiriInstanceId {
    pub(super) const fn new(compositor_pid: i32, start_time_ticks: u64) -> Self {
        Self {
            compositor_pid,
            start_time_ticks,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct NiriWindowId(u64);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FocusSource {
    Niri,
}

impl fmt::Display for FocusSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Niri => formatter.write_str("niri"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct WindowAppId(String);

impl WindowAppId {
    fn new(value: &str) -> Option<Self> {
        sanitize_window_label(value).map(Self)
    }
}

impl<'de> Deserialize<'de> for WindowAppId {
    fn deserialize<DeserializerType>(
        deserializer: DeserializerType,
    ) -> Result<Self, DeserializerType::Error>
    where
        DeserializerType: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).ok_or_else(|| serde::de::Error::custom("window app ID is empty"))
    }
}

impl fmt::Display for WindowAppId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct WindowTitle(String);

impl WindowTitle {
    fn new(value: &str) -> Option<Self> {
        sanitize_window_label(value).map(Self)
    }
}

impl<'de> Deserialize<'de> for WindowTitle {
    fn deserialize<DeserializerType>(
        deserializer: DeserializerType,
    ) -> Result<Self, DeserializerType::Error>
    where
        DeserializerType: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).ok_or_else(|| serde::de::Error::custom("window title is empty"))
    }
}

impl fmt::Display for WindowTitle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn sanitize_window_label(value: &str) -> Option<String> {
    let cleaned = value
        .chars()
        .map(|character| {
            if character.is_control() || is_log_format_control(character) {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        return None;
    }

    let mut characters = cleaned.chars();
    let mut bounded = characters
        .by_ref()
        .take(WINDOW_LABEL_LIMIT)
        .collect::<String>();
    if characters.next().is_some() {
        bounded.push('…');
    }
    Some(bounded)
}

fn is_log_format_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FocusProbeFailure {
    source: FocusSource,
    kind: FocusProbeFailureKind,
}

impl FocusProbeFailure {
    pub(super) const fn new(source: FocusSource, kind: FocusProbeFailureKind) -> Self {
        Self { source, kind }
    }
}

impl fmt::Display for FocusProbeFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} focus probe failed: {}",
            self.source, self.kind
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum FocusProbeFailureKind {
    EnvironmentUnavailable {
        variable: &'static str,
    },
    Io {
        operation: FocusProbeIoOperation,
        kind: io::ErrorKind,
    },
    TimedOut {
        operation: FocusProbeIoOperation,
        timeout: Duration,
    },
    ResponseTooLarge {
        limit: usize,
    },
    InvalidPeerIdentity,
    InvalidResponse {
        kind: FocusResponseFailureKind,
        line: usize,
        column: usize,
    },
    RequestRejected {
        message: FocusProbeMessage,
    },
}

impl fmt::Display for FocusProbeFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EnvironmentUnavailable { variable } => {
                write!(formatter, "{variable} is not set")
            }
            Self::Io { operation, kind } => write!(formatter, "{operation} ({kind:?})"),
            Self::TimedOut { operation, timeout } => write!(
                formatter,
                "{operation} timed out after {} ms",
                timeout.as_millis()
            ),
            Self::ResponseTooLarge { limit } => {
                write!(formatter, "response exceeded the {limit}-byte limit")
            }
            Self::InvalidPeerIdentity => {
                formatter.write_str("compositor process identity was invalid")
            }
            Self::InvalidResponse { kind, line, column } => write!(
                formatter,
                "invalid JSON response ({kind}) at line {line}, column {column}"
            ),
            Self::RequestRejected { message } => write!(formatter, "request rejected: {message}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FocusProbeIoOperation {
    BuildSocketAddress,
    CreateSocket,
    Connect,
    Poll,
    CheckConnection,
    ReadPeerIdentity,
    WriteRequest,
    ReadResponse,
}

impl fmt::Display for FocusProbeIoOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuildSocketAddress => {
                formatter.write_str("could not build compositor IPC address")
            }
            Self::CreateSocket => formatter.write_str("could not create compositor IPC socket"),
            Self::Connect => formatter.write_str("could not connect to compositor IPC"),
            Self::Poll => formatter.write_str("could not poll compositor IPC"),
            Self::CheckConnection => {
                formatter.write_str("could not check compositor IPC connection")
            }
            Self::ReadPeerIdentity => {
                formatter.write_str("could not read compositor process identity")
            }
            Self::WriteRequest => formatter.write_str("could not write IPC request"),
            Self::ReadResponse => formatter.write_str("could not read IPC response"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FocusResponseFailureKind {
    Io,
    Syntax,
    Data,
    EndOfFile,
}

impl FocusResponseFailureKind {
    pub(super) const fn from_json_category(category: serde_json::error::Category) -> Self {
        match category {
            serde_json::error::Category::Io => Self::Io,
            serde_json::error::Category::Syntax => Self::Syntax,
            serde_json::error::Category::Data => Self::Data,
            serde_json::error::Category::Eof => Self::EndOfFile,
        }
    }
}

impl fmt::Display for FocusResponseFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io => formatter.write_str("I/O"),
            Self::Syntax => formatter.write_str("syntax"),
            Self::Data => formatter.write_str("unexpected shape"),
            Self::EndOfFile => formatter.write_str("incomplete"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FocusProbeMessage(String);

impl FocusProbeMessage {
    pub(super) fn new(message: &str) -> Self {
        const LIMIT: usize = 240;

        let mut escaped = String::new();
        let mut written = 0;
        'message: for character in message.chars() {
            if character.is_control() || is_log_format_control(character) {
                for escaped_character in character.escape_unicode() {
                    if written == LIMIT {
                        break 'message;
                    }
                    escaped.push(escaped_character);
                    written += 1;
                }
            } else {
                if written == LIMIT {
                    break;
                }
                escaped.push(character);
                written += 1;
            }
        }
        Self(escaped)
    }
}

impl fmt::Display for FocusProbeMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[must_use]
pub fn observe() -> FocusObservation {
    backends::observe(&SessionEnvironment::read())
}

#[must_use]
pub fn snapshot() -> FocusSnapshot {
    FocusSnapshot::from_observation(&observe())
}

pub(super) struct SessionEnvironment {
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

    pub(super) fn is_niri(&self) -> bool {
        self.niri_socket.is_some()
            || self
                .current_desktop
                .as_deref()
                .is_some_and(|desktop| desktop_name_matches(desktop, "niri"))
    }

    pub(super) fn niri_socket(&self) -> Option<&OsStr> {
        self.niri_socket.as_deref()
    }

    #[cfg(test)]
    fn for_test(current_desktop: Option<&str>, niri_socket: Option<&str>) -> Self {
        Self {
            current_desktop: current_desktop.map(OsString::from),
            niri_socket: niri_socket.map(OsString::from),
        }
    }
}

fn desktop_name_matches(desktop: &std::ffi::OsStr, expected: &str) -> bool {
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

    #[test]
    fn focused_window_labels_are_safe_and_bounded_for_logs() {
        let long_title = "x".repeat(WINDOW_LABEL_LIMIT + 1);
        let window = FocusedWindow::niri(
            NiriInstanceId::new(10, 100),
            1,
            Some("com.example.Editor\nforged"),
            Some(&long_title),
        );

        let rendered = window.to_string();
        assert!(!rendered.contains('\n'));
        assert!(rendered.contains("com.example.Editor forged"));
        assert!(rendered.contains(&format!("{}…", "x".repeat(WINDOW_LABEL_LIMIT))));
    }

    #[test]
    fn focused_window_labels_remove_log_format_controls() {
        let window = FocusedWindow::niri(
            NiriInstanceId::new(10, 100),
            1,
            Some("safe\u{202e}forged\u{2028}break"),
            Some("title\u{2066}hidden\u{2069}\u{2029}paragraph"),
        );

        let rendered = window.to_string();
        assert!(!rendered.contains('\u{2028}'));
        assert!(!rendered.contains('\u{2029}'));
        assert!(!rendered.contains('\u{202e}'));
        assert!(!rendered.contains('\u{2066}'));
        assert!(!rendered.contains('\u{2069}'));
        assert!(rendered.contains("safe forged break"));
        assert!(rendered.contains("title hidden paragraph"));
    }

    #[test]
    fn target_identity_ignores_mutable_window_labels() {
        let stopped = FocusedWindow::test_niri(1, 7, "dev.editor", "first title");
        let current = FocusedWindow::test_niri(1, 7, "dev.editor", "renamed title");
        let other_window = FocusedWindow::test_niri(1, 8, "dev.editor", "first title");
        let other_instance = FocusedWindow::test_niri(2, 7, "dev.editor", "first title");

        assert!(stopped.same_target(&current));
        assert!(!stopped.same_target(&other_window));
        assert!(!stopped.same_target(&other_instance));
    }

    #[test]
    fn focus_snapshot_round_trips_with_sanitized_labels() {
        let snapshot =
            FocusSnapshot::Focused(FocusedWindow::test_niri(1, 7, "dev.editor", "README.md"));

        let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
        let decoded: FocusSnapshot =
            serde_json::from_str(&json).expect("snapshot should deserialize");

        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn probe_messages_escape_controls_and_enforce_the_output_limit() {
        let message = FocusProbeMessage::new(&format!(
            "bad\n\u{1b}[31m\u{202e}\u{2028}\u{2029}{}",
            "x".repeat(300)
        ));
        let rendered = message.to_string();

        assert!(!rendered.contains('\n'));
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{2028}'));
        assert!(!rendered.contains('\u{2029}'));
        assert!(!rendered.contains('\u{202e}'));
        assert!(rendered.contains("\\u{a}"));
        assert!(rendered.contains("\\u{1b}"));
        assert!(rendered.contains("\\u{2028}"));
        assert!(rendered.contains("\\u{2029}"));
        assert!(rendered.contains("\\u{202e}"));
        assert_eq!(rendered.chars().count(), 240);
    }

    #[test]
    fn empty_window_labels_render_as_unnamed() {
        let window = FocusedWindow::niri(NiriInstanceId::new(10, 100), 1, Some(" \n "), Some(""));

        assert_eq!(window.to_string(), "unnamed window (reported by niri)");
    }
}
