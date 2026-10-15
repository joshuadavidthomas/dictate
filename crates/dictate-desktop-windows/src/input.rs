//! `SendInput` keyboard synthesis for the paste chord and direct-typing fallback.

use dictate_desktop::ClipboardPasteChord;
use dictate_desktop::InputSynthesisFailure;
use dictate_desktop::InputSynthesisOutcome;
use dictate_desktop::InsertionText;
use dictate_desktop::PasteChordOutcome;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::Input::KeyboardAndMouse::INPUT;
use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0;
use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYBDINPUT;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_EXTENDEDKEY;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_UNICODE;
use windows::Win32::UI::Input::KeyboardAndMouse::MAPVK_VK_TO_VSC;
use windows::Win32::UI::Input::KeyboardAndMouse::MapVirtualKeyW;
use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_LCONTROL;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_LMENU;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_LSHIFT;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_LWIN;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_RCONTROL;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_RMENU;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_RSHIFT;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_RWIN;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_TAB;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_V;

/// Tags injected events so Dictate's own keyboard hook can recognize them.
pub(crate) const SYNTHETIC_EVENT_MARKER: usize = 0x4449_4354;
/// An unassigned virtual key; tapping it stops a lone Alt or Win release from opening a menu.
const MASK_KEY: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);
const MODIFIER_KEYS: [VIRTUAL_KEY; 8] = [
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LMENU,
    VK_RMENU,
    VK_LWIN,
    VK_RWIN,
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ClipboardPasteChordBackend;

impl ClipboardPasteChord for ClipboardPasteChordBackend {
    fn send_clipboard_paste_chord(&mut self) -> PasteChordOutcome {
        let mut events = release_held_modifiers();
        events.extend([
            key_event(VK_LCONTROL, true),
            key_event(VK_V, true),
            key_event(VK_V, false),
            key_event(VK_LCONTROL, false),
        ]);
        match send(&events) {
            SendResult::All => PasteChordOutcome::Sent,
            SendResult::None => PasteChordOutcome::NotSent(InputSynthesisFailure::Platform {
                operation: "injecting the Ctrl+V key sequence",
            }),
            SendResult::Partial => {
                PasteChordOutcome::DeliveryUncertain(InputSynthesisFailure::Platform {
                    operation: "injecting the complete Ctrl+V key sequence",
                })
            }
        }
    }
}

pub(crate) fn type_text(text: InsertionText<'_>) -> InputSynthesisOutcome {
    let mut events = release_held_modifiers();
    events.extend(text_events(text.as_str()));
    match send(&events) {
        SendResult::All => InputSynthesisOutcome::Completed {
            input_bytes: text.as_str().len(),
        },
        SendResult::None => InputSynthesisOutcome::NotStarted(InputSynthesisFailure::Platform {
            operation: "injecting Unicode keyboard events",
        }),
        SendResult::Partial => InputSynthesisOutcome::DeliveryUncertain {
            maybe_input_bytes: text.as_str().len(),
            failure: InputSynthesisFailure::Platform {
                operation: "injecting every Unicode keyboard event",
            },
        },
    }
}

/// Taps the mask key so that releasing a held Alt or Win key is not treated as a lone press.
pub(crate) fn mask_modifier_release() {
    let events = [key_event(MASK_KEY, true), key_event(MASK_KEY, false)];
    if send(&events) != SendResult::All {
        eprintln!("failed to mask the push-to-talk modifier release");
    }
}

/// Builds key-up events for physically held modifiers so they cannot alter injected input.
fn release_held_modifiers() -> Vec<INPUT> {
    let held = MODIFIER_KEYS
        .into_iter()
        .filter(|key| is_key_down(*key))
        .collect::<Vec<_>>();
    let mut events = Vec::with_capacity(held.len() + 2);
    if held
        .iter()
        .any(|key| matches!(*key, VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN))
    {
        events.extend([key_event(MASK_KEY, true), key_event(MASK_KEY, false)]);
    }
    events.extend(held.into_iter().map(|key| key_event(key, false)));
    events
}

pub(crate) fn is_key_down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: Reading asynchronous key state has no preconditions.
    let state = unsafe { GetAsyncKeyState(i32::from(key.0)) };
    state < 0
}

