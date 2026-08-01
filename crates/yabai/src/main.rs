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
    MissionControlEvent, MouseDragButton, MouseDragEvent, ObservedEvent, WorkspaceEvent,
    accessibility_trusted_with_prompt, active_displays, application_pids_with_windows,
    current_space_for_display, cursor_display_id, cursor_location, display_for_space, display_uuid,
    dock_pid, focused_window, focused_window_diagnostics, main_display_id, main_visible_frame,
    mission_control_spaces, move_focused_window, move_pid_window, ns_application_load,
    observe_display_reconfiguration, observe_mission_control, observe_mouse_drag,
    observe_mouse_moved, observe_pid, observe_workspace, pid_window_infos,
    regular_application_pids, set_active_display, set_drag_modifier, space_is_native_fullscreen,
    spaces_for_display, spaces_for_window, switch_space_by_gesture, tileable_pid_windows,
    visible_frame_for_display, warp_cursor_to_display_center, warp_cursor_to_point, window_alpha,
    window_is_ordered_in, window_level, windows_for_pid, windows_for_pid_diagnostics,
    windows_on_space,
};
use yabai_runtime::{
    Actor, AppState, AppliedRuleEffects, DropResult, LayoutSink, LiveWindowInfo, RecordingSink,
    Response, Runtime, StateEvent, WindowMeta,
};
use yabai_sa::{ScriptingAddition, ScriptingAdditionStatus};

mod mouse_ctl;
mod probes;
mod sa_ops;
mod service;
use mouse_ctl::*;
use sa_ops::*;

/// The yabai version. The C fork compiled the pushed git tag in; release builds
/// can override this via `YABAI_VERSION` at build time (see build.rs / CI),
/// otherwise it falls back to the current fork version.
const YABAI_VERSION: &str = match option_env!("YABAI_VERSION") {
    Some(version) => version,
    None => "v7.1.25-plus.7",
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("--version") | Some("-v") => {
            println!("yabai-{YABAI_VERSION}");
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--message") | Some("-m") => run_message(&args[1..]),
        Some("--install-service") => service::install(),
        Some("--uninstall-service") => service::uninstall(),
        Some("--start-service") => service::start(),
        Some("--restart-service") => service::restart(),
        Some("--stop-service") => service::stop(),
        // Scripting-addition install/load/uninstall + sudoers (ported from C sa.m).
        Some("--load-sa") => ExitCode::from(yabai_sa::loader::load() as u8),
        Some("--uninstall-sa") => ExitCode::from(yabai_sa::loader::uninstall() as u8),
        Some("--check-sa") => probes::run_sa_status(),
        Some("--install-sudoers") => ExitCode::from(yabai_sa::loader::install_sudoers() as u8),
        Some("--uninstall-sudoers") => ExitCode::from(yabai_sa::loader::uninstall_sudoers() as u8),
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
        // No subcommand (or only daemon flags like --config/-c/--verbose): start
        // the production WM daemon. This is what the launchd service runs.
        None => run_production_daemon(&args),
        Some(flag) if is_daemon_flag(flag) => run_production_daemon(&args),
        Some(other) => {
            eprintln!("yabai: unknown command '{other}' (try --help)");
            ExitCode::from(64)
        }
    }
}

/// Whether the leading argument is a daemon-mode flag (so `yabai --config … -V`
/// starts the daemon rather than erroring on an unknown command).
fn is_daemon_flag(flag: &str) -> bool {
    matches!(flag, "--config" | "-c" | "--verbose" | "-V")
}

