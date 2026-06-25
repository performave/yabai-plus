use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::ExitCode;
use std::sync::mpsc::{Sender, SyncSender, channel, sync_channel};
use std::thread;
use std::time::Duration;

use yabai_core::{Area, Message, Selector, SpaceAction, parse_message, parse_selector};
use yabai_ipc::{FAILURE_MARKER, daemon_socket_path, decode_client_payload, send_message};
use yabai_macos::ax::DiscoveredAxWindow;
use yabai_macos::{
    AxSink, ObservedEvent, WorkspaceEvent, accessibility_trusted_with_prompt, active_displays,
    application_pids_with_windows, current_space_for_display, focused_window,
    focused_window_diagnostics, main_visible_frame, mission_control_spaces, move_focused_window,
    move_pid_window, observe_pid, observe_workspace, regular_application_pids, spaces_for_display,
    spaces_for_window, switch_space_by_gesture, tileable_pid_windows, windows_for_pid,
    windows_for_pid_diagnostics,
};
use yabai_runtime::{
    Actor, AppState, LayoutSink, RecordingSink, Response, Runtime, StateEvent, WindowMeta,
};

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

fn spawn_workspace_observer(tx: &Sender<WmWork>) {
    let (otx, orx) = channel::<WorkspaceEvent>();
    thread::spawn(move || {
        let _ = observe_workspace(otx);
    });
    let tx = tx.clone();
    thread::spawn(move || {
        for event in orx {
            if tx.send(WmWork::Workspace(event)).is_err() {
                break;
            }
        }
    });
}

fn managed_space_for_window(state: &AppState, window_id: u32) -> Option<u64> {
    let spaces = spaces_for_window(window_id).ok()?;
    if let Some(active_sid) = state.active_space_id() {
        if spaces.contains(&active_sid) {
            return Some(active_sid);
        }
    }
    spaces.into_iter().find(|sid| state.space(*sid).is_some())
}

fn refresh_display_spaces(runtime: &mut Runtime<AxSink>, display_id: u32, usable: Area) {
    let Ok(sids) = spaces_for_display(display_id) else {
        return;
    };
    if sids.is_empty() {
        return;
    }

    let current = sids.iter().copied().collect::<HashSet<_>>();
    for sid in &sids {
        if runtime.state.space(*sid).is_none() {
            let _ = runtime
                .state
                .handle_event(StateEvent::SpaceCreatedOnDisplay {
                    sid: *sid,
                    display_id,
                    frame: usable,
                });
            let _ = runtime.state.set_space_frame(*sid, usable);
        }
    }

    for sid in runtime.state.space_ids_for_display(display_id) {
        if !current.contains(&sid) {
            let _ = runtime.state.handle_event(StateEvent::SpaceRemoved { sid });
        }
    }
}

fn refresh_active_space(runtime: &mut Runtime<AxSink>, display_id: u32) {
    let Ok(sid) = current_space_for_display(display_id) else {
        return;
    };
    if runtime.state.active_space_id() == Some(sid) || runtime.state.space(sid).is_none() {
        return;
    }

    let _ = runtime.state.handle_event(StateEvent::SpaceChanged { sid });
    runtime.state.flush_active_to(&mut runtime.sink);
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
    let current = current_space_for_display(display.id).ok();

    println!("display {} current_space {current:?}", display.id);
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
        let cur = current.and_then(|sid| spaces.iter().position(|&s| s == sid));
        let new = spaces.iter().position(|&s| s == target);
        if let (Some(cur), Some(new)) = (cur, new) {
            println!(
                "focusing sid {target} ({} step(s))",
                new as i32 - cur as i32
            );
            if let Err(error) = switch_space_by_gesture(new as i32 - cur as i32) {
                eprintln!("yabai-rust: gesture failed: {error}");
                return ExitCode::from(1);
            }
        }
    }

    ExitCode::SUCCESS
}

