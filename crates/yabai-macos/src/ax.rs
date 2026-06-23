//! Accessibility-API window moving, ported from `window_manager_move_window`
//! and `window_manager_resize_window` in `src/window_manager.c`.
//!
//! This is the first piece of the macOS boundary (Phase 5). It implements
//! [`yabai_runtime::LayoutSink`] by setting `kAXPosition`/`kAXSize` on a window's
//! `AXUIElementRef`. The control plane (`yabai-core`/`yabai-runtime`) never sees
//! these refs; it speaks in `WindowFrame`s, and this sink owns the id -> ref map.
//!
//! All FFI is declared locally to keep the crate dependency-free, mirroring the
//! workspace's other crates. The unsafe surface is small and confined here.

#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::c_char;

use yabai_core::WindowFrame;
use yabai_runtime::LayoutSink;

// --- minimal CoreFoundation / ApplicationServices FFI ---

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFAllocatorRef = *const c_void;
type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;

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

// `kAXValueTypeCGPoint` / `kAXValueTypeCGSize` from <ApplicationServices/AXValue.h>.
const K_AX_VALUE_TYPE_CG_POINT: u32 = 1;
const K_AX_VALUE_TYPE_CG_SIZE: u32 = 2;
// `kCFStringEncodingUTF8`.
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXValueCreate(the_type: u32, value_ptr: *const c_void) -> AXValueRef;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
}

/// Create a CoreFoundation string from a NUL-terminated ASCII literal.
/// Returns null on failure (callers treat null attributes as a no-op).
unsafe fn cfstring(literal: &[u8]) -> CFStringRef {
    debug_assert_eq!(literal.last(), Some(&0), "literal must be NUL-terminated");
    // SAFETY: `literal` is a valid NUL-terminated UTF-8 buffer; CoreFoundation
    // copies its contents, so the borrow need not outlive the call.
    unsafe {
        CFStringCreateWithCString(
            std::ptr::null(),
            literal.as_ptr() as *const c_char,
            K_CF_STRING_ENCODING_UTF8,
        )
    }
}

/// An owned `AXUIElementRef` for a managed window. Releases on drop, like the C
/// daemon's `CFRelease(window->ref)`.
pub struct AxWindow {
    element: AXUIElementRef,
}

impl AxWindow {
    /// Adopt an `AXUIElementRef` (this takes ownership of one retain count).
    ///
    /// # Safety
    /// `element` must be a valid, retained `AXUIElementRef`. Ownership transfers
    /// to the returned `AxWindow`, which releases it on drop.
    pub unsafe fn from_raw(element: *const c_void) -> Self {
        Self { element }
    }
}

impl Drop for AxWindow {
    fn drop(&mut self) {
        if !self.element.is_null() {
            // SAFETY: `element` was adopted via `from_raw` with one owned retain
            // count and is non-null here; this balances that retain exactly once.
            unsafe { CFRelease(self.element) };
        }
    }
}

/// A [`LayoutSink`] that applies frames to real windows via the Accessibility
/// API. Holds the window-id -> `AXUIElementRef` map the control plane lacks.
pub struct AxSink {
    windows: HashMap<u32, AxWindow>,
    position_attr: CFStringRef,
    size_attr: CFStringRef,
}

// SAFETY: AX element refs are thread-affine in practice, and like the C daemon
// this sink is owned and used by a single worker thread (the runtime actor).
// The only cross-thread move is into that actor at spawn time.
unsafe impl Send for AxSink {}

impl Default for AxSink {
    fn default() -> Self {
        Self::new()
    }
}

impl AxSink {
    pub fn new() -> Self {
        // SAFETY: both literals are NUL-terminated; the resulting CFStrings are
        // owned by the sink and released in `Drop`.
        let (position_attr, size_attr) =
            unsafe { (cfstring(b"AXPosition\0"), cfstring(b"AXSize\0")) };
        Self {
            windows: HashMap::new(),
            position_attr,
            size_attr,
        }
    }

    /// Register a window's accessibility element so frames can be applied to it.
    pub fn register(&mut self, window_id: u32, window: AxWindow) {
        self.windows.insert(window_id, window);
    }

    /// Forget a window (e.g. on `WINDOW_DESTROYED`), releasing its element.
    pub fn unregister(&mut self, window_id: u32) {
        self.windows.remove(&window_id);
    }

    pub fn is_registered(&self, window_id: u32) -> bool {
        self.windows.contains_key(&window_id)
    }
}

impl LayoutSink for AxSink {
    fn move_window(&mut self, frame: WindowFrame) {
        let Some(window) = self.windows.get(&frame.window_id) else {
            // Not yet known to the sink; nothing to move.
            return;
        };
        let element = window.element;
        if element.is_null() {
            return;
        }

        let position = CGPoint {
            x: frame.area.x as f64,
            y: frame.area.y as f64,
        };
        let size = CGSize {
            width: frame.area.w as f64,
            height: frame.area.h as f64,
        };

        // SAFETY: `element` is a live, owned AX ref; the attribute strings are
        // valid; AXValueCreate copies the point/size, and we release each value
        // after setting it — matching window_manager_move/resize_window.
        unsafe {
            let position_ref = AXValueCreate(
                K_AX_VALUE_TYPE_CG_POINT,
                &position as *const _ as *const c_void,
            );
            if !position_ref.is_null() {
                AXUIElementSetAttributeValue(element, self.position_attr, position_ref);
                CFRelease(position_ref);
            }

            let size_ref =
                AXValueCreate(K_AX_VALUE_TYPE_CG_SIZE, &size as *const _ as *const c_void);
            if !size_ref.is_null() {
                AXUIElementSetAttributeValue(element, self.size_attr, size_ref);
                CFRelease(size_ref);
            }
        }
    }
}

impl Drop for AxSink {
    fn drop(&mut self) {
        // SAFETY: both attributes were created by `CFStringCreateWithCString`
        // (or are null, which CFRelease must not see) and are owned here.
        unsafe {
            if !self.position_attr.is_null() {
                CFRelease(self.position_attr);
            }
            if !self.size_attr.is_null() {
                CFRelease(self.size_attr);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yabai_core::Area;

    #[test]
    fn move_unregistered_window_is_a_noop() {
        // No window registered, so this must not touch FFI/elements.
        let mut sink = AxSink::new();
        assert!(!sink.is_registered(7));
        sink.move_window(WindowFrame {
            window_id: 7,
            area: Area::new(0.0, 0.0, 100.0, 100.0),
        });
        assert!(!sink.is_registered(7));
    }

    #[test]
    fn register_and_unregister_track_windows() {
        let mut sink = AxSink::new();
        // SAFETY: a null element is valid to adopt; Drop guards against null.
        let window = unsafe { AxWindow::from_raw(std::ptr::null()) };
        sink.register(3, window);
        assert!(sink.is_registered(3));
        sink.unregister(3);
        assert!(!sink.is_registered(3));
    }
}
