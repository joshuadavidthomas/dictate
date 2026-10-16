//! Win32 clipboard transactions for the temporary-paste insertion path.

use std::thread;
use std::time::Duration;

use dictate_desktop::ClipboardFailure;
use dictate_desktop::ClipboardFailureKind;
use dictate_desktop::ClipboardOperation;
use dictate_desktop::ClipboardTransactionFailure;
use dictate_desktop::ClipboardTransport;
use dictate_desktop::DeliveryClipboardFailureKind;
use dictate_desktop::TemporaryOwnership;
use dictate_desktop::TransactionMarker;
use windows::Win32::Foundation::GlobalFree;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::CloseClipboard;
use windows::Win32::System::DataExchange::EmptyClipboard;
use windows::Win32::System::DataExchange::EnumClipboardFormats;
use windows::Win32::System::DataExchange::GetClipboardData;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::System::DataExchange::OpenClipboard;
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::DataExchange::SetClipboardData;
use windows::Win32::System::Memory::GMEM_MOVEABLE;
use windows::Win32::System::Memory::GlobalAlloc;
use windows::Win32::System::Memory::GlobalLock;
use windows::Win32::System::Memory::GlobalSize;
use windows::Win32::System::Memory::GlobalUnlock;
use windows::Win32::UI::WindowsAndMessaging::CreateWindowExW;
use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
use windows::Win32::UI::WindowsAndMessaging::HWND_MESSAGE;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE;
use windows::core::HSTRING;
use windows::core::w;

/// Shares the Linux MIME name so every platform marks its temporary offer the same way.
const MARKER_FORMAT: &str = "application/x-dictate-clipboard-transaction";
/// Asks clipboard history, cloud sync, and monitoring tools to ignore the temporary offer.
const EXCLUDE_FROM_MONITORING_FORMAT: &str = "ExcludeClipboardContentFromMonitorProcessing";
const HISTORY_FORMAT: &str = "CanIncludeInClipboardHistory";
const CLOUD_UPLOAD_FORMAT: &str = "CanUploadToCloudClipboard";

const CF_BITMAP: u32 = 2;
const CF_METAFILEPICT: u32 = 3;
const CF_PALETTE: u32 = 9;
const CF_UNICODETEXT: u32 = 13;
const CF_ENHMETAFILE: u32 = 14;
const CF_OWNERDISPLAY: u32 = 0x0080;
const CF_DSPBITMAP: u32 = 0x0082;
const CF_DSPMETAFILEPICT: u32 = 0x0083;
const CF_DSPENHMETAFILE: u32 = 0x008E;
const CF_PRIVATEFIRST: u32 = 0x0200;
const CF_GDIOBJLAST: u32 = 0x03FF;

