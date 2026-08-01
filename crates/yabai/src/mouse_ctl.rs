//! Mouse-driven control for the WM daemon: `focus_follows_mouse` handling and
//! the modifier-held drag-to-move / drag-to-resize / drop machinery (the active
//! `CGEventTap` feeds these). A child module of `main`, so it reaches the
//! parent's helpers/imports via `use super::*`; the daemon loop calls back in via
//! `use mouse_ctl::*`.

use super::*;

/// `focus_follows_mouse`: when the cursor moves over a different managed window,
/// focus it — without raising (`autofocus`) or with a raise (`autoraise`),
/// mirroring the C `MOUSE_MOVED` handler. No-op when the mode is off or the cursor
/// is over the already-focused window. The C's occlusion / gesture-debounce /
/// mission-control refinements are not modeled here.
pub(crate) fn handle_mouse_moved(
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
    let Some(window_id) = ffm_window_at_point(runtime, point) else {
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

/// The window `focus_follows_mouse` should focus for a cursor at `point`: the
/// top-most on-screen window under the cursor that the daemon tracks.
///
/// Uses the live CoreGraphics stacking order + geometry rather than the BSP tree,
/// so it finds floating / `config manage off` windows too — focus-follows-mouse
/// then behaves identically whether or not the window is tiled. Only windows the
/// daemon tracks are eligible: if the top-most window under the cursor is one
/// yabai never manages (a panel, the Arc picture-in-picture, other AX-ineligible
/// surfaces), focus stays put — mirroring the C, which does nothing when the
/// window at the point is not a managed window. Falls back to the tree lookup if
/// CoreGraphics reports nothing (e.g. a tiled window it momentarily omits).
pub(crate) fn ffm_window_at_point(runtime: &Runtime<AxSink>, point: Point) -> Option<u32> {
    match on_screen_windows()
        .into_iter()
        .find(|window| window.bounds.contains_point(point))
    {
        Some(window) => runtime
            .state
            .window_known_space_id(window.window_id)
            .map(|_| window.window_id),
        None => runtime.state.managed_window_at_point(point),
    }
}

/// Before dispatching a `window ... mouse` command, resolve the window under the
/// live cursor via CoreGraphics and stash it on the state, so the pure `mouse`
/// window selector reaches floating / `config manage off` windows (the tree-only
/// resolver misses them). A no-op for any command that is not a `window` command
/// referencing the `mouse` selector, so ordinary dispatch pays no CoreGraphics
/// cost. Reading the cursor here (rather than the last mouse-move) keeps the
/// selector correct even when `focus_follows_mouse` is off and no move events are
/// being tracked.
pub(crate) fn prime_mouse_window_selector(runtime: &mut Runtime<AxSink>, tokens: &[String]) {
    if tokens.first().map(String::as_str) != Some("window")
        || !tokens.iter().any(|token| token == "mouse")
    {
        return;
    }
    let Ok(point) = cursor_location() else {
        return;
    };
    runtime.state.set_cursor_point(point);
    let window = ffm_window_at_point(runtime, point);
    runtime.state.set_cursor_window(window);
}

/// Map the configured `mouse_modifier` to the compact `MOUSE_MOD_*` mask the drag
/// event tap compares against.
pub(crate) fn mouse_modifier_mask(modifier: MouseModifier) -> u8 {
    match modifier {
        MouseModifier::Alt => MOUSE_MOD_ALT,
        MouseModifier::Shift => MOUSE_MOD_SHIFT,
        MouseModifier::Cmd => MOUSE_MOD_CMD,
        MouseModifier::Ctrl => MOUSE_MOD_CTRL,
        MouseModifier::Fn => MOUSE_MOD_FN,
    }
}

/// State captured while a `mouse_modifier`-armed drag is in progress.
pub(crate) struct DragState {
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

pub(crate) fn resize_handle_for_point(frame: Area, point: Point) -> u8 {
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

pub(crate) fn resized_frame(origin: Area, handle: u8, dx: f32, dy: f32) -> Area {
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
pub(crate) fn drag_window_at_point(runtime: &Runtime<AxSink>, point: Point) -> Option<(u32, bool)> {
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
pub(crate) fn handle_drag(
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
