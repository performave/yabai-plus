//! Live display discovery through CoreGraphics.
//!
//! This is intentionally a narrow Phase 5 boundary: it discovers display ids and
//! frames, while `space.rs` handles the current read-only SkyLight space slice.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::io;

use yabai_core::Area;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFUUIDRef = *const c_void;

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MacDisplay {
    pub id: u32,
    pub frame: Area,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFUUIDRef;
    fn CGGetActiveDisplayList(
        max_displays: u32,
        active_displays: *mut u32,
        display_count: *mut u32,
    ) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGWarpMouseCursorPosition(point: CGPoint) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFUUIDCreateString(alloc: *const c_void, uuid: CFUUIDRef) -> CFStringRef;
}

#[link(name = "SkyLight", kind = "framework")]
unsafe extern "C" {
    fn SLSMainConnectionID() -> i32;
    fn SLSGetCurrentCursorLocation(cid: i32, point: *mut CGPoint) -> i32;
    fn SLSSetActiveMenuBarDisplayIdentifier(
        cid: i32,
        uuid: CFStringRef,
        repeat_uuid: CFStringRef,
    ) -> i32;
}

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

fn display_uuid_string(display_id: u32) -> io::Result<OwnedCf> {
    // SAFETY: `display_id` comes from CoreGraphics; the returned UUID, if
    // non-null, is owned and released after conversion to a CFString.
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

pub fn active_displays() -> io::Result<Vec<MacDisplay>> {
    let mut count = 0u32;
    // SAFETY: `display_count` points to a valid `u32`; passing a null display
    // array with max 0 is the documented way to query the active display count.
    let err = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) };
    if err != 0 {
        return Err(io::Error::other(format!(
            "CGGetActiveDisplayList count failed with {err}"
        )));
    }

    let mut ids = vec![0u32; count as usize];
    // SAFETY: `ids` has room for `count` display ids and both pointers are valid
    // for writes for the duration of the call.
    let err = unsafe { CGGetActiveDisplayList(count, ids.as_mut_ptr(), &mut count) };
    if err != 0 {
        return Err(io::Error::other(format!(
            "CGGetActiveDisplayList ids failed with {err}"
        )));
    }
    ids.truncate(count as usize);

    Ok(ids
        .into_iter()
        .map(|id| {
            // SAFETY: `id` was returned by CoreGraphics as an active display id.
            let frame = unsafe { CGDisplayBounds(id) };
            MacDisplay {
                id,
                frame: Area::new(
                    frame.origin.x as f32,
                    frame.origin.y as f32,
                    frame.size.width as f32,
                    frame.size.height as f32,
                ),
            }
        })
        .collect())
}

fn display_center(display_id: u32) -> CGPoint {
    // SAFETY: callers pass active CoreGraphics display ids.
    let frame = unsafe { CGDisplayBounds(display_id) };
    CGPoint {
        x: frame.origin.x + frame.size.width / 2.0,
        y: frame.origin.y + frame.size.height / 2.0,
    }
}

pub fn cursor_display_id() -> io::Result<u32> {
    let mut cursor = CGPoint { x: 0.0, y: 0.0 };
    // SAFETY: `cursor` is a valid out pointer for SkyLight to write the current
    // cursor location in global display coordinates.
    let err = unsafe { SLSGetCurrentCursorLocation(SLSMainConnectionID(), &mut cursor) };
    if err != 0 {
        return Err(io::Error::other(format!(
            "failed to read cursor location ({err})"
        )));
    }

    active_displays()?
        .into_iter()
        .find(|display| {
            let frame = display.frame;
            cursor.x >= frame.x as f64
                && cursor.x < (frame.x + frame.w) as f64
                && cursor.y >= frame.y as f64
                && cursor.y < (frame.y + frame.h) as f64
        })
        .map(|display| display.id)
        .ok_or_else(|| io::Error::other("cursor is not on an active display"))
}

pub fn warp_cursor_to_display_center(display_id: u32) -> io::Result<()> {
    let point = display_center(display_id);
    // SAFETY: `point` is a valid global display coordinate.
    let err = unsafe { CGWarpMouseCursorPosition(point) };
    if err == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to warp cursor to display {display_id} ({err})"
        )))
    }
}

pub fn set_active_display(display_id: u32) -> io::Result<()> {
    let uuid = display_uuid_string(display_id)?;
    // SAFETY: `uuid` is a valid display identifier for the duration of the call.
    let err = unsafe {
        SLSSetActiveMenuBarDisplayIdentifier(
            SLSMainConnectionID(),
            uuid.as_ptr() as CFStringRef,
            uuid.as_ptr() as CFStringRef,
        )
    };
    if err == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to activate display {display_id} ({err})"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_displays_call_is_safe() {
        let _ = active_displays().unwrap();
    }
}
