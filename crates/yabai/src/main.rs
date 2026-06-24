use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::ExitCode;
use std::sync::mpsc::{Sender, SyncSender, channel, sync_channel};
use std::thread;
use std::time::Duration;

use yabai_core::Area;
use yabai_ipc::{FAILURE_MARKER, daemon_socket_path, decode_client_payload, send_message};
use yabai_macos::ax::DiscoveredAxWindow;
use yabai_macos::{
    AxSink, ObservedEvent, accessibility_trusted_with_prompt, active_displays,
    application_pids_with_windows, focused_window, focused_window_diagnostics, main_visible_frame,
    move_focused_window, move_pid_window, observe_pid, regular_application_pids,
    tileable_pid_windows, windows_for_pid, windows_for_pid_diagnostics,
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

/// Reconcile the managed window set for one app against what AX currently
/// reports, registering newcomers in the sink and dropping windows that vanished
/// (which robustly handles closes despite unreliable AX destroy notifications),
/// then re-flow the active layout.
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
            let _ = runtime
                .state
                .handle_event(StateEvent::WindowCreated { window_id: id });
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

    let mut state = AppState::new();
    state.add_display(display.id, display.frame);
    state.add_space_to_display(1, display.id, usable);
    state.set_active_space(1);
    let _ = state.handle_tokens(&tile_config_tokens(gap, padding));
    if let Err(error) = state.set_space_frame(1, usable) {
        eprintln!("yabai-rust: failed to apply padding: {error}");
        return ExitCode::from(1);
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
        "yabai-rust: WM daemon up on {socket_path} — target {target}, {} app(s), {initial} window(s), gap {gap}, padding {padding}",
        pids.len()
    );
    eprintln!(
        "yabai-rust: tracking live window changes; send commands with a matching USER (e.g. USER=<name> yabai -m space --rotate 90)"
    );

    for work in rx {
        match work {
            WmWork::Observed(event) => reconcile_pid(&mut runtime, &mut managed, event.pid()),
            WmWork::Tick => {
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
                let response = runtime.message(&tokens);
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
}
