//! Running-application discovery via `NSWorkspace`.
//!
//! The C daemon learns about processes from `NSWorkspace` notifications and the
//! process-manager snapshot. This module provides the snapshot half: the pids of
//! "regular" (Dock-visible) applications, which is what the tiler iterates to
//! find manageable windows. The notification half (launch/terminate) is future
//! observer work.

#![cfg(target_os = "macos")]

use crate::objc::{Id, class, msg0, msg1, sel};

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
