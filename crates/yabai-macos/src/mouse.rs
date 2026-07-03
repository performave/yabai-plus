//! Mouse-moved observation via a `CGEventTap` — the input half of
//! `focus_follows_mouse`.
//!
//! The C daemon installs a passive event tap in its run loop and posts a
//! `MOUSE_MOVED` event on every `kCGEventMouseMoved` (`src/mouse_handler.c`).
//! Here a dedicated thread pumps a `CFRunLoop` with a listen-only tap and reports
//! each cursor location over a channel, which the daemon turns into a
//! focus-follows-mouse focus change. All FFI is confined to this module.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use yabai_core::Point;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFMachPortRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

// `CGEventType` values we care about (`CGEventTypes.h`).
const K_CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const K_CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const K_CG_EVENT_MOUSE_MOVED: u32 = 5;
const K_CG_EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
// The tap can be disabled by the system; these arrive as event "types" and must
// be handled by re-enabling the tap.
const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE; // (uint32)-2
const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF; // (uint32)-1

// `CGEventTapLocation` / `CGEventTapPlacement` / `CGEventTapOptions`.
const K_CG_SESSION_EVENT_TAP: u32 = 1; // kCGSessionEventTap
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0; // kCGHeadInsertEventTap
const K_CG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1; // kCGEventTapOptionListenOnly
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0; // kCGEventTapOptionDefault (can consume)

// Our own compact modifier bitmask (both sides of the tap are ours, so the exact
// bit values are arbitrary as long as they agree). Mirrors the C `MOUSE_MOD_*`.
pub const MOUSE_MOD_ALT: u8 = 1 << 0;
pub const MOUSE_MOD_SHIFT: u8 = 1 << 1;
pub const MOUSE_MOD_CMD: u8 = 1 << 2;
pub const MOUSE_MOD_CTRL: u8 = 1 << 3;
pub const MOUSE_MOD_FN: u8 = 1 << 4;

// `CGEventFlags` masks (`CGEventTypes.h`).
const K_CG_FLAG_MASK_ALTERNATE: u64 = 0x0008_0000;
const K_CG_FLAG_MASK_SHIFT: u64 = 0x0002_0000;
const K_CG_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
const K_CG_FLAG_MASK_CONTROL: u64 = 0x0004_0000;
const K_CG_FLAG_MASK_SECONDARY_FN: u64 = 0x0080_0000;

type CGEventTapCallBack = extern "C" fn(
    proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: CFStringRef;

    fn CFMachPortCreateRunLoopSource(
        allocator: CFAllocatorRef,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRun();
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouse_type: u32,
        cursor_position: CGPoint,
        mouse_button: u32,
    ) -> CGEventRef;
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    // Signature matches the `CGEventPost` declared in `space.rs` (const event ptr)
    // to avoid a cross-module redeclaration warning.
    fn CGEventPost(tap: u32, event: *const c_void);
}

fn post_mouse_event(event_type: u32, point: Point, flags: u64) {
    let position = CGPoint {
        x: point.x as f64,
        y: point.y as f64,
    };
    // SAFETY: a null source is valid; the returned event, if any, is released.
    let event = unsafe { CGEventCreateMouseEvent(std::ptr::null(), event_type, position, 0) };
    if event.is_null() {
        return;
    }
    // SAFETY: `event` is a live CGEvent; set the modifier flags, post to the session
    // tap, and release.
    unsafe {
        if flags != 0 {
            CGEventSetFlags(event, flags);
        }
        CGEventPost(K_CG_SESSION_EVENT_TAP, event as *const c_void);
        CFRelease(event as CFTypeRef);
    }
}

/// Synthesize a full left-button drag (down → dragged → up) from `from` to `to`
/// with the `fn` modifier held, to exercise `mouse_modifier` drag-to-move on a
/// headless/remote box. A real drag produces the same tap events.
pub fn post_mouse_drag(from: Point, to: Point) {
    post_mouse_event(
        K_CG_EVENT_LEFT_MOUSE_DOWN,
        from,
        K_CG_FLAG_MASK_SECONDARY_FN,
    );
    post_mouse_event(
        K_CG_EVENT_LEFT_MOUSE_DRAGGED,
        to,
        K_CG_FLAG_MASK_SECONDARY_FN,
    );
    post_mouse_event(K_CG_EVENT_LEFT_MOUSE_UP, to, K_CG_FLAG_MASK_SECONDARY_FN);
}

/// Synthesize and post a `kCGEventMouseMoved` at `point` (top-left CG coords).
/// Used to exercise `focus_follows_mouse` on a headless/remote box, where the
/// physical cursor cannot be driven; a real cursor move produces the same tap
/// event. Also warps the cursor there first so the location is consistent.
pub fn post_mouse_moved(point: Point) {
    let position = CGPoint {
        x: point.x as f64,
        y: point.y as f64,
    };
    // SAFETY: a null source is valid; the returned event, if any, is released.
    let event =
        unsafe { CGEventCreateMouseEvent(std::ptr::null(), K_CG_EVENT_MOUSE_MOVED, position, 0) };
    if event.is_null() {
        return;
    }
    // SAFETY: `event` is a live CGEvent; posting to the session tap is valid and
    // `CFRelease` balances the create.
    unsafe {
        CGEventPost(K_CG_SESSION_EVENT_TAP, event as *const c_void);
        CFRelease(event as CFTypeRef);
    }
}

