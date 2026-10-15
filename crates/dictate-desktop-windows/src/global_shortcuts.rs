//! Low-level keyboard-hook push-to-talk integration for Windows.
//!
//! `RegisterHotKey` reports only presses, so press-and-hold needs a `WH_KEYBOARD_LL` hook to see
//! the release as well.

use std::cell::Cell;
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::ptr;

use thiserror::Error;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_LWIN;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_MENU;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_RWIN;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT;
use windows::Win32::UI::WindowsAndMessaging::CallNextHookEx;
use windows::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT;
use windows::Win32::UI::WindowsAndMessaging::LLKHF_INJECTED;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::PostQuitMessage;
use windows::Win32::UI::WindowsAndMessaging::SetWindowsHookExW;
use windows::Win32::UI::WindowsAndMessaging::TranslateMessage;
use windows::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx;
use windows::Win32::UI::WindowsAndMessaging::WH_KEYBOARD_LL;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYUP;
use windows::Win32::UI::WindowsAndMessaging::WM_SYSKEYDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_SYSKEYUP;

use crate::input;

const SHIFT: u8 = 1 << 0;
const CONTROL: u8 = 1 << 1;
const ALT: u8 = 1 << 2;
const WIN: u8 = 1 << 3;
const VK_F1: u16 = 0x70;
const VK_F24: u16 = 0x87;

thread_local! {
    /// The hook context for the listener running on this thread; low-level hooks carry no
    /// user data, so the hook procedure finds its state here.
    static HOOK_CONTEXT: Cell<*mut c_void> = const { Cell::new(ptr::null_mut()) };
}

/// A parsed Windows press-and-hold shortcut.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PushToTalkShortcut {
    key: u16,
    modifiers: u8,
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
    #[error("Windows push-to-talk requires a configured shortcut")]
    MissingTrigger,
    #[error("unsupported Windows shortcut {trigger:?}: {reason}")]
    InvalidTrigger {
        trigger: String,
        reason: &'static str,
    },
    #[error("could not install the Windows keyboard hook (platform error {code})")]
    HookUnavailable { code: i32 },
    #[error("the Windows shortcut message loop failed (platform error {code})")]
    MessageLoopFailed { code: i32 },
    #[error("the Windows shortcut callback panicked")]
    CallbackPanicked,
}

struct HookContext<Callback> {
    shortcut: PushToTalkShortcut,
    callback: Callback,
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
    let mut context = Box::new(HookContext {
        shortcut: shortcut.clone(),
        callback: on_event,
        active: false,
        callback_panicked: false,
    });
    HOOK_CONTEXT.with(|slot| slot.set((&raw mut *context).cast()));
    let result = run_hook_loop::<Callback>();
    HOOK_CONTEXT.with(|slot| slot.set(ptr::null_mut()));
    result?;
    if context.callback_panicked {
        Err(PushToTalkError::CallbackPanicked)
    } else {
        Ok(())
    }
}

fn run_hook_loop<Callback: FnMut(PushToTalkEvent)>() -> Result<(), PushToTalkError> {
    // SAFETY: A null module name returns this executable's handle, which outlives the hook.
    let module =
        unsafe { GetModuleHandleW(None) }.map_err(|error| PushToTalkError::HookUnavailable {
            code: error.code().0,
        })?;
    // SAFETY: The hook procedure reads its context from this thread's slot, which stays set
    // until the hook is removed below.
    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook::<Callback>),
            Some(module.into()),
            0,
        )
    }
    .map_err(|error| PushToTalkError::HookUnavailable {
        code: error.code().0,
    })?;

    let mut message = MSG::default();
    let result = loop {
        // SAFETY: `message` is valid writable storage; low-level hooks are dispatched while
        // this thread waits here.
        let status = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        match status.0 {
            0 => break Ok(()),
            -1 => {
                break Err(PushToTalkError::MessageLoopFailed {
                    code: windows::core::Error::from_thread().code().0,
                });
            }
            _ => {
                // SAFETY: `message` was filled in by the successful GetMessageW call above.
                let _translated = unsafe { TranslateMessage(&raw const message) };
                // SAFETY: `message` was filled in by the successful GetMessageW call above.
                unsafe { DispatchMessageW(&raw const message) };
            }
        }
    };

    // SAFETY: `hook` was installed above and has not been removed.
    if let Err(error) = unsafe { UnhookWindowsHookEx(hook) } {
        eprintln!("failed to remove the Windows keyboard hook: {error}");
    }
    result
}

