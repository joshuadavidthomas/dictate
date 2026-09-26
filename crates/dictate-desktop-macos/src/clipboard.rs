use std::ffi::c_char;
use std::ffi::c_void;
use std::ptr;

use dictate_desktop::ClipboardFailureKind;
use dictate_desktop::ClipboardOperation;
use dictate_desktop::ClipboardTransactionFailure;
use dictate_desktop::ClipboardTransport;
use dictate_desktop::TemporaryOwnership;
use dictate_desktop::TransactionMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSPasteboard;
use objc2_app_kit::NSPasteboardTypeString;
use objc2_foundation::NSString;

type PasteboardRef = *const c_void;
type PasteboardItemId = *mut c_void;
type CfTypeRef = *const c_void;
type CfStringRef = *const c_void;
type CfArrayRef = *const c_void;
type CfDataRef = *const c_void;

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const BAD_PASTEBOARD_FLAVOR: i32 = -25_133;
const DUPLICATE_PASTEBOARD_FLAVOR: i32 = -25_134;
const SYSTEM_TRANSLATED_FLAVOR: u32 = 1 << 8;
const MAX_MIME_TYPES: usize = 64;
const MAX_MIME_METADATA_BYTES: usize = 64 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn PasteboardCreate(name: CfStringRef, pasteboard: *mut PasteboardRef) -> i32;
    fn PasteboardSynchronize(pasteboard: PasteboardRef) -> u32;
    fn PasteboardClear(pasteboard: PasteboardRef) -> i32;
    fn PasteboardGetItemCount(pasteboard: PasteboardRef, count: *mut usize) -> i32;
    fn PasteboardGetItemIdentifier(
        pasteboard: PasteboardRef,
        index: isize,
        item: *mut PasteboardItemId,
    ) -> i32;
    fn PasteboardCopyItemFlavors(
        pasteboard: PasteboardRef,
        item: PasteboardItemId,
        flavors: *mut CfArrayRef,
    ) -> i32;
    fn PasteboardGetItemFlavorFlags(
        pasteboard: PasteboardRef,
        item: PasteboardItemId,
        flavor: CfStringRef,
        flags: *mut u32,
    ) -> i32;
    fn PasteboardCopyItemFlavorData(
        pasteboard: PasteboardRef,
        item: PasteboardItemId,
        flavor: CfStringRef,
        data: *mut CfDataRef,
    ) -> i32;
    fn PasteboardPutItemFlavor(
        pasteboard: PasteboardRef,
        item: PasteboardItemId,
        flavor: CfStringRef,
        data: CfDataRef,
        flags: u32,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(array: CfArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CfArrayRef, index: isize) -> *const c_void;
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> CfDataRef;
    fn CFDataGetBytePtr(data: CfDataRef) -> *const u8;
    fn CFDataGetLength(data: CfDataRef) -> isize;
    fn CFStringGetLength(value: CfStringRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(
        value: CfStringRef,
        buffer: *mut c_char,
        buffer_size: isize,
        encoding: u32,
    ) -> bool;
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        is_external_representation: u8,
    ) -> CfStringRef;
    fn CFRelease(value: CfTypeRef);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardSnapshot {
    change_count: isize,
    items: Vec<Vec<ClipboardFlavor>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClipboardFlavor {
    data_type: String,
    data: Vec<u8>,
    flags: u32,
}

struct PasteboardHandle(PasteboardRef);

impl Drop for PasteboardHandle {
    fn drop(&mut self) {
        // SAFETY: `PasteboardHandle` is created only from an owned PasteboardCreate result.
        unsafe { CFRelease(self.0) };
    }
}

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
        // SAFETY: `OwnedCf` wraps a value returned by a Core Foundation create/copy function.
        unsafe { CFRelease(self.0) };
    }
}

#[derive(Debug)]
pub(super) struct PlatformClipboard {
    clipboard: Retained<NSPasteboard>,
    published_change_count: Option<isize>,
}

impl Default for PlatformClipboard {
    fn default() -> Self {
        Self {
            clipboard: NSPasteboard::generalPasteboard(),
            published_change_count: None,
        }
    }
}

impl ClipboardTransport for PlatformClipboard {
    type Snapshot = ClipboardSnapshot;

    fn snapshot(&mut self) -> Result<Self::Snapshot, ClipboardTransactionFailure> {
        capture_snapshot(&self.clipboard)
    }

    fn snapshot_is_current(
        &mut self,
        snapshot: &Self::Snapshot,
    ) -> Result<bool, ClipboardTransactionFailure> {
        Ok(self.clipboard.changeCount() == snapshot.change_count)
    }

