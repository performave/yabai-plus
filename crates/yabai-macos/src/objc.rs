//! Minimal Objective-C runtime glue shared by the AppKit-touching modules
//! (`screen.rs`, `workspace.rs`). Keeps the `objc_msgSend` transmute pattern in
//! one audited place instead of duplicating it per module.
//!
//! `objc_msgSend` is declared once and reinterpreted, per call site, with the
//! concrete method ABI. On arm64 this is the correct approach for every return
//! shape including by-value structs (the indirect-result register is handled by
//! the Rust `extern "C"` fn-pointer lowering), so no `_stret` variant is needed.

#![cfg(target_os = "macos")]

use std::ffi::CStr;
use std::ffi::c_void;
use std::mem::transmute;
use std::os::raw::c_char;

pub type Id = *mut c_void;
pub type Sel = *const c_void;
pub type Class = *mut c_void;

// AppKit is linked so AppKit classes (`NSScreen`, `NSWorkspace`, ...) are
// registered before any `objc_getClass` lookup runs.
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
}

/// Look up a registered class by name (null if it is not loaded).
pub fn class(name: &CStr) -> Class {
    // SAFETY: `name` is a valid NUL-terminated string; `objc_getClass` only reads it.
    unsafe { objc_getClass(name.as_ptr()) }
}

/// Register (intern) a selector by name.
pub fn sel(name: &CStr) -> Sel {
    // SAFETY: `name` is a valid NUL-terminated string; `sel_registerName` only reads it.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// Send a no-argument message, returning `R` by the C ABI.
///
/// # Safety
/// `receiver` must understand `selector`, and `R` must be the selector's real
/// return type. Calling with a mismatched `R` is undefined behaviour.
pub unsafe fn msg0<R>(receiver: *mut c_void, selector: Sel) -> R {
    // SAFETY: reinterpret `objc_msgSend` with the concrete `(id, SEL) -> R` ABI;
    // the caller guarantees the receiver/selector/return-type match.
    let f: extern "C" fn(*mut c_void, Sel) -> R = unsafe { transmute(objc_msgSend as *const ()) };
    f(receiver, selector)
}

/// Send a one-argument message, returning `R` by the C ABI.
///
/// # Safety
/// As [`msg0`], and `A` must be the selector's real argument type.
pub unsafe fn msg1<A, R>(receiver: *mut c_void, selector: Sel, arg: A) -> R {
    // SAFETY: reinterpret with the concrete `(id, SEL, A) -> R` ABI; the caller
    // guarantees the receiver/selector/argument/return types match.
    let f: extern "C" fn(*mut c_void, Sel, A) -> R =
        unsafe { transmute(objc_msgSend as *const ()) };
    f(receiver, selector, arg)
}
