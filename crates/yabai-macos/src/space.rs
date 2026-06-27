//! Read-only Mission Control space discovery through SkyLight.
//!
//! This is intentionally a narrow Phase 5 boundary: it only discovers the
//! current space and ordered space ids for a display. Space mutation and event
//! subscriptions stay out of this module until the scripting-addition work.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::io;
use std::os::raw::c_char;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFUUIDRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFNumberRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFIndex = isize;
type Boolean = u8;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

// `kCFNumberSInt32Type` from <CoreFoundation/CFNumber.h>.
const K_CF_NUMBER_SINT32_TYPE: i32 = 3;
// `kCFNumberSInt64Type` from <CoreFoundation/CFNumber.h>.
const K_CF_NUMBER_SINT64_TYPE: i32 = 4;
// `kCFStringEncodingUTF8`.
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

struct OwnedCf(CFTypeRef);

impl OwnedCf {
    fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `OwnedCf` only wraps objects returned by Create/Copy
            // CoreFoundation APIs, so this balances one owned retain count.
            unsafe { CFRelease(self.0) };
        }
    }
}

type CGEventRef = *const c_void;

// `kCGSessionEventTap` from <CoreGraphics/CGEventTypes.h>: post into the
// per-session event stream so the gesture reaches the WindowServer.
const K_CG_SESSION_EVENT_TAP: u32 = 1;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayGetDisplayIDFromUUID(uuid: CFUUIDRef) -> u32;
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFUUIDRef;
    fn CGEventCreate(source: *const c_void) -> CGEventRef;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventSetDoubleValueField(event: CGEventRef, field: u32, value: f64);
    fn CGEventPost(tap: u32, event: CGEventRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayCreate(
        allocator: CFAllocatorRef,
        values: *const *const c_void,
        num_values: CFIndex,
        callbacks: *const c_void,
    ) -> CFArrayRef;
    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> CFTypeRef;
    fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFEqual(cf1: CFTypeRef, cf2: CFTypeRef) -> Boolean;
    fn CFNumberCreate(
        allocator: CFAllocatorRef,
        the_type: i32,
        value_ptr: *const c_void,
    ) -> CFNumberRef;
    fn CFNumberGetValue(number: CFNumberRef, the_type: i32, value_ptr: *mut c_void) -> Boolean;
    fn CFRelease(cf: CFTypeRef);
    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFUUIDCreateFromString(alloc: CFAllocatorRef, uuid_str: CFStringRef) -> CFUUIDRef;
    fn CFUUIDCreateString(alloc: CFAllocatorRef, uuid: CFUUIDRef) -> CFStringRef;
}

#[link(name = "SkyLight", kind = "framework")]
unsafe extern "C" {
    fn SLSMainConnectionID() -> i32;
    fn SLSCopyBestManagedDisplayForRect(cid: i32, rect: CGRect) -> CFStringRef;
    fn SLSCopyManagedDisplayForWindow(cid: i32, wid: u32) -> CFStringRef;
    fn SLSManagedDisplayGetCurrentSpace(cid: i32, uuid: CFStringRef) -> u64;
    fn SLSCopyManagedDisplayForSpace(cid: i32, sid: u64) -> CFStringRef;
    fn SLSCopyManagedDisplaySpaces(cid: i32) -> CFArrayRef;
    fn SLSCopySpacesForWindows(cid: i32, selector: i32, window_list: CFArrayRef) -> CFArrayRef;
    fn SLSGetWindowBounds(cid: i32, wid: u32, frame: *mut CGRect) -> i32;
    fn SLSGetWindowAlpha(cid: i32, wid: u32, alpha: *mut f32) -> i32;
}

