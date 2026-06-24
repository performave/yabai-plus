//! Usable (visible) display frame discovery via `NSScreen`.
//!
//! `CGDisplayBounds` returns the *full* display rectangle, which includes the
//! menu bar and the Dock. The layout engine should tile inside the region the
//! user can actually use, exactly as the C daemon does when it derives a space's
//! root area. `NSScreen.visibleFrame` gives that region, but in AppKit's
//! bottom-left coordinate space; this module flips it back into the top-left
//! CoreGraphics space the rest of the code (and AX) uses.
//!
//! All FFI is local and the unsafe surface is confined here, matching `ax.rs`.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::mem::transmute;
use std::os::raw::c_char;

use yabai_core::Area;

type Id = *mut c_void;
type Sel = *const c_void;
type Class = *mut c_void;

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

// AppKit is linked so the `NSScreen` class is registered; the calls go through
// the Objective-C runtime in `libobjc`.
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
}

/// Send a no-argument message returning an object pointer.
unsafe fn msg_id(receiver: *mut c_void, sel: Sel) -> Id {
    // SAFETY: `objc_msgSend`'s address is reinterpreted with the concrete ABI of
    // a `(id, SEL) -> id` call; the caller guarantees `receiver` understands `sel`.
    let f: extern "C" fn(*mut c_void, Sel) -> Id = unsafe { transmute(objc_msgSend as *const ()) };
    f(receiver, sel)
}

/// Send a no-argument message returning an `NSRect` (by value).
unsafe fn msg_rect(receiver: Id, sel: Sel) -> NsRect {
    // SAFETY: as above, but for the `(id, SEL) -> NSRect` ABI. On arm64 a 32-byte
    // struct return is handled via the indirect-result register, which the Rust
    // extern "C" fn pointer lowering matches.
    let f: extern "C" fn(Id, Sel) -> NsRect = unsafe { transmute(objc_msgSend as *const ()) };
    f(receiver, sel)
}

/// The main display's usable frame (menu bar and Dock excluded), in top-left
/// CoreGraphics coordinates. Returns `None` if `NSScreen` is unavailable.
///
/// This is correct for a single-display setup; with multiple displays it
/// describes the main screen only (the others fall back to their full bounds).
pub fn main_visible_frame() -> Option<Area> {
    // SAFETY: standard Objective-C runtime lookups; every selector below is a
    // documented `NSScreen` message, and `mainScreen` may return nil (checked).
    unsafe {
        let class = objc_getClass(c"NSScreen".as_ptr());
        if class.is_null() {
            return None;
        }
        let main = msg_id(class, sel_registerName(c"mainScreen".as_ptr()));
        if main.is_null() {
            return None;
        }

        let frame = msg_rect(main, sel_registerName(c"frame".as_ptr()));
        let visible = msg_rect(main, sel_registerName(c"visibleFrame".as_ptr()));

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
}