/// Registered listeners for mouse-moved points. A `OnceLock<Mutex<Vec<..>>>`
/// mirrors `workspace.rs`; the tap callback is a bare `extern "C" fn` with no
/// closure environment, so it fans out through this static.
static MOUSE_MOVED_SENDERS: OnceLock<Mutex<Vec<Sender<Point>>>> = OnceLock::new();

/// The live tap's CFMachPort, so the callback can re-enable it after a system
/// disable. Set once by `observe_mouse_moved`; there is a single mouse tap.
static TAP_PORT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn senders() -> &'static Mutex<Vec<Sender<Point>>> {
    MOUSE_MOVED_SENDERS.get_or_init(|| Mutex::new(Vec::new()))
}

extern "C" fn mouse_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    // A system-disabled tap must be re-enabled or it stays dead.
    if event_type == K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT
        || event_type == K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT
    {
        let port = TAP_PORT.load(Ordering::Relaxed);
        if !port.is_null() {
            // SAFETY: `port` is the tap's own CFMachPort, stored after creation
            // and valid for the tap's lifetime.
            unsafe { CGEventTapEnable(port, true) };
        }
        return event;
    }

    if event_type == K_CG_EVENT_MOUSE_MOVED {
        // SAFETY: `event` is a live CGEvent owned by the tap for this callback.
        let location = unsafe { CGEventGetLocation(event) };
        let point = Point {
            x: location.x as f32,
            y: location.y as f32,
        };
        if let Ok(mut list) = senders().lock() {
            list.retain(|tx| tx.send(point).is_ok());
        }
    }

    event
}

/// Install a listen-only `CGEventTap` for mouse-moved events and pump its run
/// loop (blocks — call on a dedicated thread). Every cursor location is sent to
/// `tx`. Requires the process to be trusted for Accessibility (the daemon is).
///
/// Returns an error string if the tap could not be created (e.g. missing
/// permission); on success it never returns while the run loop is alive.
pub fn observe_mouse_moved(tx: Sender<Point>) -> Result<(), String> {
    senders()
        .lock()
        .map_err(|_| "poisoned".to_string())?
        .push(tx);

    let events_of_interest: u64 = 1 << K_CG_EVENT_MOUSE_MOVED;
    // SAFETY: all arguments are valid constants and a valid `extern "C"` callback.
    // The callback re-enables the tap via the `TAP_PORT` static (set below), so no
    // user-info is needed.
    let port = unsafe {
        CGEventTapCreate(
            K_CG_SESSION_EVENT_TAP,
            K_CG_HEAD_INSERT_EVENT_TAP,
            K_CG_EVENT_TAP_OPTION_LISTEN_ONLY,
            events_of_interest,
            mouse_tap_callback,
            std::ptr::null_mut(),
        )
    };
    if port.is_null() {
        return Err("failed to create mouse event tap (accessibility permission?)".to_string());
    }
    TAP_PORT.store(port, Ordering::Relaxed);

    // SAFETY: `port` is a valid CFMachPort from CGEventTapCreate; the run-loop
    // source is added to this thread's run loop and the tap is enabled, then we
    // block in CFRunLoopRun. The source is released after CFRunLoopAddSource
    // retains it.
    unsafe {
        let source = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
        if source.is_null() {
            CFRelease(port as CFTypeRef);
            return Err("failed to create mouse tap run-loop source".to_string());
        }
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
        CGEventTapEnable(port, true);
        CFRelease(source as CFTypeRef);
        CFRunLoopRun();
    }

    Ok(())
}

// ---- mouse drag (move) via an active event tap ----------------------------

/// A left-button mouse-drag event while the armed `mouse_modifier` is held. The
/// tap consumes the underlying click so the app underneath doesn't also react.
#[derive(Debug, Clone, Copy)]
pub enum MouseDragEvent {
    /// Drag began: the modifier-armed button went down at `point`.
    Down(Point),
    /// The pointer moved to `point` during an armed drag.
    Dragged(Point),
    /// The button came up at `point`, ending the drag.
    Up(Point),
}

static MOUSE_DRAG_SENDERS: OnceLock<Mutex<Vec<Sender<MouseDragEvent>>>> = OnceLock::new();
/// The armed modifier bitmask (`MOUSE_MOD_*`) the daemon sets from
/// `config.mouse_modifier`; `0` disables drag interception.
static ARMED_MOD: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// Whether we are mid-drag and thus consuming the drag/up events.
static DRAG_CONSUMING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DRAG_TAP_PORT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn drag_senders() -> &'static Mutex<Vec<Sender<MouseDragEvent>>> {
    MOUSE_DRAG_SENDERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Set the armed modifier the drag tap watches for (from `config.mouse_modifier`).