const MAX_FORMATS: usize = 64;
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const OPEN_ATTEMPTS: usize = 10;
const OPEN_RETRY_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardSnapshot {
    sequence: u32,
    formats: Vec<ClipboardFormat>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClipboardFormat {
    id: u32,
    data: Vec<u8>,
}

#[derive(Debug, Default)]
pub(super) struct PlatformClipboard {
    published_sequence: Option<u32>,
}

impl ClipboardTransport for PlatformClipboard {
    type Snapshot = ClipboardSnapshot;

    fn snapshot(&mut self) -> Result<Self::Snapshot, ClipboardTransactionFailure> {
        let operation = ClipboardOperation::ReadSnapshot;
        let sequence = clipboard_sequence();
        let session = ClipboardSession::open(operation)?;
        let formats = session.capture_formats()?;
        drop(session);
        if clipboard_sequence() != sequence {
            return Err(ClipboardTransactionFailure::ChangedDuringSnapshot);
        }
        Ok(ClipboardSnapshot { sequence, formats })
    }

    fn snapshot_is_current(
        &mut self,
        snapshot: &Self::Snapshot,
    ) -> Result<bool, ClipboardTransactionFailure> {
        Ok(clipboard_sequence() == snapshot.sequence)
    }

    fn publish(
        &mut self,
        text: &str,
        marker: &TransactionMarker,
    ) -> Result<(), ClipboardTransactionFailure> {
        let operation = ClipboardOperation::PublishTranscript;
        let marker_format = registered_format(MARKER_FORMAT, operation)?;
        let private_formats = [
            (
                registered_format(EXCLUDE_FROM_MONITORING_FORMAT, operation)?,
                0_u32,
            ),
            (registered_format(HISTORY_FORMAT, operation)?, 0_u32),
            (registered_format(CLOUD_UPLOAD_FORMAT, operation)?, 0_u32),
        ];

        let session = ClipboardSession::open(operation)?;
        session.clear(operation)?;
        session.put(CF_UNICODETEXT, &unicode_text_bytes(text), operation)?;
        session.put(marker_format, marker.as_bytes(), operation)?;
        for (format, value) in private_formats {
            session.put(format, &value.to_le_bytes(), operation)?;
        }
        drop(session);
        self.published_sequence = Some(clipboard_sequence());
        Ok(())
    }

    fn temporary_ownership(
        &mut self,
        text: &str,
        marker: &TransactionMarker,
    ) -> Result<TemporaryOwnership, ClipboardTransactionFailure> {
        if self.published_sequence != Some(clipboard_sequence()) {
            return Ok(TemporaryOwnership::Changed);
        }
        let marker_format = registered_format(MARKER_FORMAT, ClipboardOperation::VerifyMarker)?;
        let session = ClipboardSession::open(ClipboardOperation::VerifyTranscript)?;
        let marker_matches = session
            .read(marker_format)
            .is_some_and(|contents| contents == marker.as_bytes());
        let transcript_matches = session
            .read(CF_UNICODETEXT)
            .is_some_and(|contents| decode_unicode_text(&contents) == text);
        Ok(ownership(marker_matches, transcript_matches))
    }

    fn restore(&mut self, snapshot: Self::Snapshot) -> Result<(), ClipboardTransactionFailure> {
        let operation = ClipboardOperation::RestoreSnapshot;
        let session = ClipboardSession::open(operation)?;
        session.clear(operation)?;
        for format in &snapshot.formats {
            session.put(format.id, &format.data, operation)?;
        }
        drop(session);
        self.published_sequence = None;
        Ok(())
    }
}

/// Replaces the clipboard with plain text for the clipboard delivery target.
pub(super) fn copy_text(text: &str) -> Result<(), ClipboardFailure> {
    let operation = ClipboardOperation::PublishTranscript;
    let session =
        ClipboardSession::open(operation).map_err(|failure| delivery_failure(&failure))?;
    session
        .clear(operation)
        .map_err(|failure| delivery_failure(&failure))?;
    session
        .put(CF_UNICODETEXT, &unicode_text_bytes(text), operation)
        .map_err(|failure| delivery_failure(&failure))
}

fn delivery_failure(failure: &ClipboardTransactionFailure) -> ClipboardFailure {
    let kind = match failure {
        ClipboardTransactionFailure::Access {
            kind: ClipboardFailureKind::Platform { operation, .. },
            ..
        } => DeliveryClipboardFailureKind::Platform { operation },
        ClipboardTransactionFailure::Access { .. }
        | ClipboardTransactionFailure::TooManyMimeTypes { .. }
        | ClipboardTransactionFailure::MimeMetadataTooLarge { .. }
        | ClipboardTransactionFailure::SnapshotTooLarge { .. }
        | ClipboardTransactionFailure::TransferTimedOut { .. }
        | ClipboardTransactionFailure::ChangedDuringSnapshot
        | ClipboardTransactionFailure::AdvertisedMimeUnavailable => {
            DeliveryClipboardFailureKind::Unavailable
        }
    };
    ClipboardFailure::new(kind)
}

const fn ownership(marker_matches: bool, transcript_matches: bool) -> TemporaryOwnership {
    match (marker_matches, transcript_matches) {
        (true, true) => TemporaryOwnership::Marker,
        (false, true) => TemporaryOwnership::Transcript,
        (true | false, false) => TemporaryOwnership::Changed,
    }
}

/// An open clipboard owned by a hidden message-only window.
///
/// `SetClipboardData` requires the clipboard to be opened by a window, so each session creates
/// one on the calling thread and destroys it after closing the clipboard.
struct ClipboardSession {
    owner: HWND,
}

// Clipboard calls act on the process-wide open clipboard; the receiver proves it is open.
#[allow(clippy::unused_self)]
impl ClipboardSession {
    fn open(operation: ClipboardOperation) -> Result<Self, ClipboardTransactionFailure> {
        // SAFETY: The predefined STATIC class needs no registration, and HWND_MESSAGE creates a
        // message-only window that this session destroys in `Drop`.
        let owner = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(|error| platform_failure(operation, "creating the clipboard owner", &error))?;

        let mut last_error = None;
        for attempt in 0..OPEN_ATTEMPTS {
            if attempt > 0 {
                thread::sleep(OPEN_RETRY_INTERVAL);
            }
            // SAFETY: `owner` is a live window created on this thread above.
            match unsafe { OpenClipboard(Some(owner)) } {
                Ok(()) => return Ok(Self { owner }),
                Err(error) => last_error = Some(error),
            }
        }

        destroy_owner(owner);
        Err(last_error.map_or(
            ClipboardTransactionFailure::Access {
                operation,
                kind: ClipboardFailureKind::Unavailable,
            },
            |error| platform_failure(operation, "opening the clipboard", &error),
        ))
    }

    fn clear(&self, operation: ClipboardOperation) -> Result<(), ClipboardTransactionFailure> {
        // SAFETY: This session holds the clipboard open for `self.owner`.
        unsafe { EmptyClipboard() }
            .map_err(|error| platform_failure(operation, "clearing the clipboard", &error))
    }

    fn capture_formats(&self) -> Result<Vec<ClipboardFormat>, ClipboardTransactionFailure> {
        let mut formats = Vec::new();
        let mut total_bytes = 0_usize;
        let mut format = 0;
        loop {
            // SAFETY: This session holds the clipboard open; zero starts the enumeration and each
            // later call continues from the previously returned format.
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            if !is_global_memory_format(format) {
                continue;
            }
            let Some(data) = self.read(format) else {
                continue;
            };
            total_bytes = total_bytes.saturating_add(data.len());
            check_snapshot_limits(formats.len() + 1, total_bytes)?;
            formats.push(ClipboardFormat { id: format, data });
        }
        Ok(formats)
    }

    /// Copies one format's global-memory payload, or `None` when it is absent or not readable.
    fn read(&self, format: u32) -> Option<Vec<u8>> {
        // SAFETY: This session holds the clipboard open, so the returned handle stays valid
        // until the clipboard closes.
        let handle = unsafe { GetClipboardData(format) }.ok()?;
        let memory = HGLOBAL(handle.0);
        // SAFETY: Clipboard formats filtered by `is_global_memory_format` carry HGLOBAL data.
        let size = unsafe { GlobalSize(memory) };
        // SAFETY: Locking a clipboard-owned HGLOBAL is permitted while the clipboard is open.
        let bytes = unsafe { GlobalLock(memory) };
        if bytes.is_null() {
            return None;
        }
        // SAFETY: `GlobalLock` returned a pointer to at least `GlobalSize` readable bytes.
        let data = unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), size) }.to_vec();
        // SAFETY: This balances the successful `GlobalLock` above.
        drop(unsafe { GlobalUnlock(memory) });
        Some(data)
    }

    fn put(
        &self,
        format: u32,
        data: &[u8],
        operation: ClipboardOperation,
    ) -> Result<(), ClipboardTransactionFailure> {
        // SAFETY: GlobalAlloc has no preconditions; a one-byte minimum keeps empty payloads valid.
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, data.len().max(1)) }
            .map_err(|error| platform_failure(operation, "allocating clipboard memory", &error))?;
        // SAFETY: `memory` is a movable allocation owned by this function.
        let bytes = unsafe { GlobalLock(memory) };
        if bytes.is_null() {
            free_global(memory);
            return Err(platform_failure(
                operation,
                "locking clipboard memory",
                &windows::core::Error::from_thread(),
            ));
        }
        // SAFETY: The locked allocation holds at least `data.len()` writable bytes and cannot
        // overlap the borrowed source slice.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), bytes.cast::<u8>(), data.len()) };
        // SAFETY: This balances the successful `GlobalLock` above.
        drop(unsafe { GlobalUnlock(memory) });

        // SAFETY: This session holds the clipboard open; on success the system takes ownership
        // of `memory`, and on failure it remains ours to free.
        match unsafe { SetClipboardData(format, Some(HANDLE(memory.0))) } {
            Ok(_) => Ok(()),
            Err(error) => {
                free_global(memory);
                Err(platform_failure(
                    operation,
                    "writing clipboard data",
                    &error,
                ))
            }
        }
    }
}