/// Start the production WM daemon: acquire the per-user lock, bind the real
/// `/tmp/yabai_$USER.socket`, run the config file once the socket is up, and tile
/// all apps. Mirrors the C `yabai` default startup.
fn run_production_daemon(args: &[String]) -> ExitCode {
    let Ok(user) = std::env::var("USER")
        .map_err(|_| ())
        .and_then(|u| if u.is_empty() { Err(()) } else { Ok(u) })
    else {
        eprintln!("yabai: 'env USER' not set! abort..");
        return ExitCode::from(1);
    };

    // Per-user lock so launchd (KeepAlive) never runs two daemons at once.
    if !acquire_lock_file(&user) {
        eprintln!("yabai: could not acquire lock-file! abort..");
        return ExitCode::from(1);
    }

    // Resolve the config file (`--config`/`-c <path>` or the default locations)
    // and run it once the daemon socket is bound; it sends `yabai -m config …`.
    let config = resolve_config_path(args);
    let socket_path = daemon_socket_path(&user);
    if let Some(config) = config {
        let socket = socket_path.clone();
        thread::spawn(move || {
            // Wait until the daemon is actually *listening*, not merely until the
            // socket file exists: after an unclean restart a stale socket file is
            // present before the new daemon rebinds, and a bare existence check
            // would race the config's `yabai -m` calls ahead of the bind.
            for _ in 0..100 {
                if UnixStream::connect(&socket).is_ok() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            let _ = std::process::Command::new(&config).status();
        });
    }

    // The WM daemon tiles every app ("all") on the real socket; gap/padding start
    // at 0 and the config sets them.
    run_rust_wm_daemon(&[
        socket_path,
        "all".to_string(),
        "0".to_string(),
        "0".to_string(),
    ])
}

/// The config file to run at startup: `--config`/`-c <path>`, else the first of
/// `$XDG_CONFIG_HOME/yabai/yabairc`, `~/.config/yabai/yabairc`, `~/.yabairc`.
fn resolve_config_path(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--config" || arg == "-c" {
            return iter.next().cloned();
        }
    }
    let candidates = [
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(|base| format!("{base}/yabai/yabairc")),
        std::env::var("HOME")
            .ok()
            .map(|home| format!("{home}/.config/yabai/yabairc")),
        std::env::var("HOME")
            .ok()
            .map(|home| format!("{home}/.yabairc")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|path| std::path::Path::new(path).is_file())
}

/// Acquire an exclusive advisory lock on `/tmp/yabai_$USER.lock` (C
/// `acquire_lock_file`), so only one daemon runs. The lock is released when the
/// process exits (the fd is intentionally leaked for the process lifetime).
fn acquire_lock_file(user: &str) -> bool {
    use std::os::fd::IntoRawFd;
    let path = format!("/tmp/yabai_{user}.lock");
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
    else {
        return false;
    };
    let fd = file.into_raw_fd();
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    // SAFETY: `fd` is a valid open file descriptor; `flock` with LOCK_EX|LOCK_NB
    // takes an exclusive non-blocking lock. The fd is leaked so the lock lives for
    // the process lifetime.
    unsafe { flock(fd, LOCK_EX | LOCK_NB) == 0 }
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
                if let Some(uuid) = display_uuid(display.id) {
                    state.set_display_uuid(display.id, uuid);
                }
            }
        }
        Err(error) => eprintln!("yabai-rust: failed to discover displays: {error}"),
    }
    // Record the main display so `external_bar main` can be scoped to it.
    state.set_main_display(main_display_id());
}

fn bind_experimental_daemon(socket_path: &str) -> io::Result<UnixListener> {
    // A daemon that exited uncleanly leaves its socket file behind, so a fresh
    // `bind()` fails with EADDRINUSE. Probe the leftover: if something still
    // answers, a live daemon owns it — refuse (single-instance is also enforced
    // by the lock-file, but this keeps the experimental probes honest). If the
    // connect is refused, the file is stale — unlink it and bind fresh. Mirrors
    // upstream yabai's `socket_open`/`unlink` handling.
    match UnixStream::connect(socket_path) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another yabai daemon is already listening on this socket",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {
            let _ = std::fs::remove_file(socket_path);
        }
    }
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
    /// A Mission Control enter/exit transition (Dock Expose AX observer).
    MissionControl(MissionControlEvent),
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

