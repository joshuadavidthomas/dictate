//! Quartz event-tap push-to-talk integration for macOS.

use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::ptr;

use thiserror::Error;

type EventRef = *mut c_void;
type EventTapCallback = unsafe extern "C" fn(
    proxy: *mut c_void,
    event_type: u32,
    event: EventRef,
    user_info: *mut c_void,
) -> EventRef;

const EVENT_KEY_DOWN: u32 = 10;
const EVENT_KEY_UP: u32 = 11;
const EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = u32::MAX - 1;
const EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = u32::MAX;
const KEYBOARD_EVENT_KEYCODE: u32 = 9;
const ANNOTATED_SESSION_EVENT_TAP: u32 = 2;
const HEAD_INSERT_EVENT_TAP: u32 = 0;
const DEFAULT_EVENT_TAP: u32 = 0;
const SHIFT_FLAG: u64 = 1 << 17;
const CONTROL_FLAG: u64 = 1 << 18;
const OPTION_FLAG: u64 = 1 << 19;
const COMMAND_FLAG: u64 = 1 << 20;
const MODIFIER_FLAGS: u64 = SHIFT_FLAG | CONTROL_FLAG | OPTION_FLAG | COMMAND_FLAG;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: EventTapCallback,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetFlags(event: EventRef) -> u64;
    fn CGEventGetIntegerValueField(event: EventRef, field: u32) -> i64;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: *mut c_void,
        order: isize,
    ) -> *mut c_void;
    fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRemoveSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopRun();
    fn CFRunLoopStop(run_loop: *mut c_void);
    fn CFRelease(value: *const c_void);
    static kCFRunLoopCommonModes: *const c_void;
}

/// A parsed macOS press-and-hold shortcut.
#[derive(Clone, Debug)]
pub struct PushToTalkShortcut {
    key_code: u16,
    flags: u64,
}

impl PushToTalkShortcut {
    pub fn new(_app_id: &str, preferred_trigger: Option<&str>) -> Result<Self, PushToTalkError> {
        let trigger = preferred_trigger.ok_or(PushToTalkError::MissingTrigger)?;
        parse_trigger(trigger)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushToTalkEvent {
    Activated,
    Deactivated,
}

#[derive(Debug, Error)]
pub enum PushToTalkError {
    #[error("macOS push-to-talk requires a configured shortcut")]
    MissingTrigger,
    #[error("unsupported macOS shortcut {trigger:?}: {reason}")]
    InvalidTrigger {
        trigger: String,
        reason: &'static str,
    },
    #[error(
        "could not create the macOS keyboard event tap; grant Input Monitoring and Accessibility permissions"
    )]
    EventTapUnavailable,
    #[error("could not create the macOS shortcut run-loop source")]
    RunLoopSourceUnavailable,
    #[error("the macOS shortcut callback panicked")]
    CallbackPanicked,
}

struct EventTapContext<Callback> {
    shortcut: PushToTalkShortcut,
    callback: Callback,
    tap: *mut c_void,
    run_loop: *mut c_void,
    active: bool,
    callback_panicked: bool,
}

pub fn listen_push_to_talk<Callback>(
    shortcut: &PushToTalkShortcut,
    on_event: Callback,
) -> Result<(), PushToTalkError>
where
    Callback: FnMut(PushToTalkEvent),
{
    let mut context = Box::new(EventTapContext {
        shortcut: shortcut.clone(),
        callback: on_event,
        tap: ptr::null_mut(),
        run_loop: ptr::null_mut(),
        active: false,
        callback_panicked: false,
    });
    let event_mask = (1_u64 << EVENT_KEY_DOWN) | (1_u64 << EVENT_KEY_UP);
    // SAFETY: The boxed callback context stays alive until the tap is disabled and released.
    let tap = unsafe {
        CGEventTapCreate(
            ANNOTATED_SESSION_EVENT_TAP,
            HEAD_INSERT_EVENT_TAP,
            DEFAULT_EVENT_TAP,
            event_mask,
            event_callback::<Callback>,
            (&raw mut *context).cast(),
        )
    };
    if tap.is_null() {
        return Err(PushToTalkError::EventTapUnavailable);
    }
    context.tap = tap;
    // SAFETY: `tap` is a valid CFMachPort returned by `CGEventTapCreate`.
    let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap, 0) };
    if source.is_null() {
        // SAFETY: `tap` is valid and no run-loop source references it.
        unsafe { CGEventTapEnable(tap, false) };
        // SAFETY: This balances the create call above after the tap is disabled.
        unsafe { CFRelease(tap.cast_const()) };
        return Err(PushToTalkError::RunLoopSourceUnavailable);
    }
    // SAFETY: This listener owns and runs on the current thread for its entire lifetime.
    let run_loop = unsafe { CFRunLoopGetCurrent() };
    context.run_loop = run_loop;
    // SAFETY: The source and run loop are valid and owned by this listener thread.
    unsafe { CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes) };
    // SAFETY: `tap` remains valid while the run loop dispatches callbacks.
    unsafe { CGEventTapEnable(tap, true) };
    // SAFETY: The source above keeps the callback context reachable while this blocks.
    unsafe { CFRunLoopRun() };
    // SAFETY: Callback dispatch has stopped, so the tap can be disabled before teardown.
    unsafe { CGEventTapEnable(tap, false) };
    // SAFETY: The source was added to this run loop and is still valid.
    unsafe { CFRunLoopRemoveSource(run_loop, source, kCFRunLoopCommonModes) };
    // SAFETY: The source is detached and no longer needed.
    unsafe { CFRelease(source.cast_const()) };
    // SAFETY: The disabled tap has no remaining run-loop source.
    unsafe { CFRelease(tap.cast_const()) };
    if context.callback_panicked {
        Err(PushToTalkError::CallbackPanicked)
    } else {
        Ok(())
    }
}