    fn publish(
        &mut self,
        text: &str,
        _marker: &TransactionMarker,
    ) -> Result<(), ClipboardTransactionFailure> {
        self.clipboard.clearContents();
        let text = NSString::from_str(text);
        // SAFETY: This accesses AppKit's process-lifetime NSPasteboardTypeString constant.
        let data_type = unsafe { NSPasteboardTypeString };
        if !self.clipboard.setString_forType(&text, data_type) {
            return Err(platform_failure(
                ClipboardOperation::PublishTranscript,
                "writing clipboard text",
                -1,
            ));
        }
        self.published_change_count = Some(self.clipboard.changeCount());
        Ok(())
    }

    fn temporary_ownership(
        &mut self,
        text: &str,
        _marker: &TransactionMarker,
    ) -> Result<TemporaryOwnership, ClipboardTransactionFailure> {
        if self.published_change_count != Some(self.clipboard.changeCount()) {
            return Ok(TemporaryOwnership::Changed);
        }
        // SAFETY: This accesses AppKit's process-lifetime NSPasteboardTypeString constant.
        let data_type = unsafe { NSPasteboardTypeString };
        if self
            .clipboard
            .stringForType(data_type)
            .is_some_and(|current| current.to_string() == text)
        {
            Ok(TemporaryOwnership::Transcript)
        } else {
            Ok(TemporaryOwnership::Changed)
        }
    }

    fn restore(&mut self, snapshot: Self::Snapshot) -> Result<(), ClipboardTransactionFailure> {
        restore_snapshot(&self.clipboard, &snapshot)?;
        self.published_change_count = None;
        Ok(())
    }
}

fn capture_snapshot(
    clipboard: &NSPasteboard,
) -> Result<ClipboardSnapshot, ClipboardTransactionFailure> {
    let change_count = clipboard.changeCount();
    let pasteboard = create_pasteboard(clipboard, ClipboardOperation::ReadSnapshot)?;
    // SAFETY: `pasteboard` owns a valid reference returned by PasteboardCreate.
    unsafe { PasteboardSynchronize(pasteboard.0) };
    let mut item_count = 0;
    check_status(
        // SAFETY: The pasteboard is valid and the out pointer references initialized storage.
        unsafe { PasteboardGetItemCount(pasteboard.0, &raw mut item_count) },
        ClipboardOperation::ReadSnapshot,
        "counting clipboard items",
    )?;

    let mut items = Vec::with_capacity(item_count);
    let mut total_flavors = 0_usize;
    let mut metadata_bytes = 0_usize;
    let mut total_bytes = 0_usize;
    for index in 1..=item_count {
        let index = isize::try_from(index).map_err(|_conversion_error| {
            platform_failure(
                ClipboardOperation::ReadSnapshot,
                "indexing a clipboard item",
                -1,
            )
        })?;
        let mut item = ptr::null_mut();
        check_status(
            // SAFETY: The one-based index is in the range reported by this pasteboard.
            unsafe { PasteboardGetItemIdentifier(pasteboard.0, index, &raw mut item) },
            ClipboardOperation::ReadSnapshot,
            "identifying a clipboard item",
        )?;
        items.push(capture_item(
            pasteboard.0,
            item,
            &mut total_flavors,
            &mut metadata_bytes,
            &mut total_bytes,
        )?);
    }

    if clipboard.changeCount() != change_count {
        return Err(ClipboardTransactionFailure::ChangedDuringSnapshot);
    }
    Ok(ClipboardSnapshot {
        change_count,
        items,
    })
}

