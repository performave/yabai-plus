use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::ExitCode;
use std::sync::mpsc::{Sender, SyncSender, channel, sync_channel};
use std::thread;
use std::time::Duration;

use yabai_core::layout::{HANDLE_ABS, HANDLE_BOTTOM, HANDLE_LEFT, HANDLE_RIGHT, HANDLE_TOP};
use yabai_core::{
    Area, DisplayAction, FfmMode, Layer, Message, MouseAction, MouseModifier, Point, RuleCommand,
    ScratchpadAction, Selector, SignalEvent, SpaceAction, ValueType, WindowAction, grid_frame,
    parse_message, parse_selector,
};
use yabai_ipc::{FAILURE_MARKER, daemon_socket_path, decode_client_payload, send_message};
use yabai_macos::ax::DiscoveredAxWindow;
use yabai_macos::{
    AxSink, MOUSE_MOD_ALT, MOUSE_MOD_CMD, MOUSE_MOD_CTRL, MOUSE_MOD_FN, MOUSE_MOD_SHIFT,
    MouseDragButton, MouseDragEvent, ObservedEvent, WorkspaceEvent,
    accessibility_trusted_with_prompt, active_displays, application_pids_with_windows,
    current_space_for_display, cursor_display_id, cursor_location, display_for_space,
    focused_window, focused_window_diagnostics, main_visible_frame, mission_control_spaces,
    move_focused_window, move_pid_window, ns_application_load, observe_display_reconfiguration,
    observe_mouse_drag, observe_mouse_moved, observe_pid, observe_workspace, pid_window_infos,
    regular_application_pids, set_active_display, set_drag_modifier, spaces_for_display,
    spaces_for_window, switch_space_by_gesture, tileable_pid_windows, visible_frame_for_display,
    warp_cursor_to_display_center, warp_cursor_to_point, window_is_ordered_in, windows_for_pid,
    windows_for_pid_diagnostics, windows_on_space,
};
use yabai_runtime::{
    Actor, AppState, AppliedRuleEffects, DropResult, LayoutSink, RecordingSink, Response, Runtime,
    StateEvent, WindowMeta,
};
use yabai_sa::{ScriptingAddition, ScriptingAdditionStatus};

mod probes;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("--version") | Some("-v") => {
            println!("yabai-rust-{}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--message") | Some("-m") => run_message(&args[1..]),
        Some("--experimental-rust-daemon") => run_experimental_daemon(&args[1..]),
        Some("--experimental-ax-focused-window") => run_ax_focused_window_probe(),
        Some("--experimental-ax-debug") => run_ax_debug_probe(),
        Some("--experimental-ax-windows-for-pid") => run_ax_windows_for_pid(&args[1..]),
        Some("--experimental-ax-pid-debug") => run_ax_pid_debug(&args[1..]),
        Some("--experimental-ax-move-focused") => run_ax_move_focused(&args[1..]),
        Some("--experimental-ax-move-pid") => run_ax_move_pid(&args[1..]),
        Some("--experimental-ax-tile-pid") => run_ax_tile_pid(&args[1..]),
        Some("--experimental-rust-tile-daemon") => run_rust_tile_daemon(&args[1..]),
        Some("--experimental-ax-observe-pid") => run_ax_observe_pid(&args[1..]),
        Some("--experimental-rust-wm-daemon") => run_rust_wm_daemon(&args[1..]),
        Some("--experimental-space-probe") => run_space_probe(&args[1..]),
        Some("--experimental-cursor-location") => match cursor_location() {
            Ok(point) => {
                println!("cursor {} {}", point.x, point.y);
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("yabai-rust: {error}");
                ExitCode::from(1)
            }
        },
        Some("--experimental-sa-status") => probes::run_sa_status(),
        Some("--experimental-sa-opacity") => probes::run_sa_opacity(&args[1..]),
        Some("--experimental-sa-create-space") => probes::run_sa_space(&args[1..], true),
        Some("--experimental-sa-destroy-space") => probes::run_sa_space(&args[1..], false),
        Some("--experimental-sa-window-to-space") => probes::run_sa_window_to_space(&args[1..]),
        Some("--experimental-sa-focus-space") => probes::run_sa_focus_space(&args[1..]),
        Some("--experimental-window-alpha") => probes::run_window_alpha(&args[1..]),
        Some("--experimental-windows-on-space") => probes::run_windows_on_space(&args[1..]),
        Some("--experimental-post-mouse-moved") => probes::run_post_mouse_moved(&args[1..]),
        Some("--experimental-post-mouse-drag") => probes::run_post_mouse_drag(&args[1..]),
        Some("--experimental-post-right-mouse-drag") => {
            probes::run_post_right_mouse_drag(&args[1..])
        }
        Some("--experimental-window-bounds") => probes::run_window_bounds(&args[1..]),
        Some("--experimental-window-transform") => probes::run_window_transform(&args[1..]),
        _ => {
            eprintln!("yabai-rust: daemon skeleton is not implemented yet");
            ExitCode::from(64)
        }
    }
}

fn run_message(tokens: &[String]) -> ExitCode {
    if tokens.is_empty() {
        eprintln!("yabai-rust: no arguments given to --message");
        return ExitCode::from(1);
    }

    let user = match std::env::var("USER") {
        Ok(user) if !user.is_empty() => user,
        _ => {
            eprintln!("yabai-rust: 'env USER' not set! abort..");
            return ExitCode::from(1);
        }
    };

    let socket_path = daemon_socket_path(&user);
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();

    match send_message(&socket_path, tokens, &mut out, &mut err) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            let _ = writeln!(
                err,
                "yabai-rust: failed to message daemon at {socket_path}: {error}"
            );
            ExitCode::from(1)
        }
    }
}

fn run_experimental_daemon(args: &[String]) -> ExitCode {
    let Some(socket_path) = args.first() else {
        eprintln!("yabai-rust: --experimental-rust-daemon requires a socket path");
        return ExitCode::from(64);
    };

    match bind_experimental_daemon(socket_path) {
        Ok(listener) => {
            let mut state = AppState::new();
            seed_live_displays(&mut state);
            let actor = Actor::spawn(Runtime::new(state, RecordingSink::default()));
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => serve_one(stream, &actor),
                    Err(error) => eprintln!("yabai-rust: failed to accept client: {error}"),
                }
            }
            actor.shutdown();
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("yabai-rust: failed to bind daemon socket at {socket_path}: {error}");
            ExitCode::from(1)
        }
    }
}

fn seed_live_displays(state: &mut AppState) {
    match active_displays() {
        Ok(displays) => {
            for display in displays {
                state.add_display(display.id, display.frame);
            }
        }
        Err(error) => eprintln!("yabai-rust: failed to discover displays: {error}"),
    }
}

fn bind_experimental_daemon(socket_path: &str) -> io::Result<UnixListener> {
    UnixListener::bind(socket_path)
}

fn serve_one<S: LayoutSink + Send + 'static>(mut stream: UnixStream, actor: &Actor<S>) {
    let mut header = [0u8; size_of::<i32>()];
    let response = match stream.read_exact(&mut header) {
        Ok(()) => {
            let size = i32::from_ne_bytes(header);
            if size < 0 {
                Err("negative IPC payload size".to_string())
            } else {
                let mut payload = vec![0; size as usize];
                match stream.read_exact(&mut payload) {
                    Ok(()) => match decode_client_payload(&payload) {
                        Some(tokens) => {
                            let tokens = tokens.into_iter().map(str::to_owned).collect::<Vec<_>>();
                            actor.message(tokens)
                        }
                        None => Err("invalid IPC payload".to_string()),
                    },
                    Err(error) => Err(format!("failed to read IPC payload: {error}")),
                }
            }
        }
        Err(error) => Err(format!("failed to read IPC header: {error}")),
    };

    match response {
        Ok(Some(output)) => {
            let _ = stream.write_all(output.as_bytes());
        }
        Ok(None) => {}
        Err(error) => {
            let _ = stream.write_all(&[FAILURE_MARKER]);
            let _ = writeln!(stream, "{error}");
        }
    }
}

