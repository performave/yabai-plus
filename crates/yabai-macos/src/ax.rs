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
use std::io;
use std::os::raw::c_char;

use yabai_core::{Area, WindowFrame};
use yabai_runtime::LayoutSink;

// --- minimal CoreFoundation / ApplicationServices FFI ---

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFArrayRef = *const c_void;
type CFAllocatorRef = *const c_void;
type AXUIElementRef = *const c_void;
type AXValueRef = *const c_void;
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

// `kAXValueTypeCGPoint` / `kAXValueTypeCGSize` from <ApplicationServices/AXValue.h>.
const K_AX_VALUE_TYPE_CG_POINT: u32 = 1;
const K_AX_VALUE_TYPE_CG_SIZE: u32 = 2;
// `kCFStringEncodingUTF8`.
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: CFTypeRef;

    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> CFTypeRef;
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;

    fn AXIsProcessTrusted() -> Boolean;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> i32;
    fn AXValueCreate(the_type: u32, value_ptr: *const c_void) -> AXValueRef;
    fn AXValueGetValue(value: AXValueRef, the_type: u32, value_ptr: *mut c_void) -> Boolean;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
}

#[link(name = "SkyLight", kind = "framework")]
unsafe extern "C" {
    fn _AXUIElementGetWindow(element: AXUIElementRef, window_id: *mut u32) -> i32;
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

/// A discovered Accessibility window plus its CoreGraphics window id.
pub struct DiscoveredAxWindow {
    pub id: u32,
    pub window: AxWindow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxDiagnostics {
    pub trusted: bool,
    pub system_focused_window_id: Option<u32>,
    pub focused_app_pid: Option<i32>,
    pub focused_app_window_id: Option<u32>,
    pub focused_app_window_count: Option<usize>,
    pub focused_app_window_ids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxPidDiagnostics {
    pub trusted: bool,
    pub app_created: bool,
    pub app_pid: Option<i32>,
    pub windows_error: Option<i32>,
    pub windows_count: Option<usize>,
    pub window_ids: Vec<u32>,
}

pub fn accessibility_trusted() -> bool {
    // SAFETY: simple process-global Accessibility trust query; no pointers.
    unsafe { AXIsProcessTrusted() != 0 }
}

pub fn accessibility_trusted_with_prompt() -> bool {
    // SAFETY: `kAXTrustedCheckOptionPrompt` and `kCFBooleanTrue` are valid system
    // constants. The dictionary is created for the duration of the trust query
    // and then released.
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            std::ptr::null(),
            std::ptr::null(),
        );
        if options.is_null() {
            return AXIsProcessTrusted() != 0;
        }

        let trusted = AXIsProcessTrustedWithOptions(options) != 0;
        CFRelease(options);
        trusted
    }
}

/// Return the currently focused Accessibility window, if one can be resolved.
pub fn focused_window() -> io::Result<Option<DiscoveredAxWindow>> {
    if !accessibility_trusted() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Accessibility permission is not granted",
        ));
    }

    let mut value = system_attribute(b"AXFocusedWindow\0");
    if value.is_null() {
        value = focused_application_window();
    }
    if !value.is_null() && window_id(value).is_none() {
        // SAFETY: `value` is owned and will not be adopted; release before the
        // broader focused-application window-list fallback.
        unsafe { CFRelease(value) };
        value = std::ptr::null();
    }
    if value.is_null() {
        value = focused_application_first_mappable_window();
    }

    adopt_window(value)
}

