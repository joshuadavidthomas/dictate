#![cfg(target_os = "windows")]

mod audio_ducking;
mod clipboard;
mod focus;
mod global_shortcuts;
mod input;

pub use audio_ducking::AudioDucker;
pub use audio_ducking::AudioDuckingError;
pub use audio_ducking::DuckGuard;
use dictate_desktop::ClipboardFailure;
use dictate_desktop::ClipboardPasteBackend;
use dictate_desktop::ClipboardSink;
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

#[must_use]
pub fn observe() -> FocusObservation {
    focus::observe()
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
        clipboard::copy_text(text)
    }
}
