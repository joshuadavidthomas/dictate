use std::ffi::c_char;
use std::ffi::c_void;
use std::ptr;

use dictate_desktop::FocusObservation;
use dictate_desktop::FocusProbeFailure;
use dictate_desktop::FocusProbeFailureKind;
use dictate_desktop::FocusSource;
use dictate_desktop::FocusedWindow;
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;

type CfTypeRef = *const c_void;
type CfStringRef = *const c_void;
type AxUiElementRef = *const c_void;
type FocusedWindowResult = Result<Option<(Option<u64>, Option<String>)>, (&'static str, i32)>;

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

const SOURCE: FocusSource = FocusSource::MacOs;
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const CF_NUMBER_SINT64_TYPE: isize = 4;
const AX_ERROR_ATTRIBUTE_UNSUPPORTED: i32 = -25_205;
const AX_ERROR_NO_VALUE: i32 = -25_212;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AxUiElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AxUiElementRef,
        attribute: CfStringRef,
        value: *mut CfTypeRef,
    ) -> i32;
    fn AXUIElementSetMessagingTimeout(element: AxUiElementRef, timeout_in_seconds: f32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFGetTypeID(value: CfTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: CfTypeRef, number_type: isize, value: *mut c_void) -> bool;
    fn CFStringGetLength(value: CfStringRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(
        value: CfStringRef,
        buffer: *mut c_char,
        buffer_size: isize,
        encoding: u32,
    ) -> bool;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        string: *const c_char,
        encoding: u32,
    ) -> CfStringRef;
    fn CFRelease(value: CfTypeRef);
}

pub(crate) fn observe() -> FocusObservation {
    autoreleasepool(|_| {
        let Some(application) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
            return failure("reading the frontmost application", -1);
        };
        let process_id = application.processIdentifier();
        let app_id = application
            .bundleIdentifier()
            .map(|value| value.to_string());
        let app_name = application.localizedName().map(|value| value.to_string());

        match focused_window(process_id) {
            Ok(Some((window_id, title))) => FocusedWindow::macos(
                process_id,
                window_id,
                app_id.as_deref().or(app_name.as_deref()),
                title.as_deref(),
            )
            .map_or_else(
                || failure("identifying the focused Accessibility window", -1),
                FocusObservation::Focused,
            ),
            Ok(None) => FocusObservation::NoFocusedWindow { source: SOURCE },
            Err((operation, code)) => failure(operation, code),
        }
    })
}

fn focused_window(pid: i32) -> FocusedWindowResult {
    // SAFETY: Create and copy APIs return retained Core Foundation objects released below.
    let application = unsafe { AXUIElementCreateApplication(pid) };
    let Some(application) = OwnedCf::new(application) else {
        return Err(("creating the frontmost Accessibility application", -1));
    };
    // SAFETY: `application` is a valid AX object created immediately above.
    let timeout_status = unsafe { AXUIElementSetMessagingTimeout(application.as_ptr(), 0.25) };
    if timeout_status != 0 {
        return Err((
            "bounding the Accessibility application request",
            timeout_status,
        ));
    }

    let window = copy_attribute(application.as_ptr(), c"AXFocusedWindow");
    let window = match window {
        Ok(window) => window,
        Err(AX_ERROR_NO_VALUE) => return Ok(None),
        Err(code) => return Err(("reading the focused Accessibility window", code)),
    };
    let window_id = match copy_attribute(window.as_ptr(), c"AXWindowNumber") {
        Ok(id) => {
            Some(cf_number_u64(id.as_ptr()).ok_or(("decoding the focused window identifier", -1))?)
        }
        Err(AX_ERROR_ATTRIBUTE_UNSUPPORTED | AX_ERROR_NO_VALUE) => None,
        Err(code) => return Err(("reading the focused window identifier", code)),
    };

    let title = match copy_attribute(window.as_ptr(), c"AXTitle") {
        Ok(value) => cf_string(value.as_ptr()),
        Err(_) => None,
    };
    Ok(Some((window_id, title)))
}

fn copy_attribute(element: AxUiElementRef, name: &std::ffi::CStr) -> Result<OwnedCf, i32> {
    // SAFETY: `name` is a valid NUL-terminated UTF-8 C string for this call.
    let attribute =
        unsafe { CFStringCreateWithCString(ptr::null(), name.as_ptr(), CF_STRING_ENCODING_UTF8) };
    if attribute.is_null() {
        return Err(-1);
    }
    let mut value = ptr::null();
    // SAFETY: Both CF references are valid and `value` is an initialized out pointer.
    let status = unsafe { AXUIElementCopyAttributeValue(element, attribute, &raw mut value) };
    // SAFETY: This balances `CFStringCreateWithCString` after the AX call returns.
    unsafe { CFRelease(attribute) };
    if status == 0 {
        OwnedCf::new(value).ok_or(AX_ERROR_NO_VALUE)
    } else {
        Err(status)
    }
}

fn cf_number_u64(value: CfTypeRef) -> Option<u64> {
    // SAFETY: `value` is a non-null retained CF object from an AX copy operation.
    let value_type = unsafe { CFGetTypeID(value) };
    // SAFETY: Reading a Core Foundation type ID has no preconditions.
    let number_type = unsafe { CFNumberGetTypeID() };
    if value_type != number_type {
        return None;
    }
    let mut number = 0_i64;
    // SAFETY: The type IDs match and `number` is a correctly sized writable output.
    if unsafe {
        CFNumberGetValue(
            value,
            CF_NUMBER_SINT64_TYPE,
            (&raw mut number).cast::<c_void>(),
        )
    } {
        u64::try_from(number).ok()
    } else {
        None
    }
}

fn cf_string(value: CfTypeRef) -> Option<String> {
    // SAFETY: `value` is a non-null retained CF object from an AX copy operation.
    let value_type = unsafe { CFGetTypeID(value) };
    // SAFETY: Reading a Core Foundation type ID has no preconditions.
    let string_type = unsafe { CFStringGetTypeID() };
    if value_type != string_type {
        return None;
    }
    let value = value.cast();
    // SAFETY: The matching type ID above proves this is a CFString.
    let length = unsafe { CFStringGetLength(value) };
    // SAFETY: `value` is a valid CFString and the encoding constant is valid.
    let capacity = unsafe { CFStringGetMaximumSizeForEncoding(length, CF_STRING_ENCODING_UTF8) }
        .checked_add(1)?;
    let mut bytes = vec![0_u8; usize::try_from(capacity).ok()?];
    // SAFETY: `bytes` has `capacity` writable bytes and `value` is a valid CFString.
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

fn failure(operation: &'static str, code: i32) -> FocusObservation {
    FocusObservation::ProbeFailed(FocusProbeFailure::new(
        SOURCE,
        FocusProbeFailureKind::Platform { operation, code },
    ))
}