impl Drop for ClipboardSession {
    fn drop(&mut self) {
        // SAFETY: The session opened the clipboard successfully and has not closed it yet.
        if let Err(error) = unsafe { CloseClipboard() } {
            eprintln!("failed to close the clipboard: {error}");
        }
        destroy_owner(self.owner);
    }
}

fn destroy_owner(owner: HWND) {
    // SAFETY: `owner` is a message-only window created on this thread by `ClipboardSession`.
    if let Err(error) = unsafe { DestroyWindow(owner) } {
        eprintln!("failed to destroy the clipboard owner window: {error}");
    }
}

fn free_global(memory: HGLOBAL) {
    // SAFETY: Callers pass an allocation that the clipboard did not take ownership of.
    drop(unsafe { GlobalFree(Some(memory)) });
}

fn clipboard_sequence() -> u32 {
    // SAFETY: Reading the window station's clipboard sequence number has no preconditions.
    unsafe { GetClipboardSequenceNumber() }
}

fn registered_format(
    name: &str,
    operation: ClipboardOperation,
) -> Result<u32, ClipboardTransactionFailure> {
    let name = HSTRING::from(name);
    // SAFETY: `name` is a valid NUL-terminated UTF-16 string for the duration of the call.
    match unsafe { RegisterClipboardFormatW(&name) } {
        0 => Err(platform_failure(
            operation,
            "registering a clipboard format",
            &windows::core::Error::from_thread(),
        )),
        format => Ok(format),
    }
}

