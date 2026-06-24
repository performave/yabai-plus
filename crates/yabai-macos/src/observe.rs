//! AX notification observers — the callback half of the macOS boundary.
//!
//! `ax.rs` *reads and writes* window state on demand; this module *listens* for
//! the system telling us state changed: an app opened a window, a window was
//! destroyed, focus moved. It wraps `AXObserver` (create + add-notification +
//! run-loop source) and turns each callback into a typed [`ObservedEvent`].
//!
//! The C daemon runs these observers on its single event-loop thread; here a
//! caller pumps a `CFRunLoop` (typically on a dedicated thread) and receives
//! events over a channel, which the daemon will forward to `Actor::post_event`.
//!
//! Reliability caveat: `AXWindowCreated` and `AXFocusedWindowChanged` fire
//! reliably (verified live, with CG ids resolved), but `AXUIElementDestroyed`
//! for windows is delivered inconsistently by macOS — observed live registering
//! cleanly yet never firing on a Finder window close. Real yabai leans on private
//! SkyLight window events for destroys. The robust path for a consumer is to
//! *reconcile* the live `AXWindows` set against the known set on each event, so a
//! window that vanished is treated as destroyed regardless of the notification.
//!
//! All FFI is local and the unsafe surface is confined here, matching `ax.rs`.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::os::raw::c_char;
use std::sync::mpsc::Sender;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFIndex = isize;
type AXUIElementRef = *const c_void;
type AXObserverRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type Boolean = u8;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

/// `AXObserverCreate` callback signature.
type AxObserverCallback = extern "C" fn(AXObserverRef, AXUIElementRef, CFStringRef, *mut c_void);

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: CFStringRef;

    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> Boolean;
    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRun();
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXObserverCreate(
        application: i32,
        callback: AxObserverCallback,
        out: *mut AXObserverRef,
    ) -> i32;
    fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> i32;
    fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;
}

#[link(name = "SkyLight", kind = "framework")]
unsafe extern "C" {
    fn _AXUIElementGetWindow(element: AXUIElementRef, window_id: *mut u32) -> i32;
}

/// A typed AX notification, ready to map onto a `StateEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedEvent {
    /// An app created a window (`window_id` is the CG id if it resolved).
    WindowCreated { pid: i32, window_id: Option<u32> },
    /// A previously observed window element was destroyed.
    WindowDestroyed { pid: i32, window_id: Option<u32> },
    /// The app's focused window changed.
    FocusedWindowChanged { pid: i32, window_id: Option<u32> },
}

impl ObservedEvent {
    /// The pid of the application the event came from.
    pub fn pid(&self) -> i32 {
        match self {
            ObservedEvent::WindowCreated { pid, .. }
            | ObservedEvent::WindowDestroyed { pid, .. }
            | ObservedEvent::FocusedWindowChanged { pid, .. } => *pid,
        }
    }
}

/// Create a CoreFoundation string from a NUL-terminated ASCII literal.
unsafe fn cfstring(literal: &[u8]) -> CFStringRef {
    // SAFETY: `literal` is a valid NUL-terminated buffer; CF copies its contents.
    unsafe {
        CFStringCreateWithCString(
            std::ptr::null(),
            literal.as_ptr() as *const c_char,
            K_CF_STRING_ENCODING_UTF8,
        )
    }
}

