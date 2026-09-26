use std::ffi::c_void;
use std::ptr;

use dictate_desktop::AccessibilityInsertionFailure;
use dictate_desktop::ClipboardPasteChord;
use dictate_desktop::InputSynthesisFailure as WtypeFailure;
use dictate_desktop::InputSynthesisOutcome as WtypeOutcome;
use dictate_desktop::InsertionText;
use dictate_desktop::PasteChordOutcome as ClipboardPasteChordOutcome;
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;

type EventRef = *mut c_void;
type CfTypeRef = *const c_void;
type CfStringRef = *const c_void;
type AxUiElementRef = *const c_void;

const HID_EVENT_TAP: u32 = 0;
const COMMAND_FLAG: u64 = 1 << 20;
const COMMAND_KEY_CODE: u16 = 55;
const V_KEY_CODE: u16 = 9;
const EVENT_SOURCE_USER_DATA: u32 = 42;
const SYNTHETIC_EVENT_MARKER: i64 = 0x0044_4943_5441_5445;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const AX_MESSAGING_TIMEOUT_SECONDS: f32 = 0.25;

struct OwnedCf(CfTypeRef);

impl OwnedCf {
    fn new(value: CfTypeRef) -> Option<Self> {
        (!value.is_null()).then_some(Self(value))
    }

    fn as_ptr(&self) -> CfTypeRef {
        self.0
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        // SAFETY: `OwnedCf` wraps a retained value returned by a Core Foundation create/copy API.
        unsafe { CFRelease(self.0) };
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum AccessibilityInsertionOutcome {
    Completed {
        input_bytes: usize,
    },
    Unsupported,
    DeliveryUncertain {
        maybe_input_bytes: usize,
        failure: AccessibilityInsertionFailure,
    },
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AxUiElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AxUiElementRef,
        attribute: CfStringRef,
        value: *mut CfTypeRef,
    ) -> i32;
    fn AXUIElementIsAttributeSettable(
        element: AxUiElementRef,
        attribute: CfStringRef,
        settable: *mut u8,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        element: AxUiElementRef,
        attribute: CfStringRef,
        value: CfTypeRef,
    ) -> i32;
    fn AXUIElementSetMessagingTimeout(element: AxUiElementRef, timeout_in_seconds: f32) -> i32;
    fn CGEventCreateKeyboardEvent(source: *mut c_void, key: u16, down: bool) -> EventRef;
    fn CGEventKeyboardSetUnicodeString(event: EventRef, length: usize, text: *const u16);
    fn CGEventSetFlags(event: EventRef, flags: u64);
    fn CGEventSetIntegerValueField(event: EventRef, field: u32, value: i64);
    fn CGEventPost(tap: u32, event: EventRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        is_external_representation: u8,
    ) -> CfStringRef;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        string: *const std::ffi::c_char,
        encoding: u32,
    ) -> CfStringRef;
    fn CFRelease(value: *const c_void);
}

pub(crate) fn insert_accessible_text(text: InsertionText<'_>) -> AccessibilityInsertionOutcome {
    autoreleasepool(|_| {
        let Some(application) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
            return AccessibilityInsertionOutcome::Unsupported;
        };
        // SAFETY: The process identifier comes from the current NSRunningApplication.
        let application = unsafe { AXUIElementCreateApplication(application.processIdentifier()) };
        let Some(application) = OwnedCf::new(application) else {
            return AccessibilityInsertionOutcome::Unsupported;
        };
        // SAFETY: `application` is a valid AX object created immediately above.
        if unsafe {
            AXUIElementSetMessagingTimeout(application.as_ptr(), AX_MESSAGING_TIMEOUT_SECONDS)
        } != 0
        {
            return AccessibilityInsertionOutcome::Unsupported;
        }

        let Ok(focused_element) = copy_attribute(application.as_ptr(), c"AXFocusedUIElement")
        else {
            return AccessibilityInsertionOutcome::Unsupported;
        };
        let Some(selected_text_attribute) = cf_string(c"AXSelectedText") else {
            return AccessibilityInsertionOutcome::Unsupported;
        };
        let mut settable = 0_u8;
        // SAFETY: The AX element and attribute are valid retained Core Foundation objects.
        let settable_status = unsafe {
            AXUIElementIsAttributeSettable(
                focused_element.as_ptr(),
                selected_text_attribute.as_ptr(),
                &raw mut settable,
            )
        };
        if settable_status != 0 || settable == 0 {
            return AccessibilityInsertionOutcome::Unsupported;
        }

        let Some(value) = cf_string_from_bytes(text.as_str().as_bytes()) else {
            return AccessibilityInsertionOutcome::Unsupported;
        };
        // SAFETY: The focused element, attribute, and UTF-8-backed CFString are valid objects.
        let status = unsafe {
            AXUIElementSetAttributeValue(
                focused_element.as_ptr(),
                selected_text_attribute.as_ptr(),
                value.as_ptr(),
            )
        };
        accessibility_set_outcome(status, text.as_str().len())
    })
}

fn accessibility_set_outcome(status: i32, input_bytes: usize) -> AccessibilityInsertionOutcome {
    if status == 0 {
        AccessibilityInsertionOutcome::Completed { input_bytes }
    } else {
        AccessibilityInsertionOutcome::DeliveryUncertain {
            maybe_input_bytes: input_bytes,
            failure: AccessibilityInsertionFailure::new(
                "setting the focused control's selected text",
                status,
            ),
        }
    }
}