/// Intercept a standalone `space --focus <selector>` and enact the active-space
/// switch through the macOS layer, returning `Some(response)`. Any other message
/// (including a `--focus` mixed with other actions) returns `None` so the caller
/// dispatches it through the pure core. Mirrors `space_manager_focus_space`'s
/// gesture fallback; the scripting-addition path is deferred (Phase 8).
fn try_space_focus(
    runtime: &mut Runtime<AxSink>,
    display_id: u32,
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
    let active = runtime
        .state
        .active_space_id()
        .or_else(|| current_space_for_display(display_id).ok());

    let target = match resolve_space_target(&spaces, active, selector) {
        Ok(target) => target,
        Err(error) => return Some(Err(error)),
    };
    if Some(target) == active {
        return Some(Err("cannot focus an already focused space.".to_string()));
    }

    let cur = active.and_then(|sid| spaces.iter().position(|&s| s == sid));
    let new = spaces.iter().position(|&s| s == target);
    if let (Some(cur), Some(new)) = (cur, new) {
        if let Err(error) = switch_space_by_gesture(new as i32 - cur as i32) {
            return Some(Err(error.to_string()));
        }
    }

    refresh_active_space(runtime, display_id);
    Some(Ok(None))
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
fn reconcile_pid(
    runtime: &mut Runtime<AxSink>,
    managed: &mut HashMap<i32, HashSet<u32>>,
    pid: i32,
) {
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
        // Refresh metadata every pass so titles stay current.
        runtime.state.set_window_meta(
            id,
            WindowMeta {
                app: window.app,
                title: window.title,
                pid: window.pid,
            },
        );
        if !known.contains(&id) {
            // A genuinely new window: hand its element to the sink and tree.
            runtime.sink.register(id, window.window);
        }
        let _ = runtime
            .state
            .handle_event(StateEvent::WindowAssignedToSpace { window_id: id, sid });
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

    runtime.state.flush_active_to(&mut runtime.sink);
}

fn drop_pid(runtime: &mut Runtime<AxSink>, managed: &mut HashMap<i32, HashSet<u32>>, pid: i32) {
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
    runtime.state.flush_active_to(&mut runtime.sink);
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
    let current_sid = match current_space_for_display(display.id) {
        Ok(sid) => sid,
        Err(error) => {
            eprintln!("yabai-rust: failed to discover current space: {error}");
            return ExitCode::from(1);
        }
    };
    let mut display_spaces = match spaces_for_display(display.id) {
        Ok(spaces) => spaces,
        Err(error) => {
            eprintln!("yabai-rust: failed to discover display spaces: {error}");
            return ExitCode::from(1);
        }
    };
    if !display_spaces.contains(&current_sid) {
        display_spaces.push(current_sid);
    }

    let mut state = AppState::new();
    state.add_display(display.id, display.frame);
    for sid in &display_spaces {
        state.add_space_to_display(*sid, display.id, usable);
    }
    state.set_active_space(current_sid);
    let _ = state.handle_tokens(&tile_config_tokens(gap, padding));
    for sid in &display_spaces {
        if let Err(error) = state.set_space_frame(*sid, usable) {
            eprintln!("yabai-rust: failed to apply padding to space {sid}: {error}");
            return ExitCode::from(1);
        }
    }

    let mut runtime = Runtime::new(state, AxSink::new());
    let mut managed: HashMap<i32, HashSet<u32>> = HashMap::new();

    // Initial tile from the current world.
    for pid in &pids {
        reconcile_pid(&mut runtime, &mut managed, *pid);
    }
    let initial: usize = managed.values().map(HashSet::len).sum();

    // Unified event loop: observers, the periodic tick, and the socket all feed
    // one channel processed against the single `Runtime<AxSink>`.
    let (tx, rx) = channel::<WmWork>();

    // One AX observer per app on its own run-loop thread.
    let mut observed: HashSet<i32> = HashSet::new();
    for pid in &pids {
        observed.insert(*pid);
        spawn_observer(*pid, &tx);
    }
    spawn_workspace_observer(&tx);

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

    // The main thread keeps `tx` alive, so the loop runs until the process dies.
    eprintln!(
        "yabai-rust: WM daemon up on {socket_path} — target {target}, {} app(s), {initial} window(s), display {}, active space {current_sid}, {} discovered space(s), gap {gap}, padding {padding}",
        pids.len(),
        display.id,
        display_spaces.len()
    );
    eprintln!(
        "yabai-rust: tracking live window changes; send commands with a matching USER (e.g. USER=<name> yabai -m space --rotate 90)"
    );

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
                refresh_display_spaces(&mut runtime, display.id, usable);
                refresh_active_space(&mut runtime, display.id);
                reconcile_pid(&mut runtime, &mut managed, pid);
                if let Some(window_id) = focused {
                    if runtime.state.window_space_id(window_id) == runtime.state.active_space_id() {
                        let _ = runtime
                            .state
                            .handle_event(StateEvent::WindowFocused { window_id });
                    }
                }
            }
            WmWork::Workspace(event) => match event {
                WorkspaceEvent::ActiveSpaceChanged => {
                    refresh_display_spaces(&mut runtime, display.id, usable);
                    refresh_active_space(&mut runtime, display.id);
                    for pid in observed.iter().copied().collect::<Vec<_>>() {
                        reconcile_pid(&mut runtime, &mut managed, pid);
                    }
                }
                WorkspaceEvent::ApplicationLaunched { pid } => {
                    if is_all && observed.insert(pid) {
                        spawn_observer(pid, &tx);
                        refresh_display_spaces(&mut runtime, display.id, usable);
                        refresh_active_space(&mut runtime, display.id);
                        reconcile_pid(&mut runtime, &mut managed, pid);
                    }
                }
                WorkspaceEvent::ApplicationTerminated { pid } => {
                    if observed.remove(&pid) {
                        drop_pid(&mut runtime, &mut managed, pid);
                    }
                }
            },
            WmWork::Tick => {
                refresh_display_spaces(&mut runtime, display.id, usable);
                refresh_active_space(&mut runtime, display.id);
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
                    reconcile_pid(&mut runtime, &mut managed, pid);
                }
            }
            WmWork::Message { tokens, reply } => {
                refresh_display_spaces(&mut runtime, display.id, usable);
                refresh_active_space(&mut runtime, display.id);
                // A lone `space --focus <sel>` needs a macOS-layer space switch the
                // pure core can't perform; handle it here, otherwise fall through.
                let response = match try_space_focus(&mut runtime, display.id, &tokens) {
                    Some(response) => response,
                    None => runtime.message(&tokens),
                };
                let _ = reply.send(response);
            }
        }
    }
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
             --version, -v          Print Rust skeleton version to stdout and exit.\n\
             --help, -h             Print options to stdout and exit."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use yabai_ipc::encode_client_message;

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
}