fn owned_cfstring(literal: &[u8]) -> io::Result<OwnedCf> {
    debug_assert_eq!(literal.last(), Some(&0), "literal must be NUL-terminated");
    // SAFETY: `literal` is a valid NUL-terminated UTF-8 buffer and
    // CoreFoundation copies it into an owned CFString.
    let value = unsafe {
        CFStringCreateWithCString(
            std::ptr::null(),
            literal.as_ptr() as *const c_char,
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    if value.is_null() {
        Err(io::Error::other("failed to create CoreFoundation string"))
    } else {
        Ok(OwnedCf(value))
    }
}

fn display_uuid_string(display_id: u32) -> io::Result<OwnedCf> {
    // SAFETY: `display_id` comes from CoreGraphics callers; the returned UUID,
    // if non-null, is owned and released after converting it to a CFString.
    let uuid = unsafe { CGDisplayCreateUUIDFromDisplayID(display_id) };
    if uuid.is_null() {
        return Err(io::Error::other(format!(
            "failed to create UUID for display {display_id}"
        )));
    }

    // SAFETY: `uuid` is a valid owned CFUUID; CoreFoundation returns an owned
    // string or null. The UUID is released immediately after conversion.
    let uuid_string = unsafe {
        let string = CFUUIDCreateString(std::ptr::null(), uuid);
        CFRelease(uuid);
        string
    };
    if uuid_string.is_null() {
        Err(io::Error::other(format!(
            "failed to stringify UUID for display {display_id}"
        )))
    } else {
        Ok(OwnedCf(uuid_string))
    }
}

fn cfnumber_u64(number: CFNumberRef) -> Option<u64> {
    if number.is_null() {
        return None;
    }

    let mut value = 0i64;
    // SAFETY: `number` is a CFNumber borrowed from a SkyLight dictionary and
    // `value` is a valid out pointer for the requested 64-bit integer type.
    let ok = unsafe {
        CFNumberGetValue(
            number,
            K_CF_NUMBER_SINT64_TYPE,
            &mut value as *mut i64 as *mut c_void,
        )
    };
    (ok != 0 && value > 0).then_some(value as u64)
}

fn owned_cfnumber_i32(value: i32) -> io::Result<OwnedCf> {
    // SAFETY: `value` is a valid out-of-line scalar for CoreFoundation to copy
    // into an owned CFNumber.
    let number = unsafe {
        CFNumberCreate(
            std::ptr::null(),
            K_CF_NUMBER_SINT32_TYPE,
            &value as *const i32 as *const c_void,
        )
    };
    if number.is_null() {
        Err(io::Error::other("failed to create CoreFoundation number"))
    } else {
        Ok(OwnedCf(number))
    }
}

fn owned_single_value_array(value: &OwnedCf) -> io::Result<OwnedCf> {
    let values = [value.as_ptr()];
    // SAFETY: `values` points to one valid CF object for the duration of the
    // call. Null callbacks mirror the narrow C helper usage here; the caller
    // keeps `value` alive while the array is used.
    let array = unsafe { CFArrayCreate(std::ptr::null(), values.as_ptr(), 1, std::ptr::null()) };
    if array.is_null() {
        Err(io::Error::other("failed to create CoreFoundation array"))
    } else {
        Ok(OwnedCf(array))
    }
}

fn window_display_uuid(window_id: u32) -> Option<OwnedCf> {
    // SAFETY: `SLSMainConnectionID` returns the process' SkyLight connection;
    // `window_id` is a plain CG window id. The returned string, if any, is owned.
    let uuid = unsafe { SLSCopyManagedDisplayForWindow(SLSMainConnectionID(), window_id) };
    if !uuid.is_null() {
        return Some(OwnedCf(uuid));
    }

    let mut frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: 0.0,
            height: 0.0,
        },
    };
    // SAFETY: `frame` is a valid out pointer; on success SkyLight writes the
    // window's bounds so we can ask for the best managed display for that rect.
    let err = unsafe { SLSGetWindowBounds(SLSMainConnectionID(), window_id, &mut frame) };
    if err != 0 {
        return None;
    }

    // SAFETY: `frame` was initialized above and is passed by value; the returned
    // display string, if non-null, is owned by the caller.
    let uuid = unsafe { SLSCopyBestManagedDisplayForRect(SLSMainConnectionID(), frame) };
    (!uuid.is_null()).then_some(OwnedCf(uuid))
}

