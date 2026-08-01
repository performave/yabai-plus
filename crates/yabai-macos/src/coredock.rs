//! CoreDock notifications used to drive Dock/Mission Control effects.
//!
//! `CoreDockSendNotification` is a private Dock RPC (exported from
//! `ApplicationServices`) that yabai uses to trigger Mission Control behaviours
//! without any private class access. `window --toggle expose` maps to the
//! `com.apple.expose.front.awake` notification (App Exposé for the front app),
//! mirroring the C `window_manager_toggle_window_expose`.

use std::ffi::CString;
use std::os::raw::{c_char, c_void};

type CFStringRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFTypeRef = *const c_void;

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
    fn CoreDockSendNotification(notification: CFStringRef, unknown: i32) -> i32;
}

/// Send a CoreDock notification by name. Returns `false` if the notification
/// string could not be created (matching the C code, which silently no-ops).
fn send_notification(name: &str) -> bool {
    let Ok(c_name) = CString::new(name) else {
        return false;
    };
    // SAFETY: `c_name` is a valid NUL-terminated UTF-8 string for the lifetime of
    // this call. `CFStringCreateWithCString` returns an owned CFString (or null on
    // failure) that we release below; `CoreDockSendNotification` only reads it.
    unsafe {
        let cf =
            CFStringCreateWithCString(std::ptr::null(), c_name.as_ptr(), K_CF_STRING_ENCODING_UTF8);
        if cf.is_null() {
            return false;
        }
        CoreDockSendNotification(cf, 0);
        CFRelease(cf);
    }
    true
}

/// Trigger App Exposé for the front application, mirroring the C
/// `CoreDockSendNotification(CFSTR("com.apple.expose.front.awake"), 0)`.
pub fn toggle_expose() -> bool {
    send_notification("com.apple.expose.front.awake")
}

/// Toggle Mission Control, mirroring the C `space_manager_toggle_mission_control`
/// (`CoreDockSendNotification(CFSTR("com.apple.expose.awake"), 0)`).
pub fn toggle_mission_control() -> bool {
    send_notification("com.apple.expose.awake")
}

/// Toggle Show Desktop, mirroring the C `space_manager_toggle_show_desktop`
/// (`CoreDockSendNotification(CFSTR("com.apple.showdesktop.awake"), 0)`).
pub fn toggle_show_desktop() -> bool {
    send_notification("com.apple.showdesktop.awake")
}