pub fn focused_window_diagnostics() -> AxDiagnostics {
    let trusted = accessibility_trusted();
    if !trusted {
        return AxDiagnostics {
            trusted,
            system_focused_window_id: None,
            focused_app_pid: None,
            focused_app_window_id: None,
            focused_app_window_count: None,
            focused_app_window_ids: Vec::new(),
        };
    }

    let system_focused_window = system_attribute(b"AXFocusedWindow\0");
    let system_focused_window_id = window_id(system_focused_window);
    if !system_focused_window.is_null() {
        // SAFETY: `system_focused_window` is an owned AX element from CopyAttributeValue.
        unsafe { CFRelease(system_focused_window) };
    }

    let app = system_attribute(b"AXFocusedApplication\0");
    let focused_app_pid = ax_pid(app);
    let mut focused_app_window_id = None;
    let mut focused_app_window_count = None;
    let mut focused_app_window_ids = Vec::new();

    if !app.is_null() {
        // SAFETY: creates owned CFStrings for the duration of the copy attempts.
        let (focused_window_attr, windows_attr) =
            unsafe { (cfstring(b"AXFocusedWindow\0"), cfstring(b"AXWindows\0")) };
        if !focused_window_attr.is_null() {
            let focused_window = copy_attribute(app as AXUIElementRef, focused_window_attr);
            focused_app_window_id = window_id(focused_window);
            if !focused_window.is_null() {
                // SAFETY: `focused_window` is an owned AX element from CopyAttributeValue.
                unsafe { CFRelease(focused_window) };
            }
            // SAFETY: owned CFString no longer needed.
            unsafe { CFRelease(focused_window_attr) };
        }
        if !windows_attr.is_null() {
            let windows = copy_attribute(app as AXUIElementRef, windows_attr);
            if !windows.is_null() {
                // SAFETY: `windows` is an owned CFArray returned by CopyAttributeValue.
                unsafe {
                    let count = CFArrayGetCount(windows as CFArrayRef);
                    focused_app_window_count = Some(count.max(0) as usize);
                    for idx in 0..count {
                        let window = CFArrayGetValueAtIndex(windows as CFArrayRef, idx);
                        if let Some(id) = window_id(window) {
                            focused_app_window_ids.push(id);
                        }
                    }
                    CFRelease(windows);
                }
            }
            // SAFETY: owned CFString no longer needed.
            unsafe { CFRelease(windows_attr) };
        }
        // SAFETY: `app` is an owned AX element from CopyAttributeValue.
        unsafe { CFRelease(app) };
    }

    AxDiagnostics {
        trusted,
        system_focused_window_id,
        focused_app_pid,
        focused_app_window_id,
        focused_app_window_count,
        focused_app_window_ids,
    }
}

pub fn windows_for_pid(pid: i32) -> io::Result<Vec<DiscoveredAxWindow>> {
    if !accessibility_trusted() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Accessibility permission is not granted",
        ));
    }

    // SAFETY: creates an owned AX application element for `pid`, released below.
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Ok(Vec::new());
    }

    let windows = application_windows(app);
    // SAFETY: `app` is an owned AX element from CreateApplication.
    unsafe { CFRelease(app) };
    Ok(windows)
}

pub fn windows_for_pid_diagnostics(pid: i32) -> AxPidDiagnostics {
    let trusted = accessibility_trusted();
    if !trusted {
        return AxPidDiagnostics {
            trusted,
            app_created: false,
            app_pid: None,
            windows_error: None,
            windows_count: None,
            window_ids: Vec::new(),
        };
    }

    // SAFETY: creates an owned AX application element for `pid`, released below.
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return AxPidDiagnostics {
            trusted,
            app_created: false,
            app_pid: None,
            windows_error: None,
            windows_count: None,
            window_ids: Vec::new(),
        };
    }

    let app_pid = ax_pid(app);
    let (windows_error, windows_count, window_ids) = application_window_diagnostics(app);
    // SAFETY: `app` is an owned AX element from CreateApplication.
    unsafe { CFRelease(app) };

    AxPidDiagnostics {
        trusted,
        app_created: true,
        app_pid,
        windows_error,
        windows_count,
        window_ids,
    }
}

fn focused_application_window() -> CFTypeRef {
    let app = system_attribute(b"AXFocusedApplication\0");
    if app.is_null() {
        return std::ptr::null();
    }

    // SAFETY: creates an owned CFString for the duration of the copy attempt.
    let focused_window_attr = unsafe { cfstring(b"AXFocusedWindow\0") };
    if focused_window_attr.is_null() {
        // SAFETY: `app` is an owned AX element returned by CopyAttributeValue.
        unsafe { CFRelease(app) };
        return std::ptr::null();
    }

    let window = copy_attribute(app as AXUIElementRef, focused_window_attr);
    // SAFETY: both refs are owned and no longer needed.
    unsafe {
        CFRelease(app);
        CFRelease(focused_window_attr);
    }
    window
}