/// Reports whether a format's clipboard handle is an HGLOBAL that can be copied byte-for-byte.
///
/// GDI-handle formats are skipped; Windows synthesizes bitmaps from the DIB formats that remain.
const fn is_global_memory_format(format: u32) -> bool {
    !matches!(
        format,
        CF_BITMAP
            | CF_METAFILEPICT
            | CF_PALETTE
            | CF_ENHMETAFILE
            | CF_OWNERDISPLAY
            | CF_DSPBITMAP
            | CF_DSPMETAFILEPICT
            | CF_DSPENHMETAFILE
            | CF_PRIVATEFIRST..=CF_GDIOBJLAST
    )
}

fn check_snapshot_limits(
    format_count: usize,
    total_bytes: usize,
) -> Result<(), ClipboardTransactionFailure> {
    if format_count > MAX_FORMATS {
        return Err(ClipboardTransactionFailure::TooManyMimeTypes {
            count: format_count,
            limit: MAX_FORMATS,
        });
    }
    if total_bytes > MAX_SNAPSHOT_BYTES {
        return Err(ClipboardTransactionFailure::SnapshotTooLarge {
            limit: MAX_SNAPSHOT_BYTES,
        });
    }
    Ok(())
}

fn unicode_text_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn decode_unicode_text(bytes: &[u8]) -> String {
    let units = bytes
        .chunks_exact(2)
        .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&units)
}

fn platform_failure(
    operation: ClipboardOperation,
    action: &'static str,
    error: &windows::core::Error,
) -> ClipboardTransactionFailure {
    ClipboardTransactionFailure::Access {
        operation,
        kind: ClipboardFailureKind::Platform {
            operation: action,
            code: error.code().0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_text_round_trips_through_clipboard_bytes() {
        let text = "héllo — 👋";
        let bytes = unicode_text_bytes(text);

        assert_eq!(bytes[bytes.len() - 2..], [0, 0]);
        assert_eq!(decode_unicode_text(&bytes), text);
    }

    #[test]
    fn decoding_stops_at_the_first_terminator() {
        let mut bytes = unicode_text_bytes("kept");
        bytes.extend(unicode_text_bytes("ignored"));

        assert_eq!(decode_unicode_text(&bytes), "kept");
    }

    #[test]
    fn ownership_requires_the_transcript_and_prefers_the_marker() {
        assert_eq!(ownership(true, true), TemporaryOwnership::Marker);
        assert_eq!(ownership(false, true), TemporaryOwnership::Transcript);
        assert_eq!(ownership(true, false), TemporaryOwnership::Changed);
        assert_eq!(ownership(false, false), TemporaryOwnership::Changed);
    }

    #[test]
    fn snapshots_skip_gdi_handle_formats() {
        assert!(is_global_memory_format(CF_UNICODETEXT));
        assert!(is_global_memory_format(0xC000));
        assert!(!is_global_memory_format(CF_BITMAP));
        assert!(!is_global_memory_format(CF_ENHMETAFILE));
        assert!(!is_global_memory_format(0x0300));
    }

    #[test]
    fn snapshot_limits_bound_format_count_and_size() {
        assert!(check_snapshot_limits(MAX_FORMATS, MAX_SNAPSHOT_BYTES).is_ok());
        assert!(matches!(
            check_snapshot_limits(MAX_FORMATS + 1, 0),
            Err(ClipboardTransactionFailure::TooManyMimeTypes { .. })
        ));
        assert!(matches!(
            check_snapshot_limits(1, MAX_SNAPSHOT_BYTES + 1),
            Err(ClipboardTransactionFailure::SnapshotTooLarge { .. })
        ));
    }
}