/// Convert a `CFStringRef` to an owned `String` (best effort, ASCII fast path).
fn cfstring_to_string(s: CFStringRef) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let mut buf = [0i8; 256];
    // SAFETY: `s` is a valid CFString; `buf` is valid writable storage of the
    // given length; CFStringGetCString NUL-terminates on success.
    let ok = unsafe {
        CFStringGetCString(
            s,
            buf.as_mut_ptr(),
            buf.len() as CFIndex,
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: `buf` is NUL-terminated by the successful call above.
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    cstr.to_str().ok().map(str::to_owned)
}

fn window_id(element: AXUIElementRef) -> Option<u32> {
    if element.is_null() {
        return None;
    }
    let mut id = 0u32;
    // SAFETY: `element` is an AX window element; the private helper writes the CG id.
    let err = unsafe { _AXUIElementGetWindow(element, &mut id) };
    (err == 0 && id != 0).then_some(id)
}

/// Shared context handed to the C callback via `refcon`. It owns the channel the
/// events flow to and the notification name needed to register destroy-watches
/// on windows discovered at runtime.
struct ObserverCtx {
    pid: i32,
    tx: Sender<ObservedEvent>,
    observer: AXObserverRef,
    destroyed_note: CFStringRef,
}

/// The single C callback: classifies the notification and forwards an event.
extern "C" fn observer_callback(
    _observer: AXObserverRef,
    element: AXUIElementRef,
    notification: CFStringRef,
    refcon: *mut c_void,
) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: `refcon` is the `&mut ObserverCtx` we passed to every
    // `AXObserverAddNotification`; it outlives the run loop (it lives on the
    // stack of `observe_pid`). The callback is serialized on the run-loop thread.
    let ctx = unsafe { &*(refcon as *const ObserverCtx) };
    let Some(name) = cfstring_to_string(notification) else {
        return;
    };
    let id = window_id(element);

    let event = match name.as_str() {
        "AXWindowCreated" => {
            // Start watching the new window for destruction too.
            // SAFETY: `ctx.observer` and `ctx.destroyed_note` are live; `element`
            // is the newly created window; `refcon` is the same shared context.
            unsafe {
                AXObserverAddNotification(ctx.observer, element, ctx.destroyed_note, refcon);
            }
            ObservedEvent::WindowCreated {
                pid: ctx.pid,
                window_id: id,
            }
        }
        "AXUIElementDestroyed" => ObservedEvent::WindowDestroyed {
            pid: ctx.pid,
            window_id: id,
        },
        "AXFocusedWindowChanged" => ObservedEvent::FocusedWindowChanged {
            pid: ctx.pid,
            window_id: id,
        },
        _ => return,
    };
    let _ = ctx.tx.send(event);
}

/// Observe an application's window lifecycle on the current thread, forwarding
/// [`ObservedEvent`]s to `tx`. This blocks in `CFRunLoopRun` and only returns if
/// the observer could not be created; run it on a dedicated thread.
///
/// Watches app-level window-created and focused-window-changed, and per-window
/// destruction (for windows present at start and any created later).
pub fn observe_pid(pid: i32, tx: Sender<ObservedEvent>) -> Result<(), String> {
    // SAFETY: standard AX observer setup. `AXObserverCreate` returns 0 on
    // success and writes the observer; every CF/AX ref created here is released
    // or intentionally kept alive for the run loop's duration (see below).
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err(format!("no AX application element for pid {pid}"));
        }

        let mut observer: AXObserverRef = std::ptr::null_mut();
        let err = AXObserverCreate(pid, observer_callback, &mut observer);
        if err != 0 || observer.is_null() {
            CFRelease(app);
            return Err(format!("AXObserverCreate failed for pid {pid} (err {err})"));
        }

        let created_note = cfstring(b"AXWindowCreated\0");
        let destroyed_note = cfstring(b"AXUIElementDestroyed\0");
        let focus_note = cfstring(b"AXFocusedWindowChanged\0");

        // The context must outlive the run loop; it stays on this stack frame,
        // which blocks in CFRunLoopRun below and never returns on success.
        let ctx = Box::new(ObserverCtx {
            pid,
            tx,
            observer,
            destroyed_note,
        });
        let refcon = (&*ctx as *const ObserverCtx) as *mut c_void;

        // App-level notifications.
        AXObserverAddNotification(observer, app, created_note, refcon);
        AXObserverAddNotification(observer, app, focus_note, refcon);

        // Watch the windows that already exist for destruction.
        let windows_attr = cfstring(b"AXWindows\0");
        let mut windows: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(app, windows_attr, &mut windows) == 0 && !windows.is_null()
        {
            let count = CFArrayGetCount(windows as CFArrayRef);
            for idx in 0..count {
                let window = CFArrayGetValueAtIndex(windows as CFArrayRef, idx);
                if !window.is_null() {
                    AXObserverAddNotification(observer, window, destroyed_note, refcon);
                }
            }
            CFRelease(windows);
        }
        CFRelease(windows_attr);
        CFRelease(created_note);
        CFRelease(focus_note);

        // Pump notifications. `ctx` (owning `tx`/`destroyed_note`) and `app` stay
        // alive because they are owned by this stack frame, which blocks here.
        let run_loop = CFRunLoopGetCurrent();
        let source = AXObserverGetRunLoopSource(observer);
        CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
        CFRunLoopRun();

        // Only reached if the run loop is stopped externally; tidy up.
        CFRelease(ctx.destroyed_note);
        CFRelease(app);
        drop(ctx);
        Ok(())
    }
}
