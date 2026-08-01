//! Experimental read-only/diagnostic subcommands (the `--experimental-sa-*`,
//! `--experimental-window-*`, and `--experimental-post-mouse-*` probes). These are
//! self-contained entry points invoked from `main`'s argument dispatch; they talk
//! directly to the scripting addition, SkyLight, and the mouse event tap without
//! touching the WM daemon's runtime state.

use std::process::ExitCode;

use yabai_core::Point;
use yabai_macos::{
    post_mouse_drag, post_mouse_moved, post_right_mouse_drag, window_alpha, window_bounds,
    window_transform, windows_on_space,
};
use yabai_sa::ScriptingAdditionStatus;

/// Probe the scripting addition over its socket and report its status, mirroring
/// the C `scripting_addition_status` / `yabai --check-sa`. Works against an SA
/// loaded by either the C or Rust daemon (same socket protocol).
pub(crate) fn run_sa_status() -> ExitCode {
    let Ok(user) = std::env::var("USER") else {
        eprintln!("scripting-addition: cannot check -- 'env USER' not set");
        return ExitCode::from(1);
    };
    let socket = yabai_sa::common::sa_socket_path(&user);
    match yabai_sa::status(&socket) {
        ScriptingAdditionStatus::NotLoaded => {
            println!("scripting-addition: NOT loaded (no response on {socket})");
            ExitCode::from(1)
        }
        ScriptingAdditionStatus::Outdated { payload_version } => {
            println!(
                "scripting-addition: loaded but OUTDATED (payload v{payload_version}, this build expects v{})",
                yabai_sa::common::OSAX_VERSION
            );
            ExitCode::from(1)
        }
        ScriptingAdditionStatus::MissingSupport { attributes } => {
            println!(
                "scripting-addition: loaded but missing support for this macOS (attrib 0x{attributes:X})"
            );
            ExitCode::from(1)
        }
        ScriptingAdditionStatus::Healthy { payload_version } => {
            println!("scripting-addition: loaded and healthy (payload v{payload_version})");
            ExitCode::SUCCESS
        }
    }
}

