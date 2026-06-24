//! Running-application discovery and workspace notifications via `NSWorkspace`.
//!
//! The C daemon learns about processes from `NSWorkspace` notifications and the
//! process-manager snapshot. This module provides the pids of "regular"
//! (Dock-visible) applications, which is what the tiler iterates to find
//! manageable windows, plus a narrow active-space notification observer.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, Once, OnceLock};

use crate::objc::{Class, Id, Sel, class, msg0, msg1, msg4, sel};

type CFStringRef = *const c_void;
type CFAllocatorRef = *const c_void;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

static WORKSPACE_OBSERVER_CLASS: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static WORKSPACE_OBSERVER_CLASS_ONCE: Once = Once::new();
static WORKSPACE_EVENT_SENDERS: OnceLock<Mutex<Vec<Sender<WorkspaceEvent>>>> = OnceLock::new();

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRun();
    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Class, name: *const c_char, extra_bytes: usize) -> Class;
    fn objc_registerClassPair(cls: Class);
    fn class_addMethod(cls: Class, name: Sel, imp: *const c_void, types: *const c_char) -> bool;
}

/// A typed `NSWorkspace` notification for the daemon event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEvent {
    ActiveSpaceChanged,
}

extern "C" fn active_space_did_change(_this: Id, _cmd: Sel, _notification: Id) {
    let Some(senders) = WORKSPACE_EVENT_SENDERS.get() else {
        return;
    };
    let Ok(mut senders) = senders.lock() else {
        return;
    };
    senders.retain(|tx| tx.send(WorkspaceEvent::ActiveSpaceChanged).is_ok());
}

fn workspace_observer_class() -> Option<Class> {
    WORKSPACE_OBSERVER_CLASS_ONCE.call_once(|| {
        let superclass = class(c"NSObject");
        if superclass.is_null() {
            return;
        }

        let class_name = c"YabaiRustWorkspaceObserver";
        let mut cls = class(class_name);
        if cls.is_null() {
            // SAFETY: `superclass` is `NSObject`; `class_name` is a unique,
            // NUL-terminated Objective-C class name.
            cls = unsafe { objc_allocateClassPair(superclass, class_name.as_ptr(), 0) };
            if cls.is_null() {
                return;
            }

            let method: extern "C" fn(Id, Sel, Id) = active_space_did_change;
            // SAFETY: `cls` is newly allocated; the selector takes one object
            // argument and returns void (`v@:@`), matching `method`.
            let added = unsafe {
                class_addMethod(
                    cls,
                    sel(c"activeSpaceDidChange:"),
                    method as *const c_void,
                    c"v@:@".as_ptr(),
                )
            };
            if !added {
                return;
            }
            // SAFETY: `cls` has been fully configured and can now be registered.
            unsafe { objc_registerClassPair(cls) };
        }

        WORKSPACE_OBSERVER_CLASS.store(cls, Ordering::SeqCst);
    });

    let cls = WORKSPACE_OBSERVER_CLASS.load(Ordering::SeqCst) as Class;
    (!cls.is_null()).then_some(cls)
}

unsafe fn cfstring(literal: &[u8]) -> CFStringRef {
    debug_assert_eq!(literal.last(), Some(&0), "literal must be NUL-terminated");
    // SAFETY: `literal` is a valid NUL-terminated UTF-8 buffer; CoreFoundation
    // copies it into an owned CFString.
    unsafe {
        CFStringCreateWithCString(
            std::ptr::null(),
            literal.as_ptr() as *const c_char,
            K_CF_STRING_ENCODING_UTF8,
        )
    }
}

/// Pids of regular, Dock-visible running applications (the ones a tiler should
/// consider). Accessory/prohibited apps (menu-bar extras, agents) are excluded
/// via `NSApplicationActivationPolicyRegular` (`0`).
pub fn regular_application_pids() -> Vec<i32> {
    let workspace_class = class(c"NSWorkspace");
    if workspace_class.is_null() {
        return Vec::new();
    }
    // SAFETY: `sharedWorkspace`/`runningApplications` are `(id) -> id` messages;
    // each result is null-checked before use.
    let apps: Id = unsafe {
        let workspace: Id = msg0(workspace_class, sel(c"sharedWorkspace"));
        if workspace.is_null() {
            return Vec::new();
        }
        msg0(workspace, sel(c"runningApplications"))
    };
    if apps.is_null() {
        return Vec::new();
    }

    let object_at = sel(c"objectAtIndex:");
    let policy_sel = sel(c"activationPolicy");
    let pid_sel = sel(c"processIdentifier");

    // SAFETY: `count` is `(id) -> NSUInteger`, `objectAtIndex:` is
    // `(id, NSUInteger) -> id`, `activationPolicy` is `(id) -> NSInteger`, and
    // `processIdentifier` is `(id) -> pid_t (int)` — each matches its `msg*` ABI.
    unsafe {
        let count: usize = msg0(apps, sel(c"count"));
        let mut pids = Vec::new();
        for index in 0..count {
            let app: Id = msg1(apps, object_at, index);
            if app.is_null() {
                continue;
            }
            // NSApplicationActivationPolicyRegular == 0.
            let policy: isize = msg0(app, policy_sel);
            if policy != 0 {
                continue;
            }
            let pid: i32 = msg0(app, pid_sel);
            if pid > 0 {
                pids.push(pid);
            }
        }
        pids
    }
}

/// Observe active-space changes on the current thread, forwarding events to
/// `tx`. This blocks in `CFRunLoopRun`; run it on a dedicated thread.
pub fn observe_active_space(tx: Sender<WorkspaceEvent>) -> Result<(), String> {
    let Some(observer_class) = workspace_observer_class() else {
        return Err("failed to create workspace observer class".to_string());
    };
    let workspace_class = class(c"NSWorkspace");
    if workspace_class.is_null() {
        return Err("NSWorkspace class is unavailable".to_string());
    }

    // SAFETY: all Objective-C messages use documented selectors with matching
    // ABIs. The observer and notification name are intentionally kept alive for
    // the run loop duration; `CFRunLoopRun` blocks until the thread is stopped.
    unsafe {
        let observer_alloc: Id = msg0(observer_class, sel(c"alloc"));
        let observer: Id = msg0(observer_alloc, sel(c"init"));
        if observer.is_null() {
            return Err("failed to create workspace observer".to_string());
        }

        let workspace: Id = msg0(workspace_class, sel(c"sharedWorkspace"));
        if workspace.is_null() {
            return Err("NSWorkspace sharedWorkspace is unavailable".to_string());
        }
        let center: Id = msg0(workspace, sel(c"notificationCenter"));
        if center.is_null() {
            return Err("NSWorkspace notificationCenter is unavailable".to_string());
        }

        let name = cfstring(b"NSWorkspaceActiveSpaceDidChangeNotification\0") as Id;
        if name.is_null() {
            return Err("failed to create active-space notification name".to_string());
        }

        WORKSPACE_EVENT_SENDERS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .map_err(|_| "workspace observer sender lock poisoned".to_string())?
            .push(tx);

        let _: () = msg4(
            center,
            sel(c"addObserver:selector:name:object:"),
            observer,
            sel(c"activeSpaceDidChange:"),
            name,
            std::ptr::null_mut::<c_void>(),
        );

        CFRunLoopRun();
    }

    Ok(())
}