fn focused_application_first_mappable_window() -> CFTypeRef {
    let app = system_attribute(b"AXFocusedApplication\0");
    if app.is_null() {
        return std::ptr::null();
    }

    // SAFETY: creates an owned CFString for the duration of the copy attempt.
    let windows_attr = unsafe { cfstring(b"AXWindows\0") };
    if windows_attr.is_null() {
        // SAFETY: `app` is an owned AX element returned by CopyAttributeValue.
        unsafe { CFRelease(app) };
        return std::ptr::null();
    }

    let windows = copy_attribute(app as AXUIElementRef, windows_attr);
    // SAFETY: no longer need the focused app or attribute string.
    unsafe {
        CFRelease(app);
        CFRelease(windows_attr);
    }
    if windows.is_null() {
        return std::ptr::null();
    }

    let mut result = std::ptr::null();
    // SAFETY: `windows` is an owned CFArray returned by CopyAttributeValue.
    unsafe {
        let count = CFArrayGetCount(windows as CFArrayRef);
        for idx in 0..count {
            let window = CFArrayGetValueAtIndex(windows as CFArrayRef, idx);
            if window.is_null() {
                continue;
            }
            if window_id(window).is_some() {
                result = CFRetain(window);
                break;
            }
        }
        CFRelease(windows);
    }
    result
}

fn application_windows(app: AXUIElementRef) -> Vec<DiscoveredAxWindow> {
    // SAFETY: creates an owned CFString for the duration of the copy attempt.
    let windows_attr = unsafe { cfstring(b"AXWindows\0") };
    if windows_attr.is_null() {
        return Vec::new();
    }

    let windows = copy_attribute(app, windows_attr);
    // SAFETY: owned CFString no longer needed.
    unsafe { CFRelease(windows_attr) };
    if windows.is_null() {
        return Vec::new();
    }

    let mut result = Vec::new();
    // SAFETY: `windows` is an owned CFArray returned by CopyAttributeValue.
    unsafe {
        let count = CFArrayGetCount(windows as CFArrayRef);
        for idx in 0..count {
            let window = CFArrayGetValueAtIndex(windows as CFArrayRef, idx);
            let Some(id) = window_id(window) else {
                continue;
            };
            let retained = CFRetain(window);
            let window = AxWindow::from_raw(retained);
            result.push(DiscoveredAxWindow { id, window });
        }
        CFRelease(windows);
    }
    result
}

fn application_window_diagnostics(app: AXUIElementRef) -> (Option<i32>, Option<usize>, Vec<u32>) {
    // SAFETY: creates an owned CFString for the duration of the copy attempt.
    let windows_attr = unsafe { cfstring(b"AXWindows\0") };
    if windows_attr.is_null() {
        return (None, None, Vec::new());
    }

    let mut windows: CFTypeRef = std::ptr::null();
    // SAFETY: `app` and `windows_attr` are valid refs, and `windows` is valid
    // writable storage for the retained AXWindows attribute value.
    let err = unsafe { AXUIElementCopyAttributeValue(app, windows_attr, &mut windows) };
    // SAFETY: owned CFString no longer needed.
    unsafe { CFRelease(windows_attr) };
    if err != 0 || windows.is_null() {
        return (Some(err), None, Vec::new());
    }

    let mut ids = Vec::new();
    // SAFETY: `windows` is an owned CFArray returned by CopyAttributeValue.
    let count = unsafe {
        let count = CFArrayGetCount(windows as CFArrayRef);
        for idx in 0..count {
            let window = CFArrayGetValueAtIndex(windows as CFArrayRef, idx);
            if let Some(id) = window_id(window) {
                ids.push(id);
            }
        }
        CFRelease(windows);
        count.max(0) as usize
    };

    (Some(0), Some(count), ids)
}

fn system_attribute(attribute: &[u8]) -> CFTypeRef {
    // SAFETY: creates owned AX/CF refs; each non-null ref is released below.
    let (system, attr) = unsafe { (AXUIElementCreateSystemWide(), cfstring(attribute)) };
    if system.is_null() || attr.is_null() {
        // SAFETY: release only the owned refs that were actually created.
        unsafe {
            if !system.is_null() {
                CFRelease(system);
            }
            if !attr.is_null() {
                CFRelease(attr);
            }
        }
        return std::ptr::null();
    }

    let value = copy_attribute(system, attr);
    // SAFETY: these owned refs are no longer needed after the copy attempt.
    unsafe {
        CFRelease(system);
        CFRelease(attr);
    }
    value
}

fn copy_attribute(element: AXUIElementRef, attribute: CFStringRef) -> CFTypeRef {
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: `element` and `attribute` are valid refs, and `value` is valid
    // writable storage for the retained attribute value.
    let err = unsafe { AXUIElementCopyAttributeValue(element, attribute, &mut value) };
    if err == 0 { value } else { std::ptr::null() }
}

fn adopt_window(value: CFTypeRef) -> io::Result<Option<DiscoveredAxWindow>> {
    if value.is_null() {
        return Ok(None);
    }

    let Some(id) = window_id(value) else {
        // SAFETY: `value` is still owned here because it was not adopted.
        unsafe { CFRelease(value) };
        return Ok(None);
    };

    // SAFETY: `value` is a retained AXUIElementRef from CopyAttributeValue;
    // ownership transfers to `AxWindow`.
    let window = unsafe { AxWindow::from_raw(value) };
    Ok(Some(DiscoveredAxWindow { id, window }))
}

