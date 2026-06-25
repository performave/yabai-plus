//! Usable (visible) display frame discovery via `NSScreen`.
//!
//! `CGDisplayBounds` returns the *full* display rectangle, which includes the
//! menu bar and the Dock. The layout engine should tile inside the region the
//! user can actually use, exactly as the C daemon does when it derives a space's
//! root area. `NSScreen.visibleFrame` gives that region, but in AppKit's
//! bottom-left coordinate space; this module flips it back into the top-left
//! CoreGraphics space the rest of the code (and AX) uses.

#![cfg(target_os = "macos")]

use std::ffi::CString;
use std::os::raw::c_char;

use yabai_core::Area;

use crate::objc::{Id, class, msg0, msg1, sel};

#[repr(C)]
#[derive(Clone, Copy)]
struct NsPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NsSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NsRect {
    origin: NsPoint,
    size: NsSize,
}

/// The main display's usable frame (menu bar and Dock excluded), in top-left
/// CoreGraphics coordinates. Returns `None` if `NSScreen` is unavailable.
///
/// This is correct for a single-display setup; with multiple displays it
/// describes the main screen only (the others fall back to their full bounds).
pub fn main_visible_frame() -> Option<Area> {
    let screen_class = class(c"NSScreen");
    if screen_class.is_null() {
        return None;
    }
    // SAFETY: `mainScreen` is a `(id) -> id` class method and may return nil.
    let main: Id = unsafe { msg0(screen_class, sel(c"mainScreen")) };
    if main.is_null() {
        return None;
    }

    // SAFETY: `frame`/`visibleFrame` are documented `(id) -> NSRect` messages on
    // a non-nil `NSScreen`.
    let (frame, visible): (NsRect, NsRect) =
        unsafe { (msg0(main, sel(c"frame")), msg0(main, sel(c"visibleFrame"))) };

    // Flip AppKit's bottom-left origin into top-left CG space. For the main
    // screen `frame.origin` is (0, 0), so the y flip uses its height.
    let cg_y = frame.size.height - (visible.origin.y + visible.size.height);
    Some(Area::new(
        visible.origin.x as f32,
        cg_y as f32,
        visible.size.width as f32,
        visible.size.height as f32,
    ))
}

/// The usable frame (menu bar and Dock excluded) of the display with
/// CoreGraphics id `display_id`, in top-left CG coordinates. Walks
/// `[NSScreen screens]`, matching each screen's `NSScreenNumber` device key to
/// `display_id`. Returns `None` if no screen matches or `NSScreen` is
/// unavailable — callers fall back to the display's full bounds.
pub fn visible_frame_for_display(display_id: u32) -> Option<Area> {
    let screen_class = class(c"NSScreen");
    if screen_class.is_null() {
        return None;
    }
    // SAFETY: `+[NSScreen screens]` returns an `NSArray` (or nil).
    let screens: Id = unsafe { msg0(screen_class, sel(c"screens")) };
    if screens.is_null() {
        return None;
    }
    // SAFETY: `-[NSArray count]` returns `NSUInteger`.
    let count: usize = unsafe { msg0(screens, sel(c"count")) };

    // The flip reference is the height of the AppKit zero-origin (primary)
    // screen, whose top-left is the CG global origin.
    let primary_height = primary_screen_height(screen_class, screens, count);
    let key = ns_string("NSScreenNumber");

    for index in 0..count {
        // SAFETY: `index < count`; `objectAtIndex:` returns the `NSScreen` there.
        let screen: Id = unsafe { msg1(screens, sel(c"objectAtIndex:"), index) };
        if screen.is_null() {
            continue;
        }
        // SAFETY: `deviceDescription` returns an `NSDictionary` (or nil).
        let desc: Id = unsafe { msg0(screen, sel(c"deviceDescription")) };
        if desc.is_null() {
            continue;
        }
        // SAFETY: `objectForKey:` takes an `id` key and returns the value (or nil).
        let number: Id = unsafe { msg1(desc, sel(c"objectForKey:"), key) };
        if number.is_null() {
            continue;
        }
        // SAFETY: the `NSScreenNumber` value is an `NSNumber`; `unsignedIntValue`
        // returns its `u32` CoreGraphics display id.
        let sid: u32 = unsafe { msg0(number, sel(c"unsignedIntValue")) };
        if sid != display_id {
            continue;
        }

        // SAFETY: `visibleFrame` is a `(id) -> NSRect` message on a non-nil screen.
        let visible: NsRect = unsafe { msg0(screen, sel(c"visibleFrame")) };
        let cg_y = primary_height - (visible.origin.y + visible.size.height);
        return Some(Area::new(
            visible.origin.x as f32,
            cg_y as f32,
            visible.size.width as f32,
            visible.size.height as f32,
        ));
    }
    None
}

/// Height of the AppKit primary screen (the one at global origin `(0, 0)`),
/// used to flip every screen's frame into CG top-left coordinates.
fn primary_screen_height(screen_class: Id, screens: Id, count: usize) -> f64 {
    for index in 0..count {
        // SAFETY: `index < count`; the element is an `NSScreen`.
        let screen: Id = unsafe { msg1(screens, sel(c"objectAtIndex:"), index) };
        if screen.is_null() {
            continue;
        }
        // SAFETY: `frame` is a `(id) -> NSRect` message on a non-nil screen.
        let frame: NsRect = unsafe { msg0(screen, sel(c"frame")) };
        if frame.origin.x == 0.0 && frame.origin.y == 0.0 {
            return frame.size.height;
        }
    }
    // Fallback: the main (key) screen's height.
    // SAFETY: `mainScreen` may be nil; guard before messaging it.
    let main: Id = unsafe { msg0(screen_class, sel(c"mainScreen")) };
    if !main.is_null() {
        // SAFETY: `frame` is a `(id) -> NSRect` message on a non-nil screen.
        let frame: NsRect = unsafe { msg0(main, sel(c"frame")) };
        return frame.size.height;
    }
    0.0
}

/// Build an autoreleased `NSString` from a Rust string for use as a dictionary
/// key. Lives only for the current autorelease pool, which spans these calls.
fn ns_string(value: &str) -> Id {
    let c = CString::new(value).expect("NSScreenNumber key has no interior NUL");
    let string_class = class(c"NSString");
    // SAFETY: `+[NSString stringWithUTF8String:]` takes a C string and returns an
    // autoreleased `NSString`.
    unsafe {
        msg1(
            string_class,
            sel(c"stringWithUTF8String:"),
            c.as_ptr() as *const c_char,
        )
    }
}