fn current_space_for_window_display(window_id: u32) -> Option<u64> {
    let uuid = window_display_uuid(window_id)?;
    // SAFETY: `uuid` is a valid CFString display identifier for the duration of
    // the call.
    let sid = unsafe { SLSManagedDisplayGetCurrentSpace(SLSMainConnectionID(), uuid.as_ptr()) };
    (sid != 0).then_some(sid)
}

/// Return Mission Control's current space id for `display_id`.
pub fn current_space_for_display(display_id: u32) -> io::Result<u64> {
    let uuid = display_uuid_string(display_id)?;
    // SAFETY: `SLSMainConnectionID` returns the process' SkyLight connection;
    // `uuid` is a valid CFString display identifier for the duration of the call.
    let sid = unsafe { SLSManagedDisplayGetCurrentSpace(SLSMainConnectionID(), uuid.as_ptr()) };
    if sid == 0 {
        Err(io::Error::other(format!(
            "failed to discover current space for display {display_id}"
        )))
    } else {
        Ok(sid)
    }
}

/// Return the CoreGraphics display id that currently owns `sid`.
pub fn display_for_space(sid: u64) -> io::Result<u32> {
    // SAFETY: `SLSMainConnectionID` returns the process' SkyLight connection;
    // the returned display identifier string, if any, is owned by the caller.
    let uuid_string = unsafe { SLSCopyManagedDisplayForSpace(SLSMainConnectionID(), sid) };
    if uuid_string.is_null() {
        return Err(io::Error::other(format!(
            "failed to discover display for space {sid}"
        )));
    }
    let uuid_string = OwnedCf(uuid_string);

    // SAFETY: `uuid_string` is a valid CFString display UUID for the duration of
    // the call; the returned UUID, if non-null, is owned and released below.
    let uuid =
        unsafe { CFUUIDCreateFromString(std::ptr::null(), uuid_string.as_ptr() as CFStringRef) };
    if uuid.is_null() {
        return Err(io::Error::other(format!(
            "failed to parse display UUID for space {sid}"
        )));
    }

    // SAFETY: `uuid` is a valid CFUUID. CoreGraphics returns 0 if no active
    // display matches it; `CFRelease` balances `CFUUIDCreateFromString`.
    let display_id = unsafe {
        let display_id = CGDisplayGetDisplayIDFromUUID(uuid);
        CFRelease(uuid);
        display_id
    };
    if display_id == 0 {
        Err(io::Error::other(format!(
            "space {sid} is not on an active display"
        )))
    } else {
        Ok(display_id)
    }
}

/// Return Mission Control's ordered space ids for `display_id`.
pub fn spaces_for_display(display_id: u32) -> io::Result<Vec<u64>> {
    let uuid = display_uuid_string(display_id)?;
    let display_identifier_key = owned_cfstring(b"Display Identifier\0")?;
    let spaces_key = owned_cfstring(b"Spaces\0")?;
    let id_key = owned_cfstring(b"id64\0")?;

    // SAFETY: `SLSMainConnectionID` returns the process' SkyLight connection;
    // the returned array is owned and released at the end of this function.
    let display_spaces = unsafe { SLSCopyManagedDisplaySpaces(SLSMainConnectionID()) };
    if display_spaces.is_null() {
        return Err(io::Error::other("failed to copy managed display spaces"));
    }
    let display_spaces = OwnedCf(display_spaces);

    let mut result = Vec::new();
    // SAFETY: `display_spaces` is a valid CFArray of display dictionaries; all
    // dictionary and nested-array values are borrowed and null-checked before use.
    unsafe {
        let display_count = CFArrayGetCount(display_spaces.as_ptr() as CFArrayRef);
        for display_index in 0..display_count {
            let display_ref =
                CFArrayGetValueAtIndex(display_spaces.as_ptr() as CFArrayRef, display_index)
                    as CFDictionaryRef;
            if display_ref.is_null() {
                continue;
            }

            let identifier = CFDictionaryGetValue(display_ref, display_identifier_key.as_ptr());
            if identifier.is_null() || CFEqual(uuid.as_ptr(), identifier) == 0 {
                continue;
            }

            let spaces_ref = CFDictionaryGetValue(display_ref, spaces_key.as_ptr()) as CFArrayRef;
            if spaces_ref.is_null() {
                break;
            }

            let space_count = CFArrayGetCount(spaces_ref);
            for space_index in 0..space_count {
                let space_ref = CFArrayGetValueAtIndex(spaces_ref, space_index) as CFDictionaryRef;
                if space_ref.is_null() {
                    continue;
                }
                let sid_ref = CFDictionaryGetValue(space_ref, id_key.as_ptr()) as CFNumberRef;
                if let Some(sid) = cfnumber_u64(sid_ref) {
                    result.push(sid);
                }
            }
            break;
        }
    }

    Ok(result)
}

