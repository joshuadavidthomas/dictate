//! What the daemon and plugins say to each other beyond transcripts:
//! events the daemon sends, and calls plugins make into the core.

use dictate_desktop::DeliveryTarget;
use dictate_desktop::FocusSnapshot;
use dictate_speech::DictationCommand;
use serde::Deserialize;
use serde::Serialize;

/// Something that happened in the daemon, for plugin handlers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginEvent {
    Hotkey {
        name: String,
        phase: HotkeyPhase,
        /// The focused window when the key went up, when the core captured
        /// it.
        focus: Option<FocusSnapshot>,
    },
    /// `dictate record <command>`.
    Record {
        command: DictationCommand,
        /// `--delivery`, when given.
        target: Option<DeliveryTarget>,
        /// The focused window when the command was sent, when known.
        focus: Option<FocusSnapshot>,
    },
    /// A session's recording ended without `Session:stop`, for example at
    /// the length limit. `Session:stop` returns its transcript.
    SessionEnded { session: SessionId },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, schemars::JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyPhase {
    Down,
    Up,
}

/// A recording a plugin opened.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(pub u64);

/// A call from plugin code into the core.
#[derive(Clone, Debug, PartialEq)]
pub enum CoreCall {
    /// Start recording from the microphone.
    SessionOpen,
    /// End the session and wait for its transcript.
    SessionStop {
        session: SessionId,
        /// The window the text is for; observed now when absent.
        focus: Option<FocusSnapshot>,
    },
    /// End the session and drop its audio.
    SessionCancel {
        session: SessionId,
    },
    DesktopFocus,
    DesktopDeliver {
        text: String,
        target: Option<DeliveryTarget>,
        focus: Option<FocusSnapshot>,
    },
    OverlayHide,
}

/// The core's answer to a [`CoreCall`].
#[derive(Clone, Debug, PartialEq)]
pub enum CoreReply {
    Session {
        session: SessionId,
    },
    Transcript {
        /// The recognizer's text; empty when nothing was heard.
        text: String,
        /// The focused window when the recording stopped.
        focus: FocusSnapshot,
    },
    Focus {
        focus: FocusSnapshot,
    },
    Delivered {
        /// Where the text ended up; the core falls back from `insert` to the
        /// clipboard when focus moved.
        target: DeliveryTarget,
    },
    Done,
}

/// Outcome of a core call: the reply, or why the core refused or failed.
pub type CoreResult = Result<CoreReply, String>;