fn copy_attribute(element: AxUiElementRef, name: &std::ffi::CStr) -> Result<OwnedCf, i32> {
    let Some(attribute) = cf_string(name) else {
        return Err(-1);
    };
    let mut value = ptr::null();
    // SAFETY: Both CF references are valid and `value` is an initialized out pointer.
    let status =
        unsafe { AXUIElementCopyAttributeValue(element, attribute.as_ptr(), &raw mut value) };
    if status == 0 {
        OwnedCf::new(value).ok_or(-1)
    } else {
        Err(status)
    }
}

fn cf_string(value: &std::ffi::CStr) -> Option<OwnedCf> {
    // SAFETY: `value` is a valid NUL-terminated string and the encoding constant is valid.
    OwnedCf::new(unsafe {
        CFStringCreateWithCString(ptr::null(), value.as_ptr(), CF_STRING_ENCODING_UTF8)
    })
}

fn cf_string_from_bytes(value: &[u8]) -> Option<OwnedCf> {
    let length = isize::try_from(value.len()).ok()?;
    // SAFETY: `value` provides `length` readable bytes and the encoding constant is valid.
    OwnedCf::new(unsafe {
        CFStringCreateWithBytes(
            ptr::null(),
            value.as_ptr(),
            length,
            CF_STRING_ENCODING_UTF8,
            0,
        )
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ClipboardPasteChordBackend;

impl ClipboardPasteChord for ClipboardPasteChordBackend {
    fn send_clipboard_paste_chord(&mut self) -> ClipboardPasteChordOutcome {
        let specs = [
            (COMMAND_KEY_CODE, true, COMMAND_FLAG),
            (V_KEY_CODE, true, COMMAND_FLAG),
            (V_KEY_CODE, false, COMMAND_FLAG),
            (COMMAND_KEY_CODE, false, 0),
        ];
        let Some(events) = create_events(specs) else {
            return ClipboardPasteChordOutcome::NotSent(WtypeFailure::Platform {
                operation: "creating the Command-V event sequence",
            });
        };
        post_events(events);
        ClipboardPasteChordOutcome::DeliveryUncertain(WtypeFailure::Platform {
            operation: "confirming delivery of the Command-V event sequence",
        })
    }
}

pub(super) fn type_text(text: InsertionText<'_>) -> WtypeOutcome {
    let mut events = Vec::with_capacity(text.as_str().chars().count().saturating_mul(2));
    for character in text.as_str().chars() {
        let mut encoded = [0_u16; 2];
        let encoded = character.encode_utf16(&mut encoded);
        for down in [true, false] {
            let Some(event) = keyboard_event(0, down, 0) else {
                release_events(events);
                return WtypeOutcome::NotStarted(WtypeFailure::Platform {
                    operation: "creating Unicode keyboard events",
                });
            };
            // SAFETY: `event` is valid and CoreGraphics copies the UTF-16 slice synchronously.
            unsafe { CGEventKeyboardSetUnicodeString(event, encoded.len(), encoded.as_ptr()) };
            events.push(event);
        }
    }
    post_events(events);
    WtypeOutcome::DeliveryUncertain {
        maybe_input_bytes: text.as_str().len(),
        failure: WtypeFailure::Platform {
            operation: "confirming delivery of Unicode keyboard events",
        },
    }
}

fn create_events<const N: usize>(specs: [(u16, bool, u64); N]) -> Option<Vec<EventRef>> {
    let mut events = Vec::with_capacity(N);
    for (key, down, flags) in specs {
        let Some(event) = keyboard_event(key, down, flags) else {
            release_events(events);
            return None;
        };
        events.push(event);
    }
    Some(events)
}

fn keyboard_event(key: u16, down: bool, flags: u64) -> Option<EventRef> {
    // SAFETY: A null event source requests CoreGraphics' default source; key and flags are valid.
    let event = unsafe { CGEventCreateKeyboardEvent(std::ptr::null_mut(), key, down) };
    if event.is_null() {
        return None;
    }
    // SAFETY: `event` was successfully created and remains owned by this function.
    unsafe { CGEventSetFlags(event, flags) };
    // SAFETY: `event` is valid and the user-data field accepts an arbitrary marker.
    unsafe { CGEventSetIntegerValueField(event, EVENT_SOURCE_USER_DATA, SYNTHETIC_EVENT_MARKER) };
    Some(event)
}

fn post_events(events: Vec<EventRef>) {
    for event in events {
        // SAFETY: Every event in the vector was successfully created and is still valid.
        unsafe { CGEventPost(HID_EVENT_TAP, event) };
        // SAFETY: Posting does not consume the event, so this balances its create call.
        unsafe { CFRelease(event.cast_const()) };
    }
}

fn release_events(events: Vec<EventRef>) {
    for event in events {
        // SAFETY: Every event in the vector is an owned Core Foundation object.
        unsafe { CFRelease(event.cast_const()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_accessibility_set_is_confirmed() {
        assert_eq!(
            accessibility_set_outcome(0, 5),
            AccessibilityInsertionOutcome::Completed { input_bytes: 5 }
        );
    }

    #[test]
    fn failed_accessibility_set_is_uncertain_after_attempt() {
        let failure = AccessibilityInsertionFailure::new(
            "setting the focused control's selected text",
            -25_204,
        );

        assert_eq!(
            accessibility_set_outcome(-25_204, 5),
            AccessibilityInsertionOutcome::DeliveryUncertain {
                maybe_input_bytes: 5,
                failure,
            }
        );
    }
}
