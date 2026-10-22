//! What the daemon and plugins say to each other beyond transcripts:
//! events the daemon sends, and calls plugins make into the core.

use dictate_desktop::DeliveryTarget;
use dictate_desktop::FocusSnapshot;
use serde::Deserialize;
use serde::Serialize;

/// Something that happened in the daemon, for plugin handlers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PluginEvent {
    Hotkey {
        name: String,
        phase: HotkeyPhase,
        /// The focused window when the key went up, when the core captured
        /// it.
        focus: Option<FocusSnapshot>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, schemars::JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyPhase {
    Down,
    Up,
}

/// A recording a plugin opened or adopted.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct SessionId(pub u64);

/// A call from plugin code into the core.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CoreCall {
    /// Record from the microphone: adopt the recording the dictation hotkey
    /// started, or start one.
    SessionOpen,
    /// End the session and wait for its transcript.
    SessionStop {
        session: SessionId,
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
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
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
