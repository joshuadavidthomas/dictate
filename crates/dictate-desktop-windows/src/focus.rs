//! Foreground-window focus observation for Windows.

use std::path::Path;

use dictate_desktop::FocusObservation;
use dictate_desktop::FocusProbeFailure;
use dictate_desktop::FocusProbeFailureKind;
use dictate_desktop::FocusSource;
use dictate_desktop::FocusedWindow;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::OpenProcess;
use windows::Win32::System::Threading::PROCESS_NAME_WIN32;
use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
use windows::Win32::System::Threading::QueryFullProcessImageNameW;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW;
use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::core::PWSTR;

const SOURCE: FocusSource = FocusSource::Windows;
const MAX_PATH_UNITS: usize = 32_768;

pub(crate) fn observe() -> FocusObservation {
    // SAFETY: Reading the foreground window has no preconditions.
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return FocusObservation::NoFocusedWindow { source: SOURCE };
    }

    let mut process_id = 0;
    // SAFETY: `window` is a window handle and `process_id` is valid writable storage.
    let thread_id = unsafe { GetWindowThreadProcessId(window, Some(&raw mut process_id)) };
    if thread_id == 0 || process_id == 0 {
        return FocusObservation::ProbeFailed(FocusProbeFailure::new(
            SOURCE,
            FocusProbeFailureKind::Platform {
                operation: "reading the foreground window's process",
                code: windows::core::Error::from_thread().code().0,
            },
        ));
    }

    let app_id = process_image_name(process_id);
    let title = window_title(window);
    FocusObservation::Focused(FocusedWindow::windows(
        process_id,
        window_handle_identity(window),
        app_id.as_deref(),
        title.as_deref(),
    ))
}

#[allow(clippy::cast_sign_loss)]
fn window_handle_identity(window: HWND) -> u64 {
    // Window handles are 32-bit values even in 64-bit processes, so the address round-trips.
    window.0 as usize as u64
}

/// Reads the cached caption, which `GetWindowTextW` does without messaging the target window.
fn window_title(window: HWND) -> Option<String> {
    // SAFETY: `window` is a window handle; the call only reads its cached caption length.
    let length = unsafe { GetWindowTextLengthW(window) };
    let length = usize::try_from(length).ok().filter(|length| *length > 0)?;
    let mut buffer = vec![0_u16; length + 1];
    // SAFETY: `buffer` has room for the caption and its terminator.
    let copied = unsafe { GetWindowTextW(window, &mut buffer) };
    let copied = usize::try_from(copied).ok().filter(|copied| *copied > 0)?;
    Some(String::from_utf16_lossy(&buffer[..copied.min(length)]))
}

/// Names the focused application by its executable, such as `notepad.exe`.
fn process_image_name(process_id: u32) -> Option<String> {
    // SAFETY: Limited query access is requested for a process identifier; failure is handled.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }
        .ok()
        .map(OwnedHandle)?;
    let mut buffer = vec![0_u16; MAX_PATH_UNITS];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: `buffer` holds `length` writable UTF-16 units and `process` is an open handle.
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    }
    .ok()?;
    let path = String::from_utf16_lossy(&buffer[..usize::try_from(length).ok()?]);
    Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: `OwnedHandle` wraps a handle returned by a successful `OpenProcess` call.
        drop(unsafe { CloseHandle(self.0) });
    }
}