/// Spawn the Mission Control (Dock Expose) observer on its own run-loop thread,
/// forwarding enter/exit transitions into the shared `WmWork` channel. No-op if
/// the Dock pid can't be resolved. NOTE: the Dock AX observer registers cleanly,
/// but the enter/exit *firing* has not been verified over a headless SSH session
/// (Mission Control is a GUI-only transition) — same caveat as the display
/// reconfiguration signals.
fn spawn_mission_control_observer(tx: &Sender<WmWork>) {
    let Some(dock) = dock_pid() else {
        eprintln!("yabai-rust: could not resolve Dock pid; mission_control signals disabled");
        return;
    };
    let (mtx, mrx) = channel::<MissionControlEvent>();
    thread::spawn(move || {
        if let Err(error) = observe_mission_control(dock, mtx) {
            eprintln!("yabai-rust: mission control observer failed: {error}");
        }
    });
    let tx = tx.clone();
    thread::spawn(move || {
        for event in mrx {
            if tx.send(WmWork::MissionControl(event)).is_err() {
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

/// The known space that authoritatively contains `window_id` via the reliable
/// inverse mapping (`windows_on_space` / `SLSCopyWindowsWithOptionsAndTags`), or
/// `None`. Unlike [`managed_space_for_window`] this has NO `spaces_for_window`
/// fallback, so a window that no longer exists anywhere returns `None` — the
/// reconcile drop path relies on that to distinguish a moved window (still listed
/// on some space) from a destroyed/phantom one (listed nowhere).
fn window_space_strict(state: &AppState, window_id: u32) -> Option<u64> {
    state
        .space_ids()
        .into_iter()
        .find(|&sid| windows_on_space(sid).is_ok_and(|windows| windows.contains(&window_id)))
}

fn managed_space_for_window(state: &AppState, window_id: u32) -> Option<u64> {
    // Authoritative on macOS 26: ask each known space which windows it contains.
    if let Some(sid) = window_space_strict(state, window_id) {
        return Some(sid);
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
    // The main display can change on hotplug/rearrange; keep it current.
    runtime.state.set_main_display(main_display_id());

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
        if let Some(uuid) = display_uuid(display.id) {
            runtime.state.set_display_uuid(display.id, uuid);
        }

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

    // Push the live global mission-control space order so numeric space selectors
    // (`window --space 2`, `space 2 --destroy`, ...) resolve to the right sid,
    // including spaces created since startup.
    if let Ok(order) = mission_control_spaces() {
        runtime.state.set_mission_control_order(order);
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
/// Whether the message is a `query --windows` (so the daemon should populate
/// per-window live AX/SkyLight info before serving it).
fn is_window_query(tokens: &[String]) -> bool {
    tokens.first().map(String::as_str) == Some("query")
        && tokens.iter().any(|token| token == "--windows")
}

/// Read the live AX/SkyLight per-window fields the pure serializer can't (opacity
/// via SkyLight for now) into `AppState`, for every window the daemon tracks.
fn populate_window_live_info(runtime: &mut Runtime<AxSink>) {
    // Off-tree (floating/sticky/scratchpad) windows aren't in a tree, so the query
    // has no frame for them — push their live AX frame so they get listed.
    for wid in runtime.state.off_tree_window_ids() {
        if let Some(area) = runtime.sink.window_frame(wid) {
            runtime.state.set_off_tree_frame(wid, area);
        }
    }
    for wid in runtime.state.all_window_ids() {
        let level = window_level(wid).unwrap_or(0);
        let info = LiveWindowInfo {
            opacity: window_alpha(wid).unwrap_or(1.0),
            role: runtime.sink.window_role(wid).unwrap_or_default(),
            subrole: runtime.sink.window_subrole(wid).unwrap_or_default(),
            can_move: runtime.sink.window_can_move(wid),
            can_resize: runtime.sink.window_can_resize(wid),
            level: level as i64,
            // C reads sub-level via a fragile magic-id mach_msg; defer it and
            // report sub-level 0 / sub-layer "normal" for now.
            sub_level: 0,
        };
        runtime.state.set_window_live_info(wid, info);
    }
}

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

/// Split a chained `window`/`space`/`display` command into one sub-command per
/// action, so each `--action` runs through the full macOS-layer dispatch chain
/// (the single-action interceptors — grid, move, resize — only match a lone
/// action). `window [SEL] --toggle float --grid …` becomes `window [SEL] --toggle
/// float` and `window [SEL] --grid …`, each carrying the shared target selector,
/// matching upstream yabai's per-action processing.
///
/// Only the mutation domains are split: `query --spaces --space` chains a
/// selector *modifier*, not two actions, so query/config/rule/signal are left
/// intact. A command with zero or one action yields a single entry equal to the
/// input, preserving today's behavior for the common case.
fn split_action_commands(tokens: &[String]) -> Vec<Vec<String>> {
    let Some(domain) = tokens.first() else {
        return vec![tokens.to_vec()];
    };
    if !matches!(domain.as_str(), "window" | "space" | "display") {
        return vec![tokens.to_vec()];
    }
    let rest = &tokens[1..];
    // A leading non-`--` token is the target selector, shared by every action.
    let (target, actions) = match rest.first() {
        Some(tok) if !tok.starts_with("--") => (Some(tok.clone()), &rest[1..]),
        _ => (None, rest),
    };
    // Group tokens by `--flag` boundary; each flag plus its trailing args is one
    // action. A stray non-flag token before any flag means the shape is not what
    // we expect — leave the command unsplit rather than guess.
    let mut groups: Vec<Vec<String>> = Vec::new();
    for tok in actions {
        if tok.starts_with("--") {
            groups.push(vec![tok.clone()]);
        } else if let Some(last) = groups.last_mut() {
            last.push(tok.clone());
        } else {
            return vec![tokens.to_vec()];
        }
    }
    if groups.len() <= 1 {
        return vec![tokens.to_vec()];
    }
    groups
        .into_iter()
        .map(|group| {
            let mut cmd = Vec::with_capacity(2 + group.len());
            cmd.push(domain.clone());
            if let Some(target) = &target {
                cmd.push(target.clone());
            }
            cmd.extend(group);
            cmd
        })
        .collect()
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

/// Before sending the FOCUSED window off a currently-visible space, re-focus
/// another window on that source space (mirroring C
/// `window_manager_send_window_to_space`), so macOS doesn't follow the moved
/// window to the destination space. No-op when `wid` isn't focused, its source
/// space isn't visible, or no other window is there (the C Finder-drop for the
/// empty case needs a private PSN API and is deferred).
fn keep_source_space_focused(runtime: &mut Runtime<AxSink>, wid: u32) {
    if runtime.state.focused_window_id() != Some(wid) {
        return;
    }
    let Some(src) = runtime.state.window_known_space_id(wid) else {
        return;
    };
    if !runtime.state.is_space_visible(src) {
        return;
    }
    if let Some(next) = runtime.state.window_on_space_excluding(src, wid) {
        runtime.sink.focus_window(next);
        runtime.state.set_focused_window(Some(next));
    }
}

/// After a successful `window --space`/`--display` SA move `(wid, sid)`, refresh
/// live topology and reassign the window to the target space in the model so a
/// following `query --windows` reflects the move immediately (the window is
/// already physically on `sid`; without this the model shows the old space until
/// the next reconcile). Best-effort: if `sid` isn't yet a managed space in the
/// model, the periodic reconcile catches it.
fn finish_window_to_space(
    runtime: &mut Runtime<AxSink>,
    display_frames: &mut Vec<(u32, Area)>,
    result: Result<(u32, u64), String>,
) -> Response {
    match result {
        Ok((wid, sid)) => {
            refresh_live_display_state(runtime, display_frames);
            let _ = runtime
                .state
                .handle_event(StateEvent::WindowAssignedToSpace {
                    window_id: wid,
                    sid,
                });
            Ok(None)
        }
        Err(error) => Err(error),
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
                        let targets = (
                            runtime.state.resolve_window_selector(cmd.target.as_ref()),
                            runtime.state.resolve_space(Some(selector)),
                        );
                        let result = match targets {
                            (Ok(wid), Ok(sid)) => {
                                keep_source_space_focused(runtime, wid);
                                sa.move_window_to_space(sid, wid)
                                    .map(|()| (wid, sid))
                                    .map_err(|error| {
                                        format!("could not move window to space: {error}\n")
                                    })
                            }
                            (Err(error), _) | (_, Err(error)) => Err(error),
                        };
                        return Some(finish_window_to_space(runtime, display_frames, result));
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
                            (Ok(wid), Ok(sid)) => {
                                keep_source_space_focused(runtime, wid);
                                sa.move_window_to_space(sid, wid)
                                    .map(|()| (wid, sid))
                                    .map_err(|error| {
                                        format!("could not move window to space: {error}\n")
                                    })
                            }
                            (Err(error), _) | (_, Err(error)) => Err(error),
                        };
                        return Some(finish_window_to_space(runtime, display_frames, result));
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
        let Some(physical_sid) = managed_space_for_window(&runtime.state, id) else {
            continue;
        };
        current.insert(id);
        let is_new = !known.contains(&id);
        // A genuinely new window honors `window_origin_display` (focused/cursor
        // route it away from its physical space); an already-known window that
        // moved keeps following its physical space.
        let sid = if is_new {
            runtime
                .state
                .origin_space_for_new_window(physical_sid, cursor_location().ok())
        } else {
            physical_sid
        };
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
        // A window missing from this pass's AX enumeration is EITHER genuinely
        // closed OR merely on a non-visible space (AX can't enumerate those).
        // Keep it only if the RELIABLE inverse mapping still lists it on a known
        // space (a real cross-space move); the unreliable `spaces_for_window`
        // fallback must NOT keep it, or a destroyed/phantom window (which that
        // fallback wrongly reports on the active space) would linger in the model.
        if let Some(sid) = window_space_strict(&runtime.state, id) {
            let _ = runtime
                .state
                .handle_event(StateEvent::WindowAssignedToSpace { window_id: id, sid });
            current.insert(id);
        } else {
            runtime.sink.unregister(id);
            runtime.state.remove_window_meta(id);
            let _ = runtime
                .state
                .handle_event(StateEvent::WindowDestroyed { window_id: id });
        }
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
    // Seed the global mission-control space order so numeric space selectors
    // resolve like C from the first command (refreshed later on topology change).
    if let Ok(order) = mission_control_spaces() {
        state.set_mission_control_order(order);
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
    // Mission Control enter/exit via the Dock Expose AX observer.
    spawn_mission_control_observer(&tx);

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
                        // `window_known_space_id` (not `window_space_id`) so a
                        // floating/off-tree window — e.g. any window under `config
                        // manage off` — is still tracked as the focused window,
                        // letting focused-window commands act on it. A window idling
                        // on a non-visible space can fall out of the model's tracking
                        // (AX enumerates only the visible space), leaving it space-
                        // less; resolve its space authoritatively so focus still
                        // registers — otherwise focused-window keybinds (`--toggle
                        // float`, `--space`, ...) would silently act on a stale
                        // window instead of the one the user just focused.
                        let known = runtime.state.window_known_space_id(window_id);
                        let sid =
                            known.or_else(|| managed_space_for_window(&runtime.state, window_id));
                        if let Some(sid) = sid {
                            if known.is_none() {
                                // Re-attach a window the model had lost track of, so
                                // it is tracked and focus can register on it.
                                let _ =
                                    runtime
                                        .state
                                        .handle_event(StateEvent::WindowAssignedToSpace {
                                            window_id,
                                            sid,
                                        });
                            }
                            runtime.state.set_active_space(sid);
                        }
                        // Register focus even when the window's space cannot be
                        // resolved (e.g. a floating window when SkyLight's space
                        // enumeration is unavailable), so focused-window keybinds
                        // (`--toggle float`, `--space`, ...) still act on the window
                        // the user just focused rather than a stale one. Fires
                        // unconditionally; the `sid` block above only adjusts the
                        // active space when it is known.
                        let _ = runtime
                            .state
                            .handle_event(StateEvent::WindowFocused { window_id });
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
                WmWork::MissionControl(event) => {
                    let signal = match event {
                        MissionControlEvent::Enter => SignalEvent::MissionControlEnter,
                        MissionControlEvent::Exit => SignalEvent::MissionControlExit,
                    };
                    fire_signals(&runtime, signal, &[], None, None, None);
                }
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
                    // `query --windows` reports fields that need a live AX/SkyLight
                    // read (opacity, ...); populate them just before serving it.
                    if is_window_query(&tokens) {
                        populate_window_live_info(&mut runtime);
                    }
                    // `query --spaces is-native-fullscreen` needs a live SkyLight
                    // read per space; populate it before serving.
                    if tokens.first().map(String::as_str) == Some("query")
                        && tokens.iter().any(|token| token == "--spaces")
                    {
                        for sid in runtime.state.space_ids() {
                            runtime
                                .state
                                .set_space_native_fullscreen(sid, space_is_native_fullscreen(sid));
                        }
                    }
                    // A chained window/space/display command (`--a … --b …`) is
                    // split into one sub-command per action so each runs through
                    // the full dispatch chain below; a single-action command
                    // yields exactly one entry (unchanged behavior).
                    let sub_commands = split_action_commands(&tokens);
                    let mut response: Response = Ok(None);
                    for tokens in sub_commands {
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
                        response = match fullscreen_exit {
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
                                    None => {
                                        match try_window_grid(&runtime, &display_frames, &tokens) {
                                            Some(response) => response,
                                            None => match try_window_move(&runtime, &tokens) {
                                                Some(response) => response,
                                                None => {
                                                    match try_window_resize(&runtime, &tokens) {
                                                        Some(response) => response,
                                                        None => {
                                                            match try_window_windowed_fullscreen(
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
                                                            }
                                                        }
                                                    }
                                                }
                                            },
                                        }
                                    }
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
                            set_drag_modifier(mouse_modifier_mask(
                                runtime.state.config.mouse_modifier,
                            ));
                        }
                        // A failing action aborts the rest of a chained command.
                        if response.is_err() {
                            break;
                        }
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

    #[test]
    fn split_action_commands_splits_chained_window_actions() {
        // The user's `shift+alt-t` binding: two actions in one message.
        let out = split_action_commands(&toks(&[
            "window",
            "--toggle",
            "float",
            "--grid",
            "4:4:1:1:2:2",
        ]));
        assert_eq!(
            out,
            vec![
                toks(&["window", "--toggle", "float"]),
                toks(&["window", "--grid", "4:4:1:1:2:2"]),
            ]
        );
    }

    #[test]
    fn split_action_commands_preserves_the_shared_target_selector() {
        let out = split_action_commands(&toks(&[
            "window", "0x1F", "--focus", "east", "--swap", "west",
        ]));
        assert_eq!(
            out,
            vec![
                toks(&["window", "0x1F", "--focus", "east"]),
                toks(&["window", "0x1F", "--swap", "west"]),
            ]
        );
    }

    #[test]
    fn split_action_commands_leaves_single_action_and_non_mutation_domains() {
        // Single action → unchanged (one entry equal to the input).
        assert_eq!(
            split_action_commands(&toks(&["window", "--focus", "east"])),
            vec![toks(&["window", "--focus", "east"])]
        );
        // `query --spaces --space` chains a selector modifier, not two actions:
        // it must not be split.
        assert_eq!(
            split_action_commands(&toks(&["query", "--spaces", "--space"])),
            vec![toks(&["query", "--spaces", "--space"])]
        );
        // Other domains are never split.
        assert_eq!(
            split_action_commands(&toks(&["config", "layout", "bsp"])),
            vec![toks(&["config", "layout", "bsp"])]
        );
    }
}