unsafe extern "C" fn event_callback<Callback: FnMut(PushToTalkEvent)>(
    _proxy: *mut c_void,
    event_type: u32,
    event: EventRef,
    user_info: *mut c_void,
) -> EventRef {
    if user_info.is_null() {
        return event;
    }
    // SAFETY: `user_info` is the boxed context passed to `CGEventTapCreate` above.
    let context = unsafe { &mut *user_info.cast::<EventTapContext<Callback>>() };
    if matches!(
        event_type,
        EVENT_TAP_DISABLED_BY_TIMEOUT | EVENT_TAP_DISABLED_BY_USER_INPUT
    ) {
        if context.active {
            context.active = false;
            call_callback(context, PushToTalkEvent::Deactivated);
        }
        // SAFETY: The callback context retains the valid tap for the run-loop lifetime.
        unsafe { CGEventTapEnable(context.tap, true) };
        return event;
    }
    if event.is_null() {
        return event;
    }

    // SAFETY: CoreGraphics supplied a non-null keyboard event to this callback.
    let raw_key_code = unsafe { CGEventGetIntegerValueField(event, KEYBOARD_EVENT_KEYCODE) };
    let Ok(key_code) = u16::try_from(raw_key_code) else {
        return event;
    };
    if key_code != context.shortcut.key_code {
        return event;
    }
    match event_type {
        EVENT_KEY_DOWN if event_flags(event) & MODIFIER_FLAGS == context.shortcut.flags => {
            if !context.active {
                context.active = true;
                call_callback(context, PushToTalkEvent::Activated);
            }
            ptr::null_mut()
        }
        EVENT_KEY_UP if context.active => {
            context.active = false;
            call_callback(context, PushToTalkEvent::Deactivated);
            ptr::null_mut()
        }
        _ => event,
    }
}

fn call_callback<Callback: FnMut(PushToTalkEvent)>(
    context: &mut EventTapContext<Callback>,
    event: PushToTalkEvent,
) {
    if std::panic::catch_unwind(AssertUnwindSafe(|| (context.callback)(event))).is_err() {
        context.callback_panicked = true;
        if !context.run_loop.is_null() {
            // SAFETY: Core Foundation permits stopping a valid run loop from its callback.
            unsafe { CFRunLoopStop(context.run_loop) };
        }
    }
}

fn event_flags(event: EventRef) -> u64 {
    // SAFETY: The event is non-null and supplied by CoreGraphics to the active callback.
    unsafe { CGEventGetFlags(event) }
}

fn parse_trigger(trigger: &str) -> Result<PushToTalkShortcut, PushToTalkError> {
    let mut rest = trigger.trim();
    let mut flags = 0;
    while let Some(after_open) = rest.strip_prefix('<') {
        let Some(close) = after_open.find('>') else {
            return invalid_trigger(trigger, "a modifier is missing its closing '>'");
        };
        let modifier = &after_open[..close];
        flags |= match modifier.to_ascii_lowercase().as_str() {
            "super" | "command" | "cmd" => COMMAND_FLAG,
            "control" | "ctrl" => CONTROL_FLAG,
            "option" | "alt" => OPTION_FLAG,
            "shift" => SHIFT_FLAG,
            _ => return invalid_trigger(trigger, "the modifier is not recognized"),
        };
        rest = &after_open[close + 1..];
    }
    if flags == 0 {
        return invalid_trigger(trigger, "at least one modifier is required");
    }
    let key = rest.trim();
    let key_code = key_code(key).ok_or_else(|| PushToTalkError::InvalidTrigger {
        trigger: trigger.to_owned(),
        reason: "the key is not supported",
    })?;
    Ok(PushToTalkShortcut { key_code, flags })
}

fn invalid_trigger<T>(trigger: &str, reason: &'static str) -> Result<T, PushToTalkError> {
    Err(PushToTalkError::InvalidTrigger {
        trigger: trigger.to_owned(),
        reason,
    })
}

fn key_code(key: &str) -> Option<u16> {
    Some(match key.to_ascii_lowercase().as_str() {
        "a" => 0,
        "s" => 1,
        "d" => 2,
        "f" => 3,
        "h" => 4,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "b" => 11,
        "q" => 12,
        "w" => 13,
        "e" => 14,
        "r" => 15,
        "y" => 16,
        "t" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "=" => 24,
        "9" => 25,
        "7" => 26,
        "-" => 27,
        "8" => 28,
        "0" => 29,
        "]" => 30,
        "o" => 31,
        "u" => 32,
        "[" => 33,
        "i" => 34,
        "p" => 35,
        "l" => 37,
        "j" => 38,
        "'" => 39,
        "k" => 40,
        ";" => 41,
        "\\" => 42,
        "," => 43,
        "/" => 44,
        "n" => 45,
        "m" => 46,
        "." => 47,
        "space" => 49,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_portal_style_shortcuts_for_macos() {
        assert_eq!(
            parse_trigger("<Super><Shift>d")
                .expect("shortcut should parse")
                .flags,
            COMMAND_FLAG | SHIFT_FLAG
        );
        assert_eq!(
            parse_trigger("<Command>space")
                .expect("shortcut should parse")
                .key_code,
            49
        );
    }

    #[test]
    fn rejects_unmodified_or_unknown_shortcuts() {
        assert!(parse_trigger("d").is_err());
        assert!(parse_trigger("<Hyper>d").is_err());
        assert!(parse_trigger("<Command>f13").is_err());
    }
}