fn capture_item(
    pasteboard: PasteboardRef,
    item: PasteboardItemId,
    total_flavors: &mut usize,
    metadata_bytes: &mut usize,
    total_bytes: &mut usize,
) -> Result<Vec<ClipboardFlavor>, ClipboardTransactionFailure> {
    let operation = ClipboardOperation::ReadSnapshot;
    let mut flavor_array = ptr::null();
    check_status(
        // SAFETY: The pasteboard and item identifier came from the active snapshot operation.
        unsafe { PasteboardCopyItemFlavors(pasteboard, item, &raw mut flavor_array) },
        operation,
        "listing clipboard formats",
    )?;
    let flavor_array = OwnedCf::new(flavor_array)
        .ok_or_else(|| platform_failure(operation, "listing clipboard formats", -1))?;
    // SAFETY: The copy call returned a valid CFArray retained by `flavor_array`.
    let flavor_count = unsafe { CFArrayGetCount(flavor_array.as_ptr()) };
    let has_authoritative =
        has_authoritative_flavor(pasteboard, item, flavor_array.as_ptr(), flavor_count);
    let mut item_flavors = Vec::new();
    for flavor_index in 0..flavor_count {
        // SAFETY: The index is within the bounds returned by CFArrayGetCount.
        let flavor = unsafe { CFArrayGetValueAtIndex(flavor_array.as_ptr(), flavor_index) };
        if flavor.is_null() {
            return Err(platform_failure(
                operation,
                "reading a clipboard format",
                -1,
            ));
        }
        let mut flags = 0;
        check_status(
            // SAFETY: The flavor is borrowed from the item's valid flavor array.
            unsafe { PasteboardGetItemFlavorFlags(pasteboard, item, flavor, &raw mut flags) },
            operation,
            "reading clipboard format flags",
        )?;
        if has_authoritative && flags & SYSTEM_TRANSLATED_FLAVOR != 0 {
            continue;
        }
        let data_type = cf_string(flavor)
            .ok_or_else(|| platform_failure(operation, "decoding a clipboard format", -1))?;
        *total_flavors = total_flavors.saturating_add(1);
        *metadata_bytes = metadata_bytes.saturating_add(data_type.len());
        check_snapshot_limits(*total_flavors, *metadata_bytes, *total_bytes)?;

        let mut data = ptr::null();
        // SAFETY: All references are valid and `data` points to storage for the copied value.
        let status =
            unsafe { PasteboardCopyItemFlavorData(pasteboard, item, flavor, &raw mut data) };
        if status == BAD_PASTEBOARD_FLAVOR {
            continue;
        }
        check_status(status, operation, "reading clipboard format data")?;
        let data = OwnedCf::new(data)
            .ok_or_else(|| platform_failure(operation, "decoding clipboard format data", -1))?;
        let bytes = cf_data(data.as_ptr())
            .ok_or_else(|| platform_failure(operation, "decoding clipboard format data", -1))?;
        *total_bytes = total_bytes.saturating_add(bytes.len());
        check_snapshot_limits(*total_flavors, *metadata_bytes, *total_bytes)?;
        item_flavors.push(ClipboardFlavor {
            data_type,
            data: bytes,
            flags: flags & 0x0f,
        });
    }
    Ok(item_flavors)
}

fn has_authoritative_flavor(
    pasteboard: PasteboardRef,
    item: PasteboardItemId,
    flavor_array: CfArrayRef,
    flavor_count: isize,
) -> bool {
    (0..flavor_count).any(|flavor_index| {
        // SAFETY: The index is bounded by the array's reported flavor count.
        let flavor = unsafe { CFArrayGetValueAtIndex(flavor_array, flavor_index) };
        let mut flags = 0;
        !flavor.is_null()
            // SAFETY: The flavor is borrowed from this item's flavor array.
            && unsafe {
                PasteboardGetItemFlavorFlags(pasteboard, item, flavor, &raw mut flags)
            } == 0
            && flags & SYSTEM_TRANSLATED_FLAVOR == 0
    })
}

fn check_snapshot_limits(
    total_flavors: usize,
    metadata_bytes: usize,
    total_bytes: usize,
) -> Result<(), ClipboardTransactionFailure> {
    if total_flavors > MAX_MIME_TYPES {
        return Err(ClipboardTransactionFailure::TooManyMimeTypes {
            count: total_flavors,
            limit: MAX_MIME_TYPES,
        });
    }
    if metadata_bytes > MAX_MIME_METADATA_BYTES {
        return Err(ClipboardTransactionFailure::MimeMetadataTooLarge {
            limit: MAX_MIME_METADATA_BYTES,
        });
    }
    if total_bytes > MAX_SNAPSHOT_BYTES {
        return Err(ClipboardTransactionFailure::SnapshotTooLarge {
            limit: MAX_SNAPSHOT_BYTES,
        });
    }
    Ok(())
}