/// Return every Mission Control space id in global desktop order — each
/// display's spaces in turn, matching the C `space_manager_mission_control_*`
/// helpers that flatten `SLSCopyManagedDisplaySpaces` in array order. The 1-based
/// position in this list is the mission-control index shown by `query --spaces`.
pub fn mission_control_spaces() -> io::Result<Vec<u64>> {
    let spaces_key = owned_cfstring(b"Spaces\0")?;
    let id_key = owned_cfstring(b"id64\0")?;

    // SAFETY: `SLSMainConnectionID` returns the process' SkyLight connection;
    // the returned array is owned and released at the end of this function.
    let display_spaces = unsafe { SLSCopyManagedDisplaySpaces(SLSMainConnectionID()) };
    if display_spaces.is_null() {
        return Err(io::Error::other("failed to copy managed display spaces"));
    }
    let display_spaces = OwnedCf(display_spaces);

    let mut result = Vec::new();
    // SAFETY: `display_spaces` is a valid CFArray of display dictionaries; all
    // dictionary and nested-array values are borrowed and null-checked before use.
    unsafe {
        let display_count = CFArrayGetCount(display_spaces.as_ptr() as CFArrayRef);
        for display_index in 0..display_count {
            let display_ref =
                CFArrayGetValueAtIndex(display_spaces.as_ptr() as CFArrayRef, display_index)
                    as CFDictionaryRef;
            if display_ref.is_null() {
                continue;
            }

            let spaces_ref = CFDictionaryGetValue(display_ref, spaces_key.as_ptr()) as CFArrayRef;
            if spaces_ref.is_null() {
                continue;
            }

            let space_count = CFArrayGetCount(spaces_ref);
            for space_index in 0..space_count {
                let space_ref = CFArrayGetValueAtIndex(spaces_ref, space_index) as CFDictionaryRef;
                if space_ref.is_null() {
                    continue;
                }
                let sid_ref = CFDictionaryGetValue(space_ref, id_key.as_ptr()) as CFNumberRef;
                if let Some(sid) = cfnumber_u64(sid_ref) {
                    result.push(sid);
                }
            }
        }
    }

    Ok(result)
}

/// Switch the active space by `steps` desktops in the direction of its sign
/// (positive = later/right, negative = earlier/left), skipping the Mission
/// Control animation. macOS exposes no space-activation API, so — like the C
/// daemon's scripting-addition-free fallback — this synthesizes a sequence of
/// high-velocity dock-swipe gestures. Single-display only: it never warps the
/// cursor across displays.
///
/// Technique attribution (via the C `space_manager_focus_space_using_gesture`):
/// <https://github.com/jurplel/InstantSpaceSwitcher>, reverse-engineered from
/// BetterTouchTool.
pub fn switch_space_by_gesture(steps: i32) -> io::Result<()> {
    if steps == 0 {
        return Ok(());
    }

    // SAFETY: a null source is valid and yields an event with default fields.
    let event = unsafe { CGEventCreate(std::ptr::null()) };
    if event.is_null() {
        return Err(io::Error::other("failed to create dock-swipe event"));
    }
    let event = OwnedCf(event);

    let sign = if steps > 0 { 1.0 } else { -1.0 };
    // SAFETY: `event` is a valid CGEvent; the magic field ids/values mirror the C
    // daemon exactly (kCGSEventTypeField=55 -> kCGSEventDockControl=30, etc.).
    unsafe {
        let ev = event.as_ptr();
        CGEventSetIntegerValueField(ev, 55, 30); // kCGSEventTypeField -> kCGSEventDockControl
        CGEventSetIntegerValueField(ev, 110, 23); // kCGEventGestureHIDType -> kIOHIDEventTypeDockSwipe
        CGEventSetIntegerValueField(ev, 123, 1); // kCGEventGestureSwipeMotion -> horizontal
        CGEventSetDoubleValueField(ev, 124, sign); // kCGEventGestureSwipeProgress
        CGEventSetDoubleValueField(ev, 129, sign * 9999.0); // kCGEventGestureSwipeVelocityX
        for _ in 0..steps.abs() {
            CGEventSetIntegerValueField(ev, 132, 1); // kCGEventGesturePhase -> began
            CGEventPost(K_CG_SESSION_EVENT_TAP, ev);
            CGEventSetIntegerValueField(ev, 132, 4); // kCGEventGesturePhase -> ended
            CGEventPost(K_CG_SESSION_EVENT_TAP, ev);
        }
    }

    Ok(())
}