fn window_id(window: CFTypeRef) -> Option<u32> {
    if window.is_null() {
        return None;
    }
    let mut id = 0u32;
    // SAFETY: `window` is expected to be an AX window element. The private helper
    // writes the associated CoreGraphics window id when present.
    let err = unsafe { _AXUIElementGetWindow(window as AXUIElementRef, &mut id) };
    (err == 0 && id != 0).then_some(id)
}

/// Apply a frame to an AX window element by setting `kAXPosition` then `kAXSize`,
/// a faithful port of `window_manager_move_window` / `_resize_window`. The caller
/// owns `element`, `position_attr`, and `size_attr` for the duration of the call.
fn set_window_frame(
    element: AXUIElementRef,
    position_attr: CFStringRef,
    size_attr: CFStringRef,
    area: Area,
) {
    if element.is_null() {
        return;
    }
    let position = CGPoint {
        x: area.x as f64,
        y: area.y as f64,
    };
    let size = CGSize {
        width: area.w as f64,
        height: area.h as f64,
    };

    // SAFETY: `element` is a live AX ref and the attribute strings are valid.
    // AXValueCreate copies the point/size; each value is released after the set,
    // matching window_manager_move/resize_window.
    unsafe {
        let position_ref = AXValueCreate(
            K_AX_VALUE_TYPE_CG_POINT,
            &position as *const _ as *const c_void,
        );
        if !position_ref.is_null() {
            AXUIElementSetAttributeValue(element, position_attr, position_ref);
            CFRelease(position_ref);
        }

        let size_ref = AXValueCreate(K_AX_VALUE_TYPE_CG_SIZE, &size as *const _ as *const c_void);
        if !size_ref.is_null() {
            AXUIElementSetAttributeValue(element, size_attr, size_ref);
            CFRelease(size_ref);
        }
    }
}

/// Read an AX window element's current `kAXPosition`/`kAXSize` back into an `Area`.
/// Returns `None` if either attribute is missing or not a CGPoint/CGSize value.
fn read_window_frame(element: AXUIElementRef) -> Option<Area> {
    if element.is_null() {
        return None;
    }
    // SAFETY: both literals are NUL-terminated; the CFStrings are released below.
    let (position_attr, size_attr) = unsafe { (cfstring(b"AXPosition\0"), cfstring(b"AXSize\0")) };

    let position = copy_attribute(element, position_attr);
    let size = copy_attribute(element, size_attr);

    let mut point = CGPoint { x: 0.0, y: 0.0 };
    let mut dims = CGSize {
        width: 0.0,
        height: 0.0,
    };
    // SAFETY: `position`/`size` (when non-null) are AXValueRefs returned by
    // CopyAttributeValue; AXValueGetValue unpacks the CGPoint/CGSize into the
    // local storage and reports whether the value matched the requested type.
    let ok = unsafe {
        let got_point = !position.is_null()
            && AXValueGetValue(
                position,
                K_AX_VALUE_TYPE_CG_POINT,
                &mut point as *mut _ as *mut c_void,
            ) != 0;
        let got_size = !size.is_null()
            && AXValueGetValue(
                size,
                K_AX_VALUE_TYPE_CG_SIZE,
                &mut dims as *mut _ as *mut c_void,
            ) != 0;
        got_point && got_size
    };

    // SAFETY: release the owned attribute values and CFStrings (null is skipped).
    unsafe {
        if !position.is_null() {
            CFRelease(position);
        }
        if !size.is_null() {
            CFRelease(size);
        }
        if !position_attr.is_null() {
            CFRelease(position_attr);
        }
        if !size_attr.is_null() {
            CFRelease(size_attr);
        }
    }

    ok.then(|| {
        Area::new(
            point.x as f32,
            point.y as f32,
            dims.width as f32,
            dims.height as f32,
        )
    })
}