fn restore_snapshot(
    clipboard: &NSPasteboard,
    snapshot: &ClipboardSnapshot,
) -> Result<(), ClipboardTransactionFailure> {
    let operation = ClipboardOperation::RestoreSnapshot;
    let pasteboard = create_pasteboard(clipboard, operation)?;
    // SAFETY: `pasteboard` owns a valid reference returned by PasteboardCreate.
    unsafe { PasteboardSynchronize(pasteboard.0) };
    check_status(
        // SAFETY: The pasteboard reference remains valid for this restore operation.
        unsafe { PasteboardClear(pasteboard.0) },
        operation,
        "clearing the clipboard",
    )?;
    for (item_index, item) in snapshot.items.iter().enumerate() {
        let item_id = (item_index + 1) as PasteboardItemId;
        for flavor in item {
            let data_type = cf_string_create(&flavor.data_type)
                .ok_or_else(|| platform_failure(operation, "encoding a clipboard format", -1))?;
            let data_length = isize::try_from(flavor.data.len()).map_err(|_conversion_error| {
                platform_failure(operation, "encoding clipboard format data", -1)
            })?;
            // SAFETY: Core Foundation copies `data_length` bytes from the valid data slice.
            let data = unsafe { CFDataCreate(ptr::null(), flavor.data.as_ptr(), data_length) };
            let data = OwnedCf::new(data)
                .ok_or_else(|| platform_failure(operation, "encoding clipboard format data", -1))?;
            // SAFETY: The pasteboard, item ID, flavor string, and flavor data are all valid.
            let status = unsafe {
                PasteboardPutItemFlavor(
                    pasteboard.0,
                    item_id,
                    data_type.as_ptr(),
                    data.as_ptr(),
                    flavor.flags,
                )
            };
            if status != DUPLICATE_PASTEBOARD_FLAVOR {
                check_status(status, operation, "restoring a clipboard format")?;
            }
        }
    }
    Ok(())
}

fn create_pasteboard(
    clipboard: &NSPasteboard,
    operation: ClipboardOperation,
) -> Result<PasteboardHandle, ClipboardTransactionFailure> {
    let mut pasteboard = ptr::null();
    let name = clipboard.name();
    check_status(
        // SAFETY: The AppKit string is valid for the duration of the call and the out pointer
        // references storage for PasteboardCreate's retained result.
        unsafe {
            PasteboardCreate(
                ptr::from_ref::<NSString>(name.as_ref()).cast(),
                &raw mut pasteboard,
            )
        },
        operation,
        "opening the clipboard",
    )?;
    if pasteboard.is_null() {
        Err(platform_failure(operation, "opening the clipboard", -1))
    } else {
        Ok(PasteboardHandle(pasteboard))
    }
}

fn check_status(
    status: i32,
    operation: ClipboardOperation,
    action: &'static str,
) -> Result<(), ClipboardTransactionFailure> {
    if status == 0 {
        Ok(())
    } else {
        Err(platform_failure(operation, action, status))
    }
}

fn platform_failure(
    operation: ClipboardOperation,
    action: &'static str,
    code: i32,
) -> ClipboardTransactionFailure {
    ClipboardTransactionFailure::Access {
        operation,
        kind: ClipboardFailureKind::Platform {
            operation: action,
            code,
        },
    }
}

fn cf_string_create(value: &str) -> Option<OwnedCf> {
    let length = isize::try_from(value.len()).ok()?;
    // SAFETY: Core Foundation copies `length` bytes from the valid UTF-8 string.
    let value = unsafe {
        CFStringCreateWithBytes(
            ptr::null(),
            value.as_ptr(),
            length,
            CF_STRING_ENCODING_UTF8,
            0,
        )
    };
    OwnedCf::new(value)
}

fn cf_string(value: CfStringRef) -> Option<String> {
    // SAFETY: `value` is a valid CFString borrowed from a retained flavor array.
    let length = unsafe { CFStringGetLength(value) };
    // SAFETY: The string length came from this CFString and the encoding is valid.
    let capacity = unsafe { CFStringGetMaximumSizeForEncoding(length, CF_STRING_ENCODING_UTF8) }
        .checked_add(1)?;
    let mut bytes = vec![0; usize::try_from(capacity).ok()?];
    // SAFETY: The output buffer has `capacity` bytes and `value` is a valid CFString.
    if !unsafe {
        CFStringGetCString(
            value,
            bytes.as_mut_ptr().cast(),
            capacity,
            CF_STRING_ENCODING_UTF8,
        )
    } {
        return None;
    }
    let length = bytes.iter().position(|byte| *byte == 0)?;
    String::from_utf8(bytes[..length].to_vec()).ok()
}

fn cf_data(value: CfDataRef) -> Option<Vec<u8>> {
    if value.is_null() {
        return None;
    }
    // SAFETY: `value` is a valid retained CFData object.
    let length = unsafe { CFDataGetLength(value) };
    let length = usize::try_from(length).ok()?;
    if length == 0 {
        return Some(Vec::new());
    }
    // SAFETY: Non-empty CFData exposes a byte pointer valid for the object's lifetime.
    let bytes = unsafe { CFDataGetBytePtr(value) };
    if bytes.is_null() {
        None
    } else {
        // SAFETY: Core Foundation reports `length` readable bytes at this non-null pointer.
        Some(unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec())
    }
}
