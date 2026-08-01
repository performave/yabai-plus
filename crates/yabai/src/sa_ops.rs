//! Scripting-addition operation helpers for the WM daemon: the `*_via_sa`
//! functions that translate a resolved `window`/`space` command into SA opcodes
//! (opacity, z-order, sub-layer, sticky/shadow/pip, scratchpads, and space
//! move/swap/switch/to-display), plus their small pure helpers. Called from the
//! daemon interceptors in `main.rs`; as a child module they freely reach the
//! parent's helpers and imports via `use super::*`.

use super::*;

pub(crate) fn window_opacity_via_sa(
    sa: &ScriptingAddition,
    runtime: &Runtime<AxSink>,
    target: Option<&Selector>,
    opacity: f32,
) -> Response {
    let wid = runtime.state.resolve_window_selector(target)?;
    window_opacity_for_id_via_sa(sa, runtime, wid, opacity).map(|()| None)
}

pub(crate) fn window_opacity_for_id_via_sa(
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
pub(crate) fn window_order_via_sa(
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
pub(crate) fn window_sub_layer_via_sa(
    sa: &ScriptingAddition,
    runtime: &Runtime<AxSink>,
    target: Option<&Selector>,
    layer: Layer,
) -> Response {
    let wid = runtime.state.resolve_window_selector(target)?;
    window_sub_layer_for_id_via_sa(sa, runtime, wid, layer).map(|()| None)
}

pub(crate) fn window_sub_layer_for_id_via_sa(
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

pub(crate) fn validate_scratchpad_label(label: &str) -> Result<(), String> {
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
pub(crate) fn window_scratchpad_via_sa(
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
pub(crate) fn window_toggle_scratchpad_via_sa(
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
pub(crate) fn window_toggle_sticky_via_sa(
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

pub(crate) fn window_set_sticky_via_sa(
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
pub(crate) fn window_toggle_shadow_via_sa(
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
pub(crate) fn window_toggle_pip_via_sa(
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
pub(crate) fn space_to_display_via_sa(
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
pub(crate) fn prev_space_on_display(did: u32, sid: u64) -> u64 {
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
pub(crate) fn space_move_via_sa(
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
pub(crate) fn space_swap_via_sa(
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
pub(crate) fn space_swap_cross_display(
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
pub(crate) fn space_switch_via_sa(
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
pub(crate) fn global_prev_space(order: &[u64], sid: u64) -> Option<u64> {
    match order.iter().position(|&s| s == sid) {
        Some(index) if index > 0 => Some(order[index - 1]),
        _ => None,
    }
}

/// The 1-based mission-control index of `sid` in the global order (`0` if absent),
/// matching the C `space_manager_mission_control_index`.
pub(crate) fn mission_control_index(order: &[u64], sid: u64) -> usize {
    order
        .iter()
        .position(|&s| s == sid)
        .map_or(0, |index| index + 1)
}
