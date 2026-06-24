//! Usable (visible) display frame discovery via `NSScreen`.
//!
//! `CGDisplayBounds` returns the *full* display rectangle, which includes the
//! menu bar and the Dock. The layout engine should tile inside the region the
//! user can actually use, exactly as the C daemon does when it derives a space's
//! root area. `NSScreen.visibleFrame` gives that region, but in AppKit's
//! bottom-left coordinate space; this module flips it back into the top-left
//! CoreGraphics space the rest of the code (and AX) uses.

#![cfg(target_os = "macos")]

use yabai_core::Area;

use crate::objc::{Id, class, msg0, sel};

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
