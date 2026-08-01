//! On-screen window discovery via `CGWindowListCopyWindowInfo`.
//!
//! Unlike `NSWorkspace.runningApplications` (which only refreshes with a pumped
//! main run loop), the CoreGraphics window list reflects the *current* on-screen
//! windows on every call, so it works from a run-loop-free event loop. We read
//! only the owner pid, window number, and layer — never `kCGWindowName` — so this
//! does not require Screen Recording permission.
//!
//! All FFI is local and the unsafe surface is confined here, matching `ax.rs`.

#![cfg(target_os = "macos")]

use std::collections::HashSet;
use std::ffi::c_void;
use yabai_core::Area;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFNumberRef = *const c_void;
type CFIndex = isize;
type Boolean = u8;

#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}
#[repr(C)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

// `kCFNumberSInt32Type` from <CoreFoundation/CFNumber.h>.
const K_CF_NUMBER_SINT32_TYPE: i32 = 3;
// `kCGWindowListOptionOnScreenOnly` | `kCGWindowListExcludeDesktopElements`.
const K_CG_WINDOW_LIST_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP: u32 = 1 << 4;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGWindowOwnerPID: CFStringRef;
    static kCGWindowNumber: CFStringRef;
    static kCGWindowLayer: CFStringRef;
    static kCGWindowBounds: CFStringRef;

    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
    fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> Boolean;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> CFTypeRef;
    fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: CFNumberRef, the_type: i32, value_ptr: *mut c_void) -> Boolean;
    fn CFRelease(cf: CFTypeRef);
}

/// A normal on-screen window: its CG id, owning pid, and on-screen bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CgWindow {
    pub window_id: u32,
    pub pid: i32,
    /// The window's frame in top-left global CoreGraphics coordinates
    /// (`kCGWindowBounds`), the same space as cursor/warp coordinates.
    pub bounds: Area,
}

/// Read `kCGWindowBounds` (a CFDictionary rect representation) into an [`Area`].
fn dict_bounds(dict: CFDictionaryRef) -> Option<Area> {
    // SAFETY: `dict` is a valid CFDictionary; `kCGWindowBounds` is a valid key
    // constant. `CGRectMakeWithDictionaryRepresentation` fills `rect` on success
    // (returns false / leaves it untouched otherwise, guarded by the return).
    unsafe {
        let bounds = CFDictionaryGetValue(dict, kCGWindowBounds);
        if bounds.is_null() {
            return None;
        }
        let mut rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 0.0,
                height: 0.0,
            },
        };
        let ok = CGRectMakeWithDictionaryRepresentation(bounds as CFDictionaryRef, &mut rect);
        (ok != 0).then(|| {
            Area::new(
                rect.origin.x as f32,
                rect.origin.y as f32,
                rect.size.width as f32,
                rect.size.height as f32,
            )
        })
    }
}

/// Read a 32-bit integer value out of a CFDictionary entry.
fn dict_i32(dict: CFDictionaryRef, key: CFStringRef) -> Option<i32> {
    // SAFETY: `dict` is a valid CFDictionary; `key` is a valid CFString constant.
    // `CFDictionaryGetValue` returns a borrowed value or null (checked), and
    // `CFNumberGetValue` writes into the local on success.
    unsafe {
        let value = CFDictionaryGetValue(dict, key);
        if value.is_null() {
            return None;
        }
        let mut out: i32 = 0;
        let ok = CFNumberGetValue(
            value,
            K_CF_NUMBER_SINT32_TYPE,
            &mut out as *mut i32 as *mut c_void,
        );
        (ok != 0).then_some(out)
    }
}

/// Every normal (layer 0) on-screen window, with its CG id, owner pid, and
/// bounds. Returned in CoreGraphics stacking order, front-most first, so callers
/// can resolve the top window under a point.
pub fn on_screen_windows() -> Vec<CgWindow> {
    let option = K_CG_WINDOW_LIST_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP;
    // SAFETY: `CGWindowListCopyWindowInfo` returns an owned CFArray (or null) of
    // borrowed CFDictionary entries; the array is released at the end.
    unsafe {
        let info = CGWindowListCopyWindowInfo(option, 0);
        if info.is_null() {
            return Vec::new();
        }
        let count = CFArrayGetCount(info as CFArrayRef);
        let mut windows = Vec::new();
        for idx in 0..count {
            let dict = CFArrayGetValueAtIndex(info as CFArrayRef, idx) as CFDictionaryRef;
            if dict.is_null() {
                continue;
            }
            // Layer 0 == ordinary application windows (skip menubar/Dock/UI).
            if dict_i32(dict, kCGWindowLayer) != Some(0) {
                continue;
            }
            let (Some(pid), Some(number), Some(bounds)) = (
                dict_i32(dict, kCGWindowOwnerPID),
                dict_i32(dict, kCGWindowNumber),
                dict_bounds(dict),
            ) else {
                continue;
            };
            if pid > 0 && number > 0 {
                windows.push(CgWindow {
                    window_id: number as u32,
                    pid,
                    bounds,
                });
            }
        }
        CFRelease(info);
        windows
    }
}

/// Distinct pids of applications that currently have a normal on-screen window.
/// This refreshes live, so it sees apps launched after the daemon started.
pub fn application_pids_with_windows() -> Vec<i32> {
    let mut seen = HashSet::new();
    let mut pids = Vec::new();
    for window in on_screen_windows() {
        if seen.insert(window.pid) {
            pids.push(window.pid);
        }
    }
    pids
}