fn text_events(text: &str) -> Vec<INPUT> {
    let mut events = Vec::with_capacity(text.len().saturating_mul(2));
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() != Some(&'\n') {
                    events.extend([key_event(VK_RETURN, true), key_event(VK_RETURN, false)]);
                }
            }
            '\n' => events.extend([key_event(VK_RETURN, true), key_event(VK_RETURN, false)]),
            '\t' => events.extend([key_event(VK_TAB, true), key_event(VK_TAB, false)]),
            character => {
                let mut units = [0_u16; 2];
                for unit in character.encode_utf16(&mut units) {
                    events.extend([unicode_event(*unit, true), unicode_event(*unit, false)]);
                }
            }
        }
    }
    events
}

fn key_event(key: VIRTUAL_KEY, down: bool) -> INPUT {
    // SAFETY: MapVirtualKeyW only translates the key code and has no other preconditions.
    let scan = unsafe { MapVirtualKeyW(u32::from(key.0), MAPVK_VK_TO_VSC) };
    let mut flags = if down {
        KEYBD_EVENT_FLAGS(0)
    } else {
        KEYEVENTF_KEYUP
    };
    if matches!(key, VK_RCONTROL | VK_RMENU | VK_LWIN | VK_RWIN) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    keyboard_input(KEYBDINPUT {
        wVk: key,
        wScan: u16::try_from(scan).unwrap_or(0),
        dwFlags: flags,
        time: 0,
        dwExtraInfo: SYNTHETIC_EVENT_MARKER,
    })
}

fn unicode_event(unit: u16, down: bool) -> INPUT {
    let flags = if down {
        KEYEVENTF_UNICODE
    } else {
        KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
    };
    keyboard_input(KEYBDINPUT {
        wVk: VIRTUAL_KEY(0),
        wScan: unit,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: SYNTHETIC_EVENT_MARKER,
    })
}

const fn keyboard_input(ki: KEYBDINPUT) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SendResult {
    All,
    None,
    Partial,
}

fn send(events: &[INPUT]) -> SendResult {
    if events.is_empty() {
        return SendResult::All;
    }
    let size = i32::try_from(size_of::<INPUT>()).unwrap_or(i32::MAX);
    // SAFETY: `events` is a valid slice of initialized keyboard INPUT records of `size` bytes.
    let sent = unsafe { SendInput(events, size) };
    send_result(sent, events.len())
}

fn send_result(sent: u32, expected: usize) -> SendResult {
    match usize::try_from(sent) {
        Ok(0) => SendResult::None,
        Ok(sent) if sent == expected => SendResult::All,
        Ok(_) | Err(_) => SendResult::Partial,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyboard(event: &INPUT) -> KEYBDINPUT {
        // SAFETY: Every event built by this module is a keyboard INPUT.
        unsafe { event.Anonymous.ki }
    }

    #[test]
    fn text_events_press_and_release_each_utf16_unit() {
        let events = text_events("a👋");
        let units = events
            .iter()
            .map(|event| {
                let key = keyboard(event);
                (key.wScan, key.dwFlags.contains(KEYEVENTF_KEYUP))
            })
            .collect::<Vec<_>>();

        assert_eq!(
            units,
            vec![
                (u16::from(b'a'), false),
                (u16::from(b'a'), true),
                (0xD83D, false),
                (0xD83D, true),
                (0xDC4B, false),
                (0xDC4B, true),
            ]
        );
        assert!(events.iter().all(|event| {
            let key = keyboard(event);
            key.dwFlags.contains(KEYEVENTF_UNICODE) && key.dwExtraInfo == SYNTHETIC_EVENT_MARKER
        }));
    }

    #[test]
    fn line_breaks_become_single_return_presses() {
        let keys = text_events("a\r\nb\nc\rd")
            .iter()
            .map(|event| keyboard(event).wVk)
            .filter(|key| *key == VK_RETURN)
            .count();

        assert_eq!(keys, 6);
    }

    #[test]
    fn send_results_distinguish_none_partial_and_complete_injection() {
        assert_eq!(send_result(0, 4), SendResult::None);
        assert_eq!(send_result(2, 4), SendResult::Partial);
        assert_eq!(send_result(4, 4), SendResult::All);
    }
}