unsafe extern "system" fn keyboard_hook<Callback: FnMut(PushToTalkEvent)>(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 && handle_key_event::<Callback>(wparam, lparam) {
        return LRESULT(1);
    }
    // SAFETY: Forwarding the unmodified hook arguments is always valid.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Returns `true` when the event belongs to the shortcut and must not reach other applications.
fn handle_key_event<Callback: FnMut(PushToTalkEvent)>(wparam: WPARAM, lparam: LPARAM) -> bool {
    let context = HOOK_CONTEXT.with(Cell::get).cast::<HookContext<Callback>>();
    if context.is_null() || lparam.0 == 0 {
        return false;
    }
    // SAFETY: For `HC_ACTION`, a low-level keyboard hook's `lparam` points to a KBDLLHOOKSTRUCT.
    let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    // SAFETY: The slot points at the listener's boxed context, which outlives the hook.
    let context = unsafe { &mut *context };
    if event.flags.contains(LLKHF_INJECTED) || event.dwExtraInfo == input::SYNTHETIC_EVENT_MARKER {
        return false;
    }
    if u16::try_from(event.vkCode).ok() != Some(context.shortcut.key) {
        return false;
    }

    let transition = u32::try_from(wparam.0).ok();
    match transition {
        Some(WM_KEYDOWN | WM_SYSKEYDOWN) => {
            if context.active {
                return true;
            }
            let held = held_modifiers();
            if held != context.shortcut.modifiers {
                return false;
            }
            context.active = true;
            if held & (ALT | WIN) != 0 {
                input::mask_modifier_release();
            }
            call_callback(context, PushToTalkEvent::Activated);
            true
        }
        Some(WM_KEYUP | WM_SYSKEYUP) if context.active => {
            context.active = false;
            call_callback(context, PushToTalkEvent::Deactivated);
            true
        }
        Some(_) | None => false,
    }
}

fn call_callback<Callback: FnMut(PushToTalkEvent)>(
    context: &mut HookContext<Callback>,
    event: PushToTalkEvent,
) {
    if std::panic::catch_unwind(AssertUnwindSafe(|| (context.callback)(event))).is_err() {
        context.callback_panicked = true;
        // SAFETY: The hook runs on the listener thread, so this ends that thread's message loop.
        unsafe { PostQuitMessage(0) };
    }
}

fn held_modifiers() -> u8 {
    let mut modifiers = 0;
    if input::is_key_down(VK_SHIFT) {
        modifiers |= SHIFT;
    }
    if input::is_key_down(VK_CONTROL) {
        modifiers |= CONTROL;
    }
    if input::is_key_down(VK_MENU) {
        modifiers |= ALT;
    }
    if input::is_key_down(VK_LWIN) || input::is_key_down(VK_RWIN) {
        modifiers |= WIN;
    }
    modifiers
}

fn parse_trigger(trigger: &str) -> Result<PushToTalkShortcut, PushToTalkError> {
    let mut rest = trigger.trim();
    let mut modifiers = 0;
    while let Some(after_open) = rest.strip_prefix('<') {
        let Some(close) = after_open.find('>') else {
            return invalid_trigger(trigger, "a modifier is missing its closing '>'");
        };
        modifiers |= match after_open[..close].to_ascii_lowercase().as_str() {
            "super" | "win" | "windows" | "meta" => WIN,
            "control" | "ctrl" | "primary" => CONTROL,
            "alt" | "option" => ALT,
            "shift" => SHIFT,
            _ => return invalid_trigger(trigger, "the modifier is not recognized"),
        };
        rest = &after_open[close + 1..];
    }
    let key = virtual_key(rest.trim()).ok_or_else(|| PushToTalkError::InvalidTrigger {
        trigger: trigger.to_owned(),
        reason: "the key is not supported",
    })?;
    if modifiers == 0 && !(VK_F1..=VK_F24).contains(&key.0) {
        return invalid_trigger(
            trigger,
            "at least one modifier is required unless the key is F1-F24",
        );
    }
    Ok(PushToTalkShortcut {
        key: key.0,
        modifiers,
    })
}

fn invalid_trigger<T>(trigger: &str, reason: &'static str) -> Result<T, PushToTalkError> {
    Err(PushToTalkError::InvalidTrigger {
        trigger: trigger.to_owned(),
        reason,
    })
}

/// Maps key names to US-layout virtual keys, matching the macOS parser's key set.
fn virtual_key(key: &str) -> Option<VIRTUAL_KEY> {
    let key = key.to_ascii_lowercase();
    if let [character] = key.as_bytes()
        && (character.is_ascii_lowercase() || character.is_ascii_digit())
    {
        return Some(VIRTUAL_KEY(u16::from(character.to_ascii_uppercase())));
    }
    if let Some(number) = key
        .strip_prefix('f')
        .and_then(|number| number.parse::<u16>().ok())
        && (1..=24).contains(&number)
    {
        return Some(VIRTUAL_KEY(VK_F1 + number - 1));
    }
    let code = match key.as_str() {
        "space" => 0x20,
        ";" => 0xBA,
        "=" => 0xBB,
        "," => 0xBC,
        "-" => 0xBD,
        "." => 0xBE,
        "/" => 0xBF,
        "`" | "grave" => 0xC0,
        "[" => 0xDB,
        "\\" => 0xDC,
        "]" => 0xDD,
        "'" => 0xDE,
        _ => return None,
    };
    Some(VIRTUAL_KEY(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_portal_style_shortcuts_for_windows() {
        assert_eq!(
            parse_trigger("<Super><Shift>d").expect("shortcut should parse"),
            PushToTalkShortcut {
                key: u16::from(b'D'),
                modifiers: WIN | SHIFT,
            }
        );
        assert_eq!(
            parse_trigger("<Control>space").expect("shortcut should parse"),
            PushToTalkShortcut {
                key: 0x20,
                modifiers: CONTROL,
            }
        );
        assert_eq!(
            parse_trigger("<Ctrl><Alt>7").expect("shortcut should parse"),
            PushToTalkShortcut {
                key: u16::from(b'7'),
                modifiers: CONTROL | ALT,
            }
        );
    }

    #[test]
    fn function_keys_may_be_used_without_modifiers() {
        assert_eq!(
            parse_trigger("F13").expect("shortcut should parse"),
            PushToTalkShortcut {
                key: 0x7C,
                modifiers: 0,
            }
        );
        assert_eq!(
            parse_trigger("<Shift>f24")
                .expect("shortcut should parse")
                .key,
            VK_F24
        );
    }

    #[test]
    fn rejects_unmodified_or_unknown_shortcuts() {
        assert!(parse_trigger("d").is_err());
        assert!(parse_trigger("<Hyper>d").is_err());
        assert!(parse_trigger("<Control>f25").is_err());
        assert!(parse_trigger("<Control").is_err());
        assert!(PushToTalkShortcut::new("app", None).is_err());
    }
}