pub fn set_drag_modifier(mask: u8) {
    ARMED_MOD.store(mask, Ordering::Relaxed);
}

fn mouse_mod_from_cgflags(flags: u64) -> u8 {
    let mut mods = 0;
    if flags & K_CG_FLAG_MASK_ALTERNATE == K_CG_FLAG_MASK_ALTERNATE {
        mods |= MOUSE_MOD_ALT;
    }
    if flags & K_CG_FLAG_MASK_SHIFT == K_CG_FLAG_MASK_SHIFT {
        mods |= MOUSE_MOD_SHIFT;
    }
    if flags & K_CG_FLAG_MASK_COMMAND == K_CG_FLAG_MASK_COMMAND {
        mods |= MOUSE_MOD_CMD;
    }
    if flags & K_CG_FLAG_MASK_CONTROL == K_CG_FLAG_MASK_CONTROL {
        mods |= MOUSE_MOD_CTRL;
    }
    if flags & K_CG_FLAG_MASK_SECONDARY_FN == K_CG_FLAG_MASK_SECONDARY_FN {
        mods |= MOUSE_MOD_FN;
    }
    mods
}

fn send_drag(event: MouseDragEvent) {
    if let Ok(mut list) = drag_senders().lock() {
        list.retain(|tx| tx.send(event).is_ok());
    }
}

extern "C" fn mouse_drag_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    if event_type == K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT
        || event_type == K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT
    {
        let port = DRAG_TAP_PORT.load(Ordering::Relaxed);
        if !port.is_null() {
            // SAFETY: `port` is the drag tap's own CFMachPort, valid for its lifetime.
            unsafe { CGEventTapEnable(port, true) };
        }
        return event;
    }

    let point = {
        // SAFETY: `event` is a live CGEvent owned by the tap for this callback.
        let location = unsafe { CGEventGetLocation(event) };
        Point {
            x: location.x as f32,
            y: location.y as f32,
        }
    };

    match event_type {
        K_CG_EVENT_LEFT_MOUSE_DOWN => {
            let armed = ARMED_MOD.load(Ordering::Relaxed);
            // SAFETY: `event` is live for this callback.
            let mods = mouse_mod_from_cgflags(unsafe { CGEventGetFlags(event) });
            if armed != 0 && mods == armed {
                DRAG_CONSUMING.store(true, Ordering::Relaxed);
                send_drag(MouseDragEvent::Down(point));
                return std::ptr::null_mut(); // consume the click
            }
        }
        K_CG_EVENT_LEFT_MOUSE_DRAGGED => {
            if DRAG_CONSUMING.load(Ordering::Relaxed) {
                send_drag(MouseDragEvent::Dragged(point));
                return std::ptr::null_mut();
            }
        }
        K_CG_EVENT_LEFT_MOUSE_UP => {
            if DRAG_CONSUMING.swap(false, Ordering::Relaxed) {
                send_drag(MouseDragEvent::Up(point));
                return std::ptr::null_mut();
            }
        }
        _ => {}
    }

    event
}

/// Install an active `CGEventTap` for left mouse down/dragged/up and pump its run
/// loop (blocks — call on a dedicated thread). While the armed `mouse_modifier`
/// (set via [`set_drag_modifier`]) is held on mouse-down, the click is consumed and
/// the drag is reported to `tx`. Requires Accessibility trust (the daemon has it).
pub fn observe_mouse_drag(tx: Sender<MouseDragEvent>) -> Result<(), String> {
    drag_senders()
        .lock()
        .map_err(|_| "poisoned".to_string())?
        .push(tx);

    let events_of_interest: u64 = (1 << K_CG_EVENT_LEFT_MOUSE_DOWN)
        | (1 << K_CG_EVENT_LEFT_MOUSE_UP)
        | (1 << K_CG_EVENT_LEFT_MOUSE_DRAGGED);
    // SAFETY: valid constants and a valid callback; the tap re-enables itself via
    // the `DRAG_TAP_PORT` static set below.
    let port = unsafe {
        CGEventTapCreate(
            K_CG_SESSION_EVENT_TAP,
            K_CG_HEAD_INSERT_EVENT_TAP,
            K_CG_EVENT_TAP_OPTION_DEFAULT,
            events_of_interest,
            mouse_drag_callback,
            std::ptr::null_mut(),
        )
    };
    if port.is_null() {
        return Err("failed to create mouse drag tap (accessibility permission?)".to_string());
    }
    DRAG_TAP_PORT.store(port, Ordering::Relaxed);

    // SAFETY: `port` is a valid CFMachPort; add its source to this thread's run
    // loop, enable the tap, and block in CFRunLoopRun.
    unsafe {
        let source = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
        if source.is_null() {
            CFRelease(port as CFTypeRef);
            return Err("failed to create mouse drag tap run-loop source".to_string());
        }
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
        CGEventTapEnable(port, true);
        CFRelease(source as CFTypeRef);
        CFRunLoopRun();
    }

    Ok(())
}
