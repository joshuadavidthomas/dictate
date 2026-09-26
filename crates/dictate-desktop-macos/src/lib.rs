#![cfg(target_os = "macos")]

mod audio_ducking;
mod clipboard;
mod focus;
mod global_shortcuts;
mod input;
mod login_item;
mod permissions;

pub use audio_ducking::AudioDucker;
pub use audio_ducking::AudioDuckingError;
pub use audio_ducking::DuckGuard;
use dictate_desktop::ClipboardFailure;
use dictate_desktop::ClipboardPasteBackend;
use dictate_desktop::ClipboardSink;
use dictate_desktop::CompletedInsertion;
use dictate_desktop::DeliveryClipboardFailureKind;
use dictate_desktop::DeliveryReport;
use dictate_desktop::DeliveryTarget;
use dictate_desktop::DirectTyper;
use dictate_desktop::FocusObservation;
use dictate_desktop::FocusSnapshot;
use dictate_desktop::InputSynthesisOutcome;
use dictate_desktop::InsertionBackend;
use dictate_desktop::InsertionOutcome;
use dictate_desktop::InsertionText;
use dictate_desktop::UncertainInsertion;
pub use global_shortcuts::PushToTalkError;
pub use global_shortcuts::PushToTalkEvent;
pub use global_shortcuts::PushToTalkShortcut;
pub use global_shortcuts::listen_push_to_talk;
pub use login_item::LoginItemError;
pub use login_item::LoginItemStatus;
pub use login_item::login_item_status;
pub use login_item::open_login_item_settings;
pub use login_item::set_login_item_enabled;
use objc2_app_kit::NSPasteboard;
use objc2_app_kit::NSPasteboardTypeString;
use objc2_foundation::NSString;
pub use permissions::PermissionKind;
pub use permissions::PermissionState;
pub use permissions::PermissionStatus;
pub use permissions::PermissionStatusError;
pub use permissions::open_permission_settings;
pub use permissions::permission_status;
pub use permissions::request_microphone_permission;

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
    let mut insertion = PlatformInsertionBackend::new();
    let mut clipboard = PlatformClipboardSink;
    dictate_desktop::deliver(target, text, &mut insertion, &mut clipboard)
}

type FallbackInsertionBackend = ClipboardPasteBackend<
    clipboard::PlatformClipboard,
    input::ClipboardPasteChordBackend,
    DirectInputBackend,
>;

struct PlatformInsertionBackend {
    fallback: FallbackInsertionBackend,
}

impl PlatformInsertionBackend {
    fn new() -> Self {
        Self {
            fallback: ClipboardPasteBackend::new(
                clipboard::PlatformClipboard::default(),
                input::ClipboardPasteChordBackend,
                DirectInputBackend,
            ),
        }
    }
}

impl InsertionBackend for PlatformInsertionBackend {
    fn insert(&mut self, text: InsertionText<'_>) -> InsertionOutcome {
        finish_accessibility_insertion(
            input::insert_accessible_text(text),
            text,
            &mut self.fallback,
        )
    }
}

fn finish_accessibility_insertion(
    accessibility: input::AccessibilityInsertionOutcome,
    text: InsertionText<'_>,
    fallback: &mut impl InsertionBackend,
) -> InsertionOutcome {
    match accessibility {
        input::AccessibilityInsertionOutcome::Completed { input_bytes } => {
            InsertionOutcome::Completed(CompletedInsertion::Accessibility { input_bytes })
        }
        input::AccessibilityInsertionOutcome::Unsupported => fallback.insert(text),
        input::AccessibilityInsertionOutcome::DeliveryUncertain {
            maybe_input_bytes,
            failure,
        } => InsertionOutcome::DeliveryUncertain(UncertainInsertion::Accessibility {
            maybe_input_bytes,
            failure,
        }),
    }
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
        let clipboard = NSPasteboard::generalPasteboard();
        clipboard.clearContents();
        let text = NSString::from_str(text);
        // SAFETY: This accesses AppKit's process-lifetime NSPasteboardTypeString constant.
        let data_type = unsafe { NSPasteboardTypeString };
        clipboard
            .setString_forType(&text, data_type)
            .then_some(())
            .ok_or_else(|| {
                ClipboardFailure::new(DeliveryClipboardFailureKind::Platform {
                    operation: "macOS rejected the clipboard write",
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use dictate_desktop::AccessibilityInsertionFailure;
    use dictate_desktop::ClipboardRestoration;

    use super::*;

    struct FakeFallback {
        calls: usize,
        outcome: Option<InsertionOutcome>,
    }

    impl FakeFallback {
        fn new(outcome: InsertionOutcome) -> Self {
            Self {
                calls: 0,
                outcome: Some(outcome),
            }
        }
    }

    impl InsertionBackend for FakeFallback {
        fn insert(&mut self, _text: InsertionText<'_>) -> InsertionOutcome {
            self.calls += 1;
            self.outcome.take().expect("fallback should run once")
        }
    }

    fn text() -> InsertionText<'static> {
        InsertionText::new("hello").expect("fixture should be non-empty")
    }

    fn fallback_outcome() -> InsertionOutcome {
        InsertionOutcome::Completed(CompletedInsertion::ClipboardPaste {
            transcript_bytes: 5,
            restoration: ClipboardRestoration::Restored,
        })
    }

    #[test]
    fn accessibility_completion_does_not_run_fallback() {
        let mut fallback = FakeFallback::new(fallback_outcome());

        let outcome = finish_accessibility_insertion(
            input::AccessibilityInsertionOutcome::Completed { input_bytes: 5 },
            text(),
            &mut fallback,
        );

        assert_eq!(
            outcome,
            InsertionOutcome::Completed(CompletedInsertion::Accessibility { input_bytes: 5 })
        );
        assert_eq!(fallback.calls, 0);
    }

    #[test]
    fn unsupported_accessibility_uses_fallback_once() {
        let mut fallback = FakeFallback::new(fallback_outcome());

        let outcome = finish_accessibility_insertion(
            input::AccessibilityInsertionOutcome::Unsupported,
            text(),
            &mut fallback,
        );

        assert_eq!(outcome, fallback_outcome());
        assert_eq!(fallback.calls, 1);
    }

    #[test]
    fn uncertain_accessibility_never_retries_with_fallback() {
        let mut fallback = FakeFallback::new(fallback_outcome());
        let failure = AccessibilityInsertionFailure::new("setting selected text", -25_204);

        let outcome = finish_accessibility_insertion(
            input::AccessibilityInsertionOutcome::DeliveryUncertain {
                maybe_input_bytes: 5,
                failure: failure.clone(),
            },
            text(),
            &mut fallback,
        );

        assert_eq!(
            outcome,
            InsertionOutcome::DeliveryUncertain(UncertainInsertion::Accessibility {
                maybe_input_bytes: 5,
                failure,
            })
        );
        assert_eq!(fallback.calls, 0);
    }
}