/// Return the Mission Control space ids containing `window_id`.
pub fn spaces_for_window(window_id: u32) -> io::Result<Vec<u64>> {
    let window_number = owned_cfnumber_i32(window_id as i32)?;
    let window_list = owned_single_value_array(&window_number)?;

    // SAFETY: `window_list` is a valid CFArray containing one CFNumber window id.
    // Selector `0x7` matches yabai's C `window_space_list` query.
    let space_list = unsafe {
        SLSCopySpacesForWindows(
            SLSMainConnectionID(),
            0x7,
            window_list.as_ptr() as CFArrayRef,
        )
    };
    let mut result = Vec::new();
    if !space_list.is_null() {
        let space_list = OwnedCf(space_list);
        // SAFETY: `space_list` is a valid CFArray of borrowed CFNumber values;
        // every entry is null-checked by `cfnumber_u64`.
        unsafe {
            let count = CFArrayGetCount(space_list.as_ptr() as CFArrayRef);
            for index in 0..count {
                let sid_ref =
                    CFArrayGetValueAtIndex(space_list.as_ptr() as CFArrayRef, index) as CFNumberRef;
                if let Some(sid) = cfnumber_u64(sid_ref) {
                    result.push(sid);
                }
            }
        }
    }

    if result.is_empty() {
        if let Some(sid) = current_space_for_window_display(window_id) {
            result.push(sid);
        }
    }

    if result.is_empty() {
        Err(io::Error::other(format!(
            "failed to discover spaces for window {window_id}"
        )))
    } else {
        Ok(result)
    }
}

/// Read a window's current alpha (opacity in `0.0..=1.0`) via SkyLight.
///
/// Read-only and needs no special permissions; mirrors the C
/// `SLSGetWindowAlpha` usage. Used to verify the scripting-addition opacity
/// opcode took effect (the SA write itself returns only an ack byte).
pub fn window_alpha(window_id: u32) -> io::Result<f32> {
    let mut alpha = 0.0f32;
    // SAFETY: `SLSMainConnectionID` returns the process' SkyLight connection;
    // `window_id` is a plain CG window id and `alpha` is a valid out pointer for
    // the float SkyLight writes on success.
    let err = unsafe { SLSGetWindowAlpha(SLSMainConnectionID(), window_id, &mut alpha) };
    if err != 0 {
        Err(io::Error::other(format!(
            "failed to read alpha for window {window_id} (SkyLight error {err})"
        )))
    } else {
        Ok(alpha)
    }
}

#[cfg(test)]
mod tests {
    use crate::active_displays;

    use super::*;

    #[test]
    fn current_space_belongs_to_display_space_list() {
        let displays = active_displays().unwrap();
        let Some(display) = displays.first() else {
            return;
        };

        let current_sid = current_space_for_display(display.id).unwrap();
        let spaces = spaces_for_display(display.id).unwrap();
        assert!(spaces.contains(&current_sid));
    }
}