/// Resolve the system-wide focused window's AX element directly (without going
/// through the `_AXUIElementGetWindow` CG-id mapping) and apply `area` to it.
///
/// This is the live, end-to-end exercise of the move path: it proves `AxSink`'s
/// position/size logic works on a real window even while the CG-id discovery is
/// still unresolved. Returns the window's frame as read back after the move.
pub fn move_focused_window(area: Area) -> io::Result<Area> {
    if !accessibility_trusted() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Accessibility permission is not granted",
        ));
    }

    let window = system_attribute(b"AXFocusedWindow\0");
    let window = if window.is_null() {
        focused_application_window()
    } else {
        window
    };
    if window.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no focused AX window could be resolved",
        ));
    }

    // SAFETY: both literals are NUL-terminated; the CFStrings are released below.
    let (position_attr, size_attr) = unsafe { (cfstring(b"AXPosition\0"), cfstring(b"AXSize\0")) };
    set_window_frame(window, position_attr, size_attr, area);
    // SAFETY: owned CFStrings no longer needed after the set.
    unsafe {
        if !position_attr.is_null() {
            CFRelease(position_attr);
        }
        if !size_attr.is_null() {
            CFRelease(size_attr);
        }
    }

    let result = read_window_frame(window).unwrap_or(area);
    // SAFETY: `window` is an owned AX element from CopyAttributeValue / CFRetain.
    unsafe { CFRelease(window) };
    Ok(result)
}

/// Move/resize the `index`-th `AXWindows` entry of the application with `pid`,
/// operating directly on the retained AX element without requiring its
/// CoreGraphics window id. This is the reliable live-movement path while the
/// `_AXUIElementGetWindow` CG-id mapping is still unresolved (see handoff).
/// Returns the window's frame as read back after the move.
pub fn move_pid_window(pid: i32, index: usize, area: Area) -> io::Result<Area> {
    if !accessibility_trusted() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Accessibility permission is not granted",
        ));
    }

    // SAFETY: creates an owned AX application element for `pid`, released below.
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no AX application element for pid {pid}"),
        ));
    }

    let window = retained_application_window(app, index);
    // SAFETY: `app` is an owned AX element from CreateApplication.
    unsafe { CFRelease(app) };

    let Some(window) = window else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("pid {pid} has no AX window at index {index}"),
        ));
    };

    // SAFETY: both literals are NUL-terminated; the CFStrings are released below.
    let (position_attr, size_attr) = unsafe { (cfstring(b"AXPosition\0"), cfstring(b"AXSize\0")) };
    set_window_frame(window.element, position_attr, size_attr, area);
    // SAFETY: owned CFStrings no longer needed after the set.
    unsafe {
        if !position_attr.is_null() {
            CFRelease(position_attr);
        }
        if !size_attr.is_null() {
            CFRelease(size_attr);
        }
    }

    Ok(read_window_frame(window.element).unwrap_or(area))
}

/// Retain and return the `index`-th `AXWindows` element of `app`, regardless of
/// whether it has a resolvable CG window id.
fn retained_application_window(app: AXUIElementRef, index: usize) -> Option<AxWindow> {
    // SAFETY: creates an owned CFString for the duration of the copy attempt.
    let windows_attr = unsafe { cfstring(b"AXWindows\0") };
    if windows_attr.is_null() {
        return None;
    }
    let windows = copy_attribute(app, windows_attr);
    // SAFETY: owned CFString no longer needed.
    unsafe { CFRelease(windows_attr) };
    if windows.is_null() {
        return None;
    }

    // SAFETY: `windows` is an owned CFArray returned by CopyAttributeValue; the
    // element at `index` is borrowed, so retain it before the array is released.
    let element = unsafe {
        let count = CFArrayGetCount(windows as CFArrayRef);
        let result = if (index as CFIndex) < count {
            let window = CFArrayGetValueAtIndex(windows as CFArrayRef, index as CFIndex);
            if window.is_null() {
                std::ptr::null()
            } else {
                CFRetain(window)
            }
        } else {
            std::ptr::null()
        };
        CFRelease(windows);
        result
    };

    if element.is_null() {
        return None;
    }
    // SAFETY: `element` was just retained; ownership transfers to `AxWindow`.
    Some(unsafe { AxWindow::from_raw(element) })
}

fn ax_pid(element: CFTypeRef) -> Option<i32> {
    if element.is_null() {
        return None;
    }
    let mut pid = 0i32;
    // SAFETY: `element` is expected to be an AX application/window element. The
    // call writes a pid on success.
    let err = unsafe { AXUIElementGetPid(element as AXUIElementRef, &mut pid) };
    (err == 0).then_some(pid)
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
        set_window_frame(
            window.element,
            self.position_attr,
            self.size_attr,
            frame.area,
        );
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

    #[test]
    fn focused_window_probe_does_not_panic_when_trusted() {
        if accessibility_trusted() {
            let _ = focused_window().unwrap();
        }
    }
}