fn run_ax_focused_window_probe() -> ExitCode {
    if !accessibility_trusted_with_prompt() {
        eprintln!("yabai-rust: Accessibility permission is not granted; grant it and rerun");
        return ExitCode::from(1);
    }

    match focused_window() {
        Ok(Some(window)) => {
            println!("{}", window.id);
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!("yabai-rust: no focused AX window could be resolved");
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("yabai-rust: failed to resolve focused AX window: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_ax_pid_debug(args: &[String]) -> ExitCode {
    if !accessibility_trusted_with_prompt() {
        eprintln!("yabai-rust: Accessibility permission is not granted; grant it and rerun");
        return ExitCode::from(1);
    }
    let Some(pid) = args.first().and_then(|arg| arg.parse::<i32>().ok()) else {
        eprintln!("yabai-rust: --experimental-ax-pid-debug requires a pid");
        return ExitCode::from(64);
    };

    let diag = windows_for_pid_diagnostics(pid);
    println!("trusted={}", diag.trusted);
    println!("app_created={}", diag.app_created);
    println!("app_pid={:?}", diag.app_pid);
    println!("windows_error={:?}", diag.windows_error);
    println!("windows_count={:?}", diag.windows_count);
    println!("window_ids={:?}", diag.window_ids);
    ExitCode::SUCCESS
}

fn run_ax_windows_for_pid(args: &[String]) -> ExitCode {
    if !accessibility_trusted_with_prompt() {
        eprintln!("yabai-rust: Accessibility permission is not granted; grant it and rerun");
        return ExitCode::from(1);
    }
    let Some(pid) = args.first().and_then(|arg| arg.parse::<i32>().ok()) else {
        eprintln!("yabai-rust: --experimental-ax-windows-for-pid requires a pid");
        return ExitCode::from(64);
    };

    match windows_for_pid(pid) {
        Ok(windows) => {
            for window in windows {
                println!("{}", window.id);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("yabai-rust: failed to list AX windows for pid {pid}: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_ax_move_focused(args: &[String]) -> ExitCode {
    if !accessibility_trusted_with_prompt() {
        eprintln!("yabai-rust: Accessibility permission is not granted; grant it and rerun");
        return ExitCode::from(1);
    }

    let coords: Option<Vec<f32>> = (args.len() == 4)
        .then(|| args.iter().map(|arg| arg.parse::<f32>().ok()).collect())
        .flatten();
    let Some(coords) = coords else {
        eprintln!("yabai-rust: --experimental-ax-move-focused requires <x> <y> <w> <h>");
        return ExitCode::from(64);
    };

    let area = Area::new(coords[0], coords[1], coords[2], coords[3]);
    match move_focused_window(area) {
        Ok(result) => {
            println!(
                "moved focused window to x={:.1} y={:.1} w={:.1} h={:.1}",
                result.x, result.y, result.w, result.h
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("yabai-rust: failed to move focused AX window: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_ax_move_pid(args: &[String]) -> ExitCode {
    if !accessibility_trusted_with_prompt() {
        eprintln!("yabai-rust: Accessibility permission is not granted; grant it and rerun");
        return ExitCode::from(1);
    }

    // <pid> <index> <x> <y> <w> <h>
    if args.len() != 6 {
        eprintln!("yabai-rust: --experimental-ax-move-pid requires <pid> <index> <x> <y> <w> <h>");
        return ExitCode::from(64);
    }
    let Some(pid) = args[0].parse::<i32>().ok() else {
        eprintln!("yabai-rust: invalid pid '{}'", args[0]);
        return ExitCode::from(64);
    };
    let Some(index) = args[1].parse::<usize>().ok() else {
        eprintln!("yabai-rust: invalid window index '{}'", args[1]);
        return ExitCode::from(64);
    };
    let coords: Option<Vec<f32>> = args[2..]
        .iter()
        .map(|arg| arg.parse::<f32>().ok())
        .collect();
    let Some(coords) = coords else {
        eprintln!("yabai-rust: invalid x/y/w/h coordinates");
        return ExitCode::from(64);
    };

    let area = Area::new(coords[0], coords[1], coords[2], coords[3]);
    match move_pid_window(pid, index, area) {
        Ok(result) => {
            println!(
                "moved pid {pid} window {index} to x={:.1} y={:.1} w={:.1} h={:.1}",
                result.x, result.y, result.w, result.h
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("yabai-rust: failed to move pid {pid} AX window: {error}");
            ExitCode::from(1)
        }
    }
}

/// BSP-tile every real window of an application using the full pure control
/// plane (`AppState` BSP `Tree`) driving the real `AxSink`. This is the first
/// time `Runtime -> AppState -> AxSink` runs against live windows: window
/// discovery and movement are macOS, but every placement decision is pure Rust.
fn run_ax_tile_pid(args: &[String]) -> ExitCode {
    if !accessibility_trusted_with_prompt() {
        eprintln!("yabai-rust: Accessibility permission is not granted; grant it and rerun");
        return ExitCode::from(1);
    }
    let Some(pid) = args.first().and_then(|arg| arg.parse::<i32>().ok()) else {
        eprintln!("yabai-rust: --experimental-ax-tile-pid requires a pid [gap] [padding]");
        return ExitCode::from(64);
    };
    let gap: i32 = args.get(1).and_then(|arg| arg.parse().ok()).unwrap_or(12);
    // Padding is the outer margin (window-to-screen-edge); window_gap is only the
    // gap *between* windows, exactly as in yabai. Defaults to the gap value.
    let padding: i32 = args.get(2).and_then(|arg| arg.parse().ok()).unwrap_or(gap);

    let display = match active_displays() {
        Ok(displays) => match displays.into_iter().next() {
            Some(display) => display,
            None => {
                eprintln!("yabai-rust: no active displays found");
                return ExitCode::from(1);
            }
        },
        Err(error) => {
            eprintln!("yabai-rust: failed to discover displays: {error}");
            return ExitCode::from(1);
        }
    };

    // Discover by settable-position rather than CG id, so apps whose windows
    // don't resolve via `_AXUIElementGetWindow` still tile, and non-movable
    // background/desktop windows are excluded.
    let windows = match tileable_pid_windows(pid) {
        Ok(windows) if !windows.is_empty() => windows,
        Ok(_) => {
            eprintln!("yabai-rust: pid {pid} has no tileable AX windows");
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!("yabai-rust: failed to list AX windows for pid {pid}: {error}");
            return ExitCode::from(1);
        }
    };

    // Tile inside the usable frame (menu bar + Dock excluded), like the C daemon,
    // so windows don't tuck under the menu bar. Falls back to the full bounds.
    let usable = main_visible_frame().unwrap_or(display.frame);

    let mut rt = Runtime::new(AppState::new(), AxSink::new());
    rt.state.add_display(display.id, display.frame);
    rt.state.add_space_to_display(1, display.id, usable);
    rt.state.set_active_space(1);
    // window_gap controls the between-window gap; the four paddings control the
    // outer margin. Set both, then inset the space's root area by the paddings.
    let _ = rt.message(&tile_config_tokens(gap, padding));
    if let Err(error) = rt.state.set_space_frame(1, usable) {
        eprintln!("yabai-rust: failed to apply padding: {error}");
        return ExitCode::from(1);
    }

    let count = windows.len();
    for window in windows {
        let id = window.id;
        rt.sink.register(id, window.window);
        if let Err(error) = rt.event(StateEvent::WindowCreated { window_id: id }) {
            eprintln!("yabai-rust: failed to tile window {id}: {error}");
            return ExitCode::from(1);
        }
    }

    let frames = rt.state.flush_active().unwrap_or_default();
    println!(
        "tiled {count} window(s) of pid {pid} across display {} ({:.0}x{:.0}), gap {gap}, padding {padding}:",
        display.id, display.frame.w, display.frame.h
    );
    for frame in frames {
        println!(
            "  window {} -> x={:.1} y={:.1} w={:.1} h={:.1}",
            frame.window_id, frame.area.x, frame.area.y, frame.area.w, frame.area.h
        );
    }
    ExitCode::SUCCESS
}

/// Tokens for a `config` message that sets the between-window gap and the four
/// outer paddings, matching yabai's two-axis spacing model.
fn tile_config_tokens(gap: i32, padding: i32) -> Vec<String> {
    let g = gap.to_string();
    let p = padding.to_string();
    [
        "config",
        "window_gap",
        &g,
        "top_padding",
        &p,
        "bottom_padding",
        &p,
        "left_padding",
        &p,
        "right_padding",
        &p,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Collect tileable windows for a daemon target: a single pid, or every regular
/// (Dock-visible) application when `target` is `"all"`. Discovery failures for an
/// individual app are skipped so one bad app can't abort multi-app tiling.
fn collect_tileable_windows(target: &str) -> Vec<DiscoveredAxWindow> {
    let pids: Vec<i32> = if target == "all" {
        regular_application_pids()
    } else {
        target.parse::<i32>().into_iter().collect()
    };

    let mut windows = Vec::new();
    for pid in pids {
        if let Ok(found) = tileable_pid_windows(pid) {
            windows.extend(found);
        }
    }
    windows
}

/// A persistent Rust tiling daemon: it tiles one app's windows through
/// `Actor<AxSink>` and keeps serving the socket, so live `-m` commands
/// (`space --rotate`, `--balance`, `window --resize`, ...) re-tile real windows.
///
/// CRITICAL: this binds the caller-provided socket path only; it must never bind
/// `/tmp/yabai_$USER.socket` while a C daemon runs. To message it, use a socket
/// named `/tmp/yabai_<name>.socket` and query with `USER=<name>`.
fn run_rust_tile_daemon(args: &[String]) -> ExitCode {
    let Some(socket_path) = args.first() else {
        eprintln!(
            "yabai-rust: --experimental-rust-tile-daemon requires <socket> <pid|all> [gap] [padding]"
        );
        return ExitCode::from(64);
    };
    let Some(target) = args.get(1) else {
        eprintln!("yabai-rust: --experimental-rust-tile-daemon requires a pid or 'all'");
        return ExitCode::from(64);
    };
    if target != "all" && target.parse::<i32>().is_err() {
        eprintln!("yabai-rust: tile target must be a pid or 'all', got '{target}'");
        return ExitCode::from(64);
    }
    let gap: i32 = args.get(2).and_then(|arg| arg.parse().ok()).unwrap_or(12);
    let padding: i32 = args.get(3).and_then(|arg| arg.parse().ok()).unwrap_or(gap);

    if !accessibility_trusted_with_prompt() {
        eprintln!("yabai-rust: Accessibility permission is not granted; grant it and rerun");
        return ExitCode::from(1);
    }

    // Bind the socket before touching any windows, so a bind failure (e.g. a
    // stale socket or a conflicting daemon) aborts without rearranging anything.
    let listener = match bind_experimental_daemon(socket_path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("yabai-rust: failed to bind daemon socket at {socket_path}: {error}");
            return ExitCode::from(1);
        }
    };

    let display = match active_displays() {
        Ok(displays) => match displays.into_iter().next() {
            Some(display) => display,
            None => {
                eprintln!("yabai-rust: no active displays found");
                return ExitCode::from(1);
            }
        },
        Err(error) => {
            eprintln!("yabai-rust: failed to discover displays: {error}");
            return ExitCode::from(1);
        }
    };
    let usable = main_visible_frame().unwrap_or(display.frame);

    let windows = collect_tileable_windows(target);
    if windows.is_empty() {
        eprintln!("yabai-rust: no tileable AX windows found for target '{target}'");
        return ExitCode::from(1);
    }

    // Register every window's AX element in the sink before it moves to the actor
    // thread (the one allowed cross-thread move, per the single-actor invariant).
    let mut sink = AxSink::new();
    let mut ids = Vec::with_capacity(windows.len());
    for window in windows {
        ids.push(window.id);
        sink.register(window.id, window.window);
    }

    let mut state = AppState::new();
    state.add_display(display.id, display.frame);
    state.add_space_to_display(1, display.id, usable);
    state.set_active_space(1);
    let _ = state.handle_tokens(&tile_config_tokens(gap, padding));
    if let Err(error) = state.set_space_frame(1, usable) {
        eprintln!("yabai-rust: failed to apply padding: {error}");
        return ExitCode::from(1);
    }

    let actor = Actor::spawn(Runtime::new(state, sink));
    // Drive the initial tile from the discovered windows.
    for id in &ids {
        actor.post_event(StateEvent::WindowCreated { window_id: *id });
    }

    eprintln!(
        "yabai-rust: tiling daemon up on {socket_path} — target {target}, {} window(s), gap {gap}, padding {padding}",
        ids.len()
    );
    eprintln!(
        "yabai-rust: send commands with a matching USER, e.g. USER=<name> yabai -m space --rotate 90"
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => serve_one(stream, &actor),
            Err(error) => eprintln!("yabai-rust: failed to accept client: {error}"),
        }
    }
    actor.shutdown();
    ExitCode::SUCCESS
}

/// Unified work for the WM daemon's single-threaded event loop, mirroring the
/// serialized queue in `src/event_loop.c`: AX observer events and socket `-m`
/// messages funnel into one channel processed against one `Runtime<AxSink>`.
enum WmWork {
    Observed(ObservedEvent),
    Workspace(WorkspaceEvent),
    /// The cursor moved to `point` (from the `focus_follows_mouse` event tap).
    MouseMoved(Point),
    /// A mouse-drag event (down/dragged/up) while the `mouse_modifier` is held.
    Drag(MouseDragEvent),
    /// Periodic self-heal: re-reconcile known apps and (in `all` mode) discover
    /// apps launched after startup.
    Tick,
    Message {
        tokens: Vec<String>,
        reply: SyncSender<Response>,
    },
}

/// Spawn an AX observer for `pid` on its own run-loop thread, forwarding its
/// events into the shared `WmWork` channel.
fn spawn_observer(pid: i32, tx: &Sender<WmWork>) {
    let (otx, orx) = channel::<ObservedEvent>();
    thread::spawn(move || {
        let _ = observe_pid(pid, otx);
    });
    let tx = tx.clone();
    thread::spawn(move || {
        for event in orx {
            if tx.send(WmWork::Observed(event)).is_err() {
                break;
            }
        }
    });
}

/// Bridge NSWorkspace events into the daemon channel. Returns the sender to hand
/// to `observe_workspace`, which must run on the **main thread**: it blocks in
/// `[NSApp run]`, and that main-thread run loop (set up by `NSApplicationLoad`)
/// is the only one that services NSWorkspace notifications and the
/// CGDisplayReconfiguration callback — mirroring the C daemon's `[NSApp run]` on
/// its main thread while the event loop runs on a worker pthread.
fn start_workspace_bridge(tx: &Sender<WmWork>) -> Sender<WorkspaceEvent> {
    let (otx, orx) = channel::<WorkspaceEvent>();
    let tx = tx.clone();
    thread::spawn(move || {
        for event in orx {
            if tx.send(WmWork::Workspace(event)).is_err() {
                break;
            }
        }
    });
    otx
}

fn managed_space_for_window(state: &AppState, window_id: u32) -> Option<u64> {
    // Authoritative on macOS 26: ask each known space which windows it contains
    // (`windows_on_space` / `SLSCopyWindowsWithOptionsAndTags`). `SLSCopySpacesForWindows`
    // (below) only ever reports the *current* space on macOS 26, so a window on a
    // non-visible space would otherwise be mis-assigned to the active space.
    for sid in state.space_ids() {
        if windows_on_space(sid).is_ok_and(|windows| windows.contains(&window_id)) {
            return Some(sid);
        }
    }

    // Fallback for older macOS (or if the enumeration missed the window): trust
    // `spaces_for_window`, preferring the active space when it is a candidate.
    let spaces = spaces_for_window(window_id).ok()?;
    if let Some(active_sid) = state.active_space_id() {
        if spaces.contains(&active_sid) {
            return Some(active_sid);
        }
    }
    spaces.into_iter().find(|sid| state.space(*sid).is_some())
}

/// Track `display_id`'s currently visible space and re-tile every display if it
/// changed (a space switch on any display reflows that display's new space).
fn refresh_display_active_space(runtime: &mut Runtime<AxSink>, display_id: u32) {
    let Ok(sid) = current_space_for_display(display_id) else {
        return;
    };
    if runtime.state.space(sid).is_none()
        || runtime.state.display_active_space_id(display_id) == Some(sid)
    {
        return;
    }
    runtime.state.set_display_active_space(display_id, sid);
    runtime.state.flush_all_active_to(&mut runtime.sink);
}

/// Refresh the currently visible space of every display.
fn refresh_all_active_spaces(runtime: &mut Runtime<AxSink>, display_frames: &[(u32, Area)]) {
    for (display_id, _) in display_frames {
        refresh_display_active_space(runtime, *display_id);
    }
}

/// Re-read the live display topology and space/display ownership. This handles
/// display hot-plug and also preserves a space's existing tree when macOS rehomes
/// that same space id onto a different display.
fn refresh_live_display_state(
    runtime: &mut Runtime<AxSink>,
    display_frames: &mut Vec<(u32, Area)>,
) {
    let displays = match active_displays() {
        Ok(displays) => displays,
        Err(error) => {
            eprintln!("yabai-rust: failed to refresh displays: {error}");
            return;
        }
    };
    if displays.is_empty() {
        return;
    }

    // Snapshot the known topology before this refresh registers/removes anything,
    // so we can fire *_added / *_removed signals for the diff.
    let prior_display_ids: HashSet<u32> = runtime.state.display_ids().into_iter().collect();
    let prior_space_ids: HashSet<u64> = runtime.state.space_ids().into_iter().collect();

    let mut refreshed_frames = Vec::with_capacity(displays.len());
    let mut active_display_ids = HashSet::with_capacity(displays.len());
    let mut live_space_ids = HashSet::new();
    let mut complete_space_snapshot = true;

    for display in displays {
        active_display_ids.insert(display.id);
        let usable = visible_frame_for_display(display.id).unwrap_or(display.frame);
        refreshed_frames.push((display.id, usable));
        let _ = runtime.state.handle_event(StateEvent::DisplayCreated {
            display_id: display.id,
            frame: display.frame,
        });

        let mut spaces = match spaces_for_display(display.id) {
            Ok(spaces) => spaces,
            Err(error) => {
                complete_space_snapshot = false;
                eprintln!(
                    "yabai-rust: failed to refresh spaces for display {}: {error}",
                    display.id
                );
                Vec::new()
            }
        };
        let current = current_space_for_display(display.id).ok();
        if let Some(current) = current {
            if !spaces.contains(&current) {
                spaces.push(current);
            }
        }

        for sid in spaces {
            live_space_ids.insert(sid);
            let _ = runtime
                .state
                .handle_event(StateEvent::SpaceCreatedOnDisplay {
                    sid,
                    display_id: display.id,
                    frame: usable,
                });
            let _ = runtime.state.set_space_frame(sid, usable);
        }

        if let Some(current) = current {
            if runtime.state.space(current).is_some() {
                runtime.state.set_display_active_space(display.id, current);
            }
        }
    }

    if complete_space_snapshot {
        for sid in runtime.state.space_ids() {
            if !live_space_ids.contains(&sid) {
                let _ = runtime.state.handle_event(StateEvent::SpaceRemoved { sid });
            }
        }
    }

    for display_id in runtime.state.display_ids() {
        if !active_display_ids.contains(&display_id) {
            let _ = runtime
                .state
                .handle_event(StateEvent::DisplayRemoved { display_id });
        }
    }

    // Fire topology-change signals for the diff. space_created carries
    // YABAI_SPACE_ID + YABAI_SPACE_INDEX; space_destroyed carries just the id
    // (the space is already gone from state), mirroring `event_signal.c`.
    for &sid in &live_space_ids {
        if !prior_space_ids.contains(&sid) {
            let order = mission_control_spaces().unwrap_or_else(|_| runtime.state.space_ids());
            let env = vec![
                ("YABAI_SPACE_ID", sid.to_string()),
                (
                    "YABAI_SPACE_INDEX",
                    mission_control_index(&order, sid).to_string(),
                ),
            ];
            fire_signals(runtime, SignalEvent::SpaceCreated, &env, None, None, None);
        }
    }
    if complete_space_snapshot {
        for &sid in &prior_space_ids {
            if !live_space_ids.contains(&sid) {
                fire_signals(
                    runtime,
                    SignalEvent::SpaceDestroyed,
                    &[("YABAI_SPACE_ID", sid.to_string())],
                    None,
                    None,
                    None,
                );
            }
        }
    }

    // display_added carries
    // YABAI_DISPLAY_ID + YABAI_DISPLAY_INDEX; display_removed carries just the id
    // (the display is already gone from state), mirroring `event_signal.c`.
    for &display_id in &active_display_ids {
        if !prior_display_ids.contains(&display_id) {
            let mut env = vec![("YABAI_DISPLAY_ID", display_id.to_string())];
            if let Some(index) = runtime.state.display_index(display_id) {
                env.push(("YABAI_DISPLAY_INDEX", index.to_string()));
            }
            fire_signals(runtime, SignalEvent::DisplayAdded, &env, None, None, None);
        }
    }
    for &display_id in &prior_display_ids {
        if !active_display_ids.contains(&display_id) {
            fire_signals(
                runtime,
                SignalEvent::DisplayRemoved,
                &[("YABAI_DISPLAY_ID", display_id.to_string())],
                None,
                None,
                None,
            );
        }
    }

    let frames_changed = *display_frames != refreshed_frames;
    *display_frames = refreshed_frames;
    refresh_all_active_spaces(runtime, display_frames);
    if frames_changed {
        runtime.state.flush_all_active_to(&mut runtime.sink);
    }
}

fn focus_space_by_gesture(
    spaces: &[u64],
    fallback_active: Option<u64>,
    target: u64,
) -> Result<Option<i32>, String> {
    let target_display = display_for_space(target).ok();
    let base = target_display
        .and_then(|display_id| current_space_for_display(display_id).ok())
        .or(fallback_active);
    let cur = base.and_then(|sid| spaces.iter().position(|&s| s == sid));
    let new = spaces.iter().position(|&s| s == target);

    let Some((cur, new)) = cur.zip(new) else {
        return Ok(None);
    };

    let focus_display = if let Some(display_id) = target_display {
        let focus_display = cursor_display_id().map_or(true, |cursor| cursor != display_id);
        if focus_display {
            warp_cursor_to_display_center(display_id).map_err(|error| error.to_string())?;
        }
        focus_display
    } else {
        false
    };

    let steps = new as i32 - cur as i32;
    if steps != 0 {
        switch_space_by_gesture(steps).map_err(|error| error.to_string())?;
    }

    if focus_display {
        if let Some(display_id) = target_display {
            set_active_display(display_id).map_err(|error| error.to_string())?;
        }
    }

    Ok(Some(steps))
}

/// Diagnostic: dump the Mission Control space layout (global desktop order with
/// 1-based indices, marking the current space) and, with `--focus <selector>`,
/// resolve the selector and switch to it via the dock-swipe gesture. Proves the
/// SkyLight discovery + gesture path in isolation, without AX, sockets, or the
/// full WM daemon. Read-only unless `--focus` is given.
fn run_space_probe(args: &[String]) -> ExitCode {
    let displays = match active_displays() {
        Ok(displays) => displays,
        Err(error) => {
            eprintln!("yabai-rust: failed to enumerate displays: {error}");
            return ExitCode::from(1);
        }
    };
    let Some(display) = displays.first() else {
        eprintln!("yabai-rust: no active displays");
        return ExitCode::from(1);
    };

    let spaces = match mission_control_spaces() {
        Ok(spaces) if !spaces.is_empty() => spaces,
        Ok(_) => {
            eprintln!("yabai-rust: SkyLight returned no spaces (no GUI session?)");
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!("yabai-rust: failed to query spaces: {error}");
            return ExitCode::from(1);
        }
    };
    let current = cursor_display_id()
        .ok()
        .and_then(|display_id| current_space_for_display(display_id).ok())
        .or_else(|| current_space_for_display(display.id).ok());

    // Per-display summary: full bounds, usable (visible) frame, current space.
    for display in &displays {
        let usable = visible_frame_for_display(display.id);
        let current = current_space_for_display(display.id).ok();
        let display_spaces = spaces_for_display(display.id).unwrap_or_default();
        println!(
            "display {} bounds={:?} usable={:?} current_space={current:?} spaces={display_spaces:?}",
            display.id, display.frame, usable
        );
    }
    println!("mission-control order:");
    for (index, sid) in spaces.iter().enumerate() {
        let marker = if Some(*sid) == current {
            " <- current"
        } else {
            ""
        };
        println!("  mc-index {} -> sid {sid}{marker}", index + 1);
    }

    if let Some(pos) = args.iter().position(|arg| arg == "--focus") {
        let Some(token) = args.get(pos + 1) else {
            eprintln!("yabai-rust: --focus requires a selector");
            return ExitCode::from(64);
        };
        let selector = parse_selector(token);
        let target = match resolve_space_target(&spaces, current, &selector) {
            Ok(target) => target,
            Err(error) => {
                eprintln!("yabai-rust: {error}");
                return ExitCode::from(1);
            }
        };
        if current == Some(target) {
            eprintln!("yabai-rust: cannot focus an already focused space.");
            return ExitCode::from(1);
        }
        match focus_space_by_gesture(&spaces, current, target) {
            Ok(Some(steps)) => println!("focusing sid {target} ({steps} step(s))"),
            Ok(None) => println!("focusing sid {target}"),
            Err(error) => {
                eprintln!("yabai-rust: gesture failed: {error}");
                return ExitCode::from(1);
            }
        }
    }

    ExitCode::SUCCESS
}

/// True if `tokens` is a `window --focus <selector>` command — the pure core
/// resolves the target into `focused_window`, and the daemon then raises the real
/// window. A bare `window --focus` (no selector) carries no target, so it's not
/// treated as a focus move.
/// True if `tokens` request a window focus in either grammar form:
/// `window --focus <selector>` (the action carries the selector) or
/// `window <selector> --focus` (the leading target is the window to focus). A
/// bare `window --focus` with no target and no selector is not a focus request.
fn is_window_focus(tokens: &[String]) -> bool {
    let Ok(Message::Window(cmd)) = parse_message(tokens) else {
        return false;
    };
    let has_focus_action = cmd
        .actions
        .iter()
        .any(|action| matches!(action, WindowAction::Focus(_)));
    let has_focus_selector = cmd
        .actions
        .iter()
        .any(|action| matches!(action, WindowAction::Focus(Some(_))));
    has_focus_action && (cmd.target.is_some() || has_focus_selector)
}

/// True if `tokens` is a `window --minimize` command.
fn is_window_minimize(tokens: &[String]) -> bool {
    matches!(
        parse_message(tokens),
        Ok(Message::Window(cmd))
            if cmd.actions.iter().any(|action| matches!(action, WindowAction::Minimize(_)))
    )
}

/// True if `tokens` is a `window --close` command.
fn is_window_close(tokens: &[String]) -> bool {
    matches!(
        parse_message(tokens),
        Ok(Message::Window(cmd))
            if cmd.actions.iter().any(|action| matches!(action, WindowAction::Close(_)))
    )
}

/// Extract the target from a standalone `window [sel] --deminimize` or
/// `window --deminimize <sel>`. Minimized windows are no longer in the layout
/// tree, so only numeric ids and registry order selectors are resolved here.
fn window_deminimize_target(
    tokens: &[String],
    minimized_ids: &[u32],
) -> Option<Result<u32, String>> {
    let Ok(Message::Window(cmd)) = parse_message(tokens) else {
        return None;
    };
    let [WindowAction::Deminimize(selector)] = cmd.actions.as_slice() else {
        return None;
    };
    let target = selector.as_ref().or(cmd.target.as_ref());
    Some(resolve_deminimize_target(target, minimized_ids))
}

fn resolve_deminimize_target(
    target: Option<&Selector>,
    minimized_ids: &[u32],
) -> Result<u32, String> {
    match target {
        Some(Selector::Index(id)) => Ok(*id),
        Some(Selector::First) => minimized_ids
            .first()
            .copied()
            .ok_or_else(|| "could not locate a minimized window.".to_string()),
        Some(Selector::Last) => minimized_ids
            .last()
            .copied()
            .ok_or_else(|| "could not locate a minimized window.".to_string()),
        Some(_) => Err("window --deminimize selector is not yet supported.".to_string()),
        None => Err("window --deminimize requires a window id.".to_string()),
    }
}

/// Intercept `window <id> --deminimize`: the pure core intentionally drops
/// minimized windows from BSP trees, while the macOS sink keeps their AX element
/// so this path can restore the window and re-reconcile its app.
fn try_window_deminimize(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    managed: &mut HashMap<i32, HashSet<u32>>,
    signaled: &mut HashMap<i32, HashMap<u32, WindowMeta>>,
    display_frames: &[(u32, Area)],
    minimized_pids: &mut HashMap<u32, i32>,
    tokens: &[String],
) -> Option<Response> {
    let minimized_ids = runtime.sink.minimized_window_ids();
    let window_id = match window_deminimize_target(tokens, &minimized_ids)? {
        Ok(window_id) => window_id,
        Err(error) => return Some(Err(error)),
    };

    if !runtime.sink.is_minimized_registered(window_id) {
        return Some(Err(format!(
            "window with id '{window_id}' is not minimized."
        )));
    }
    let Some(pid) = minimized_pids.remove(&window_id) else {
        runtime.sink.unregister_minimized(window_id);
        return Some(Err(format!(
            "could not deminimize window with id '{window_id}'."
        )));
    };
    if !runtime.sink.set_minimized(window_id, false) {
        minimized_pids.insert(window_id, pid);
        return Some(Err(format!(
            "could not deminimize window with id '{window_id}'."
        )));
    }

    reconcile_pid(sa, runtime, managed, signaled, display_frames, pid);
    let meta = runtime.state.window_meta(window_id).or_else(|| {
        signaled
            .get(&pid)
            .and_then(|windows| windows.get(&window_id))
    });
    fire_signals(
        runtime,
        SignalEvent::WindowDeminimized,
        &[("YABAI_WINDOW_ID", window_id.to_string())],
        meta.map(|m| m.app.as_str()),
        meta.map(|m| m.title.as_str()),
        None,
    );
    Some(Ok(None))
}

/// Extract the target selector from a standalone `window [sel] --toggle
/// native-fullscreen` or `window --toggle native-fullscreen [sel]` command.
fn standalone_native_fullscreen_selector(tokens: &[String]) -> Option<Option<Selector>> {
    match tokens {
        [domain, toggle, name] if domain == "window" && toggle == "--toggle" => {
            (name == "native-fullscreen").then_some(None)
        }
        [domain, sel, toggle, name] if domain == "window" && toggle == "--toggle" => {
            (name == "native-fullscreen").then(|| Some(parse_selector(sel)))
        }
        [domain, toggle, name, sel] if domain == "window" && toggle == "--toggle" => {
            (name == "native-fullscreen").then(|| Some(parse_selector(sel)))
        }
        _ => None,
    }
}

/// True if `tokens` is a `window [sel] --toggle native-fullscreen` command.
fn is_window_native_fullscreen(tokens: &[String]) -> bool {
    matches!(
        parse_message(tokens),
        Ok(Message::Window(cmd))
            if cmd.actions.iter().any(|action| matches!(
                action,
                WindowAction::Toggle(name) if name == "native-fullscreen"
            ))
    )
}

/// Resolve the target of `window [sel] --toggle native-fullscreen` or `window
/// --toggle native-fullscreen [sel]` against the set of windows currently in
/// native fullscreen (which have left the layout trees). The outer `Option`
/// distinguishes "not this toggle" (`None`) from "this toggle"; the inner
/// `Option` is the resolved fullscreen window id, or `None` when no fullscreen
/// window matches — meaning this is an *enter* request that the normal command
/// path handles. Numeric ids, `first`/`last`, and a bare command when exactly one
/// window is fullscreen are resolved here.
fn window_fullscreen_exit_target(tokens: &[String], fullscreen_ids: &[u32]) -> Option<Option<u32>> {
    let target = standalone_native_fullscreen_selector(tokens)?;
    let resolved = match target.as_ref() {
        Some(Selector::Index(id)) if fullscreen_ids.contains(id) => Some(*id),
        Some(Selector::First) => fullscreen_ids.first().copied(),
        Some(Selector::Last) => fullscreen_ids.last().copied(),
        None if fullscreen_ids.len() == 1 => fullscreen_ids.first().copied(),
        _ => None,
    };
    Some(resolved)
}

/// Intercept the *exit* half of `window <sel> --toggle native-fullscreen`: a
/// fullscreen window has left the layout trees, so the pure core cannot act on
/// it. When `tokens` resolves to a registered-fullscreen window, clear its
/// `AXFullscreen` attribute and reconcile its app back into the layout. Returns
/// `None` for an *enter* request (no fullscreen target), letting the normal
/// command path validate the focused window so the post-step enters fullscreen.
fn try_window_native_fullscreen_exit(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    managed: &mut HashMap<i32, HashSet<u32>>,
    signaled: &mut HashMap<i32, HashMap<u32, WindowMeta>>,
    display_frames: &[(u32, Area)],
    fullscreen_pids: &mut HashMap<u32, i32>,
    tokens: &[String],
) -> Option<Response> {
    let fullscreen_ids = runtime.sink.fullscreen_window_ids();
    let window_id = window_fullscreen_exit_target(tokens, &fullscreen_ids)??;

    if !runtime.sink.is_fullscreen_registered(window_id) {
        return None;
    }
    let Some(pid) = fullscreen_pids.remove(&window_id) else {
        runtime.sink.unregister_fullscreen(window_id);
        return Some(Err(format!(
            "could not exit fullscreen for window with id '{window_id}'."
        )));
    };
    if !runtime.sink.exit_native_fullscreen(window_id) {
        fullscreen_pids.insert(window_id, pid);
        return Some(Err(format!(
            "could not exit fullscreen for window with id '{window_id}'."
        )));
    }

    reconcile_pid(sa, runtime, managed, signaled, display_frames, pid);
    Some(Ok(None))
}

/// Intercept `window [sel] --grid r:c:x:y:w:h`, mirroring the C
/// `window_manager_apply_grid`. Grid targets an **unmanaged** (floating/untracked)
/// window: a managed (tiled) window is rejected. The frame is computed purely from
/// the acting window's display's usable bounds inset by that space's padding/gap
/// ([`grid_frame`]) and applied directly via AX. Returns `None` for any other
/// command so the normal dispatch chain handles it.
fn try_window_grid(
    runtime: &Runtime<AxSink>,
    display_frames: &[(u32, Area)],
    tokens: &[String],
) -> Option<Response> {
    let Ok(Message::Window(cmd)) = parse_message(tokens) else {
        return None;
    };
    let [WindowAction::Grid(spec)] = cmd.actions.as_slice() else {
        return None;
    };
    let spec = *spec;
    let wid = match runtime.state.resolve_window_selector(cmd.target.as_ref()) {
        Ok(wid) => wid,
        Err(error) => return Some(Err(error)),
    };
    Some(window_grid_for_id(runtime, display_frames, wid, spec).map(|()| None))
}

fn window_grid_for_id(
    runtime: &Runtime<AxSink>,
    display_frames: &[(u32, Area)],
    wid: u32,
    spec: [i32; 6],
) -> Result<(), String> {
    // A managed (tiled) window lives in a layout tree; grid only applies to
    // unmanaged windows (C returns WINDOW_OP_ERROR_INVALID_SRC_VIEW).
    if runtime.state.window_space_id(wid).is_some() {
        return Err("cannot apply grid layout to a managed window.\n".to_string());
    }
    // Locate the window's display from its live frame, then inset that display's
    // usable bounds by the display's active-space padding/gap, as the C view does.
    let Some(frame) = runtime.sink.window_frame(wid) else {
        return Err(format!(
            "could not locate window with the given id '{wid}'.\n"
        ));
    };
    let center = Point {
        x: frame.x + frame.w / 2.0,
        y: frame.y + frame.h / 2.0,
    };
    let Some(&(did, bounds)) = display_frames
        .iter()
        .find(|(_, area)| area.contains_point(center))
    else {
        return Err(format!("could not locate the display of window '{wid}'.\n"));
    };
    let (padding, gap) = match runtime.state.display_active_space_id(did) {
        Some(sid) => runtime.state.grid_insets(sid),
        None => (
            [
                runtime.state.config.top_padding,
                runtime.state.config.bottom_padding,
                runtime.state.config.left_padding,
                runtime.state.config.right_padding,
            ],
            runtime.state.config.window_gap,
        ),
    };
    let target = grid_frame(bounds, padding, gap, spec);
    if runtime.sink.set_frame(wid, target) {
        Ok(())
    } else {
        Err(format!("could not apply grid layout to window '{wid}'.\n"))
    }
}

/// Intercept `window [sel] --move abs|rel:dx:dy`, mirroring the C
/// `window_manager_move_window_relative`. Move targets an **unmanaged**
/// (floating/untracked) window: a managed (tiled) window is rejected. `abs` sets
/// the window's origin to `(dx, dy)`; `rel` offsets the current origin by
/// `(dx, dy)`. The size is left unchanged. Returns `None` for any other command
/// so the normal dispatch chain handles it.
fn try_window_move(runtime: &Runtime<AxSink>, tokens: &[String]) -> Option<Response> {
    let Ok(Message::Window(cmd)) = parse_message(tokens) else {
        return None;
    };
    let [WindowAction::Move { kind, dx, dy }] = cmd.actions.as_slice() else {
        return None;
    };
    let (kind, dx, dy) = (*kind, *dx, *dy);
    let wid = match runtime.state.resolve_window_selector(cmd.target.as_ref()) {
        Ok(wid) => wid,
        Err(error) => return Some(Err(error)),
    };
    // A managed (tiled) window lives in a layout tree; move only applies to
    // unmanaged windows (C returns WINDOW_OP_ERROR_INVALID_SRC_VIEW).
    if runtime.state.window_space_id(wid).is_some() {
        return Some(Err("cannot move a managed window.\n".to_string()));
    }
    let Some(frame) = runtime.sink.window_frame(wid) else {
        return Some(Err(format!(
            "could not locate window with the given id '{wid}'.\n"
        )));
    };
    let (x, y) = match kind {
        ValueType::Abs => (dx, dy),
        ValueType::Rel => (frame.x + dx, frame.y + dy),
    };
    let target = Area {
        x,
        y,
        w: frame.w,
        h: frame.h,
    };
    if runtime.sink.set_frame(wid, target) {
        Some(Ok(None))
    } else {
        Some(Err(format!("could not move window '{wid}'.\n")))
    }
}

/// Intercept `window [sel] --resize handle:dw:dh`, mirroring
/// `window_manager_resize_window_relative`'s unmanaged branch. Only the
/// **unmanaged** (floating/untracked) case is handled here via AX; a managed
/// window's directional resize is left to the pure core's fence math (return
/// `None` to fall through), while absolute resizing of a managed window is
/// rejected. For an unmanaged window, `abs` sets the size to `(dw, dh)` leaving
/// the origin fixed; a directional handle grows/shrinks the frame from the
/// dragged edge (top/left handles also move the origin so the opposite edge
/// stays put). Returns `None` for any other command.
fn try_window_resize(runtime: &Runtime<AxSink>, tokens: &[String]) -> Option<Response> {
    let Ok(Message::Window(cmd)) = parse_message(tokens) else {
        return None;
    };
    let [WindowAction::Resize { handle, dw, dh }] = cmd.actions.as_slice() else {
        return None;
    };
    let (handle, dw, dh) = (*handle, *dw, *dh);
    let wid = match runtime.state.resolve_window_selector(cmd.target.as_ref()) {
        Ok(wid) => wid,
        Err(error) => return Some(Err(error)),
    };
    // Managed (tiled) windows are fence-resized by the pure core; only reject
    // absolute resizing here and let the directional case fall through.
    if runtime.state.window_space_id(wid).is_some() {
        if handle == HANDLE_ABS {
            return Some(Err(
                "cannot use absolute resizing on a managed window.\n".to_string()
            ));
        }
        return None;
    }
    let Some(frame) = runtime.sink.window_frame(wid) else {
        return Some(Err(format!(
            "could not locate window with the given id '{wid}'.\n"
        )));
    };
    let target = if handle == HANDLE_ABS {
        // Absolute: keep the origin, set the size.
        Area {
            x: frame.x,
            y: frame.y,
            w: dw,
            h: dh,
        }
    } else {
        // Relative: mirror `window_manager_resize_window_relative_internal`.
        let x_mod = if handle & HANDLE_LEFT != 0 {
            -1.0
        } else if handle & HANDLE_RIGHT != 0 {
            1.0
        } else {
            0.0
        };
        let y_mod = if handle & HANDLE_TOP != 0 {
            -1.0
        } else if handle & HANDLE_BOTTOM != 0 {
            1.0
        } else {
            0.0
        };
        let fw = (frame.w + dw * x_mod).max(1.0);
        let fh = (frame.h + dh * y_mod).max(1.0);
        let fx = if handle & HANDLE_LEFT != 0 {
            frame.x + frame.w - fw
        } else {
            frame.x
        };
        let fy = if handle & HANDLE_TOP != 0 {
            frame.y + frame.h - fh
        } else {
            frame.y
        };
        Area {
            x: fx,
            y: fy,
            w: fw,
            h: fh,
        }
    };
    if runtime.sink.set_frame(wid, target) {
        Some(Ok(None))
    } else {
        Some(Err(format!("could not resize window '{wid}'.\n")))
    }
}

/// Intercept `window [sel] --toggle windowed-fullscreen`, mirroring
/// `window_manager_toggle_window_windowed_fullscreen`. Entering saves the
/// window's current frame and resizes it to fill its display's usable bounds
/// (menu bar / dock excluded, no yabai padding — the C `display_bounds_constrained`
/// with `ignore_external_bar`); exiting restores the saved frame. State lives in
/// `windowed_frames` (presence = the C `WINDOW_WINDOWED` flag). Applied to any
/// window, managed or not, exactly as C does. Returns `None` for any other
/// command so the normal dispatch chain handles it.
fn try_window_windowed_fullscreen(
    runtime: &Runtime<AxSink>,
    display_frames: &[(u32, Area)],
    windowed_frames: &mut HashMap<u32, Area>,
    tokens: &[String],
) -> Option<Response> {
    let Ok(Message::Window(cmd)) = parse_message(tokens) else {
        return None;
    };
    let [WindowAction::Toggle(name)] = cmd.actions.as_slice() else {
        return None;
    };
    if name != "windowed-fullscreen" {
        return None;
    }
    let wid = match runtime.state.resolve_window_selector(cmd.target.as_ref()) {
        Ok(wid) => wid,
        Err(error) => return Some(Err(error)),
    };
    // Toggle off: restore the frame saved on entry.
    if let Some(saved) = windowed_frames.remove(&wid) {
        if runtime.sink.set_frame(wid, saved) {
            return Some(Ok(None));
        }
        // Restore failed — keep the flag so a retry can try again.
        windowed_frames.insert(wid, saved);
        return Some(Err(format!(
            "could not restore window '{wid}' from windowed-fullscreen.\n"
        )));
    }
    // Toggle on: locate the window's display, save its frame, fill the display.
    let Some(frame) = runtime.sink.window_frame(wid) else {
        return Some(Err(format!(
            "could not locate window with the given id '{wid}'.\n"
        )));
    };
    let center = Point {
        x: frame.x + frame.w / 2.0,
        y: frame.y + frame.h / 2.0,
    };
    let Some(&(_, bounds)) = display_frames
        .iter()
        .find(|(_, area)| area.contains_point(center))
    else {
        return Some(Err(format!(
            "could not locate the display of window '{wid}'.\n"
        )));
    };
    if runtime.sink.set_frame(wid, bounds) {
        windowed_frames.insert(wid, frame);
        Some(Ok(None))
    } else {
        Some(Err(format!(
            "could not make window '{wid}' windowed-fullscreen.\n"
        )))
    }
}

/// Intercept `window [sel] --toggle expose`, mirroring
/// `window_manager_toggle_window_expose`: focus the acting window with a raise,
/// then trigger App Exposé for its application via the CoreDock
/// `com.apple.expose.front.awake` notification. Returns `None` for any other
/// command so the normal dispatch chain handles it.
fn try_window_expose(runtime: &Runtime<AxSink>, tokens: &[String]) -> Option<Response> {
    let Ok(Message::Window(cmd)) = parse_message(tokens) else {
        return None;
    };
    let [WindowAction::Toggle(name)] = cmd.actions.as_slice() else {
        return None;
    };
    if name != "expose" {
        return None;
    }
    let wid = match runtime.state.resolve_window_selector(cmd.target.as_ref()) {
        Ok(wid) => wid,
        Err(error) => return Some(Err(error)),
    };
    // C focuses with a raise before notifying the Dock; a focus failure is not
    // fatal there, so we mirror that and still fire the notification.
    runtime.sink.focus_window(wid);
    yabai_macos::coredock::toggle_expose();
    Some(Ok(None))
}

/// Intercept a standalone `space --focus <selector>` and enact the active-space
/// switch through the macOS layer, returning `Some(response)`. Any other message
/// (including a `--focus` mixed with other actions) returns `None` so the caller
/// dispatches it through the pure core. Mirrors `space_manager_focus_space`'s
/// gesture fallback; the scripting-addition path is deferred (Phase 8).
/// Intercept the `space`/`window` commands that require the scripting addition
/// (`space --create`/`--destroy`, `window --space`). Returns `None` for any other
/// command so the normal dispatch chain handles it. On success it refreshes live
/// display state so the changed topology is reflected immediately.
fn try_scripting_addition(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    display_frames: &mut Vec<(u32, Area)>,
    tokens: &[String],
) -> Option<Response> {
    match parse_message(tokens) {
        Ok(Message::Space(cmd)) => {
            for action in &cmd.actions {
                let result = match action {
                    SpaceAction::Create => {
                        // Create a space on the display of the acting (target/active)
                        // space, mirroring the C `space --create`.
                        match runtime.state.resolve_space(cmd.target.as_ref()) {
                            Ok(sid) => sa
                                .create_space(sid)
                                .map(|()| None)
                                .map_err(|error| format!("could not create space: {error}\n")),
                            Err(error) => Err(error),
                        }
                    }
                    SpaceAction::Destroy(selector) => {
                        let selector = selector.as_ref().or(cmd.target.as_ref());
                        match runtime.state.resolve_space(selector) {
                            Ok(sid) => sa
                                .destroy_space(sid)
                                .map(|()| None)
                                .map_err(|error| format!("could not destroy space: {error}\n")),
                            Err(error) => Err(error),
                        }
                    }
                    SpaceAction::Display(selector) => {
                        space_to_display_via_sa(sa, runtime, cmd.target.as_ref(), selector)
                    }
                    SpaceAction::Move(selector) => {
                        space_move_via_sa(sa, runtime, cmd.target.as_ref(), selector)
                    }
                    SpaceAction::Swap(selector) => {
                        space_swap_via_sa(sa, runtime, cmd.target.as_ref(), selector)
                    }
                    SpaceAction::Switch(selector) => {
                        space_switch_via_sa(sa, runtime, display_frames, selector)
                    }
                    // `--toggle mission-control`/`show-desktop`: focus the acting
                    // space (best-effort, like C), then fire the CoreDock
                    // notification. `--toggle padding`/`gap` are pure and handled by
                    // `AppState`, so let them fall through.
                    SpaceAction::Toggle(name)
                        if name == "mission-control" || name == "show-desktop" =>
                    {
                        match runtime.state.resolve_space(cmd.target.as_ref()) {
                            Ok(sid) => {
                                let _ = sa.focus_space(sid);
                                let _ = activate_space_display_if_cross(sid);
                                if name == "mission-control" {
                                    yabai_macos::coredock::toggle_mission_control();
                                } else {
                                    yabai_macos::coredock::toggle_show_desktop();
                                }
                                Ok(None)
                            }
                            Err(error) => Err(error),
                        }
                    }
                    _ => continue,
                };
                if result.is_ok() {
                    refresh_live_display_state(runtime, display_frames);
                }
                return Some(result);
            }
            None
        }
        Ok(Message::Window(cmd)) => {
            for action in &cmd.actions {
                match action {
                    WindowAction::Space(selector) => {
                        // Move the acting (target/focused) window to the selected space,
                        // mirroring `window --space` (`scripting_addition_move_window_to_space`).
                        let result = match (
                            runtime.state.resolve_window_selector(cmd.target.as_ref()),
                            runtime.state.resolve_space(Some(selector)),
                        ) {
                            (Ok(wid), Ok(sid)) => sa
                                .move_window_to_space(sid, wid)
                                .map(|()| None)
                                .map_err(|error| {
                                    format!("could not move window to space: {error}\n")
                                }),
                            (Err(error), _) | (_, Err(error)) => Err(error),
                        };
                        if result.is_ok() {
                            refresh_live_display_state(runtime, display_frames);
                        }
                        return Some(result);
                    }
                    WindowAction::Display(selector) => {
                        // Move the acting window to the selected display's active space,
                        // mirroring the C `window --display` (which resolves the display's
                        // current space and reuses `send_window_to_space`). Same SA opcode
                        // as `window --space`.
                        let result = match (
                            runtime.state.resolve_window_selector(cmd.target.as_ref()),
                            runtime
                                .state
                                .resolve_display(Some(selector))
                                .and_then(|did| {
                                    runtime.state.display_active_space_id(did).ok_or_else(|| {
                                        format!(
                                            "could not locate the active space of display '{did}'.\n"
                                        )
                                    })
                                }),
                        ) {
                            (Ok(wid), Ok(sid)) => sa
                                .move_window_to_space(sid, wid)
                                .map(|()| None)
                                .map_err(|error| {
                                    format!("could not move window to space: {error}\n")
                                }),
                            (Err(error), _) | (_, Err(error)) => Err(error),
                        };
                        if result.is_ok() {
                            refresh_live_display_state(runtime, display_frames);
                        }
                        return Some(result);
                    }
                    // `window --opacity <float>` sets the window alpha through the
                    // SA; it is purely visual, so no tree re-flow is needed.
                    WindowAction::Opacity(opacity) => {
                        return Some(window_opacity_via_sa(
                            sa,
                            runtime,
                            cmd.target.as_ref(),
                            *opacity,
                        ));
                    }
                    // `window --sub-layer below|normal|above|auto` sets the
                    // window's SkyLight sub-level through the SA (purely visual,
                    // no re-tile).
                    WindowAction::SubLayer(layer) => {
                        return Some(window_sub_layer_via_sa(
                            sa,
                            runtime,
                            cmd.target.as_ref(),
                            *layer,
                        ));
                    }
                    // `window --scratchpad [label|recover]` assigns/removes a
                    // scratchpad label or orders hidden windows back in.
                    WindowAction::Scratchpad(action) => {
                        let result =
                            window_scratchpad_via_sa(sa, runtime, cmd.target.as_ref(), action);
                        if result.is_ok() {
                            refresh_live_display_state(runtime, display_frames);
                        }
                        return Some(result);
                    }
                    // `window --toggle sticky` / `--toggle shadow` need the SA and
                    // toggle-state tracking; other toggles fall through to the AX path.
                    WindowAction::Toggle(value) if value == "sticky" => {
                        let result = window_toggle_sticky_via_sa(sa, runtime, cmd.target.as_ref());
                        if result.is_ok() {
                            refresh_live_display_state(runtime, display_frames);
                        }
                        return Some(result);
                    }
                    WindowAction::Toggle(value) if value == "shadow" => {
                        return Some(window_toggle_shadow_via_sa(
                            sa,
                            runtime,
                            cmd.target.as_ref(),
                        ));
                    }
                    // `window --toggle pip` scales the acting window into a
                    // picture-in-picture miniature (and back) through the SA
                    // `scale_window` opcode, which self-toggles between the scaled
                    // and identity transforms — so no daemon-side state is kept.
                    WindowAction::Toggle(value) if value == "pip" => {
                        return Some(window_toggle_pip_via_sa(
                            sa,
                            runtime,
                            display_frames,
                            cmd.target.as_ref(),
                        ));
                    }
                    WindowAction::Toggle(value)
                        if runtime.state.scratchpad_window(value).is_some() =>
                    {
                        let result = window_toggle_scratchpad_via_sa(sa, runtime, value);
                        if result.is_ok() {
                            refresh_live_display_state(runtime, display_frames);
                        }
                        return Some(result);
                    }
                    // `window --raise [sel]` / `--lower [sel]` reorder the acting
                    // window above/below the (optional) reference window through
                    // the SA `order_window` opcode. Purely a z-order change.
                    WindowAction::Raise(selector) => {
                        return Some(window_order_via_sa(
                            sa,
                            runtime,
                            cmd.target.as_ref(),
                            selector.as_ref(),
                            1,
                        ));
                    }
                    WindowAction::Lower(selector) => {
                        return Some(window_order_via_sa(
                            sa,
                            runtime,
                            cmd.target.as_ref(),
                            selector.as_ref(),
                            -1,
                        ));
                    }
                    _ => continue,
                }
            }
            None
        }
        Ok(Message::Rule(RuleCommand::Apply(apply))) => {
            let result = runtime
                .state
                .apply_rule_and_collect_effects(apply)
                .map(|applications| {
                    apply_rule_effects_at_boundary(sa, runtime, display_frames, &applications);
                    runtime.state.flush_all_active_to(&mut runtime.sink);
                    None
                });
            if result.is_ok() {
                refresh_live_display_state(runtime, display_frames);
            }
            Some(result)
        }
        _ => None,
    }
}

fn apply_rule_effects_at_boundary(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    display_frames: &[(u32, Area)],
    applications: &[AppliedRuleEffects],
) {
    for application in applications {
        let wid = application.window_id;
        let effects = &application.effects;
        let _ = move_window_for_rule_effect(sa, runtime, wid, effects);
        if let Some(sticky) = effects.sticky {
            let _ = window_set_sticky_via_sa(sa, runtime, wid, sticky, application.sid);
        }
        if let Some(layer) = effects.layer {
            let _ = window_sub_layer_for_id_via_sa(sa, runtime, wid, layer);
        }
        if let Some(opacity) = effects.opacity {
            let _ = window_opacity_for_id_via_sa(sa, runtime, wid, opacity);
        }
        if let Some(grid) = effects.grid {
            let spec = grid.map(|value| value as i32);
            let _ = window_grid_for_id(runtime, display_frames, wid, spec);
        }
    }
}

fn move_window_for_rule_effect(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    wid: u32,
    effects: &yabai_core::RuleEffects,
) -> Result<(), String> {
    let sid = if let Some(space) = &effects.space {
        let selector = parse_selector(space);
        runtime.state.resolve_space(Some(&selector))?
    } else if let Some(display) = &effects.display {
        let selector = parse_selector(display);
        let did = runtime.state.resolve_display(Some(&selector))?;
        runtime
            .state
            .display_active_space_id(did)
            .ok_or_else(|| format!("could not locate the active space of display '{did}'.\n"))?
    } else {
        return Ok(());
    };

    sa.move_window_to_space(sid, wid)
        .map_err(|error| format!("could not move window to space: {error}\n"))?;
    let _ = runtime.state.assign_window_to_space(wid, sid);
    if effects.follow_space || effects.fullscreen == Some(true) {
        let _ = sa.focus_space(sid);
        let _ = activate_space_display_if_cross(sid);
        runtime.state.set_active_space(sid);
    }
    Ok(())
}

/// Set the acting window's opacity through the scripting addition, mirroring the
/// C `window --opacity` (`scripting_addition_set_opacity` with the configured
/// `window_opacity_duration`). The parser has already validated the opacity
/// range, so this only resolves the acting window and performs the SA call.
fn window_opacity_via_sa(
    sa: &ScriptingAddition,
    runtime: &Runtime<AxSink>,
    target: Option<&Selector>,
    opacity: f32,
) -> Response {
    let wid = runtime.state.resolve_window_selector(target)?;
    window_opacity_for_id_via_sa(sa, runtime, wid, opacity).map(|()| None)
}

fn window_opacity_for_id_via_sa(
    sa: &ScriptingAddition,
    runtime: &Runtime<AxSink>,
    wid: u32,
    opacity: f32,
) -> Result<(), String> {
    let duration = runtime.state.config.window_opacity_duration;
    sa.set_opacity(wid, opacity, duration)
        .map_err(|_| {
            format!(
                "could not change opacity of window with id '{wid}' due to an error with the scripting-addition.\n"
            )
        })
}

/// Reorder the acting window relative to an optional reference window through the
/// scripting addition, mirroring the C `window --raise`/`--lower`
/// (`scripting_addition_order_window(acting, ±1, reference)`). `order` is `+1`
/// (raise/above) or `-1` (lower/below); a reference window id of `0` orders the
/// acting window above/below everything. Faithful to the C `daemon_fail` strings.
fn window_order_via_sa(
    sa: &ScriptingAddition,
    runtime: &Runtime<AxSink>,
    target: Option<&Selector>,
    reference: Option<&Selector>,
    order: i32,
) -> Response {
    let wid = runtime.state.resolve_window_selector(target)?;
    // A bare command orders relative to all windows (reference id 0); a given
    // selector orders relative to that specific window.
    let reference_wid = match reference {
        Some(selector) => runtime.state.resolve_window_selector(Some(selector))?,
        None => 0,
    };
    let verb = if order >= 0 { "raise" } else { "lower" };
    sa.order_window(wid, order, reference_wid)
        .map(|()| None)
        .map_err(|_| {
            format!(
                "could not {verb} window with id '{wid}' due to an error with the scripting-addition.\n"
            )
        })
}

// `CGWindowLevelKey` values passed to the SA layer opcode, which resolves them via
// `CGWindowLevelForKey` (mirroring the C `LAYER_*` macros in `misc/macros.h`).
// LAYER_AUTO (key 0) is always resolved to below/normal before sending, so it is
// never passed to the SA directly.
const CG_WINDOW_LEVEL_KEY_BACKSTOP: i32 = 3; // kCGBackstopMenuLevelKey (LAYER_BELOW)
const CG_WINDOW_LEVEL_KEY_NORMAL: i32 = 4; // kCGNormalWindowLevelKey (LAYER_NORMAL)
const CG_WINDOW_LEVEL_KEY_FLOATING: i32 = 5; // kCGFloatingWindowLevelKey (LAYER_ABOVE)

/// Set the acting window's SkyLight sub-level through the SA, mirroring the C
/// `window --sub-layer` (`window_manager_set_window_layer` →
/// `scripting_addition_set_layer`). `auto` resolves to `below` for a managed
/// (tiled) window and `normal` otherwise, matching the C default; the C's
/// associated-child-window propagation is not modeled here (single-window sublevel).
fn window_sub_layer_via_sa(
    sa: &ScriptingAddition,
    runtime: &Runtime<AxSink>,
    target: Option<&Selector>,
    layer: Layer,
) -> Response {
    let wid = runtime.state.resolve_window_selector(target)?;
    window_sub_layer_for_id_via_sa(sa, runtime, wid, layer).map(|()| None)
}

fn window_sub_layer_for_id_via_sa(
    sa: &ScriptingAddition,
    runtime: &Runtime<AxSink>,
    wid: u32,
    layer: Layer,
) -> Result<(), String> {
    let layer = match layer {
        Layer::Below => CG_WINDOW_LEVEL_KEY_BACKSTOP,
        Layer::Normal => CG_WINDOW_LEVEL_KEY_NORMAL,
        Layer::Above => CG_WINDOW_LEVEL_KEY_FLOATING,
        Layer::Auto => {
            // LAYER_AUTO: a managed (tiled) window sinks below floats; otherwise normal.
            if runtime.state.window_space_id(wid).is_some() && !runtime.state.is_floating(wid) {
                CG_WINDOW_LEVEL_KEY_BACKSTOP
            } else {
                CG_WINDOW_LEVEL_KEY_NORMAL
            }
        }
    };
    sa.set_layer(wid, layer).map_err(|_| {
        format!(
            "could not change sub-layer of window with id '{wid}' due to an error with the scripting-addition.\n"
        )
    })
}

const RESERVED_SCRATCHPAD_LABELS: &[&str] = &[
    "float",
    "sticky",
    "shadow",
    "split",
    "zoom-parent",
    "zoom-fullscreen",
    "windowed-fullscreen",
    "native-fullscreen",
    "expose",
    "pip",
    "recover",
];

fn validate_scratchpad_label(label: &str) -> Result<(), String> {
    if label.parse::<u64>().is_ok() {
        return Err(format!("'{label}' cannot be used as a label.\n"));
    }
    if RESERVED_SCRATCHPAD_LABELS.contains(&label) {
        return Err(format!(
            "'{label}' is a reserved keyword and cannot be used as a scratchpad.\n"
        ));
    }
    Ok(())
}

/// Handle `window --scratchpad [label|recover]`. Assigning a label records it in
/// the runtime and floats the window; a bare command removes the assignment and
/// tiles the window back on the active space. `recover` orders all registered
/// windows back in, mirroring the C recovery path over the daemon's known window
/// set.
fn window_scratchpad_via_sa(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    target: Option<&Selector>,
    action: &ScratchpadAction,
) -> Response {
    if matches!(action, ScratchpadAction::Recover) {
        return sa
            .order_window_in(&runtime.sink.active_window_ids())
            .map(|()| None)
            .map_err(|_| {
                "could not recover scratchpad windows due to an error with the scripting-addition.\n"
                    .to_string()
            });
    }

    let wid = runtime.state.resolve_window_selector(target)?;
    let active_sid = runtime
        .state
        .active_space_id()
        .ok_or_else(|| "no active space".to_string())?;

    if matches!(action, ScratchpadAction::Remove) {
        if runtime.state.window_scratchpad(wid).is_none() {
            return Err("the selected window was not assigned to a scratchpad!\n".to_string());
        }
        sa.move_window_to_space(active_sid, wid).map_err(|_| {
            format!(
                "could not move scratchpad window with id '{wid}' due to an error with the scripting-addition.\n"
            )
        })?;
        sa.order_window(wid, 1, 0).map_err(|_| {
            format!(
                "could not order scratchpad window with id '{wid}' due to an error with the scripting-addition.\n"
            )
        })?;
        runtime.sink.focus_window(wid);
        runtime
            .state
            .remove_window_scratchpad(wid, true, active_sid);
        runtime.state.flush_all_active_to(&mut runtime.sink);
        return Ok(None);
    }

    let ScratchpadAction::Label(label) = action else {
        unreachable!();
    };
    validate_scratchpad_label(label)?;
    let sid = runtime
        .state
        .window_known_space_id(wid)
        .or_else(|| runtime.state.active_space_id())
        .ok_or_else(|| "no active space".to_string())?;
    runtime
        .state
        .set_window_scratchpad(wid, label.clone(), sid)?;
    runtime.state.flush_all_active_to(&mut runtime.sink);
    Ok(None)
}

/// Toggle a scratchpad by label, mirroring C `window --toggle <label>`: hide an
/// ordered-in scratchpad on the visible space, order it back in if hidden, or move
/// it to the active space and show it when it lives elsewhere.
fn window_toggle_scratchpad_via_sa(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    label: &str,
) -> Response {
    let wid = runtime
        .state
        .scratchpad_window(label)
        .ok_or_else(|| format!("unknown scratchpad '{label}'.\n"))?;
    let active_sid = runtime
        .state
        .active_space_id()
        .ok_or_else(|| "no active space".to_string())?;
    let visible_space = runtime.state.is_sticky(wid)
        || runtime.state.window_known_space_id(wid) == Some(active_sid);
    let ordered_in = window_is_ordered_in(wid).unwrap_or(false);

    if visible_space && ordered_in {
        if runtime.state.focused_window_id() == Some(wid) {
            if let Some(next) = runtime
                .state
                .flush(active_sid)
                .and_then(|frames| frames.into_iter().find(|frame| frame.window_id != wid))
            {
                runtime.sink.focus_window(next.window_id);
                runtime.state.set_focused_window(Some(next.window_id));
            }
        }
        sa.order_window(wid, 0, 0).map(|()| None).map_err(|_| {
            format!(
                "could not hide scratchpad window with id '{wid}' due to an error with the scripting-addition.\n"
            )
        })
    } else {
        if !visible_space {
            sa.move_window_to_space(active_sid, wid).map_err(|_| {
                format!(
                    "could not move scratchpad window with id '{wid}' due to an error with the scripting-addition.\n"
                )
            })?;
            let _ = runtime.state.assign_window_to_space(wid, active_sid);
        }
        sa.order_window(wid, 1, 0).map_err(|_| {
            format!(
                "could not show scratchpad window with id '{wid}' due to an error with the scripting-addition.\n"
            )
        })?;
        runtime.sink.focus_window(wid);
        runtime.state.set_focused_window(Some(wid));
        Ok(None)
    }
}

/// Toggle the acting window's sticky flag through the SA, mirroring the C
/// `window --toggle sticky` (`window_manager_make_window_sticky`): making a window
/// sticky untiles it (it now shows on every space); un-sticky re-tiles it into its
/// space unless it is also floating. The daemon tracks the toggle state.
fn window_toggle_sticky_via_sa(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    target: Option<&Selector>,
) -> Response {
    let wid = runtime.state.resolve_window_selector(target)?;
    let sticky = !runtime.state.is_sticky(wid);
    let sid = runtime
        .state
        .window_space_id(wid)
        .or_else(|| runtime.state.active_space_id())
        .unwrap_or(0);
    window_set_sticky_via_sa(sa, runtime, wid, sticky, sid).map(|()| None)
}

fn window_set_sticky_via_sa(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    wid: u32,
    sticky: bool,
    sid: u64,
) -> Result<(), String> {
    sa.set_sticky(wid, sticky).map_err(|_| {
        format!(
            "could not change sticky of window with id '{wid}' due to an error with the scripting-addition.\n"
        )
    })?;
    runtime.state.set_window_sticky(wid, sticky, sid);
    runtime.state.flush_all_active_to(&mut runtime.sink);
    Ok(())
}

/// Toggle the acting window's shadow through the SA, mirroring the C
/// `window --toggle shadow` (`window_manager_toggle_window_shadow`). Purely visual,
/// so no re-tile; the daemon tracks the toggle state.
fn window_toggle_shadow_via_sa(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    target: Option<&Selector>,
) -> Response {
    let wid = runtime.state.resolve_window_selector(target)?;
    let has_shadow = !runtime.state.window_has_shadow(wid);
    sa.set_shadow(wid, has_shadow).map_err(|_| {
        format!(
            "could not change shadow of window with id '{wid}' due to an error with the scripting-addition.\n"
        )
    })?;
    runtime.state.set_window_shadow(wid, has_shadow);
    Ok(None)
}

/// `window --toggle pip`, mirroring `window_manager_toggle_window_pip`: scale the
/// acting window into (or out of) a picture-in-picture miniature via the SA
/// `scale_window` opcode, targeting the usable bounds of the window's display
/// inset by that display's active-space padding (as the C view does). The SA
/// opcode itself flips between the scaled and identity transforms, so this is
/// stateless on the daemon side. Applies to any window, managed or not, like C.
fn window_toggle_pip_via_sa(
    sa: &ScriptingAddition,
    runtime: &Runtime<AxSink>,
    display_frames: &[(u32, Area)],
    target: Option<&Selector>,
) -> Response {
    let wid = runtime.state.resolve_window_selector(target)?;
    let Some(frame) = runtime.sink.window_frame(wid) else {
        return Err(format!(
            "could not locate window with the given id '{wid}'.\n"
        ));
    };
    let center = Point {
        x: frame.x + frame.w / 2.0,
        y: frame.y + frame.h / 2.0,
    };
    let Some(&(did, bounds)) = display_frames
        .iter()
        .find(|(_, area)| area.contains_point(center))
    else {
        return Err(format!("could not locate the display of window '{wid}'.\n"));
    };
    // Inset the display's usable bounds by its active-space padding, matching the
    // C `if (view_check_flag(dview, VIEW_ENABLE_PADDING))` branch. `[top, bottom,
    // left, right]`.
    let bounds = match runtime.state.display_active_space_id(did) {
        Some(sid) => {
            let ([top, bottom, left, right], _gap) = runtime.state.grid_insets(sid);
            Area {
                x: bounds.x + left as f32,
                y: bounds.y + top as f32,
                w: bounds.w - (left + right) as f32,
                h: bounds.h - (top + bottom) as f32,
            }
        }
        None => bounds,
    };
    sa.scale_window(wid, bounds.x, bounds.y, bounds.w, bounds.h)
        .map_err(|_| {
        format!(
            "could not scale window with id '{wid}' due to an error with the scripting-addition.\n"
        )
    })?;
    Ok(None)
}

/// Move the acting (target/active) space to another display's active space
/// through the scripting addition, mirroring the C `space --display`
/// (`space_manager_move_space_to_display`). Faithful to the C validation order
/// and `daemon_fail` strings (the mission-control / display-animating guards are
/// omitted — the standalone daemon has no cheap detection for them).
fn space_to_display_via_sa(
    sa: &ScriptingAddition,
    runtime: &Runtime<AxSink>,
    target: Option<&Selector>,
    selector: &Selector,
) -> Response {
    let acting_sid = runtime.state.resolve_space(target)?;
    let did = runtime.state.resolve_display(Some(selector))?;
    let src_did = runtime
        .state
        .space_display(acting_sid)
        .ok_or_else(|| "could not locate the space to act on.\n".to_string())?;
    if src_did == did {
        return Err("acting space is already located on the given display.\n".to_string());
    }
    if runtime.state.space_ids_for_display(src_did).len() <= 1 {
        return Err(
            "acting space is the last user-space on the source display and cannot be moved.\n"
                .to_string(),
        );
    }
    let dst_sid = runtime
        .state
        .display_active_space_id(did)
        .ok_or_else(|| "could not locate the active space of the given display.\n".to_string())?;
    // The C focuses the source display's previous space after moving away its
    // active space; pass that previous space (in live mission-control order) so
    // the SA can restore focus there. For a non-active source space, `0`.
    let focus = runtime.state.active_space_id() == Some(acting_sid);
    let src_prev = if focus {
        prev_space_on_display(src_did, acting_sid)
    } else {
        0
    };
    sa.move_space_to_display(acting_sid, dst_sid, src_prev, focus)
        .map(|()| None)
        .map_err(|_| {
            "cannot send space to display due to an error with the scripting-addition.\n"
                .to_string()
        })
}

/// The space immediately before `sid` on `did` in live mission-control order
/// (`0` if none / on lookup failure) — used to tell the SA which space the
/// source display should focus after its active space is moved away.
fn prev_space_on_display(did: u32, sid: u64) -> u64 {
    let spaces = match spaces_for_display(did) {
        Ok(spaces) => spaces,
        Err(_) => return 0,
    };
    match spaces.iter().position(|&s| s == sid) {
        Some(index) if index > 0 => spaces[index - 1],
        _ => 0,
    }
}

/// Reorder the acting (target/active) space relative to the selected space on the
/// same display through the scripting addition, mirroring the C `space --move`
/// (`space_manager_move_space_to_space`). Faithful to the C validation order,
/// the "is this space first on its display?" test (global mission-control order),
/// the three reordering branches, and the `daemon_fail` strings. The
/// mission-control-active / display-animating guards are omitted (no cheap
/// detection in the standalone daemon).
fn space_move_via_sa(
    sa: &ScriptingAddition,
    runtime: &Runtime<AxSink>,
    target: Option<&Selector>,
    selector: &Selector,
) -> Response {
    let acting_sid = runtime.state.resolve_space(target)?;
    let selector_sid = runtime.state.resolve_space(Some(selector))?;
    if acting_sid == selector_sid {
        return Err("cannot move space to itself.\n".to_string());
    }
    let acting_did = runtime
        .state
        .space_display(acting_sid)
        .ok_or_else(|| "could not locate the space to act on.\n".to_string())?;
    let selector_did = runtime
        .state
        .space_display(selector_sid)
        .ok_or_else(|| "could not locate the space to act on.\n".to_string())?;
    if acting_did != selector_did {
        return Err(
            "cannot move space across display boundaries. use --display instead.\n".to_string(),
        );
    }

    // The reordering decision needs the global mission-control order (each
    // display's spaces flattened in turn), matching the C
    // `space_manager_prev_space` / `space_manager_mission_control_index`.
    let order = mission_control_spaces().unwrap_or_default();
    let acting_prev = global_prev_space(&order, acting_sid);
    let selector_prev = global_prev_space(&order, selector_sid);
    // A space is "first on its display" when it has no global predecessor, or its
    // predecessor lives on a different display.
    let acting_is_first =
        acting_prev.is_none_or(|prev| runtime.state.space_display(prev) != Some(acting_did));
    let selector_is_first =
        selector_prev.is_none_or(|prev| runtime.state.space_display(prev) != Some(selector_did));
    let focus = runtime.state.active_space_id() == Some(acting_sid);

    let ok = if acting_is_first && !selector_is_first {
        sa.move_space_after_space(acting_sid, selector_sid, focus)
            .is_ok()
    } else if !acting_is_first && selector_is_first {
        sa.move_space_after_space(acting_sid, selector_sid, focus)
            .is_ok()
            && sa
                .move_space_after_space(selector_sid, acting_sid, false)
                .is_ok()
    } else if !acting_is_first && !selector_is_first {
        // Both mid-list: insert acting after the selector, or after the selector's
        // predecessor when acting currently sits later in the order.
        let acting_mci = mission_control_index(&order, acting_sid);
        let selector_mci = mission_control_index(&order, selector_sid);
        let anchor = if acting_mci > selector_mci {
            selector_prev.unwrap_or(0)
        } else {
            selector_sid
        };
        sa.move_space_after_space(acting_sid, anchor, focus).is_ok()
    } else {
        // Both first on the same display is impossible (one predecessor test),
        // so this is a no-op that still reports success.
        true
    };

    if ok {
        Ok(None)
    } else {
        Err("cannot move space due to an error with the scripting-addition.\n".to_string())
    }
}

/// Swap the acting (target/active) space with the selected space through the
/// scripting addition, mirroring the C `space --swap`
/// (`space_manager_swap_space_with_space`). Same-display: the 5-branch reordering
/// that exchanges the two spaces' slots via `move_space_after_space`.
/// Cross-display: exchange the two spaces' window contents (like the C
/// `..._on_display`, which moves each space's window list to the other) — now
/// enabled by the macOS-26 window→space fix. The mission-control-active /
/// display-animating guards are omitted (no cheap detection in the standalone
/// daemon).
fn space_swap_via_sa(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    target: Option<&Selector>,
    selector: &Selector,
) -> Response {
    let acting_sid = runtime.state.resolve_space(target)?;
    let selector_sid = runtime.state.resolve_space(Some(selector))?;
    if acting_sid == selector_sid {
        return Err("cannot swap space with itself.\n".to_string());
    }
    let acting_did = runtime
        .state
        .space_display(acting_sid)
        .ok_or_else(|| "could not locate the space to act on.\n".to_string())?;
    let selector_did = runtime
        .state
        .space_display(selector_sid)
        .ok_or_else(|| "could not locate the space to act on.\n".to_string())?;
    if acting_did != selector_did {
        return space_swap_cross_display(sa, runtime, acting_sid, selector_sid);
    }

    let order = mission_control_spaces().unwrap_or_default();
    let acting_prev = global_prev_space(&order, acting_sid);
    let selector_prev = global_prev_space(&order, selector_sid);
    let acting_is_first =
        acting_prev.is_none_or(|prev| runtime.state.space_display(prev) != Some(acting_did));
    let selector_is_first =
        selector_prev.is_none_or(|prev| runtime.state.space_display(prev) != Some(selector_did));
    let acting_mci = mission_control_index(&order, acting_sid) as i64;
    let selector_mci = mission_control_index(&order, selector_sid) as i64;
    let focus_acting = runtime.state.active_space_id() == Some(acting_sid);
    let focus_selector = runtime.state.active_space_id() == Some(selector_sid);
    let acting_prev = acting_prev.unwrap_or(0);
    let selector_prev = selector_prev.unwrap_or(0);

    // The five branches of the C same-display swap, in order.
    let ok = if acting_is_first && !selector_is_first && selector_mci - acting_mci == 1 {
        sa.move_space_after_space(acting_sid, selector_sid, focus_acting)
            .is_ok()
    } else if !acting_is_first && selector_is_first && acting_mci - selector_mci == 1 {
        sa.move_space_after_space(selector_sid, acting_sid, focus_selector)
            .is_ok()
    } else if acting_is_first && !selector_is_first {
        sa.move_space_after_space(selector_sid, acting_sid, false)
            .is_ok()
            && sa
                .move_space_after_space(acting_sid, selector_prev, focus_acting)
                .is_ok()
    } else if !acting_is_first && selector_is_first {
        sa.move_space_after_space(acting_sid, selector_sid, focus_acting)
            .is_ok()
            && sa
                .move_space_after_space(selector_sid, acting_prev, false)
                .is_ok()
    } else if acting_mci > selector_mci {
        sa.move_space_after_space(selector_sid, acting_sid, false)
            .is_ok()
            && sa
                .move_space_after_space(acting_sid, selector_prev, focus_acting)
                .is_ok()
    } else {
        sa.move_space_after_space(acting_sid, selector_sid, focus_acting)
            .is_ok()
            && sa
                .move_space_after_space(selector_sid, acting_prev, false)
                .is_ok()
    };

    if ok {
        Ok(None)
    } else {
        Err("cannot swap space due to an error with the scripting-addition.\n".to_string())
    }
}

/// Cross-display `space --swap`: exchange the two spaces' window contents,
/// mirroring the C `space_manager_swap_space_with_space_on_display` (which moves
/// each space's window list to the other space). Only the daemon's *managed* app
/// windows are moved (from the per-space trees, now correct on macOS 26 thanks to
/// `windows_on_space`), so desktop/helper windows are never disturbed. The model is
/// updated to match and both displays' active spaces are re-tiled.
fn space_swap_cross_display(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    acting_sid: u64,
    selector_sid: u64,
) -> Response {
    let acting_windows = runtime
        .state
        .space(acting_sid)
        .map(|tree| tree.window_list())
        .unwrap_or_default();
    let selector_windows = runtime
        .state
        .space(selector_sid)
        .map(|tree| tree.window_list())
        .unwrap_or_default();

    // Physically move each space's windows to the other space via the SA.
    let swap_err =
        || "cannot swap space due to an error with the scripting-addition.\n".to_string();
    if !acting_windows.is_empty() {
        sa.move_window_list_to_space(selector_sid, &acting_windows)
            .map_err(|_| swap_err())?;
    }
    if !selector_windows.is_empty() {
        sa.move_window_list_to_space(acting_sid, &selector_windows)
            .map_err(|_| swap_err())?;
    }

    // Reflect the swap in the daemon model (captured lists are disjoint, so the
    // reassignments don't interfere).
    for wid in &acting_windows {
        let _ = runtime.state.assign_window_to_space(*wid, selector_sid);
    }
    for wid in &selector_windows {
        let _ = runtime.state.assign_window_to_space(*wid, acting_sid);
    }

    // Re-tile every display's active space so the moved windows are laid out.
    runtime.state.flush_all_active_to(&mut runtime.sink);
    Ok(None)
}

/// `space --switch <sel>`, mirroring the C `space_manager_switch_space`. The
/// destination is resolved by mission-control index (like `space --focus`); the
/// acting side is always the current active space. Same display → SA `focus_space`
/// (SA-only, no gesture fallback, matching the C). Different display → swap the two
/// spaces' window contents and keep focus on the source display (the C
/// `swap_space_with_space_on_display` + `focus_display` branch).
fn space_switch_via_sa(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    display_frames: &[(u32, Area)],
    selector: &Selector,
) -> Response {
    let spaces = match mission_control_spaces() {
        Ok(spaces) if !spaces.is_empty() => spaces,
        _ => return Err("could not enumerate spaces.\n".to_string()),
    };
    let cur_sid = runtime.state.active_space_id().or_else(|| {
        display_frames
            .first()
            .and_then(|(display_id, _)| current_space_for_display(*display_id).ok())
    });
    let sid = resolve_space_target(&spaces, cur_sid, selector)?;
    let cur_sid = cur_sid.ok_or_else(|| "could not locate the active space.\n".to_string())?;
    if cur_sid == sid {
        return Err("cannot focus an already focused space.\n".to_string());
    }

    let cur_did = runtime.state.space_display(cur_sid);
    let did = runtime.state.space_display(sid);
    if let (Some(cur_did), Some(did)) = (cur_did, did) {
        if cur_did != did {
            // Cross-display: swap the two spaces' window contents; focus stays on the
            // source display (the content swap leaves each space current on its display).
            space_swap_cross_display(sa, runtime, cur_sid, sid)?;
            runtime.state.set_active_space(cur_sid);
            return Ok(None);
        }
    }

    // Same display: the SA `focus_space` opcode, matching the C (no gesture fallback).
    sa.focus_space(sid).map_err(|_| {
        "cannot focus space due to an error with the scripting-addition.\n".to_string()
    })?;
    activate_space_display_if_cross(sid)?;
    refresh_all_active_spaces(runtime, display_frames);
    runtime.state.set_active_space(sid);
    Ok(None)
}

/// The space immediately before `sid` in the global mission-control order, if any.
fn global_prev_space(order: &[u64], sid: u64) -> Option<u64> {
    match order.iter().position(|&s| s == sid) {
        Some(index) if index > 0 => Some(order[index - 1]),
        _ => None,
    }
}

/// The 1-based mission-control index of `sid` in the global order (`0` if absent),
/// matching the C `space_manager_mission_control_index`.
fn mission_control_index(order: &[u64], sid: u64) -> usize {
    order
        .iter()
        .position(|&s| s == sid)
        .map_or(0, |index| index + 1)
}

/// `display --focus <DISPLAY_SEL>` and `display --space <SPACE_SEL>` need live
/// macOS state (cursor warp, active-display activation, the SA `focus_space`
/// opcode), so they are intercepted here. `display --label` is a pure model
/// change, so this returns `None` for it (handled by `AppState`). Mirrors the C
/// `handle_domain_display`.
fn try_display(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    display_frames: &[(u32, Area)],
    tokens: &[String],
) -> Option<Response> {
    let Ok(Message::Display(cmd)) = parse_message(tokens) else {
        return None;
    };

    // The acting display: an explicit leading selector, else the display under the
    // cursor (C `display_manager_active_display_id`), else the model's active display.
    let acting = match cmd.target.as_ref() {
        Some(selector) => match runtime.state.resolve_display(Some(selector)) {
            Ok(did) => did,
            Err(error) => return Some(Err(error)),
        },
        None => match cursor_display_id()
            .ok()
            .or_else(|| runtime.state.resolve_display(None).ok())
        {
            Some(did) => did,
            None => return Some(Err("could not locate the display to act on!\n".to_string())),
        },
    };

    // A display command carries a single acting sub-command; `--label` and a bare
    // `--focus` are pure/ill-formed and fall through to `AppState`.
    match cmd.actions.first()? {
        DisplayAction::Focus(Some(selector)) => {
            let dest = match runtime.state.resolve_display(Some(selector)) {
                Ok(did) => did,
                Err(error) => return Some(Err(error)),
            };
            if dest == acting {
                return Some(Err("cannot focus an already focused display.\n".to_string()));
            }
            Some(focus_display(runtime, display_frames, dest))
        }
        DisplayAction::Space(selector) => {
            let sid = match runtime.state.resolve_space(Some(selector)) {
                Ok(sid) => sid,
                Err(error) => return Some(Err(error)),
            };
            // The space must belong to the acting display (C SAME_DISPLAY).
            if display_for_space(sid).ok() != Some(acting) {
                return Some(Err(
                    "acting display does not contain the given space.\n".to_string()
                ));
            }
            if sa.focus_space(sid).is_err() {
                return Some(Err(
                    "cannot focus space due to an error with the scripting-addition.\n".to_string(),
                ));
            }
            if let Err(error) = activate_space_display_if_cross(sid) {
                return Some(Err(error));
            }
            refresh_all_active_spaces(runtime, display_frames);
            runtime.state.set_active_space(sid);
            Some(Ok(None))
        }
        // `--focus` with no selector and `--label` are not intercepted.
        DisplayAction::Focus(None) | DisplayAction::Label(_) => None,
    }
}

/// Focus a display: focus the first window on its active space if one exists
/// (raise + center cursor + activate the display), otherwise warp the cursor to
/// the display center and activate it. Mirrors the C
/// `display_manager_focus_display`.
fn focus_display(
    runtime: &mut Runtime<AxSink>,
    display_frames: &[(u32, Area)],
    dest: u32,
) -> Response {
    let dest_space = runtime
        .state
        .display_active_space_id(dest)
        .or_else(|| current_space_for_display(dest).ok());

    if let Some(sid) = dest_space {
        if let Some(wid) = runtime.state.first_window_on_space(sid) {
            runtime.sink.focus_window(wid);
            runtime.state.set_focused_window(Some(wid));
            center_mouse_on_focus(runtime, wid);
            set_active_display(dest).map_err(|error| error.to_string())?;
            runtime.state.set_active_space(sid);
            refresh_all_active_spaces(runtime, display_frames);
            return Ok(None);
        }
    }

    warp_cursor_to_display_center(dest).map_err(|error| error.to_string())?;
    set_active_display(dest).map_err(|error| error.to_string())?;
    if let Some(sid) = dest_space {
        runtime.state.set_active_space(sid);
    }
    refresh_all_active_spaces(runtime, display_frames);
    Ok(None)
}

fn try_space_focus(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    display_frames: &[(u32, Area)],
    tokens: &[String],
) -> Option<Response> {
    let Ok(Message::Space(cmd)) = parse_message(tokens) else {
        return None;
    };
    let [SpaceAction::Focus(Some(selector))] = cmd.actions.as_slice() else {
        return None;
    };

    let spaces = match mission_control_spaces() {
        Ok(spaces) if !spaces.is_empty() => spaces,
        _ => return Some(Err("could not enumerate spaces.".to_string())),
    };
    let active = runtime.state.active_space_id().or_else(|| {
        display_frames
            .first()
            .and_then(|(display_id, _)| current_space_for_display(*display_id).ok())
    });

    let target = match resolve_space_target(&spaces, active, selector) {
        Ok(target) => target,
        Err(error) => return Some(Err(error)),
    };
    if Some(target) == active {
        return Some(Err("cannot focus an already focused space.".to_string()));
    }

    // Prefer the scripting addition's instant `focus_space` opcode (mirroring the
    // C `space_manager_focus_space`, which always uses the SA when it is loaded);
    // fall back to the dock-swipe gesture only when the SA is unavailable or the
    // opcode fails.
    //
    // NOTE: the C yabai-plus additionally drops the front process to Finder after
    // an SA focus of an *empty* same-display space, to stop macOS bouncing back to
    // the previous space (the previously frontmost app keeps its key window on the
    // old space). That needs process-manager state this daemon does not model yet;
    // the bounce-back has not reproduced with this standalone daemon (verified on
    // macOS 26: SA-focusing an empty space with another app frontmost stayed put),
    // so it is deferred — add the Finder-drop if it surfaces. We deliberately do
    // NOT gate the SA path on the destination having a managed window: per-space
    // window tracking is unreliable for non-current spaces on macOS 26, so such a
    // gate would make the SA path unreachable.
    let mut used_scripting_addition = false;
    match sa.focus_space(target) {
        Ok(()) => {
            if let Err(error) = activate_space_display_if_cross(target) {
                return Some(Err(error));
            }
            eprintln!("yabai-rust: focused space {target} via scripting addition");
            used_scripting_addition = true;
        }
        Err(error) => {
            eprintln!("yabai-rust: scripting-addition space focus failed ({error}); using gesture");
        }
    }

    if !used_scripting_addition {
        if let Err(error) = focus_space_by_gesture(&spaces, active, target) {
            return Some(Err(error));
        }
        eprintln!("yabai-rust: focused space {target} via gesture");
    }

    refresh_all_active_spaces(runtime, display_frames);
    runtime.state.set_active_space(target);
    Some(Ok(None))
}

/// After a scripting-addition space focus, activate the destination display when
/// it differs from the cursor's current display, mirroring the gesture path's
/// `focus_display` branch and the C `display_manager_focus_display`. No-op on a
/// single display (the cursor is already on the only display).
fn activate_space_display_if_cross(target: u64) -> Result<(), String> {
    let Ok(display_id) = display_for_space(target) else {
        return Ok(());
    };
    let cross_display = cursor_display_id().map_or(true, |cursor| cursor != display_id);
    if cross_display {
        warp_cursor_to_display_center(display_id).map_err(|error| error.to_string())?;
        set_active_display(display_id).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Resolve a `space` selector to a concrete space id against the global,
/// mission-control-ordered space list (1-based indices), matching
/// `parse_space_selector`. `recent`/`mouse`/labels and the unsupported direction
/// and stack forms are reported rather than silently ignored.
fn resolve_space_target(
    spaces: &[u64],
    active: Option<u64>,
    selector: &Selector,
) -> Result<u64, String> {
    let relative = |offset: i32| -> Result<u64, String> {
        let active = active.ok_or_else(|| "could not locate the selected space.".to_string())?;
        let index = spaces
            .iter()
            .position(|&s| s == active)
            .ok_or_else(|| "could not locate the selected space.".to_string())?;
        usize::try_from(index as i32 + offset)
            .ok()
            .and_then(|i| spaces.get(i))
            .copied()
            .ok_or_else(|| "could not locate the requested space.".to_string())
    };

    match selector {
        Selector::Index(n) => (*n >= 1)
            .then(|| spaces.get(*n as usize - 1).copied())
            .flatten()
            .ok_or_else(|| format!("could not locate space with mission-control index '{n}'.")),
        Selector::First => spaces
            .first()
            .copied()
            .ok_or_else(|| "could not locate the first space.".to_string()),
        Selector::Last => spaces
            .last()
            .copied()
            .ok_or_else(|| "could not locate the last space.".to_string()),
        Selector::Prev => relative(-1),
        Selector::Next => relative(1),
        _ => Err("space selector not yet supported by the Rust WM daemon.".to_string()),
    }
}

/// Reconcile the managed window set for one app against what AX currently
/// reports, registering newcomers in the sink and dropping windows that vanished
/// (which robustly handles closes despite unreliable AX destroy notifications),
/// then re-flow the active layout. Windows outside the seeded first-display
/// spaces are ignored, which also drops windows moved to untracked displays.
/// Fire every signal subscribed to `event`, running each action as
/// `/usr/bin/env sh -c <action>` with the given `YABAI_*` env vars set, mirroring
/// the C `event_signal_flush` (`fork` + `execvp`). Fire-and-forget: a child is
/// spawned and not awaited, and spawn failures are ignored, like the daemon.
fn fire_signals(
    runtime: &Runtime<AxSink>,
    event: SignalEvent,
    env: &[(&str, String)],
    app: Option<&str>,
    title: Option<&str>,
    active: Option<bool>,
) {
    for action in runtime
        .state
        .signal_actions_for_context(event, app, title, active)
    {
        let mut cmd = std::process::Command::new("/usr/bin/env");
        cmd.arg("sh").arg("-c").arg(&action);
        for (key, value) in env {
            cmd.env(key, value);
        }
        let _ = cmd.spawn();
    }
}

/// If `mouse_follows_focus` is enabled, warp the cursor to the focused window's
/// center, unless it is already inside the window. Mirrors
/// `window_manager_center_mouse`: read the live cursor, skip when contained, and
/// warp to the frame center.
fn center_mouse_on_focus(runtime: &Runtime<AxSink>, window_id: u32) {
    if !runtime.state.config.mouse_follows_focus {
        return;
    }
    let Some(area) = runtime.state.window_area(window_id) else {
        return;
    };
    if let Ok(cursor) = cursor_location() {
        if cursor.x >= area.x
            && cursor.x < area.x + area.w
            && cursor.y >= area.y
            && cursor.y < area.y + area.h
        {
            return;
        }
    }
    let center = Point {
        x: area.x + area.w / 2.0,
        y: area.y + area.h / 2.0,
    };
    let _ = warp_cursor_to_point(center);
}

/// `focus_follows_mouse`: when the cursor moves over a different managed window,
/// focus it — without raising (`autofocus`) or with a raise (`autoraise`),
/// mirroring the C `MOUSE_MOVED` handler. No-op when the mode is off or the cursor
/// is over the already-focused window. The C's occlusion / gesture-debounce /
/// mission-control refinements are not modeled here.
fn handle_mouse_moved(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    last_focus_signal: &mut Option<u32>,
    point: Point,
) {
    let mode = runtime.state.config.focus_follows_mouse;
    if mode == FfmMode::Disabled {
        return;
    }
    runtime.state.set_cursor_point(point);
    let Some(window_id) = runtime.state.managed_window_at_point(point) else {
        return;
    };
    if runtime.state.focused_window_id() == Some(window_id) {
        return;
    }

    let focused = match mode {
        FfmMode::Autoraise => runtime.sink.focus_window(window_id),
        FfmMode::Autofocus => runtime.sink.focus_window_without_raise(window_id),
        FfmMode::Disabled => return,
    };
    if !focused {
        return;
    }
    runtime.state.set_focused_window(Some(window_id));
    if runtime.state.config.enable_window_opacity {
        apply_auto_opacity(sa, runtime, Some(window_id));
    }
    // Note: no `mouse_follows_focus` cursor warp here — the user is moving the
    // mouse, so warping it back would fight them.
    // Fire `window_focused` once per real change, de-duped with the observer /
    // command-focus paths via `last_focus_signal`.
    if *last_focus_signal != Some(window_id) {
        *last_focus_signal = Some(window_id);
        let meta = runtime.state.window_meta(window_id);
        fire_signals(
            runtime,
            SignalEvent::WindowFocused,
            &[("YABAI_WINDOW_ID", window_id.to_string())],
            meta.map(|m| m.app.as_str()),
            meta.map(|m| m.title.as_str()),
            None,
        );
    }
}

/// Map the configured `mouse_modifier` to the compact `MOUSE_MOD_*` mask the drag
/// event tap compares against.
fn mouse_modifier_mask(modifier: MouseModifier) -> u8 {
    match modifier {
        MouseModifier::Alt => MOUSE_MOD_ALT,
        MouseModifier::Shift => MOUSE_MOD_SHIFT,
        MouseModifier::Cmd => MOUSE_MOD_CMD,
        MouseModifier::Ctrl => MOUSE_MOD_CTRL,
        MouseModifier::Fn => MOUSE_MOD_FN,
    }
}

/// State captured while a `mouse_modifier`-armed drag is in progress.
struct DragState {
    window_id: u32,
    action: MouseAction,
    /// The window's frame when the drag began.
    origin: Area,
    /// The cursor point when the drag began.
    down: Point,
    /// The last cursor point handled, for incremental tiled BSP resizing.
    last: Point,
    /// Resize handle selected from the initial cursor quadrant.
    handle: u8,
    /// Whether the dragged window is tiled (managed in a tree) vs floating.
    tiled: bool,
}

fn resize_handle_for_point(frame: Area, point: Point) -> u8 {
    let mid_x = frame.x + frame.w / 2.0;
    let mid_y = frame.y + frame.h / 2.0;
    let mut handle = 0;
    if point.x < mid_x {
        handle |= HANDLE_LEFT;
    }
    if point.x > mid_x {
        handle |= HANDLE_RIGHT;
    }
    if point.y < mid_y {
        handle |= HANDLE_TOP;
    }
    if point.y > mid_y {
        handle |= HANDLE_BOTTOM;
    }
    handle
}

fn resized_frame(origin: Area, handle: u8, dx: f32, dy: f32) -> Area {
    let x_mod = if handle & HANDLE_LEFT != 0 {
        -1.0
    } else if handle & HANDLE_RIGHT != 0 {
        1.0
    } else {
        0.0
    };
    let y_mod = if handle & HANDLE_TOP != 0 {
        -1.0
    } else if handle & HANDLE_BOTTOM != 0 {
        1.0
    } else {
        0.0
    };
    let w = (origin.w + dx * x_mod).max(1.0);
    let h = (origin.h + dy * y_mod).max(1.0);
    let x = if handle & HANDLE_LEFT != 0 {
        origin.x + origin.w - w
    } else {
        origin.x
    };
    let y = if handle & HANDLE_TOP != 0 {
        origin.y + origin.h - h
    } else {
        origin.y
    };
    Area::new(x, y, w, h)
}

/// The window under `point` eligible for a drag-move. Floating windows are checked
/// first (they sit above tiles in z-order), by hit-testing their live AX frames;
/// otherwise the tiled window on the visible space. Returns `(window_id, tiled)`.
fn drag_window_at_point(runtime: &Runtime<AxSink>, point: Point) -> Option<(u32, bool)> {
    let floating = runtime
        .state
        .all_window_ids()
        .into_iter()
        .filter(|&wid| runtime.state.is_floating(wid))
        .find(|&wid| {
            runtime
                .sink
                .window_frame(wid)
                .is_some_and(|f| f.contains_point(point))
        });
    if let Some(wid) = floating {
        return Some((wid, false));
    }
    runtime
        .state
        .managed_window_at_point(point)
        .map(|wid| (wid, true))
}

/// Handle a `mouse_modifier`-armed drag event: `mouse_action1` applies to the left
/// button and `mouse_action2` to the right button. Floating windows keep direct AX
/// move/resize changes; tiled moves attempt a drop action on release (or snap back
/// when there is no target), while tiled resizes update the BSP tree and flush
/// immediately.
fn handle_drag(
    runtime: &mut Runtime<AxSink>,
    drag: &mut Option<DragState>,
    event: MouseDragEvent,
    sa: &ScriptingAddition,
) {
    match event {
        MouseDragEvent::Down(point, button) => {
            let action = match button {
                MouseDragButton::Left => runtime.state.config.mouse_action1,
                MouseDragButton::Right => runtime.state.config.mouse_action2,
            };
            *drag = drag_window_at_point(runtime, point).and_then(|(window_id, tiled)| {
                let origin = runtime.sink.window_frame(window_id)?;
                let handle = resize_handle_for_point(origin, point);
                Some(DragState {
                    window_id,
                    action,
                    origin,
                    down: point,
                    last: point,
                    handle,
                    tiled,
                })
            });
        }
        MouseDragEvent::Dragged(point) => {
            if let Some(state) = drag.as_mut() {
                let dx = point.x - state.down.x;
                let dy = point.y - state.down.y;
                match state.action {
                    MouseAction::Move => {
                        let moved = Area::new(
                            state.origin.x + dx,
                            state.origin.y + dy,
                            state.origin.w,
                            state.origin.h,
                        );
                        runtime.sink.set_frame(state.window_id, moved);
                    }
                    MouseAction::Resize => {
                        if state.tiled {
                            let dx = point.x - state.last.x;
                            let dy = point.y - state.last.y;
                            if runtime.state.resize_tiled_window(
                                state.window_id,
                                state.handle,
                                dx,
                                dy,
                            ) {
                                runtime.state.flush_all_active_to(&mut runtime.sink);
                                state.last = point;
                            }
                        } else {
                            let resized = resized_frame(state.origin, state.handle, dx, dy);
                            runtime.sink.set_frame(state.window_id, resized);
                        }
                    }
                }
            }
        }
        MouseDragEvent::Up(point) => {
            if let Some(state) = drag.take() {
                // A tiled move becomes a drop action when released over another
                // tiled window; otherwise it snaps back. Floating changes and tiled
                // resizes have already been applied.
                if state.tiled && state.action == MouseAction::Move {
                    let result = runtime.state.drop_tiled_window_at_point(
                        state.window_id,
                        point,
                        runtime.state.config.mouse_drop_action,
                    );

                    // Cross-space drops need the scripting addition to actually
                    // relocate the window(s) to the destination space; same-space
                    // drops are enacted by the re-tile below.
                    if let DropResult::CrossSpace {
                        dragged_id,
                        dragged_new_sid,
                        swapped_id,
                        swapped_new_sid,
                    } = result
                    {
                        let _ = sa.move_window_to_space(dragged_new_sid, dragged_id);
                        if let (Some(swapped_id), Some(swapped_new_sid)) =
                            (swapped_id, swapped_new_sid)
                        {
                            let _ = sa.move_window_to_space(swapped_new_sid, swapped_id);
                        }
                    }

                    runtime.state.flush_all_active_to(&mut runtime.sink);
                    if result != DropResult::Ignored {
                        runtime.state.set_focused_window(Some(state.window_id));
                    }
                }
            }
        }
    }
}

/// Apply auto window opacity (`window_opacity on`) across every managed window:
/// `focused` → `active_window_opacity`, the rest → `normal_window_opacity`,
/// mirroring the C `window_manager_set_window_opacity` on focus. When the feature
/// is off, all windows are reset to fully opaque — an intentional divergence from
/// the C, which leaves the last opacity in place (so windows would otherwise stay
/// dimmed after disabling). Applying to all windows (rather than just the old/new
/// pair) keeps it correct without tracking the previously focused window; it is a
/// no-op on the SA side beyond the couple of windows whose opacity actually changes.
fn apply_auto_opacity(sa: &ScriptingAddition, runtime: &Runtime<AxSink>, focused: Option<u32>) {
    let c = &runtime.state.config;
    let dur = c.window_opacity_duration;
    let enabled = c.enable_window_opacity;
    let active = c.active_window_opacity;
    let normal = c.normal_window_opacity;
    for wid in runtime.state.all_window_ids() {
        let opacity = if !enabled {
            1.0
        } else if Some(wid) == focused {
            active
        } else {
            normal
        };
        let _ = sa.set_opacity(wid, opacity, dur);
    }
}

/// True if `tokens` is a `config` command that changes an opacity setting, so the
/// daemon knows to re-apply auto opacity afterwards.
fn is_opacity_config(tokens: &[String]) -> bool {
    matches!(parse_message(tokens), Ok(Message::Config(cmd))
    if cmd.ops.iter().any(|op| matches!(
        op,
        yabai_core::ConfigOp::Set(key, _)
            if matches!(
                key.as_str(),
                "window_opacity" | "active_window_opacity" | "normal_window_opacity"
            )
    )))
}

fn observed_geometry_signal(
    runtime: &Runtime<AxSink>,
    event: &ObservedEvent,
) -> Option<(SignalEvent, u32)> {
    let (signal, window_id) = match event {
        ObservedEvent::WindowMoved {
            window_id: Some(id),
            ..
        } => (SignalEvent::WindowMoved, *id),
        ObservedEvent::WindowResized {
            window_id: Some(id),
            ..
        } => (SignalEvent::WindowResized, *id),
        _ => return None,
    };
    let expected = runtime.state.window_area(window_id)?;
    let actual = runtime.sink.window_frame(window_id)?;
    geometry_signal_frame_changed(signal, expected, actual).then_some((signal, window_id))
}

fn geometry_signal_frame_changed(signal: SignalEvent, expected: Area, actual: Area) -> bool {
    const AX_DIFF_THRESHOLD: f32 = 1.5;
    let changed = |a: f32, b: f32| (a - b).abs() >= AX_DIFF_THRESHOLD;
    match signal {
        SignalEvent::WindowMoved => changed(expected.x, actual.x) || changed(expected.y, actual.y),
        SignalEvent::WindowResized => {
            changed(expected.w, actual.w) || changed(expected.h, actual.h)
        }
        _ => false,
    }
}

/// Apply matching window rules to a window. Currently enacts the `manage` effect
/// (off -> float, on -> tile), scratchpad assignment, and SA-backed sticky,
/// sub-layer, opacity, display/space movement, plus AX-backed grid placement.
/// Other effects (fullscreen) are parsed and stored but their application is
/// deferred. Role/subrole are unknown at the AX layer here, so rules filtering on
/// them will not match yet.
fn apply_window_rules(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    display_frames: &[(u32, Area)],
    window_id: u32,
    app: &str,
    title: &str,
    sid: u64,
) {
    let effects = runtime
        .state
        .apply_new_window_rules(window_id, app, title, "", "", sid);
    let application = AppliedRuleEffects {
        window_id,
        sid,
        effects,
    };
    apply_rule_effects_at_boundary(sa, runtime, display_frames, &[application]);
}

fn sync_window_lifecycle_signals(
    runtime: &Runtime<AxSink>,
    signaled: &mut HashMap<i32, HashMap<u32, WindowMeta>>,
    pid: i32,
) {
    let Ok(infos) = pid_window_infos(pid) else {
        return;
    };

    let known = signaled.entry(pid).or_default();
    let mut current = HashMap::with_capacity(infos.len());
    for info in infos {
        let id = info.id;
        let meta = WindowMeta {
            app: info.app,
            title: info.title,
            pid: info.pid,
        };
        if !known.contains_key(&id) {
            fire_signals(
                runtime,
                SignalEvent::WindowCreated,
                &[("YABAI_WINDOW_ID", id.to_string())],
                Some(meta.app.as_str()),
                Some(meta.title.as_str()),
                None,
            );
        } else if known.get(&id).is_some_and(|old| old.title != meta.title) {
            let active = runtime.state.focused_window_id() == Some(id);
            fire_signals(
                runtime,
                SignalEvent::WindowTitleChanged,
                &[("YABAI_WINDOW_ID", id.to_string())],
                Some(meta.app.as_str()),
                Some(meta.title.as_str()),
                Some(active),
            );
        }
        current.insert(id, meta);
    }

    for (id, meta) in known.iter() {
        if !current.contains_key(id) {
            let active = runtime.state.focused_window_id() == Some(*id);
            fire_signals(
                runtime,
                SignalEvent::WindowDestroyed,
                &[("YABAI_WINDOW_ID", id.to_string())],
                Some(meta.app.as_str()),
                None,
                Some(active),
            );
        }
    }
    *known = current;
}

fn reconcile_pid(
    sa: &ScriptingAddition,
    runtime: &mut Runtime<AxSink>,
    managed: &mut HashMap<i32, HashSet<u32>>,
    signaled: &mut HashMap<i32, HashMap<u32, WindowMeta>>,
    display_frames: &[(u32, Area)],
    pid: i32,
) {
    sync_window_lifecycle_signals(runtime, signaled, pid);

    let Ok(discovered) = tileable_pid_windows(pid) else {
        return;
    };
    let known = managed.entry(pid).or_default();
    let mut current = HashSet::with_capacity(discovered.len());

    for window in discovered {
        let id = window.id;
        let Some(sid) = managed_space_for_window(&runtime.state, id) else {
            continue;
        };
        current.insert(id);
        let is_new = !known.contains(&id);
        let (app, title) = (window.app.clone(), window.title.clone());
        // Refresh metadata every pass so titles stay current.
        runtime.state.set_window_meta(
            id,
            WindowMeta {
                app: window.app,
                title: window.title,
                pid: window.pid,
            },
        );
        if is_new {
            // A genuinely new window: hand its element to the sink and tree.
            runtime.sink.register(id, window.window);
        }
        let _ = runtime
            .state
            .handle_event(StateEvent::WindowAssignedToSpace { window_id: id, sid });
        // Apply window rules once, when the window is first seen.
        if is_new {
            apply_window_rules(sa, runtime, display_frames, id, &app, &title, sid);
        }
        // Else it is already managed; the freshly discovered duplicate element
        // drops here, leaving the existing registration intact.
    }

    for id in known.difference(&current).copied().collect::<Vec<_>>() {
        runtime.sink.unregister(id);
        runtime.state.remove_window_meta(id);
        let _ = runtime
            .state
            .handle_event(StateEvent::WindowDestroyed { window_id: id });
    }
    *known = current;

    runtime.state.flush_all_active_to(&mut runtime.sink);
}

fn drop_pid(
    runtime: &mut Runtime<AxSink>,
    managed: &mut HashMap<i32, HashSet<u32>>,
    signaled: &mut HashMap<i32, HashMap<u32, WindowMeta>>,
    pid: i32,
) {
    if let Some(infos) = signaled.remove(&pid) {
        for (id, meta) in infos {
            let active = runtime.state.focused_window_id() == Some(id);
            fire_signals(
                runtime,
                SignalEvent::WindowDestroyed,
                &[("YABAI_WINDOW_ID", id.to_string())],
                Some(meta.app.as_str()),
                None,
                Some(active),
            );
        }
    }

    let Some(ids) = managed.remove(&pid) else {
        return;
    };
    for id in ids {
        runtime.sink.unregister(id);
        runtime.state.remove_window_meta(id);
        let _ = runtime
            .state
            .handle_event(StateEvent::WindowDestroyed { window_id: id });
    }
    runtime.state.flush_all_active_to(&mut runtime.sink);
}

/// Read one framed `-m` request off a socket, route it through the WM event loop,
/// and write the response back (same wire contract as `serve_one`).
fn serve_via_channel(mut stream: UnixStream, tx: &Sender<WmWork>) {
    let mut header = [0u8; size_of::<i32>()];
    let response = match stream.read_exact(&mut header) {
        Ok(()) => {
            let size = i32::from_ne_bytes(header);
            if size < 0 {
                Err("negative IPC payload size".to_string())
            } else {
                let mut payload = vec![0; size as usize];
                match stream.read_exact(&mut payload) {
                    Ok(()) => match decode_client_payload(&payload) {
                        Some(tokens) => {
                            let tokens = tokens.into_iter().map(str::to_owned).collect::<Vec<_>>();
                            let (reply, rx) = sync_channel(0);
                            match tx.send(WmWork::Message { tokens, reply }) {
                                Ok(()) => rx
                                    .recv()
                                    .unwrap_or_else(|_| Err("event loop is gone".to_string())),
                                Err(_) => Err("event loop is gone".to_string()),
                            }
                        }
                        None => Err("invalid IPC payload".to_string()),
                    },
                    Err(error) => Err(format!("failed to read IPC payload: {error}")),
                }
            }
        }
        Err(error) => Err(format!("failed to read IPC header: {error}")),
    };

    match response {
        Ok(Some(output)) => {
            let _ = stream.write_all(output.as_bytes());
        }
        Ok(None) => {}
        Err(error) => {
            let _ = stream.write_all(&[FAILURE_MARKER]);
            let _ = writeln!(stream, "{error}");
        }
    }
}

/// A dynamic Rust tiling WM: it tiles an app (or `all` regular apps) and then
/// *stays in sync* with the world via AX observers — new windows tile in, closed
/// windows are reconciled out — while serving live `-m` commands on the socket.
///
/// CRITICAL: binds only the caller-provided socket (never `/tmp/yabai_$USER`).
fn run_rust_wm_daemon(args: &[String]) -> ExitCode {
    let Some(socket_path) = args.first() else {
        eprintln!(
            "yabai-rust: --experimental-rust-wm-daemon requires <socket> <pid|all> [gap] [padding]"
        );
        return ExitCode::from(64);
    };
    let Some(target) = args.get(1) else {
        eprintln!("yabai-rust: --experimental-rust-wm-daemon requires a pid or 'all'");
        return ExitCode::from(64);
    };
    let gap: i32 = args.get(2).and_then(|arg| arg.parse().ok()).unwrap_or(12);
    let padding: i32 = args.get(3).and_then(|arg| arg.parse().ok()).unwrap_or(gap);

    if !accessibility_trusted_with_prompt() {
        eprintln!("yabai-rust: Accessibility permission is not granted; grant it and rerun");
        return ExitCode::from(1);
    }

    // Initialize AppKit so this non-bundled tool actually receives NSWorkspace
    // notifications (application launch/terminate/activate/hide, active space).
    // Mirrors `NSApplicationLoad()` in the C daemon's `main`. Must run on the main
    // thread before the workspace observer spawns.
    ns_application_load();

    let is_all = target == "all";
    let pids: Vec<i32> = if is_all {
        // CGWindowList reflects current on-screen windows and refreshes live.
        application_pids_with_windows()
    } else if let Ok(pid) = target.parse::<i32>() {
        vec![pid]
    } else {
        eprintln!("yabai-rust: tile target must be a pid or 'all', got '{target}'");
        return ExitCode::from(64);
    };

    let listener = match bind_experimental_daemon(socket_path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("yabai-rust: failed to bind daemon socket at {socket_path}: {error}");
            return ExitCode::from(1);
        }
    };

    let displays = match active_displays() {
        Ok(displays) if !displays.is_empty() => displays,
        Ok(_) => {
            eprintln!("yabai-rust: no active displays found");
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!("yabai-rust: failed to discover displays: {error}");
            return ExitCode::from(1);
        }
    };
    // Each display tiles inside its own visible frame (per-display menu bar/Dock
    // insets); fall back to the full bounds if NSScreen can't resolve it.
    let mut display_frames: Vec<(u32, Area)> = displays
        .iter()
        .map(|display| {
            (
                display.id,
                visible_frame_for_display(display.id).unwrap_or(display.frame),
            )
        })
        .collect();

    let mut state = AppState::new();
    for display in &displays {
        state.add_display(display.id, display.frame);
    }
    // Seed every display's spaces, then apply config, then inset by padding —
    // the same order the single-display path used.
    let mut seeded: Vec<(u64, Area)> = Vec::new();
    for (display_id, usable) in &display_frames {
        let mut spaces = spaces_for_display(*display_id).unwrap_or_default();
        let current = current_space_for_display(*display_id).ok();
        if let Some(current) = current {
            if !spaces.contains(&current) {
                spaces.push(current);
            }
        }
        for sid in &spaces {
            state.add_space_to_display(*sid, *display_id, *usable);
            seeded.push((*sid, *usable));
        }
        if let Some(current) = current {
            state.set_display_active_space(*display_id, current);
        }
    }
    let _ = state.handle_tokens(&tile_config_tokens(gap, padding));
    for (sid, usable) in &seeded {
        if let Err(error) = state.set_space_frame(*sid, *usable) {
            eprintln!("yabai-rust: failed to apply padding to space {sid}: {error}");
            return ExitCode::from(1);
        }
    }
    // Command dispatch targets the focused display's space. Prefer the display
    // containing the cursor, then fall back to the first display if the cursor
    // cannot be resolved (e.g. a non-GUI SSH session before wake).
    let initial_active = cursor_display_id()
        .ok()
        .and_then(|display_id| state.display_active_space_id(display_id))
        .or_else(|| {
            display_frames
                .first()
                .and_then(|(display_id, _)| state.display_active_space_id(*display_id))
        });
    if let Some(sid) = initial_active {
        state.set_active_space(sid);
    }
    let space_count = seeded.len();
    let active_sid = state.active_space_id();

    let mut runtime = Runtime::new(state, AxSink::new());
    let mut managed: HashMap<i32, HashSet<u32>> = HashMap::new();
    let mut signaled: HashMap<i32, HashMap<u32, WindowMeta>> = HashMap::new();
    let mut minimized_pids: HashMap<u32, i32> = HashMap::new();
    let mut fullscreen_pids: HashMap<u32, i32> = HashMap::new();
    // Windows currently in `--toggle windowed-fullscreen`, mapped to the frame
    // saved when they entered it (restored on toggle-off). Presence in this map
    // is the Rust analogue of the C `WINDOW_WINDOWED` flag + `windowed_frame`.
    let mut windowed_frames: HashMap<u32, Area> = HashMap::new();

    // Scripting-addition client for privileged ops the AX API cannot do (space
    // create/destroy/move, rule opacity/layer/sticky effects, etc.). Built from
    // the daemon's real login user, not the per-command USER override the IPC
    // client uses. Calls fail gracefully when the SA payload is not loaded.
    let scripting_addition =
        ScriptingAddition::for_user(&std::env::var("USER").unwrap_or_default());
    match scripting_addition.status() {
        ScriptingAdditionStatus::Healthy { payload_version } => eprintln!(
            "yabai-rust: scripting addition available (payload v{payload_version}) — space create/destroy enabled"
        ),
        other => eprintln!(
            "yabai-rust: scripting addition not usable ({other:?}); space create/destroy will fail until it is loaded"
        ),
    }

    // Initial tile from the current world.
    for pid in &pids {
        reconcile_pid(
            &scripting_addition,
            &mut runtime,
            &mut managed,
            &mut signaled,
            &display_frames,
            *pid,
        );
    }
    let initial: usize = managed.values().map(HashSet::len).sum();

    // Most-recently signaled focused window, so `window_focused` fires once per
    // real focus change whether the change came from a command or an AX observer.
    let mut last_focus_signal: Option<u32> = None;
    // In-progress `mouse_modifier`-armed drag (drag-to-move), if any.
    let mut drag_state: Option<DragState> = None;
    // The front (active) app pid, tracked from NSWorkspace activate notifications.
    // Used as the `active` context for application_hidden/terminated signals and
    // as `YABAI_RECENT_PROCESS_ID` for application_front_switched.
    let mut front_pid: Option<i32> = None;

    // Unified event loop: observers, the periodic tick, and the socket all feed
    // one channel processed against the single `Runtime<AxSink>`.
    let (tx, rx) = channel::<WmWork>();

    // One AX observer per app on its own run-loop thread.
    let mut observed: HashSet<i32> = HashSet::new();
    for pid in &pids {
        observed.insert(*pid);
        spawn_observer(*pid, &tx);
    }
    let workspace_tx = start_workspace_bridge(&tx);

    // Periodic self-heal tick (also picks up newly launched apps in `all` mode).
    {
        let tx = tx.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(3));
                if tx.send(WmWork::Tick).is_err() {
                    break;
                }
            }
        });
    }

    // Socket acceptor thread.
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => serve_via_channel(stream, &tx),
                    Err(error) => eprintln!("yabai-rust: failed to accept client: {error}"),
                }
            }
        });
    }

    // Mouse-moved event tap for `focus_follows_mouse`. The tap always runs (it is
    // cheap and listen-only); the handler no-ops unless the mode is enabled. A
    // dedicated thread pumps the tap's run loop and forwards points as `WmWork`.
    {
        let (mtx, mrx) = channel::<Point>();
        thread::spawn(move || {
            if let Err(error) = observe_mouse_moved(mtx) {
                eprintln!("yabai-rust: focus_follows_mouse tap unavailable: {error}");
            }
        });
        let tx = tx.clone();
        thread::spawn(move || {
            for point in mrx {
                if tx.send(WmWork::MouseMoved(point)).is_err() {
                    break;
                }
            }
        });
    }

    // Active mouse-drag tap for `mouse_modifier` + drag to move. It consumes the
    // click only while the armed modifier is held. Arm it from the current config.
    set_drag_modifier(mouse_modifier_mask(runtime.state.config.mouse_modifier));
    {
        let (dtx, drx) = channel::<MouseDragEvent>();
        thread::spawn(move || {
            if let Err(error) = observe_mouse_drag(dtx) {
                eprintln!("yabai-rust: mouse-drag tap unavailable: {error}");
            }
        });
        let tx = tx.clone();
        thread::spawn(move || {
            for event in drx {
                if tx.send(WmWork::Drag(event)).is_err() {
                    break;
                }
            }
        });
    }

    // The main thread keeps `tx` alive, so the loop runs until the process dies.
    eprintln!(
        "yabai-rust: WM daemon up on {socket_path} — target {target}, {} app(s), {initial} window(s), {} display(s), active space {active_sid:?}, {space_count} discovered space(s), gap {gap}, padding {padding}",
        pids.len(),
        display_frames.len()
    );
    eprintln!(
        "yabai-rust: tracking live window changes; send commands with a matching USER (e.g. USER=<name> yabai -m space --rotate 90)"
    );

    let worker = thread::spawn(move || {
        for work in rx {
            match work {
                WmWork::Observed(event) => {
                    let pid = event.pid();
                    let focused = match &event {
                        ObservedEvent::FocusedWindowChanged {
                            window_id: Some(id),
                            ..
                        } => Some(*id),
                        _ => None,
                    };
                    refresh_live_display_state(&mut runtime, &mut display_frames);
                    if let Some((signal, window_id)) = observed_geometry_signal(&runtime, &event) {
                        let active = runtime.state.focused_window_id() == Some(window_id);
                        let meta = runtime.state.window_meta(window_id);
                        fire_signals(
                            &runtime,
                            signal,
                            &[("YABAI_WINDOW_ID", window_id.to_string())],
                            meta.map(|m| m.app.as_str()),
                            meta.map(|m| m.title.as_str()),
                            Some(active),
                        );
                    }
                    reconcile_pid(
                        &scripting_addition,
                        &mut runtime,
                        &mut managed,
                        &mut signaled,
                        &display_frames,
                        pid,
                    );
                    // Focus may have moved to a window on another display; point the
                    // command-active space at the focused window's space.
                    if let Some(window_id) = focused {
                        if let Some(sid) = runtime.state.window_space_id(window_id) {
                            runtime.state.set_active_space(sid);
                            let _ = runtime
                                .state
                                .handle_event(StateEvent::WindowFocused { window_id });
                        }
                        center_mouse_on_focus(&runtime, window_id);
                        // `window_focused` signal (observer-driven focus, e.g. a
                        // click). De-duplicated against the command path below.
                        if last_focus_signal != Some(window_id) {
                            if runtime.state.config.enable_window_opacity {
                                apply_auto_opacity(&scripting_addition, &runtime, Some(window_id));
                            }
                            last_focus_signal = Some(window_id);
                            let meta = runtime.state.window_meta(window_id);
                            fire_signals(
                                &runtime,
                                SignalEvent::WindowFocused,
                                &[("YABAI_WINDOW_ID", window_id.to_string())],
                                meta.map(|m| m.app.as_str()),
                                meta.map(|m| m.title.as_str()),
                                None,
                            );
                        }
                    }
                }
                WmWork::Workspace(event) => match event {
                    WorkspaceEvent::ActiveSpaceChanged => {
                        refresh_live_display_state(&mut runtime, &mut display_frames);
                        for pid in observed.iter().copied().collect::<Vec<_>>() {
                            reconcile_pid(
                                &scripting_addition,
                                &mut runtime,
                                &mut managed,
                                &mut signaled,
                                &display_frames,
                                pid,
                            );
                        }
                        if let Some(sid) = runtime.state.active_space_id() {
                            fire_signals(
                                &runtime,
                                SignalEvent::SpaceChanged,
                                &[("YABAI_SPACE_ID", sid.to_string())],
                                None,
                                None,
                                None,
                            );
                        }
                    }
                    WorkspaceEvent::ApplicationLaunched { pid, app } => {
                        // Fire the signal regardless of tiling mode; it is about the
                        // event, not whether this daemon manages the app.
                        fire_signals(
                            &runtime,
                            SignalEvent::ApplicationLaunched,
                            &[("YABAI_PROCESS_ID", pid.to_string())],
                            Some(&app),
                            None,
                            None,
                        );
                        if is_all && observed.insert(pid) {
                            spawn_observer(pid, &tx);
                            refresh_live_display_state(&mut runtime, &mut display_frames);
                            reconcile_pid(
                                &scripting_addition,
                                &mut runtime,
                                &mut managed,
                                &mut signaled,
                                &display_frames,
                                pid,
                            );
                        }
                    }
                    WorkspaceEvent::ApplicationActivated { pid, app } => {
                        // The frontmost app changed: fire front_switched with the
                        // new and previous front pids, mirroring the C process
                        // manager (YABAI_PROCESS_ID = front, YABAI_RECENT_PROCESS_ID
                        // = last front). front_switched is unfiltered in the C
                        // event filter.
                        if front_pid != Some(pid) {
                            let recent = front_pid.unwrap_or(pid);
                            fire_signals(
                                &runtime,
                                SignalEvent::ApplicationFrontSwitched,
                                &[
                                    ("YABAI_PROCESS_ID", pid.to_string()),
                                    ("YABAI_RECENT_PROCESS_ID", recent.to_string()),
                                ],
                                None,
                                None,
                                None,
                            );
                        }
                        front_pid = Some(pid);
                        fire_signals(
                            &runtime,
                            SignalEvent::ApplicationActivated,
                            &[("YABAI_PROCESS_ID", pid.to_string())],
                            Some(&app),
                            None,
                            None,
                        );
                    }
                    WorkspaceEvent::ApplicationDeactivated { pid, app } => {
                        fire_signals(
                            &runtime,
                            SignalEvent::ApplicationDeactivated,
                            &[("YABAI_PROCESS_ID", pid.to_string())],
                            Some(&app),
                            None,
                            None,
                        );
                    }
                    WorkspaceEvent::ApplicationVisible { pid, app } => {
                        fire_signals(
                            &runtime,
                            SignalEvent::ApplicationVisible,
                            &[("YABAI_PROCESS_ID", pid.to_string())],
                            Some(&app),
                            None,
                            None,
                        );
                    }
                    WorkspaceEvent::ApplicationHidden { pid, app } => {
                        // The hidden category filters on app + active (front app), per
                        // `event_signal.c`.
                        let active = front_pid == Some(pid);
                        fire_signals(
                            &runtime,
                            SignalEvent::ApplicationHidden,
                            &[("YABAI_PROCESS_ID", pid.to_string())],
                            Some(&app),
                            None,
                            Some(active),
                        );
                    }
                    WorkspaceEvent::ApplicationTerminated { pid, app } => {
                        fire_signals(
                            &runtime,
                            SignalEvent::ApplicationTerminated,
                            &[("YABAI_PROCESS_ID", pid.to_string())],
                            Some(&app),
                            None,
                            Some(front_pid == Some(pid)),
                        );
                        if observed.remove(&pid) {
                            for window_id in minimized_pids
                                .iter()
                                .filter_map(|(&window_id, &window_pid)| {
                                    (window_pid == pid).then_some(window_id)
                                })
                                .collect::<Vec<_>>()
                            {
                                minimized_pids.remove(&window_id);
                                runtime.sink.unregister_minimized(window_id);
                            }
                            for window_id in fullscreen_pids
                                .iter()
                                .filter_map(|(&window_id, &window_pid)| {
                                    (window_pid == pid).then_some(window_id)
                                })
                                .collect::<Vec<_>>()
                            {
                                fullscreen_pids.remove(&window_id);
                                runtime.sink.unregister_fullscreen(window_id);
                            }
                            drop_pid(&mut runtime, &mut managed, &mut signaled, pid);
                        }
                    }
                    // Context-free, always-fire signals (no YABAI_* env vars, no
                    // app/title/active filtering — the C event filter never rejects
                    // these). Mirrors the C `workspace_context` notification set.
                    WorkspaceEvent::DisplayChanged => {
                        refresh_live_display_state(&mut runtime, &mut display_frames);
                        fire_signals(&runtime, SignalEvent::DisplayChanged, &[], None, None, None);
                    }
                    WorkspaceEvent::DisplayAdded(did) => {
                        refresh_live_display_state(&mut runtime, &mut display_frames);
                        fire_signals(
                            &runtime,
                            SignalEvent::DisplayAdded,
                            &[("YABAI_DISPLAY_ID", did.to_string())],
                            None,
                            None,
                            None,
                        );
                    }
                    WorkspaceEvent::DisplayRemoved(did) => {
                        refresh_live_display_state(&mut runtime, &mut display_frames);
                        fire_signals(
                            &runtime,
                            SignalEvent::DisplayRemoved,
                            &[("YABAI_DISPLAY_ID", did.to_string())],
                            None,
                            None,
                            None,
                        );
                    }
                    WorkspaceEvent::DisplayMoved(did) => {
                        refresh_live_display_state(&mut runtime, &mut display_frames);
                        fire_signals(
                            &runtime,
                            SignalEvent::DisplayMoved,
                            &[("YABAI_DISPLAY_ID", did.to_string())],
                            None,
                            None,
                            None,
                        );
                    }
                    WorkspaceEvent::DisplayResized(did) => {
                        refresh_live_display_state(&mut runtime, &mut display_frames);
                        fire_signals(
                            &runtime,
                            SignalEvent::DisplayResized,
                            &[("YABAI_DISPLAY_ID", did.to_string())],
                            None,
                            None,
                            None,
                        );
                    }
                    WorkspaceEvent::SystemWoke => {
                        fire_signals(&runtime, SignalEvent::SystemWoke, &[], None, None, None);
                    }
                    WorkspaceEvent::DockDidRestart => {
                        fire_signals(&runtime, SignalEvent::DockDidRestart, &[], None, None, None);
                    }
                    WorkspaceEvent::DockDidChangePref => {
                        fire_signals(
                            &runtime,
                            SignalEvent::DockDidChangePref,
                            &[],
                            None,
                            None,
                            None,
                        );
                    }
                    WorkspaceEvent::MenuBarHiddenChanged => {
                        fire_signals(
                            &runtime,
                            SignalEvent::MenuBarHiddenChanged,
                            &[],
                            None,
                            None,
                            None,
                        );
                    }
                },
                WmWork::MouseMoved(point) => {
                    handle_mouse_moved(
                        &scripting_addition,
                        &mut runtime,
                        &mut last_focus_signal,
                        point,
                    );
                }
                WmWork::Drag(event) => {
                    handle_drag(&mut runtime, &mut drag_state, event, &scripting_addition);
                }
                WmWork::Tick => {
                    refresh_live_display_state(&mut runtime, &mut display_frames);
                    // In `all` mode, pick up apps launched after startup via the live
                    // CGWindowList scan, and start observing each.
                    if is_all {
                        for pid in application_pids_with_windows() {
                            if observed.insert(pid) {
                                spawn_observer(pid, &tx);
                            }
                        }
                    }
                    // Self-heal: re-reconcile every known app, catching any window
                    // change an observer missed (e.g. the unreliable AX destroy).
                    for pid in observed.iter().copied().collect::<Vec<_>>() {
                        reconcile_pid(
                            &scripting_addition,
                            &mut runtime,
                            &mut managed,
                            &mut signaled,
                            &display_frames,
                            pid,
                        );
                    }
                }
                WmWork::Message { tokens, reply } => {
                    refresh_live_display_state(&mut runtime, &mut display_frames);
                    // Record the live cursor so the `mouse` selector can resolve
                    // the window/space/display under the pointer.
                    if let Ok(cursor) = cursor_location() {
                        runtime.state.set_cursor_point(cursor);
                    }
                    // Some commands need macOS-layer state/effects the pure core can't
                    // perform; handle those here, otherwise fall through.
                    let fullscreen_exit = try_window_native_fullscreen_exit(
                        &scripting_addition,
                        &mut runtime,
                        &mut managed,
                        &mut signaled,
                        &display_frames,
                        &mut fullscreen_pids,
                        &tokens,
                    );
                    let was_fullscreen_exit = fullscreen_exit.is_some();
                    let response = match fullscreen_exit {
                        Some(response) => response,
                        None => match try_window_deminimize(
                            &scripting_addition,
                            &mut runtime,
                            &mut managed,
                            &mut signaled,
                            &display_frames,
                            &mut minimized_pids,
                            &tokens,
                        ) {
                            Some(response) => response,
                            None => match try_scripting_addition(
                                &scripting_addition,
                                &mut runtime,
                                &mut display_frames,
                                &tokens,
                            ) {
                                Some(response) => response,
                                None => match try_window_grid(&runtime, &display_frames, &tokens) {
                                    Some(response) => response,
                                    None => match try_window_move(&runtime, &tokens) {
                                        Some(response) => response,
                                        None => match try_window_resize(&runtime, &tokens) {
                                            Some(response) => response,
                                            None => match try_window_windowed_fullscreen(
                                                &runtime,
                                                &display_frames,
                                                &mut windowed_frames,
                                                &tokens,
                                            ) {
                                                Some(response) => response,
                                                None => {
                                                    match try_window_expose(&runtime, &tokens) {
                                                        Some(response) => response,
                                                        None => match try_space_focus(
                                                            &scripting_addition,
                                                            &mut runtime,
                                                            &display_frames,
                                                            &tokens,
                                                        ) {
                                                            Some(response) => response,
                                                            None => match try_display(
                                                                &scripting_addition,
                                                                &mut runtime,
                                                                &display_frames,
                                                                &tokens,
                                                            ) {
                                                                Some(response) => response,
                                                                None => runtime.message(&tokens),
                                                            },
                                                        },
                                                    }
                                                }
                                            },
                                        },
                                    },
                                },
                            },
                        },
                    };
                    // A successful `window --focus` updated the pure focus target;
                    // now enact it on the real window (raise + make key).
                    if response.is_ok() && is_window_focus(&tokens) {
                        if let Some(window_id) = runtime.state.focused_window_id() {
                            runtime.sink.focus_window(window_id);
                            center_mouse_on_focus(&runtime, window_id);
                            // Fire `window_focused` here too: command-driven focus does
                            // not always produce an AX observer notification. De-dup
                            // with the observer path via `last_focus_signal`.
                            if last_focus_signal != Some(window_id) {
                                if runtime.state.config.enable_window_opacity {
                                    apply_auto_opacity(
                                        &scripting_addition,
                                        &runtime,
                                        Some(window_id),
                                    );
                                }
                                last_focus_signal = Some(window_id);
                                let meta = runtime.state.window_meta(window_id);
                                fire_signals(
                                    &runtime,
                                    SignalEvent::WindowFocused,
                                    &[("YABAI_WINDOW_ID", window_id.to_string())],
                                    meta.map(|m| m.app.as_str()),
                                    meta.map(|m| m.title.as_str()),
                                    None,
                                );
                            }
                        }
                    }
                    // `window --minimize`: AX-minimize the focused window, then
                    // reconcile its app so the now-untileable window leaves the tree
                    // and the rest re-tile.
                    if response.is_ok() && is_window_minimize(&tokens) {
                        if let Some(window_id) = runtime.state.focused_window_id() {
                            let pid = runtime.state.window_pid(window_id);
                            let active = runtime.state.focused_window_id() == Some(window_id);
                            let meta = runtime.state.window_meta(window_id).cloned();
                            if runtime.sink.set_minimized(window_id, true) {
                                if let Some(pid) = pid {
                                    minimized_pids.insert(window_id, pid);
                                    reconcile_pid(
                                        &scripting_addition,
                                        &mut runtime,
                                        &mut managed,
                                        &mut signaled,
                                        &display_frames,
                                        pid,
                                    );
                                }
                                fire_signals(
                                    &runtime,
                                    SignalEvent::WindowMinimized,
                                    &[("YABAI_WINDOW_ID", window_id.to_string())],
                                    meta.as_ref().map(|m| m.app.as_str()),
                                    meta.as_ref().map(|m| m.title.as_str()),
                                    Some(active),
                                );
                            }
                        }
                    }
                    // `window --close`: press the AX close button for the acting
                    // window. AX destroy is unreliable, so reconcile immediately and
                    // let the 3s tick catch any delayed close.
                    if response.is_ok() && is_window_close(&tokens) {
                        if let Some(window_id) = runtime.state.focused_window_id() {
                            let pid = runtime.state.window_pid(window_id);
                            if runtime.sink.close_window(window_id) {
                                if let Some(pid) = pid {
                                    reconcile_pid(
                                        &scripting_addition,
                                        &mut runtime,
                                        &mut managed,
                                        &mut signaled,
                                        &display_frames,
                                        pid,
                                    );
                                }
                            }
                        }
                    }
                    // `window --toggle native-fullscreen` (enter half): focus the
                    // window (the AX attribute is only honored on the key window, per
                    // the C daemon), set AXFullscreen, then reconcile so the window —
                    // now on its own fullscreen space — leaves the tiled layout. The
                    // exit half is handled by the intercept above.
                    if !was_fullscreen_exit
                        && response.is_ok()
                        && is_window_native_fullscreen(&tokens)
                    {
                        if let Some(window_id) = runtime.state.focused_window_id() {
                            let pid = runtime.state.window_pid(window_id);
                            runtime.sink.focus_window(window_id);
                            if runtime.sink.enter_native_fullscreen(window_id) {
                                if let Some(pid) = pid {
                                    fullscreen_pids.insert(window_id, pid);
                                    reconcile_pid(
                                        &scripting_addition,
                                        &mut runtime,
                                        &mut managed,
                                        &mut signaled,
                                        &display_frames,
                                        pid,
                                    );
                                }
                            }
                        }
                    }
                    // A `window_opacity` / `active`/`normal_window_opacity` change
                    // re-applies auto opacity across all managed windows.
                    if response.is_ok() && is_opacity_config(&tokens) {
                        let focused = runtime.state.focused_window_id();
                        apply_auto_opacity(&scripting_addition, &runtime, focused);
                    }
                    // Keep the drag tap's armed modifier in sync with `mouse_modifier`.
                    if response.is_ok() {
                        set_drag_modifier(mouse_modifier_mask(runtime.state.config.mouse_modifier));
                    }
                    let _ = reply.send(response);
                }
            }
        }
    });

    // Run the NSWorkspace observer on the main thread. It blocks in `[NSApp run]`,
    // the only run loop that services NSWorkspace notifications (application
    // launch/terminate/activate/deactivate/hide/unhide and active-space changes)
    // and the CGDisplayReconfiguration callback registered just below. This mirrors
    // `[NSApp run]` on the C daemon's main thread while its event loop runs on a
    // worker pthread.
    ns_application_load();
    // Register the display add/remove/move/resize callback before entering the run
    // loop that delivers it.
    observe_display_reconfiguration().unwrap();
    let _ = observe_workspace(workspace_tx);
    let _ = worker.join();
    ExitCode::SUCCESS
}

/// Diagnostic: print AX window lifecycle events for an app as they happen.
/// Proves the observer/run-loop callback path on a live app before the daemon
/// consumes these events. Runs until interrupted (Ctrl-C).
fn run_ax_observe_pid(args: &[String]) -> ExitCode {
    if !accessibility_trusted_with_prompt() {
        eprintln!("yabai-rust: Accessibility permission is not granted; grant it and rerun");
        return ExitCode::from(1);
    }
    let Some(pid) = args.first().and_then(|arg| arg.parse::<i32>().ok()) else {
        eprintln!("yabai-rust: --experimental-ax-observe-pid requires a pid");
        return ExitCode::from(64);
    };

    let (tx, rx) = std::sync::mpsc::channel();
    // The run loop must own a thread; print events from the main thread.
    let observer = std::thread::spawn(move || observe_pid(pid, tx));
    eprintln!("yabai-rust: observing pid {pid} — open/close/focus its windows (Ctrl-C to stop)");
    for event in rx {
        println!("{event:?}");
    }
    // The channel only closes if the observer thread returned (setup failure).
    match observer.join() {
        Ok(Err(error)) => {
            eprintln!("yabai-rust: observer stopped: {error}");
            ExitCode::from(1)
        }
        _ => ExitCode::SUCCESS,
    }
}

fn run_ax_debug_probe() -> ExitCode {
    if !accessibility_trusted_with_prompt() {
        eprintln!("yabai-rust: Accessibility permission is not granted; grant it and rerun");
        return ExitCode::from(1);
    }

    let diag = focused_window_diagnostics();
    println!("trusted={}", diag.trusted);
    println!(
        "system_focused_window_id={:?}",
        diag.system_focused_window_id
    );
    println!("focused_app_pid={:?}", diag.focused_app_pid);
    println!("focused_app_window_id={:?}", diag.focused_app_window_id);
    println!(
        "focused_app_window_count={:?}",
        diag.focused_app_window_count
    );
    println!("focused_app_window_ids={:?}", diag.focused_app_window_ids);
    ExitCode::SUCCESS
}

fn print_help() {
    println!(
        "Usage: yabai-rust [option]\n\
         Options:\n\
             --message, -m <msg>    Send message to a running yabai instance.\n\
             --experimental-rust-daemon <socket>\n\
                                     Run dry-run Rust daemon on an explicit socket.\n\
             --experimental-ax-focused-window\n\
                                     Print the focused AX window's CG window id.\n\
             --experimental-ax-debug\n\
                                     Print AX focused-window diagnostics.\n\
             --experimental-ax-windows-for-pid <pid>\n\
                                     Print CG ids for an app's AX windows.\n\
             --experimental-ax-pid-debug <pid>\n\
                                     Print AX diagnostics for an app pid.\n\
             --experimental-ax-move-focused <x> <y> <w> <h>\n\
                                     Move/resize the focused AX window directly.\n\
             --experimental-ax-move-pid <pid> <index> <x> <y> <w> <h>\n\
                                     Move/resize an app's index-th AX window.\n\
             --experimental-ax-tile-pid <pid> [gap] [padding]\n\
                                     BSP-tile an app's windows via the Rust core.\n\
             --experimental-rust-tile-daemon <socket> <pid|all> [gap] [padding]\n\
                                     Persistent tiling daemon (serves -m commands).\n\
             --experimental-ax-observe-pid <pid>\n\
                                     Print live AX window lifecycle events.\n\
             --experimental-rust-wm-daemon <socket> <pid|all> [gap] [padding]\n\
                                     Dynamic tiling WM: tracks live window changes.\n\
             --experimental-post-right-mouse-drag <x1> <y1> <x2> <y2>\n\
                                     Synthesize a fn+right-drag for mouse_action2 tests.\n\
             --version, -v          Print Rust skeleton version to stdout and exit.\n\
             --help, -h             Print options to stdout and exit."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use yabai_ipc::encode_client_message;

    fn toks(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn experimental_daemon_serves_one_message() {
        let dir = std::env::temp_dir();
        let path = dir
            .join(format!(
                "yabai-rust-daemon-test-{}.socket",
                std::process::id()
            ))
            .to_string_lossy()
            .into_owned();
        let _ = std::fs::remove_file(&path);
        let listener = bind_experimental_daemon(&path).unwrap();
        let actor = Actor::spawn(Runtime::new(AppState::new(), RecordingSink::default()));

        thread::scope(|scope| {
            scope.spawn(|| {
                let (stream, _) = listener.accept().unwrap();
                serve_one(stream, &actor);
            });

            let mut client = UnixStream::connect(&path).unwrap();
            client
                .write_all(&encode_client_message(["query", "--windows", "id"]).unwrap())
                .unwrap();
            client.shutdown(std::net::Shutdown::Write).unwrap();

            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            assert_eq!(response, "[]\n");
        });

        actor.shutdown();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_space_target_by_index_and_ends() {
        let spaces = [10, 20, 30];
        assert_eq!(
            resolve_space_target(&spaces, Some(20), &Selector::Index(1)),
            Ok(10)
        );
        assert_eq!(
            resolve_space_target(&spaces, Some(20), &Selector::First),
            Ok(10)
        );
        assert_eq!(
            resolve_space_target(&spaces, Some(20), &Selector::Last),
            Ok(30)
        );
        assert!(resolve_space_target(&spaces, Some(20), &Selector::Index(4)).is_err());
        assert!(resolve_space_target(&spaces, Some(20), &Selector::Index(0)).is_err());
    }

    #[test]
    fn resolve_space_target_relative_does_not_wrap() {
        let spaces = [10, 20, 30];
        assert_eq!(
            resolve_space_target(&spaces, Some(20), &Selector::Prev),
            Ok(10)
        );
        assert_eq!(
            resolve_space_target(&spaces, Some(20), &Selector::Next),
            Ok(30)
        );
        // No wrap at the ends, matching space_manager_{prev,next}_space.
        assert!(resolve_space_target(&spaces, Some(10), &Selector::Prev).is_err());
        assert!(resolve_space_target(&spaces, Some(30), &Selector::Next).is_err());
    }

    #[test]
    fn resolve_space_target_rejects_unsupported_selectors() {
        let spaces = [10, 20, 30];
        assert!(resolve_space_target(&spaces, Some(20), &Selector::Recent).is_err());
        assert!(resolve_space_target(&spaces, Some(20), &Selector::Mouse).is_err());
        assert!(resolve_space_target(&spaces, None, &Selector::Next).is_err());
    }

    #[test]
    fn detects_window_close_messages() {
        assert!(is_window_close(&toks(&["window", "--close"])));
        assert!(is_window_close(&toks(&["window", "42", "--close"])));
        assert!(!is_window_close(&toks(&["window", "--minimize"])));
    }

    #[test]
    fn geometry_signal_frame_diff_matches_c_threshold() {
        let expected = Area::new(10.0, 20.0, 300.0, 400.0);
        assert!(!geometry_signal_frame_changed(
            SignalEvent::WindowMoved,
            expected,
            Area::new(11.49, 20.0, 300.0, 400.0)
        ));
        assert!(geometry_signal_frame_changed(
            SignalEvent::WindowMoved,
            expected,
            Area::new(11.5, 20.0, 300.0, 400.0)
        ));
        assert!(!geometry_signal_frame_changed(
            SignalEvent::WindowResized,
            expected,
            Area::new(10.0, 21.5, 300.0, 400.0)
        ));
        assert!(geometry_signal_frame_changed(
            SignalEvent::WindowResized,
            expected,
            Area::new(10.0, 20.0, 301.5, 400.0)
        ));
    }

    #[test]
    fn window_deminimize_target_resolves_supported_selectors() {
        let target = window_deminimize_target(
            &["window".into(), "42".into(), "--deminimize".into()],
            &[7, 9],
        )
        .unwrap();
        assert_eq!(target, Ok(42));

        let first = window_deminimize_target(
            &["window".into(), "first".into(), "--deminimize".into()],
            &[7, 9],
        )
        .unwrap();
        assert_eq!(first, Ok(7));

        let trailing_first = window_deminimize_target(
            &["window".into(), "--deminimize".into(), "first".into()],
            &[7, 9],
        )
        .unwrap();
        assert_eq!(trailing_first, Ok(7));

        let last = window_deminimize_target(
            &["window".into(), "last".into(), "--deminimize".into()],
            &[7, 9],
        )
        .unwrap();
        assert_eq!(last, Ok(9));

        let missing =
            window_deminimize_target(&["window".into(), "--deminimize".into()], &[]).unwrap();
        assert!(missing.unwrap_err().contains("requires a window id"));

        let empty_first = window_deminimize_target(
            &["window".into(), "first".into(), "--deminimize".into()],
            &[],
        )
        .unwrap();
        assert!(empty_first.unwrap_err().contains("minimized window"));

        let unsupported = window_deminimize_target(
            &["window".into(), "next".into(), "--deminimize".into()],
            &[7, 9],
        )
        .unwrap();
        assert!(unsupported.unwrap_err().contains("not yet supported"));
    }

    fn fs_toggle(sel: Option<&str>) -> Vec<String> {
        let mut tokens = vec!["window".to_string()];
        if let Some(sel) = sel {
            tokens.push(sel.to_string());
        }
        tokens.push("--toggle".to_string());
        tokens.push("native-fullscreen".to_string());
        tokens
    }

    fn fs_toggle_trailing(sel: &str) -> Vec<String> {
        vec![
            "window".to_string(),
            "--toggle".to_string(),
            "native-fullscreen".to_string(),
            sel.to_string(),
        ]
    }

    #[test]
    fn fullscreen_exit_target_resolves_only_registered_windows() {
        // Not the native-fullscreen toggle: outer None so the normal path runs.
        let other = ["window".to_string(), "--toggle".into(), "float".into()];
        assert_eq!(window_fullscreen_exit_target(&other, &[7]), None);

        // The toggle with no fullscreen windows is an *enter* request: Some(None).
        assert_eq!(
            window_fullscreen_exit_target(&fs_toggle(None), &[]),
            Some(None)
        );
        assert_eq!(
            window_fullscreen_exit_target(&fs_toggle(Some("42")), &[7, 9]),
            Some(None),
            "id not in the fullscreen set is an enter request"
        );

        // Exact id, first, last, and the single-entry bare form all resolve.
        assert_eq!(
            window_fullscreen_exit_target(&fs_toggle(Some("9")), &[7, 9]),
            Some(Some(9))
        );
        assert_eq!(
            window_fullscreen_exit_target(&fs_toggle_trailing("9"), &[7, 9]),
            Some(Some(9))
        );
        assert_eq!(
            window_fullscreen_exit_target(&fs_toggle(Some("first")), &[7, 9]),
            Some(Some(7))
        );
        assert_eq!(
            window_fullscreen_exit_target(&fs_toggle(Some("last")), &[7, 9]),
            Some(Some(9))
        );
        assert_eq!(
            window_fullscreen_exit_target(&fs_toggle(None), &[5]),
            Some(Some(5))
        );
        // Ambiguous bare toggle with several fullscreen windows stays an enter.
        assert_eq!(
            window_fullscreen_exit_target(&fs_toggle(None), &[5, 6]),
            Some(None)
        );
    }
}
