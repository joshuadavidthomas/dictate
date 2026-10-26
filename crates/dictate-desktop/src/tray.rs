//! The tray (menu bar) icon: what it shows and offers in each dictation
//! state. Platform crates only draw this menu and report clicks.

/// What the tray icon shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrayStatus {
    #[default]
    Idle,
    Recording,
    Transcribing,
}

impl TrayStatus {
    #[must_use]
    pub const fn tooltip(self) -> &'static str {
        match self {
            Self::Idle => "Ready to dictate",
            Self::Recording => "Recording",
            Self::Transcribing => "Transcribing",
        }
    }

    /// The tray menu, top to bottom; platforms put a separator before Quit.
    #[must_use]
    pub const fn menu(self) -> [TrayMenuItem; 4] {
        let toggle = match self {
            Self::Idle => TrayMenuItem::new(TrayCommand::ToggleRecording, "Start Dictation"),
            Self::Recording => TrayMenuItem::new(TrayCommand::ToggleRecording, "Stop Dictation"),
            Self::Transcribing => TrayMenuItem {
                enabled: false,
                ..TrayMenuItem::new(TrayCommand::ToggleRecording, "Transcribing…")
            },
        };
        [
            TrayMenuItem::new(TrayCommand::OpenWindow, "Open Dictate"),
            toggle,
            TrayMenuItem::new(TrayCommand::OpenSettings, "Settings…"),
            TrayMenuItem::new(TrayCommand::Quit, "Quit Dictate"),
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrayMenuItem {
    pub command: TrayCommand,
    pub label: &'static str,
    pub enabled: bool,
}

impl TrayMenuItem {
    const fn new(command: TrayCommand, label: &'static str) -> Self {
        Self {
            command,
            label,
            enabled: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayCommand {
    OpenWindow,
    OpenSettings,
    ToggleRecording,
    Quit,
}

/// A click on the tray.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayEvent {
    pub command: TrayCommand,
    /// Lets a Wayland compositor focus the window the click opens.
    pub activation_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toggle(status: TrayStatus) -> TrayMenuItem {
        status.menu()[1]
    }

    #[test]
    fn toggle_follows_the_dictation_state() {
        assert_eq!(toggle(TrayStatus::Idle).label, "Start Dictation");
        assert_eq!(toggle(TrayStatus::Recording).label, "Stop Dictation");
        assert!(toggle(TrayStatus::Recording).enabled);
        // A toggle while transcribing would start a new recording before the text lands.
        assert!(!toggle(TrayStatus::Transcribing).enabled);
    }
}