/// Direct, non-destructive probe of a mutating SA opcode: set a window's opacity
/// via the runtime client against the live (C- or Rust-loaded) payload. Exercises
/// the full pack/frame/send path with a real privileged effect.
/// Usage: `--experimental-sa-opacity <window_id> <opacity> [duration]`
pub(crate) fn run_sa_opacity(args: &[String]) -> ExitCode {
    let (Some(wid), Some(opacity)) = (
        args.first().and_then(|a| a.parse::<u32>().ok()),
        args.get(1).and_then(|a| a.parse::<f32>().ok()),
    ) else {
        eprintln!("usage: --experimental-sa-opacity <window_id> <opacity> [duration]");
        return ExitCode::from(64);
    };
    let duration = args
        .get(2)
        .and_then(|a| a.parse::<f32>().ok())
        .unwrap_or(0.0);

    let Ok(user) = std::env::var("USER") else {
        eprintln!("scripting-addition: 'env USER' not set");
        return ExitCode::from(1);
    };
    let sa = yabai_sa::ScriptingAddition::for_user(&user);
    match sa.set_opacity(wid, opacity, duration) {
        Ok(()) => {
            println!(
                "scripting-addition: set opacity of window {wid} to {opacity} (dur {duration})"
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("scripting-addition: opacity request failed: {error}");
            ExitCode::from(1)
        }
    }
}

/// Direct probe of the space create/destroy SA opcodes against the live payload.
/// Usage: `--experimental-sa-create-space <relative_sid>` /
/// `--experimental-sa-destroy-space <sid>`.
pub(crate) fn run_sa_space(args: &[String], create: bool) -> ExitCode {
    let Some(sid) = args.first().and_then(|a| a.parse::<u64>().ok()) else {
        eprintln!("usage: --experimental-sa-{{create,destroy}}-space <sid>");
        return ExitCode::from(64);
    };
    let Ok(user) = std::env::var("USER") else {
        eprintln!("scripting-addition: 'env USER' not set");
        return ExitCode::from(1);
    };
    let sa = yabai_sa::ScriptingAddition::for_user(&user);
    let result = if create {
        sa.create_space(sid)
    } else {
        sa.destroy_space(sid)
    };
    let verb = if create { "create" } else { "destroy" };
    match result {
        Ok(()) => {
            println!("scripting-addition: {verb} space request sent (sid {sid})");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("scripting-addition: {verb} space failed: {error}");
            ExitCode::from(1)
        }
    }
}

/// Direct probe of the move-window-to-space SA opcode against the live payload.
/// Usage: `--experimental-sa-window-to-space <window_id> <sid>`.
pub(crate) fn run_sa_window_to_space(args: &[String]) -> ExitCode {
    let (Some(wid), Some(sid)) = (
        args.first().and_then(|a| a.parse::<u32>().ok()),
        args.get(1).and_then(|a| a.parse::<u64>().ok()),
    ) else {
        eprintln!("usage: --experimental-sa-window-to-space <window_id> <sid>");
        return ExitCode::from(64);
    };
    let Ok(user) = std::env::var("USER") else {
        eprintln!("scripting-addition: 'env USER' not set");
        return ExitCode::from(1);
    };
    match yabai_sa::ScriptingAddition::for_user(&user).move_window_to_space(sid, wid) {
        Ok(()) => {
            println!("scripting-addition: moved window {wid} to space {sid}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("scripting-addition: move window to space failed: {error}");
            ExitCode::from(1)
        }
    }
}

/// Direct probe of the focus-space SA opcode against the live payload. Switches
/// Mission Control to `<sid>` instantly (no gesture). Reversible: re-run with the
/// previous sid. Usage: `--experimental-sa-focus-space <sid>`.
pub(crate) fn run_sa_focus_space(args: &[String]) -> ExitCode {
    let Some(sid) = args.first().and_then(|a| a.parse::<u64>().ok()) else {
        eprintln!("usage: --experimental-sa-focus-space <sid>");
        return ExitCode::from(64);
    };
    let Ok(user) = std::env::var("USER") else {
        eprintln!("scripting-addition: 'env USER' not set");
        return ExitCode::from(1);
    };
    match yabai_sa::ScriptingAddition::for_user(&user).focus_space(sid) {
        Ok(()) => {
            println!("scripting-addition: focused space {sid}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("scripting-addition: focus space failed: {error}");
            ExitCode::from(1)
        }
    }
}

/// Read-only probe of a window's live alpha via SkyLight (`SLSGetWindowAlpha`).
/// Used to verify the scripting-addition opacity opcode took effect, since the SA
/// write itself only returns an ack byte. Usage: `--experimental-window-alpha <window_id>`.
pub(crate) fn run_window_alpha(args: &[String]) -> ExitCode {
    let Some(wid) = args.first().and_then(|a| a.parse::<u32>().ok()) else {
        eprintln!("usage: --experimental-window-alpha <window_id>");
        return ExitCode::from(64);
    };
    match window_alpha(wid) {
        Ok(alpha) => {
            println!("window {wid} alpha {alpha}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("yabai-rust: {error}");
            ExitCode::from(1)
        }
    }
}

pub(crate) fn run_window_bounds(args: &[String]) -> ExitCode {
    let Some(wid) = args.first().and_then(|a| a.parse::<u32>().ok()) else {
        eprintln!("usage: --experimental-window-bounds <window_id>");
        return ExitCode::from(64);
    };
    match window_bounds(wid) {
        Ok((x, y, w, h)) => {
            println!("window {wid} bounds {x} {y} {w} {h}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("yabai-rust: {error}");
            ExitCode::from(1)
        }
    }
}

/// Read-only probe of a window's live affine transform via SkyLight
/// (`SLSGetWindowTransform`). Used to verify the scripting-addition `scale_window`
/// (pip) opcode, since a scale transform is invisible to the AX/CG frame. An
/// untouched window reads a pure translation (a=d=1, b=c=0); pip sets a=d<1.
/// Usage: `--experimental-window-transform <window_id>`.
pub(crate) fn run_window_transform(args: &[String]) -> ExitCode {
    let Some(wid) = args.first().and_then(|a| a.parse::<u32>().ok()) else {
        eprintln!("usage: --experimental-window-transform <window_id>");
        return ExitCode::from(64);
    };
    match window_transform(wid) {
        Ok(t) => {
            println!(
                "window {wid} transform a {} b {} c {} d {} tx {} ty {}",
                t.a, t.b, t.c, t.d, t.tx, t.ty
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("yabai-rust: {error}");
            ExitCode::from(1)
        }
    }
}

pub(crate) fn run_post_mouse_drag(args: &[String]) -> ExitCode {
    let coords: Vec<f32> = args
        .iter()
        .take(4)
        .filter_map(|a| a.parse::<f32>().ok())
        .collect();
    let [x1, y1, x2, y2] = coords[..] else {
        eprintln!("usage: --experimental-post-mouse-drag <x1> <y1> <x2> <y2>");
        return ExitCode::from(64);
    };
    post_mouse_drag(Point { x: x1, y: y1 }, Point { x: x2, y: y2 });
    println!("posted fn+drag {x1},{y1} -> {x2},{y2}");
    ExitCode::SUCCESS
}

pub(crate) fn run_post_right_mouse_drag(args: &[String]) -> ExitCode {
    let coords: Vec<f32> = args
        .iter()
        .take(4)
        .filter_map(|a| a.parse::<f32>().ok())
        .collect();
    let [x1, y1, x2, y2] = coords[..] else {
        eprintln!("usage: --experimental-post-right-mouse-drag <x1> <y1> <x2> <y2>");
        return ExitCode::from(64);
    };
    post_right_mouse_drag(Point { x: x1, y: y1 }, Point { x: x2, y: y2 });
    println!("posted fn+right-drag {x1},{y1} -> {x2},{y2}");
    ExitCode::SUCCESS
}

pub(crate) fn run_post_mouse_moved(args: &[String]) -> ExitCode {
    let (Some(x), Some(y)) = (
        args.first().and_then(|a| a.parse::<f32>().ok()),
        args.get(1).and_then(|a| a.parse::<f32>().ok()),
    ) else {
        eprintln!("usage: --experimental-post-mouse-moved <x> <y>");
        return ExitCode::from(64);
    };
    post_mouse_moved(Point { x, y });
    println!("posted mouse-moved at {x} {y}");
    ExitCode::SUCCESS
}

pub(crate) fn run_windows_on_space(args: &[String]) -> ExitCode {
    let Some(sid) = args.first().and_then(|a| a.parse::<u64>().ok()) else {
        eprintln!("usage: --experimental-windows-on-space <space_id>");
        return ExitCode::from(64);
    };
    match windows_on_space(sid) {
        Ok(windows) => {
            println!("space {sid} windows {windows:?}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("yabai-rust: {error}");
            ExitCode::from(1)
        }
    }
}
