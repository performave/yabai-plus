//! Running-application discovery via `NSWorkspace`.
//!
//! The C daemon learns about processes from `NSWorkspace` notifications and the
//! process-manager snapshot. This module provides the snapshot half: the pids of
//! "regular" (Dock-visible) applications, which is what the tiler iterates to
//! find manageable windows. The notification half (launch/terminate) is future
//! observer work.
//!
//! All FFI is local and the unsafe surface is confined here, matching `ax.rs`
//! and `screen.rs`.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::mem::transmute;
use std::os::raw::c_char;

type Id = *mut c_void;
type Sel = *const c_void;
type Class = *mut c_void;

// AppKit is linked so `NSWorkspace` is registered; calls go through `libobjc`.
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
}

/// `(id, SEL) -> id`.
unsafe fn msg_id(receiver: *mut c_void, sel: Sel) -> Id {
    // SAFETY: reinterpret `objc_msgSend` with the concrete `(id, SEL) -> id` ABI.
    let f: extern "C" fn(*mut c_void, Sel) -> Id = unsafe { transmute(objc_msgSend as *const ()) };
    f(receiver, sel)
}

/// `(id, SEL, NSUInteger) -> id` (for `objectAtIndex:`).
unsafe fn msg_id_at(receiver: Id, sel: Sel, index: usize) -> Id {
    // SAFETY: reinterpret with the `(id, SEL, NSUInteger) -> id` ABI.
    let f: extern "C" fn(Id, Sel, usize) -> Id = unsafe { transmute(objc_msgSend as *const ()) };
    f(receiver, sel, index)
}

/// `(id, SEL) -> NSUInteger`.
unsafe fn msg_count(receiver: Id, sel: Sel) -> usize {
    // SAFETY: reinterpret with the `(id, SEL) -> NSUInteger` ABI.
    let f: extern "C" fn(Id, Sel) -> usize = unsafe { transmute(objc_msgSend as *const ()) };
    f(receiver, sel)
}

/// `(id, SEL) -> NSInteger`.
unsafe fn msg_isize(receiver: Id, sel: Sel) -> isize {
    // SAFETY: reinterpret with the `(id, SEL) -> NSInteger` ABI.
    let f: extern "C" fn(Id, Sel) -> isize = unsafe { transmute(objc_msgSend as *const ()) };
    f(receiver, sel)
}

/// `(id, SEL) -> pid_t` (a 32-bit int).
unsafe fn msg_pid(receiver: Id, sel: Sel) -> i32 {
    // SAFETY: reinterpret with the `(id, SEL) -> int` ABI.
    let f: extern "C" fn(Id, Sel) -> i32 = unsafe { transmute(objc_msgSend as *const ()) };
    f(receiver, sel)
}

/// Pids of regular, Dock-visible running applications (the ones a tiler should
/// consider). Accessory/prohibited apps (menu-bar extras, agents) are excluded
/// via `NSApplicationActivationPolicyRegular` (`0`).
pub fn regular_application_pids() -> Vec<i32> {
    // SAFETY: a sequence of documented `NSWorkspace` / `NSArray` /
    // `NSRunningApplication` messages; every receiver is null-checked, and the
    // selectors match the declared return ABIs of the `msg_*` helpers.
    unsafe {
        let workspace_class = objc_getClass(c"NSWorkspace".as_ptr());
        if workspace_class.is_null() {
            return Vec::new();
        }
        let workspace = msg_id(
            workspace_class,
            sel_registerName(c"sharedWorkspace".as_ptr()),
        );
        if workspace.is_null() {
            return Vec::new();
        }
        let apps = msg_id(workspace, sel_registerName(c"runningApplications".as_ptr()));
        if apps.is_null() {
            return Vec::new();
        }

        let count = msg_count(apps, sel_registerName(c"count".as_ptr()));
        let object_at = sel_registerName(c"objectAtIndex:".as_ptr());
        let policy_sel = sel_registerName(c"activationPolicy".as_ptr());
        let pid_sel = sel_registerName(c"processIdentifier".as_ptr());

        let mut pids = Vec::new();
        for index in 0..count {
            let app = msg_id_at(apps, object_at, index);
            if app.is_null() {
                continue;
            }
            // NSApplicationActivationPolicyRegular == 0.
            if msg_isize(app, policy_sel) != 0 {
                continue;
            }
            let pid = msg_pid(app, pid_sel);
            if pid > 0 {
                pids.push(pid);
            }
        }
        pids
    }
}
