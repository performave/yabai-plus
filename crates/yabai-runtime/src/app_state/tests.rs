use super::*;
fn toks(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn state_with_space() -> AppState {
    let mut state = AppState::new();
    state.add_space(1, Area::new(0.0, 0.0, 1000.0, 1000.0));
    state
}

fn state_with_displays() -> AppState {
    let mut state = AppState::new();
    state.add_display(42, Area::new(0.0, 0.0, 1440.0, 900.0));
    state.add_display(77, Area::new(1440.0, 0.0, 1280.0, 720.0));
    state.add_space_to_display(1, 42, Area::new(0.0, 0.0, 1440.0, 900.0));
    state.add_space_to_display(2, 77, Area::new(1440.0, 0.0, 1280.0, 720.0));
    state
}

#[test]
fn display_arrangement_order_reorders_index_and_selector() {
    // Display 100 sits on the right, 200 on the left — id order is the reverse
    // of spatial (x) order, so the arrangement setting actually changes indices.
    let mut state = AppState::new();
    state.add_display(100, Area::new(1500.0, 0.0, 1500.0, 1000.0));
    state.add_display(200, Area::new(0.0, 500.0, 1500.0, 1000.0));

    // Default: id-ascending.
    assert_eq!(state.display_ids(), vec![100, 200]);
    assert_eq!(state.display_index(100), Some(1));

    // Horizontal: by center x — the left display (200) becomes index 1.
    state
        .handle_tokens(&toks(&[
            "config",
            "display_arrangement_order",
            "horizontal",
        ]))
        .unwrap();
    assert_eq!(state.display_ids(), vec![200, 100]);
    assert_eq!(state.display_index(200), Some(1));
    // The numeric selector now resolves index 1 to the left display.
    assert_eq!(
        state.handle_tokens(&toks(&["display", "1", "--label", "leftmost"])),
        Ok(None)
    );
    assert_eq!(
        state.resolve_display(Some(&Selector::Label("leftmost".into()))),
        Ok(200)
    );

    // Vertical: by center y — display 100 (y=500) is above 200 (y=1000).
    state
        .handle_tokens(&toks(&["config", "display_arrangement_order", "vertical"]))
        .unwrap();
    assert_eq!(state.display_ids(), vec![100, 200]);
}

#[test]
fn toggle_float_untiles_then_retiles() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(1));

    // Float 1: it leaves the tree, 2 fills the space, 1 is not captured.
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--toggle", "float"])),
        Ok(None)
    );
    assert!(state.is_floating(1));
    assert_eq!(state.space(1).unwrap().window_list(), vec![2]);
    assert!(state.flush(1).unwrap().iter().all(|f| f.window_id != 1));

    // A reconcile re-assignment must stay a no-op while floating.
    state.assign_window_to_space(1, 1).unwrap();
    assert_eq!(state.space(1).unwrap().window_list(), vec![2]);

    // Toggle off: 1 tiles back in.
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--toggle", "float"])),
        Ok(None)
    );
    assert!(!state.is_floating(1));
    let mut list = state.space(1).unwrap().window_list();
    list.sort_unstable();
    assert_eq!(list, vec![1, 2]);
}

#[test]
fn scratchpad_assignment_floats_and_removal_retiles() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();

    state
        .set_window_scratchpad(1, "notes".to_string(), 1)
        .unwrap();
    assert_eq!(state.window_scratchpad(1), Some("notes"));
    assert_eq!(state.scratchpad_window("notes"), Some(1));
    assert!(state.is_floating(1));
    assert_eq!(state.space(1).unwrap().window_list(), vec![2]);

    assert_eq!(
        state.set_window_scratchpad(2, "notes".to_string(), 1),
        Err("the given scratchpad is already assigned to a different window!\n".to_string())
    );

    assert!(state.remove_window_scratchpad(1, true, 1));
    assert_eq!(state.window_scratchpad(1), None);
    assert!(!state.is_floating(1));
    let mut list = state.space(1).unwrap().window_list();
    list.sort_unstable();
    assert_eq!(list, vec![1, 2]);
}

#[test]
fn window_area_returns_the_captured_frame() {
    let mut state = state_with_space();
    // A lone window fills the whole space; its area is what the daemon centers
    // the cursor on for mouse_follows_focus.
    state.add_window(1).unwrap();
    assert_eq!(
        state.window_area(1),
        Some(Area::new(0.0, 0.0, 1000.0, 1000.0))
    );
    assert_eq!(state.window_area(999), None);
}

#[test]
fn minimize_requires_a_focused_window() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    // Focused after add: minimize validates and succeeds (macOS effect is the
    // daemon's job, so the pure layer is a no-op that leaves the tree intact).
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--minimize"])),
        Ok(None)
    );
    assert_eq!(state.space(1).unwrap().window_list(), vec![1]);
    state.add_window(2).unwrap();
    state.set_focused_window(Some(1));
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--minimize", "last"])),
        Ok(None)
    );
    assert_eq!(state.focused_window, Some(2));
    // With nothing focused, it reports the same error as other window ops.
    state.set_focused_window(None);
    assert!(
        state
            .handle_tokens(&toks(&["window", "--minimize"]))
            .is_err()
    );
}

#[test]
fn native_fullscreen_toggle_requires_a_focused_window() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    // Focused after add: the toggle validates and is a pure no-op (the daemon
    // sets AXFullscreen and reconcile drops the window from the tree).
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--toggle", "native-fullscreen"])),
        Ok(None)
    );
    assert_eq!(state.space(1).unwrap().window_list(), vec![1]);
    // With nothing focused (e.g. the window already left for its fullscreen
    // space), the pure layer errors and the daemon's exit intercept takes over.
    state.set_focused_window(None);
    assert!(
        state
            .handle_tokens(&toks(&["window", "--toggle", "native-fullscreen"]))
            .is_err()
    );
}

#[test]
fn close_requires_a_focused_or_selected_window() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(None);

    assert_eq!(
        state.handle_tokens(&toks(&["window", "--close"])),
        Err("no focused window".to_string())
    );

    assert_eq!(
        state.handle_tokens(&toks(&["window", "1", "--close"])),
        Ok(None)
    );
    assert_eq!(state.focused_window, Some(1));

    assert_eq!(
        state.handle_tokens(&toks(&["window", "--close", "last"])),
        Ok(None)
    );
    assert_eq!(state.focused_window, Some(2));
}

#[test]
fn leading_window_selector_sets_the_acting_window() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(2));

    assert_eq!(
        state.handle_tokens(&toks(&["window", "1", "--minimize"])),
        Ok(None)
    );
    assert_eq!(state.focused_window, Some(1));

    assert_eq!(
        state.handle_tokens(&toks(&["window", "2", "--focus"])),
        Ok(None)
    );
    assert_eq!(state.focused_window, Some(2));
}

#[test]
fn destroying_a_floating_window_clears_the_mark() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(1));
    state
        .handle_tokens(&toks(&["window", "--toggle", "float"]))
        .unwrap();
    assert!(state.is_floating(1));
    state.remove_window(1).unwrap();
    assert!(!state.is_floating(1));
}

#[test]
fn config_set_then_get_roundtrips() {
    let mut state = AppState::new();
    assert_eq!(
        state.handle_tokens(&toks(&["config", "window_gap", "8"])),
        Ok(None)
    );
    assert_eq!(
        state.handle_tokens(&toks(&["config", "window_gap"])),
        Ok(Some("8\n".to_string()))
    );
    assert_eq!(state.config.window_gap, 8);
}

#[test]
fn config_get_layout_matches_c_string() {
    let mut state = AppState::new();
    assert_eq!(
        state.handle_tokens(&toks(&["config", "layout"])),
        Ok(Some("bsp\n".to_string()))
    );
    assert_eq!(
        state.handle_tokens(&toks(&["config", "auto_balance"])),
        Ok(Some("off\n".to_string()))
    );
}

#[test]
fn config_gap_change_reflows_spaces() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    // Apply a gap; the divider should leave a 10px gap between halves.
    state
        .handle_tokens(&toks(&["config", "window_gap", "10"]))
        .unwrap();
    let tree = state.space(1).unwrap();
    let leaves = tree.leaves();
    let left = tree.node(leaves[0]).area;
    let right = tree.node(leaves[1]).area;
    // left ends at ~495, right starts at ~505 -> a 10px gap.
    assert!((right.x - (left.x + left.w)) as i32 >= 9);
}

#[test]
fn space_gap_dispatch_sets_and_adjusts() {
    let mut state = state_with_space();
    // Absolute set.
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--gap", "abs:10"])),
        Ok(None)
    );
    assert_eq!(state.space(1).unwrap().config.gap, 10);
    // Relative adjust accumulates.
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--gap", "rel:5"])),
        Ok(None)
    );
    assert_eq!(state.space(1).unwrap().config.gap, 15);
    // Relative adjust clamps to zero (C add_and_clamp_to_zero).
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--gap", "rel:-100"])),
        Ok(None)
    );
    assert_eq!(state.space(1).unwrap().config.gap, 0);
}

#[test]
fn space_padding_dispatch_reinsets_from_usable_frame() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    // Seed the usable frame so the per-space padding re-insets immediately
    // (mirrors the daemon handing each space its usable frame on reconcile).
    state
        .set_space_frame(1, Area::new(0.0, 0.0, 1000.0, 1000.0))
        .unwrap();
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--padding", "abs:20:20:10:10"])),
        Ok(None)
    );
    let frame = state.flush(1).unwrap()[0].area;
    // Single window fills the padded area: x=left, y=top, w=1000-l-r, h=1000-t-b.
    assert_eq!(
        (
            frame.x as i32,
            frame.y as i32,
            frame.w as i32,
            frame.h as i32
        ),
        (10, 20, 980, 960)
    );
    // Relative adds to the current override (10 more on left/right each).
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--padding", "rel:0:0:10:10"])),
        Ok(None)
    );
    let frame = state.flush(1).unwrap()[0].area;
    assert_eq!((frame.x as i32, frame.w as i32), (20, 960));
}

#[test]
fn space_gap_and_padding_error_on_float_space() {
    let mut state = state_with_space();
    state.spaces.get_mut(&1).unwrap().layout = ViewType::Float;
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--gap", "abs:10"])),
        Err("cannot set gap for a non-managed space.".to_string())
    );
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--padding", "abs:1:1:1:1"])),
        Err("cannot set padding for a non-managed space.".to_string())
    );
}

#[test]
fn space_toggle_padding_zeros_and_restores() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state
        .set_space_frame(1, Area::new(0.0, 0.0, 1000.0, 1000.0))
        .unwrap();
    state
        .handle_tokens(&toks(&["space", "--padding", "abs:20:20:10:10"]))
        .unwrap();
    assert_eq!(state.flush(1).unwrap()[0].area.x as i32, 10);
    // Toggle off: padding is ignored, the window fills the raw usable frame.
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--toggle", "padding"])),
        Ok(None)
    );
    let frame = state.flush(1).unwrap()[0].area;
    assert_eq!(
        (
            frame.x as i32,
            frame.y as i32,
            frame.w as i32,
            frame.h as i32
        ),
        (0, 0, 1000, 1000)
    );
    // Toggle on: the override applies again.
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--toggle", "padding"])),
        Ok(None)
    );
    assert_eq!(state.flush(1).unwrap()[0].area.x as i32, 10);
}

#[test]
fn space_toggle_gap_zeros_and_restores_preserving_value() {
    let mut state = state_with_space();
    state
        .handle_tokens(&toks(&["space", "--gap", "abs:12"]))
        .unwrap();
    assert_eq!(state.space(1).unwrap().config.gap, 12);
    // Toggle off: the live gap drops to zero but the value is remembered.
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--toggle", "gap"])),
        Ok(None)
    );
    assert_eq!(state.space(1).unwrap().config.gap, 0);
    // `space --gap` while toggled off updates the saved value, not the live one.
    state
        .handle_tokens(&toks(&["space", "--gap", "abs:30"]))
        .unwrap();
    assert_eq!(state.space(1).unwrap().config.gap, 0);
    // Toggle on: the updated saved gap is restored.
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--toggle", "gap"])),
        Ok(None)
    );
    assert_eq!(state.space(1).unwrap().config.gap, 30);
}

#[test]
fn space_toggle_gap_padding_error_on_float_and_unknown_value() {
    let mut state = state_with_space();
    state.spaces.get_mut(&1).unwrap().layout = ViewType::Float;
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--toggle", "gap"])),
        Err("cannot toggle gap for a non-managed space.".to_string())
    );
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--toggle", "padding"])),
        Err("cannot toggle padding for a non-managed space.".to_string())
    );
}

#[test]
fn window_swap_reorders_active_space() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    // Focus is on window 2 (last added); swap it with window 1.
    state.set_focused_window(Some(2));
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--swap", "1"])),
        Ok(None)
    );
    let tree = state.space(1).unwrap();
    assert_eq!(tree.window_list(), vec![2, 1]);
}

#[test]
fn window_swap_uses_focused_windows_own_space() {
    // Windows live on space 2 while space 1 is active. Before the own-space fix,
    // --swap acted on the (empty) active space 1 and silently did nothing.
    let mut state = state_with_space();
    state.add_space(2, Area::new(0.0, 0.0, 1000.0, 1000.0));
    state.set_active_space(2);
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(2));
    state.set_active_space(1);

    assert_eq!(
        state.handle_tokens(&toks(&["window", "--swap", "1"])),
        Ok(None)
    );
    assert_eq!(state.space(2).unwrap().window_list(), vec![2, 1]);
}

#[test]
fn first_window_on_space_returns_first_or_none() {
    // Backs the daemon's `display --focus`, which focuses the first window on
    // the destination display's active space (else warps the cursor).
    let mut state = state_with_space();
    assert_eq!(state.first_window_on_space(1), None);
    state.add_window(7).unwrap();
    state.add_window(9).unwrap();
    assert_eq!(state.first_window_on_space(1), Some(7));
    // An unknown space has no window.
    assert_eq!(state.first_window_on_space(999), None);
}

#[test]
fn window_swap_without_focus_errors() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.set_focused_window(None);
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--swap", "1"])),
        Err("no focused window".to_string())
    );
}

#[test]
fn window_stack_moves_focused_window_into_target_leaf() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.add_window(3).unwrap();
    state.set_focused_window(Some(1));

    assert_eq!(
        state.handle_tokens(&toks(&["window", "--stack", "3"])),
        Ok(None)
    );
    let tree = state.space(1).unwrap();
    let target = tree.find_window_node(3).unwrap();
    assert_eq!(tree.node(target).window_list, vec![3, 1]);
    assert_eq!(tree.node(target).window_order[0], 1);
}

#[test]
fn window_toggle_split_flips_parent_axis() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(2));

    // The two windows share a parent split node; `--toggle split` flips its
    // axis, and a second toggle restores it.
    let before = state.window_split_info(2).0;
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--toggle", "split"])),
        Ok(None)
    );
    let after = state.window_split_info(2).0;
    assert_ne!(before, after);
    assert!(matches!(after, "vertical" | "horizontal"));

    assert_eq!(
        state.handle_tokens(&toks(&["window", "--toggle", "split"])),
        Ok(None)
    );
    assert_eq!(state.window_split_info(2).0, before);
}

#[test]
fn window_toggle_split_is_noop_on_root_window() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.set_focused_window(Some(1));

    // A lone (root) window has no parent split; the toggle is a silent no-op.
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--toggle", "split"])),
        Ok(None)
    );
    assert_eq!(state.window_split_info(1).0, "none");
}

#[test]
fn window_ratio_dispatches_and_errors_on_root() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(1));
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--ratio", "abs:0.7"])),
        Ok(None)
    );

    // A lone window is a root node with no parent ratio to adjust.
    let mut single = state_with_space();
    single.add_window(1).unwrap();
    single.set_focused_window(Some(1));
    assert_eq!(
        single.handle_tokens(&toks(&["window", "--ratio", "rel:0.1"])),
        Err("cannot adjust ratio of a root node.".to_string())
    );
}

#[test]
fn window_ratio_uses_focused_windows_own_space() {
    // Two windows live on space 2 while space 1 is active — `--ratio` must act on
    // the focused window's own view, not the active space (which lacks it).
    let mut state = state_with_space();
    state.add_space(2, Area::new(0.0, 0.0, 1000.0, 1000.0));
    state.set_active_space(2);
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(1));
    state.set_active_space(1);

    assert_eq!(
        state.handle_tokens(&toks(&["window", "--ratio", "abs:0.7"])),
        Ok(None)
    );
}

#[test]
fn window_resize_managed_rejects_abs_and_adjusts_fence() {
    // Two windows split the space; resizing the left one's right fence moves
    // the divider. The window's *own* space is used, not the active one.
    let mut state = state_with_space();
    state.add_space(2, Area::new(0.0, 0.0, 1000.0, 1000.0));
    state.set_active_space(2);
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(1));
    state.set_active_space(1);

    let before = state
        .space(2)
        .unwrap()
        .capture()
        .iter()
        .find(|f| f.window_id == 1)
        .unwrap()
        .area
        .w;
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--resize", "right:100:0"])),
        Ok(None)
    );
    let after = state
        .space(2)
        .unwrap()
        .capture()
        .iter()
        .find(|f| f.window_id == 1)
        .unwrap()
        .area
        .w;
    assert!(after > before, "moving the right fence widens window 1");

    // Absolute resizing of a managed window is rejected with the C string.
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--resize", "abs:400:400"])),
        Err("cannot use absolute resizing on a managed window.\n".to_string())
    );
}

#[test]
fn window_resize_managed_without_fence_reports_error() {
    // A lone window is a root node: no fence can move in any direction.
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.set_focused_window(Some(1));
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--resize", "right:100:0"])),
        Err("cannot locate a bsp node fence.\n".to_string())
    );
}

#[test]
fn window_resize_unmanaged_is_a_pure_noop() {
    // An unmanaged (never-added) focused window has no tree; the pure model
    // leaves it to the macOS layer and does not error.
    let mut state = state_with_space();
    state.set_focused_window(Some(99));
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--resize", "abs:400:400"])),
        Ok(None)
    );
}

#[test]
fn window_insert_sets_direction_and_reports_errors() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.set_focused_window(Some(1));

    // A valid direction dispatches, and the next window lands east of window 1.
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--insert", "east"])),
        Ok(None)
    );
    state.add_window(2).unwrap();
    let cap = state.space(1).unwrap().capture();
    let x1 = cap.iter().find(|f| f.window_id == 1).unwrap().area.x;
    let x2 = cap.iter().find(|f| f.window_id == 2).unwrap().area.x;
    assert!(x2 > x1, "new window should be east of the target");

    // Invalid directions now fail during typed command parsing.
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--insert", "sideways"])),
        Err("unknown value 'sideways' given to command '--insert' for domain 'window'".to_string())
    );
}

#[test]
fn window_focus_directional_and_relative_selectors_resolve() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    // Focus window 1, then focus east -> window 2 (the right half).
    state.set_focused_window(Some(1));
    state
        .handle_tokens(&toks(&["window", "--focus", "east"]))
        .unwrap();
    assert_eq!(state.focused_window, Some(2));

    // prev from window 2 (tree order [1, 2]) -> window 1.
    state
        .handle_tokens(&toks(&["window", "--focus", "prev"]))
        .unwrap();
    assert_eq!(state.focused_window, Some(1));
}

#[test]
fn window_mouse_selector_resolves_window_under_cursor() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(2));

    // Two windows split the 1000x1000 space; window 1 is the left half.
    // A cursor in the left half resolves `mouse` to window 1.
    state.set_cursor_point(Point { x: 100.0, y: 500.0 });
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--focus", "mouse"])),
        Ok(None)
    );
    assert_eq!(state.focused_window_id(), Some(1));

    // With no window under the cursor, `mouse` fails with a clear message.
    state.set_cursor_point(Point {
        x: 5000.0,
        y: 5000.0,
    });
    let err = state
        .handle_tokens(&toks(&["window", "--focus", "mouse"]))
        .unwrap_err();
    assert!(err.contains("under the cursor"));
}

#[test]
fn space_rotate_and_layout() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    let before = state
        .space(1)
        .unwrap()
        .node(state.space(1).unwrap().root())
        .ratio;
    state
        .handle_tokens(&toks(&["space", "--rotate", "180"]))
        .unwrap();
    let after = state
        .space(1)
        .unwrap()
        .node(state.space(1).unwrap().root())
        .ratio;
    assert!((before + after - 1.0).abs() < 1e-6);

    state
        .handle_tokens(&toks(&["space", "--layout", "stack"]))
        .unwrap();
    assert_eq!(state.space(1).unwrap().layout, ViewType::Stack);
}

#[test]
fn flush_returns_a_frame_per_window() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    let frames = state.flush_active().unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].window_id, 1);
    assert_eq!(frames[1].window_id, 2);
}

#[test]
fn set_space_frame_insets_by_padding() {
    let mut state = AppState::new();
    state.config.top_padding = 20;
    state.config.bottom_padding = 20;
    state.config.left_padding = 10;
    state.config.right_padding = 10;
    state.add_space(1, Area::new(0.0, 0.0, 1000.0, 1000.0));
    state.add_window(1).unwrap();
    state
        .set_space_frame(1, Area::new(0.0, 0.0, 1000.0, 1000.0))
        .unwrap();
    let frame = state.flush(1).unwrap()[0].area;
    // Single window fills the padded area: 10,20 .. 980x960.
    assert_eq!(frame.x as i32, 10);
    assert_eq!(frame.y as i32, 20);
    assert_eq!(frame.w as i32, 980);
    assert_eq!(frame.h as i32, 960);
}

#[test]
fn numeric_space_selector_is_a_mission_control_index() {
    // Regression for the live-found bug: `window --space 2` targeted raw sid 2
    // instead of the 2nd mission-control space (a real sid like 18).
    let mut state = state_with_space();
    // Without a pushed order, a numeric selector falls back to the raw sid (the
    // pure-test convention where small sids double as indices).
    assert_eq!(state.resolve_space(Some(&Selector::Index(1))), Ok(1));

    // With the live order pushed, index 2 resolves to the 2nd space's sid.
    state.set_mission_control_order(vec![1, 18]);
    assert_eq!(state.resolve_space(Some(&Selector::Index(1))), Ok(1));
    assert_eq!(state.resolve_space(Some(&Selector::Index(2))), Ok(18));
    // Out-of-range index errors like C.
    assert!(state.resolve_space(Some(&Selector::Index(3))).is_err());
}

#[test]
fn window_origin_display_routes_new_windows() {
    // Two displays: space 1 on display 10 (active), space 2 on display 20.
    let mut state = AppState::new();
    state.add_display(10, Area::new(0.0, 0.0, 1000.0, 1000.0));
    state.add_display(20, Area::new(1000.0, 0.0, 1000.0, 1000.0));
    state.add_space_to_display(1, 10, Area::new(0.0, 0.0, 1000.0, 1000.0));
    state.add_space_to_display(2, 20, Area::new(1000.0, 0.0, 1000.0, 1000.0));
    state.set_display_active_space(10, 1);
    state.set_display_active_space(20, 2);
    state.set_active_space(1);
    let cursor = Some(Point {
        x: 1500.0,
        y: 500.0,
    }); // over display 20 (space 2)

    // default: keeps the window's physical space.
    assert_eq!(state.origin_space_for_new_window(2, cursor), 2);

    // focused: routes to the active space (1), regardless of physical space.
    state
        .handle_tokens(&toks(&["config", "window_origin_display", "focused"]))
        .unwrap();
    assert_eq!(state.origin_space_for_new_window(2, cursor), 1);

    // cursor: routes to the space under the cursor (2), even though active is 1.
    state
        .handle_tokens(&toks(&["config", "window_origin_display", "cursor"]))
        .unwrap();
    assert_eq!(state.origin_space_for_new_window(1, cursor), 2);
    // cursor off every display falls back to the physical space.
    assert_eq!(state.origin_space_for_new_window(1, None), 1);
}

#[test]
fn external_bar_reserves_space_and_scopes_to_mode() {
    // Two displays; space 1 on display 10, space 2 on display 20 (the main one).
    let mut state = AppState::new();
    state.add_display(10, Area::new(0.0, 0.0, 1000.0, 1000.0));
    state.add_display(20, Area::new(1000.0, 0.0, 1000.0, 1000.0));
    state.add_space_to_display(1, 10, Area::new(0.0, 0.0, 1000.0, 1000.0));
    state.add_space_to_display(2, 20, Area::new(1000.0, 0.0, 1000.0, 1000.0));
    state.set_main_display(20);
    state.add_window(1).unwrap(); // lands on the active space (1)
    state.set_active_space(2);
    state.add_window(2).unwrap();
    state
        .set_space_frame(1, Area::new(0.0, 0.0, 1000.0, 1000.0))
        .unwrap();
    state
        .set_space_frame(2, Area::new(1000.0, 0.0, 1000.0, 1000.0))
        .unwrap();

    // `all`: both displays reserve 30 top / 10 bottom.
    state
        .handle_tokens(&toks(&["config", "external_bar", "all:30:10"]))
        .unwrap();
    let f1 = state.flush(1).unwrap()[0].area;
    assert_eq!((f1.y as i32, f1.h as i32), (30, 960));
    let f2 = state.flush(2).unwrap()[0].area;
    assert_eq!((f2.y as i32, f2.h as i32), (30, 960));

    // `main`: only display 20 (space 2) reserves; space 1 is full-height again.
    state
        .handle_tokens(&toks(&["config", "external_bar", "main:30:10"]))
        .unwrap();
    let f1 = state.flush(1).unwrap()[0].area;
    assert_eq!((f1.y as i32, f1.h as i32), (0, 1000));
    let f2 = state.flush(2).unwrap()[0].area;
    assert_eq!((f2.y as i32, f2.h as i32), (30, 960));

    // `off`: no reservation anywhere.
    state
        .handle_tokens(&toks(&["config", "external_bar", "off:0:0"]))
        .unwrap();
    let f2 = state.flush(2).unwrap()[0].area;
    assert_eq!((f2.y as i32, f2.h as i32), (0, 1000));
}

#[test]
fn events_drive_state_end_to_end() {
    let mut state = AppState::new();
    state
        .handle_event(StateEvent::SpaceCreated {
            sid: 1,
            frame: Area::new(0.0, 0.0, 1000.0, 1000.0),
        })
        .unwrap();
    state
        .handle_event(StateEvent::WindowCreated { window_id: 1 })
        .unwrap();
    state
        .handle_event(StateEvent::WindowCreated { window_id: 2 })
        .unwrap();
    // Two windows tiled; flush yields a frame for each.
    assert_eq!(state.flush_active().unwrap().len(), 2);

    state
        .handle_event(StateEvent::WindowFocused { window_id: 1 })
        .unwrap();
    assert_eq!(state.focused_window, Some(1));

    state
        .handle_event(StateEvent::WindowDestroyed { window_id: 2 })
        .unwrap();
    assert_eq!(state.space(1).unwrap().window_list(), vec![1]);
}

#[test]
fn window_assignment_targets_specific_space() {
    let mut state = state_with_displays();
    state.set_active_space(1);
    state
        .handle_event(StateEvent::WindowAssignedToSpace {
            window_id: 20,
            sid: 2,
        })
        .unwrap();

    assert!(state.space(1).unwrap().window_list().is_empty());
    assert_eq!(state.space(2).unwrap().window_list(), vec![20]);
    assert_eq!(state.window_space_id(20), Some(2));
    assert!(state.flush_active().unwrap().is_empty());
}

#[test]
fn window_assignment_moves_between_spaces_without_refocusing() {
    let mut state = state_with_displays();
    state.set_active_space(1);
    state
        .handle_event(StateEvent::WindowAssignedToSpace {
            window_id: 10,
            sid: 1,
        })
        .unwrap();
    state
        .handle_event(StateEvent::WindowAssignedToSpace {
            window_id: 20,
            sid: 1,
        })
        .unwrap();
    state.set_focused_window(Some(10));

    state
        .handle_event(StateEvent::WindowAssignedToSpace {
            window_id: 20,
            sid: 2,
        })
        .unwrap();

    assert_eq!(state.space(1).unwrap().window_list(), vec![10]);
    assert_eq!(state.space(2).unwrap().window_list(), vec![20]);
    assert_eq!(state.focused_window, Some(10));
}

#[test]
fn space_removed_drops_tree_and_active_space() {
    let mut state = state_with_displays();
    state.set_active_space(2);
    state.set_display_active_space(77, 2);
    state
        .handle_event(StateEvent::WindowAssignedToSpace {
            window_id: 20,
            sid: 2,
        })
        .unwrap();

    state
        .handle_event(StateEvent::SpaceRemoved { sid: 2 })
        .unwrap();

    assert!(state.space(2).is_none());
    assert_eq!(state.space_ids_for_display(77), Vec::<u64>::new());
    assert_eq!(state.active_space_id(), Some(1));
    assert_eq!(state.display_active_space_id(77), None);
    assert_eq!(state.focused_window, None);
}

#[test]
fn rediscovered_space_on_new_display_preserves_tree() {
    let mut state = state_with_displays();
    state.set_active_space(1);
    state
        .handle_event(StateEvent::WindowAssignedToSpace {
            window_id: 10,
            sid: 1,
        })
        .unwrap();
    state
        .handle_event(StateEvent::WindowAssignedToSpace {
            window_id: 20,
            sid: 1,
        })
        .unwrap();

    state.add_space_to_display(1, 77, Area::new(1440.0, 0.0, 1280.0, 720.0));
    state
        .set_space_frame(1, Area::new(1440.0, 0.0, 1280.0, 720.0))
        .unwrap();

    assert_eq!(state.space_ids_for_display(42), Vec::<u64>::new());
    assert_eq!(state.space_ids_for_display(77), vec![1, 2]);
    assert_eq!(state.space(1).unwrap().window_list(), vec![10, 20]);
    assert_eq!(state.flush(1).unwrap()[0].area.x as i32, 1440);
}

#[test]
fn display_removed_clears_active_display_space() {
    let mut state = state_with_displays();
    state.set_display_active_space(77, 2);

    state.remove_display(77);

    assert_eq!(state.display_ids(), vec![42]);
    assert_eq!(state.display_active_space_id(77), None);
    assert_eq!(state.space_ids_for_display(77), Vec::<u64>::new());
}

#[test]
fn flush_through_sink_records_moves() {
    let mut state = AppState::new();
    let mut sink = RecordingSink::default();
    state
        .handle_event_and_flush(
            StateEvent::SpaceCreated {
                sid: 1,
                frame: Area::new(0.0, 0.0, 1000.0, 1000.0),
            },
            &mut sink,
        )
        .unwrap();
    state
        .handle_event_and_flush(StateEvent::WindowCreated { window_id: 1 }, &mut sink)
        .unwrap();
    let placed = state
        .handle_event_and_flush(StateEvent::WindowCreated { window_id: 2 }, &mut sink)
        .unwrap();
    // The last flush placed both windows.
    assert_eq!(placed, 2);
    // The recorded moves end with the two-window layout.
    let last_two = &sink.moves[sink.moves.len() - 2..];
    assert_eq!(last_two[0].window_id, 1);
    assert_eq!(last_two[1].window_id, 2);
    assert_ne!(last_two[0].area, last_two[1].area);
}

#[test]
fn window_created_ignored_when_manage_off() {
    let mut state = state_with_space();
    state.config.manage = false;
    state
        .handle_event(StateEvent::WindowCreated { window_id: 1 })
        .unwrap();
    assert!(state.flush_active().unwrap().is_empty());
}

#[test]
fn display_frame_change_reinsets_with_padding() {
    let mut state = AppState::new();
    state.config.left_padding = 10;
    state
        .handle_event(StateEvent::SpaceCreated {
            sid: 1,
            frame: Area::new(0.0, 0.0, 1000.0, 1000.0),
        })
        .unwrap();
    state
        .handle_event(StateEvent::WindowCreated { window_id: 1 })
        .unwrap();
    state
        .handle_event(StateEvent::DisplayFrameChanged {
            sid: 1,
            frame: Area::new(0.0, 0.0, 800.0, 600.0),
        })
        .unwrap();
    let frame = state.flush(1).unwrap()[0].area;
    assert_eq!(frame.x as i32, 10);
    assert_eq!(frame.w as i32, 790);
}

#[test]
fn query_windows_serializes_c_style_json() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(2));

    assert_eq!(
            state.handle_tokens(&toks(&["query", "--windows", "id,frame,has-focus"])),
            Ok(Some(
                "[{\n\t\"id\":1,\n\t\"frame\":{\n\t\t\"x\":0.0000,\n\t\t\"y\":0.0000,\n\t\t\"w\":500.0000,\n\t\t\"h\":1000.0000\n\t},\n\t\"has-focus\":false\n},{\n\t\"id\":2,\n\t\"frame\":{\n\t\t\"x\":500.0000,\n\t\t\"y\":0.0000,\n\t\t\"w\":500.0000,\n\t\t\"h\":1000.0000\n\t},\n\t\"has-focus\":true\n}]\n".to_string()
            ))
        );
}

#[test]
fn query_windows_serializes_space_display_and_tree_properties() {
    let mut state = state_with_displays();
    state.set_active_space(1);
    state.add_window(10).unwrap();
    state.add_window(20).unwrap();

    let out = state
        .handle_tokens(&toks(&[
            "query",
            "--windows",
            "id,space,display,is-visible,split-type,split-child,has-fullscreen-zoom",
            "--space",
            "1",
        ]))
        .unwrap()
        .unwrap();
    assert_eq!(
        out,
        "[{\n\t\"id\":10,\n\t\"space\":1,\n\t\"display\":1,\n\t\"is-visible\":true,\n\t\"split-type\":\"vertical\",\n\t\"split-child\":\"first_child\",\n\t\"has-fullscreen-zoom\":false\n},{\n\t\"id\":20,\n\t\"space\":1,\n\t\"display\":1,\n\t\"is-visible\":true,\n\t\"split-type\":\"vertical\",\n\t\"split-child\":\"second_child\",\n\t\"has-fullscreen-zoom\":false\n}]\n"
    );
}

#[test]
fn query_windows_serializes_stack_index() {
    let mut state = state_with_space();
    state
        .handle_tokens(&toks(&["space", "--layout", "stack"]))
        .unwrap();
    state.add_window(10).unwrap();
    state.add_window(20).unwrap();
    state.add_window(30).unwrap();

    assert_eq!(
            state.handle_tokens(&toks(&["query", "--windows", "id,stack-index"])),
            Ok(Some(
                "[{\n\t\"id\":10,\n\t\"stack-index\":1\n},{\n\t\"id\":20,\n\t\"stack-index\":2\n},{\n\t\"id\":30,\n\t\"stack-index\":3\n}]\n"
                    .to_string()
            ))
        );
}

#[test]
fn query_windows_serializes_opacity_from_live_info() {
    // The daemon pushes live AX/SkyLight info before a query; the pure serializer
    // reports it. Absent info defaults to opacity 0.
    let mut state = state_with_space();
    state.add_window(10).unwrap();
    state.add_window(20).unwrap();
    state.set_window_live_info(
        10,
        LiveWindowInfo {
            opacity: 0.55,
            ..Default::default()
        },
    );
    assert_eq!(
        state.handle_tokens(&toks(&["query", "--windows", "id,opacity"])),
        Ok(Some(
            "[{\n\t\"id\":10,\n\t\"opacity\":0.5500\n},{\n\t\"id\":20,\n\t\"opacity\":0.0000\n}]\n"
                .to_string()
        ))
    );
}

#[test]
fn query_windows_serializes_role_subrole_and_can_flags() {
    let mut state = state_with_space();
    state.add_window(10).unwrap();
    state.set_window_live_info(
        10,
        LiveWindowInfo {
            role: "AXWindow".into(),
            subrole: "AXStandardWindow".into(),
            can_move: true,
            can_resize: false,
            ..Default::default()
        },
    );
    assert_eq!(
        state.handle_tokens(&toks(&[
            "query",
            "--windows",
            "id,role,subrole,can-move,can-resize"
        ])),
        Ok(Some(
            "[{\n\t\"id\":10,\n\t\"role\":\"AXWindow\",\n\t\"subrole\":\"AXStandardWindow\",\n\t\"can-move\":true,\n\t\"can-resize\":false\n}]\n"
                .to_string()
        ))
    );
}

#[test]
fn query_windows_serializes_is_floating_and_is_sticky() {
    // Tiled windows report both as false; the fields are now requestable instead
    // of erroring (C never rejects is-floating/is-sticky). Floating/sticky windows
    // themselves leave the tree, so they don't appear in this tree-based query.
    let mut state = state_with_space();
    state.add_window(10).unwrap();
    assert_eq!(
        state.handle_tokens(&toks(&["query", "--windows", "id,is-floating,is-sticky"])),
        Ok(Some(
            "[{\n\t\"id\":10,\n\t\"is-floating\":false,\n\t\"is-sticky\":false\n}]\n".to_string()
        ))
    );
}

#[test]
fn query_windows_serializes_has_shadow() {
    let mut state = state_with_space();
    state.add_window(10).unwrap();
    state.add_window(20).unwrap();
    state.set_window_shadow(20, false);

    assert_eq!(
            state.handle_tokens(&toks(&["query", "--windows", "id,has-shadow"])),
            Ok(Some(
                "[{\n\t\"id\":10,\n\t\"has-shadow\":true\n},{\n\t\"id\":20,\n\t\"has-shadow\":false\n}]\n"
                    .to_string()
            ))
        );
}

#[test]
fn query_windows_serializes_scratchpad_property() {
    let mut state = state_with_space();
    state.add_window(10).unwrap();

    assert_eq!(
        state.handle_tokens(&toks(&["query", "--windows", "id,scratchpad"])),
        Ok(Some(
            "[{\n\t\"id\":10,\n\t\"scratchpad\":\"\"\n}]\n".to_string()
        ))
    );
}

#[test]
fn query_spaces_serializes_supported_properties() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();

    assert_eq!(
            state.handle_tokens(&toks(&[
                "query",
                "--spaces",
                "id,type,windows,first-window,last-window,has-focus,is-visible",
                "--space",
                "1",
            ])),
            Ok(Some(
                "{\n\t\"id\":1,\n\t\"type\":\"bsp\",\n\t\"windows\":[1, 2],\n\t\"first-window\":1,\n\t\"last-window\":2,\n\t\"has-focus\":true,\n\t\"is-visible\":true\n}\n".to_string()
            ))
        );
}

#[test]
fn display_label_set_select_and_validation() {
    let mut state = state_with_displays();
    state.set_active_space(1);

    // Label display 2 (arrangement index 2, id 77) and resolve it back.
    assert_eq!(
        state.handle_tokens(&toks(&["display", "2", "--label", "side"])),
        Ok(None)
    );
    assert_eq!(
        state.handle_tokens(&toks(&[
            "query",
            "--displays",
            "id,label",
            "--display",
            "side"
        ])),
        Ok(Some(
            "{\n\t\"id\":77,\n\t\"label\":\"side\"\n}\n".to_string()
        ))
    );

    // Reserved direction keyword and numeric labels are rejected.
    assert_eq!(
        state.handle_tokens(&toks(&["display", "--label", "west"])),
        Err("'west' is a reserved keyword and cannot be used as a label.\n".to_string())
    );
    assert_eq!(
        state.handle_tokens(&toks(&["display", "--label", "3"])),
        Err("'3' cannot be used as a label.\n".to_string())
    );
}

#[test]
fn recent_window_selector_resolves_previous_focus() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_focused_window(Some(1));
    state.set_focused_window(Some(2));

    // `recent` is the window focused before the current one (1).
    assert_eq!(
        state.handle_tokens(&toks(&["window", "--focus", "recent"])),
        Ok(None)
    );
    assert_eq!(state.focused_window_id(), Some(1));
}

#[test]
fn mouse_selector_resolves_space_and_display_under_cursor() {
    let mut state = state_with_displays();
    state.set_display_active_space(42, 1);
    state.set_display_active_space(77, 2);
    // Cursor over the second display (frame x:1440..2720).
    state.set_cursor_point(Point {
        x: 1500.0,
        y: 100.0,
    });

    assert_eq!(
        state.handle_tokens(&toks(&["query", "--displays", "id", "--display", "mouse"])),
        Ok(Some("{\n\t\"id\":77\n}\n".to_string()))
    );
    assert_eq!(
        state.handle_tokens(&toks(&["query", "--spaces", "id", "--space", "mouse"])),
        Ok(Some("{\n\t\"id\":2\n}\n".to_string()))
    );
}

#[test]
fn recent_space_selector_resolves_previous_active() {
    let mut state = state_with_displays();
    state.set_active_space(1);
    state.set_active_space(2);

    assert_eq!(
        state.handle_tokens(&toks(&["query", "--spaces", "id", "--space", "recent"])),
        Ok(Some("{\n\t\"id\":1\n}\n".to_string()))
    );
}

#[test]
fn space_label_set_select_and_validation() {
    let mut state = state_with_displays();
    state.set_active_space(1);

    // Label the active space, then a labeled space query / selector resolves.
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--label", "web"])),
        Ok(None)
    );
    assert_eq!(
        state.handle_tokens(&toks(&["query", "--spaces", "id,label", "--space", "web"])),
        Ok(Some("{\n\t\"id\":1,\n\t\"label\":\"web\"\n}\n".to_string()))
    );

    // Labels are unique: moving "web" to space 2 clears it from space 1.
    assert_eq!(
        state.handle_tokens(&toks(&["space", "2", "--label", "web"])),
        Ok(None)
    );
    assert_eq!(
        state.handle_tokens(&toks(&["query", "--spaces", "id,label", "--space", "1"])),
        Ok(Some("{\n\t\"id\":1,\n\t\"label\":\"\"\n}\n".to_string()))
    );
    assert_eq!(
        state.handle_tokens(&toks(&["query", "--spaces", "id", "--space", "web"])),
        Ok(Some("{\n\t\"id\":2\n}\n".to_string()))
    );

    // An empty label clears it.
    assert_eq!(
        state.handle_tokens(&toks(&["space", "2", "--label", ""])),
        Ok(None)
    );
    assert!(
        state
            .handle_tokens(&toks(&["query", "--spaces", "id", "--space", "web"]))
            .is_err()
    );

    // Numeric and reserved-keyword labels are rejected like the C daemon.
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--label", "7"])),
        Err("'7' cannot be used as a label.\n".to_string())
    );
    assert_eq!(
        state.handle_tokens(&toks(&["space", "--label", "next"])),
        Err("'next' is a reserved keyword and cannot be used as a label.\n".to_string())
    );
}

#[test]
fn query_spaces_serializes_display_property() {
    let mut state = state_with_displays();
    state.set_active_space(2);

    // Space 1 is on display 42 (arrangement index 1); space 2 on display 77
    // (index 2). Only space 2 is the active/visible one.
    assert_eq!(
        state.handle_tokens(&toks(&[
            "query",
            "--spaces",
            "id,display,is-visible",
            "--space",
            "1",
        ])),
        Ok(Some(
            "{\n\t\"id\":1,\n\t\"display\":1,\n\t\"is-visible\":false\n}\n".to_string()
        ))
    );
    assert_eq!(
        state.handle_tokens(&toks(&[
            "query",
            "--spaces",
            "id,display,is-visible",
            "--space",
            "2",
        ])),
        Ok(Some(
            "{\n\t\"id\":2,\n\t\"display\":2,\n\t\"is-visible\":true\n}\n".to_string()
        ))
    );
}

#[test]
fn query_displays_serializes_registered_displays() {
    let mut state = state_with_displays();
    state.set_active_space(2);

    assert_eq!(
            state.handle_tokens(&toks(&[
                "query",
                "--displays",
                "id,index,frame,spaces,has-focus",
            ])),
            Ok(Some(
                "[{\n\t\"id\":42,\n\t\"index\":1,\n\t\"frame\":{\n\t\t\"x\":0.0000,\n\t\t\"y\":0.0000,\n\t\t\"w\":1440.0000,\n\t\t\"h\":900.0000\n\t},\n\t\"spaces\":[1],\n\t\"has-focus\":false\n},{\n\t\"id\":77,\n\t\"index\":2,\n\t\"frame\":{\n\t\t\"x\":1440.0000,\n\t\t\"y\":0.0000,\n\t\t\"w\":1280.0000,\n\t\t\"h\":720.0000\n\t},\n\t\"spaces\":[2],\n\t\"has-focus\":true\n}]\n".to_string()
            ))
        );
}

#[test]
fn query_display_scope_filters_spaces_and_windows() {
    let mut state = state_with_displays();
    state.set_active_space(1);
    state.add_window(10).unwrap();
    state.set_active_space(2);
    state.add_window(20).unwrap();

    assert_eq!(
        state.handle_tokens(&toks(&["query", "--spaces", "id", "--display", "2"])),
        Ok(Some("[{\n\t\"id\":2\n}]\n".to_string()))
    );
    assert_eq!(
        state.handle_tokens(&toks(&["query", "--windows", "id", "--display", "1"])),
        Ok(Some("[{\n\t\"id\":10\n}]\n".to_string()))
    );
    assert_eq!(
        state.handle_tokens(&toks(&["query", "--displays", "id", "--space", "1"])),
        Ok(Some("{\n\t\"id\":42\n}\n".to_string()))
    );
}

#[test]
fn query_unsupported_property_reports() {
    // `level` is a still-deferred field (needs a fragile version-specific private
    // SkyLight read), so it stays rejected; role/opacity/etc. are now supported.
    let mut state = state_with_space();
    assert_eq!(
        state.handle_tokens(&toks(&["query", "--windows", "level"])),
        Err("'level' is not available from pure window state".to_string())
    );
}

#[test]
fn query_windows_serializes_metadata() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_window_meta(
        1,
        WindowMeta {
            app: "Finder".to_string(),
            title: "Downloads \"x\"".to_string(),
            pid: 527,
        },
    );
    assert_eq!(
            state.handle_tokens(&toks(&["query", "--windows", "id,pid,app,title"])),
            Ok(Some(
                "[{\n\t\"id\":1,\n\t\"pid\":527,\n\t\"app\":\"Finder\",\n\t\"title\":\"Downloads \\\"x\\\"\"\n},{\n\t\"id\":2,\n\t\"pid\":0,\n\t\"app\":\"\",\n\t\"title\":\"\"\n}]\n"
                    .to_string()
            ))
        );
}

#[test]
fn unhandled_domain_reports() {
    let mut state = AppState::new();
    // `display` effects still need the macOS layer; it reports rather than
    // silently succeeding. (`rule`/`signal` are now handled.)
    assert!(
        state
            .handle_tokens(&toks(&["display", "--focus", "1"]))
            .is_err()
    );
}

#[test]
fn rules_add_match_list_remove() {
    let mut state = AppState::new();
    state
        .handle_tokens(&toks(&[
            "rule",
            "--add",
            "app=^Finder$",
            "manage=off",
            "label=fin",
        ]))
        .unwrap();
    // Matching window gets manage=off; a non-match gets no effects.
    assert_eq!(
        state.rule_effects_for_window("Finder", "", "", "").manage,
        Some(false)
    );
    assert_eq!(
        state.rule_effects_for_window("Safari", "", "", "").manage,
        None
    );

    // An exclusion filter inverts the match.
    state
        .handle_tokens(&toks(&["rule", "--add", "app!=^Finder$", "manage=on"]))
        .unwrap();
    assert_eq!(
        state.rule_effects_for_window("Safari", "", "", "").manage,
        Some(true)
    );

    // `--list` serializes both rules; spot-check the first.
    let list = state
        .handle_tokens(&toks(&["rule", "--list"]))
        .unwrap()
        .unwrap();
    assert!(list.contains("\"app\":\"^Finder$\""));
    assert!(list.contains("\"manage\":false"));
    assert!(list.contains("\"label\":\"fin\""));

    // Remove by label, then a bad index/label errors.
    state
        .handle_tokens(&toks(&["rule", "--remove", "fin"]))
        .unwrap();
    assert_eq!(
        state.rule_effects_for_window("Finder", "", "", "").manage,
        None
    );
    assert!(
        state
            .handle_tokens(&toks(&["rule", "--remove", "ghost"]))
            .unwrap_err()
            .contains("rule with label 'ghost' not found")
    );
}

#[test]
fn rule_add_rejects_bad_regex() {
    let mut state = AppState::new();
    let err = state
        .handle_tokens(&toks(&["rule", "--add", "app=("]))
        .unwrap_err();
    assert!(err.contains("invalid regex pattern '(' for key 'app'"));
}

#[test]
fn rule_apply_enacts_manage_for_known_windows() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_window_meta(
        1,
        WindowMeta {
            app: "Finder".to_string(),
            title: "One".to_string(),
            pid: 10,
        },
    );
    state.set_window_meta(
        2,
        WindowMeta {
            app: "Safari".to_string(),
            title: "Two".to_string(),
            pid: 20,
        },
    );

    state
        .handle_tokens(&toks(&[
            "rule",
            "--add",
            "app=^Finder$",
            "manage=off",
            "label=fin",
        ]))
        .unwrap();
    state
        .handle_tokens(&toks(&["rule", "--apply", "fin"]))
        .unwrap();
    assert!(state.is_floating(1));
    assert_eq!(state.space(1).unwrap().window_list(), vec![2]);

    state
        .handle_tokens(&toks(&["rule", "--apply", "app=^Finder$", "manage=on"]))
        .unwrap();
    assert!(!state.is_floating(1));
    let mut list = state.space(1).unwrap().window_list();
    list.sort_unstable();
    assert_eq!(list, vec![1, 2]);
}

#[test]
fn rule_apply_enacts_scratchpad_for_known_windows() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_window_meta(
        1,
        WindowMeta {
            app: "Finder".to_string(),
            title: "One".to_string(),
            pid: 10,
        },
    );
    state.set_window_meta(
        2,
        WindowMeta {
            app: "Safari".to_string(),
            title: "Two".to_string(),
            pid: 20,
        },
    );

    state
        .handle_tokens(&toks(&[
            "rule",
            "--add",
            "app=^Finder$",
            "scratchpad=notes",
        ]))
        .unwrap();
    state.handle_tokens(&toks(&["rule", "--apply"])).unwrap();

    assert_eq!(state.window_scratchpad(1), Some("notes"));
    assert_eq!(state.scratchpad_window("notes"), Some(1));
    assert!(state.is_floating(1));
    assert_eq!(state.space(1).unwrap().window_list(), vec![2]);
    assert_eq!(state.window_scratchpad(2), None);
}

#[test]
fn rule_apply_collects_daemon_boundary_effects() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.add_window(2).unwrap();
    state.set_window_meta(
        1,
        WindowMeta {
            app: "Finder".to_string(),
            title: "One".to_string(),
            pid: 10,
        },
    );
    state.set_window_meta(
        2,
        WindowMeta {
            app: "Safari".to_string(),
            title: "Two".to_string(),
            pid: 20,
        },
    );

    state
        .handle_tokens(&toks(&[
            "rule",
            "--add",
            "app=^Finder$",
            "sticky=on",
            "opacity=0.5",
            "sub-layer=above",
            "grid=2:2:0:1:1:1",
            "display=2",
            "space=2",
        ]))
        .unwrap();

    let applications = state
        .apply_rule_and_collect_effects(yabai_core::RuleApply::All)
        .unwrap();
    assert_eq!(applications.len(), 1);
    assert_eq!(applications[0].window_id, 1);
    assert_eq!(applications[0].sid, 1);
    assert_eq!(applications[0].effects.sticky, Some(true));
    assert_eq!(applications[0].effects.opacity, Some(0.5));
    assert_eq!(applications[0].effects.layer, Some(Layer::Above));
    assert_eq!(applications[0].effects.grid, Some([2, 2, 0, 1, 1, 1]));
    assert_eq!(applications[0].effects.display.as_deref(), Some("2"));
    assert_eq!(applications[0].effects.space.as_deref(), Some("2"));
}

#[test]
fn rule_apply_prefers_label_before_adhoc_rule() {
    let mut state = state_with_space();
    state.add_window(1).unwrap();
    state.set_window_meta(
        1,
        WindowMeta {
            app: "Finder".to_string(),
            title: "One".to_string(),
            pid: 10,
        },
    );
    state
        .handle_tokens(&toks(&[
            "rule",
            "--add",
            "app=^Finder$",
            "manage=off",
            "label=app=^Finder$",
        ]))
        .unwrap();

    state
        .handle_tokens(&toks(&["rule", "--apply", "app=^Finder$"]))
        .unwrap();
    assert!(state.is_floating(1));
}

#[test]
fn one_shot_rule_is_removed_after_new_window_match() {
    let mut state = state_with_space();
    state
        .handle_tokens(&toks(&[
            "rule",
            "--add",
            "--one-shot",
            "app=^Finder$",
            "manage=off",
        ]))
        .unwrap();

    state.add_window(1).unwrap();
    state.apply_new_window_rules(1, "Finder", "", "", "", 1);
    assert!(state.is_floating(1));
    assert_eq!(
        state.handle_tokens(&toks(&["rule", "--list"])).unwrap(),
        Some("[]\n".to_string())
    );

    state.add_window(2).unwrap();
    state.apply_new_window_rules(2, "Finder", "", "", "", 1);
    assert!(!state.is_floating(2));
    assert_eq!(state.space(1).unwrap().window_list(), vec![2]);
}

#[test]
fn parse_error_surfaces_as_response_error() {
    let mut state = AppState::new();
    assert_eq!(
        state.handle_tokens(&toks(&["bogus"])),
        Err("unknown domain 'bogus'".to_string())
    );
}

#[test]
fn signals_add_list_remove_and_fire() {
    let mut state = AppState::new();
    // Add two signals on different events plus one sharing a label to replace.
    state
        .handle_tokens(&toks(&[
            "signal",
            "--add",
            "event=window_focused",
            "action=echo focus",
            "label=a",
        ]))
        .unwrap();
    state
        .handle_tokens(&toks(&[
            "signal",
            "--add",
            "event=application_launched",
            "action=echo launch",
        ]))
        .unwrap();
    // Re-adding label `a` replaces the first signal's action.
    state
        .handle_tokens(&toks(&[
            "signal",
            "--add",
            "event=window_focused",
            "action=echo refocus",
            "label=a",
        ]))
        .unwrap();

    // Firing resolves by event in registration order.
    assert_eq!(
        state.signal_actions_for(SignalEvent::WindowFocused),
        vec!["echo refocus".to_string()]
    );
    assert_eq!(
        state.signal_actions_for(SignalEvent::ApplicationLaunched),
        vec!["echo launch".to_string()]
    );
    assert!(
        state
            .signal_actions_for(SignalEvent::SpaceChanged)
            .is_empty()
    );

    // `--list` is event-grouped (application_* before window_*) and indexed.
    let list = state
        .handle_tokens(&toks(&["signal", "--list"]))
        .unwrap()
        .unwrap();
    assert_eq!(
        list,
        "[{\n\t\"index\":0,\n\t\"label\":\"\",\n\t\"app\":\"\",\n\t\"title\":\"\",\n\t\"active\":null,\n\t\"event\":\"application_launched\",\n\t\"action\":\"echo launch\"\n},{\n\t\"index\":1,\n\t\"label\":\"a\",\n\t\"app\":\"\",\n\t\"title\":\"\",\n\t\"active\":null,\n\t\"event\":\"window_focused\",\n\t\"action\":\"echo refocus\"\n}]\n"
    );

    // Remove by label, then the remaining one by index 0.
    state
        .handle_tokens(&toks(&["signal", "--remove", "a"]))
        .unwrap();
    assert!(
        state
            .signal_actions_for(SignalEvent::WindowFocused)
            .is_empty()
    );
    state
        .handle_tokens(&toks(&["signal", "--remove", "0"]))
        .unwrap();
    assert!(
        state
            .signal_actions_for(SignalEvent::ApplicationLaunched)
            .is_empty()
    );

    // Missing label / index errors carry the C text.
    assert!(
        state
            .handle_tokens(&toks(&["signal", "--remove", "ghost"]))
            .unwrap_err()
            .contains("signal with label 'ghost' not found")
    );
    assert!(
        state
            .handle_tokens(&toks(&["signal", "--remove", "9"]))
            .unwrap_err()
            .contains("signal with index '9' not found")
    );
}

#[test]
fn signal_filters_match_c_event_categories() {
    let mut state = AppState::new();
    state
        .handle_tokens(&toks(&[
            "signal",
            "--add",
            "event=window_focused",
            "app=^Finder$",
            "title!=Scratch",
            "action=echo focus",
        ]))
        .unwrap();
    state
        .handle_tokens(&toks(&[
            "signal",
            "--add",
            "event=application_launched",
            "app=^Finder$",
            "action=echo app",
        ]))
        .unwrap();
    state
        .handle_tokens(&toks(&[
            "signal",
            "--add",
            "event=window_minimized",
            "app=^Finder$",
            "title=Downloads",
            "active=yes",
            "action=echo min",
        ]))
        .unwrap();
    state
        .handle_tokens(&toks(&[
            "signal",
            "--add",
            "event=window_deminimized",
            "app=^Finder$",
            "title=Downloads",
            "active=yes",
            "action=echo demin",
        ]))
        .unwrap();
    state
        .handle_tokens(&toks(&[
            "signal",
            "--add",
            "event=window_title_changed",
            "app=^Finder$",
            "title=Renamed",
            "active=no",
            "action=echo title",
        ]))
        .unwrap();

    assert_eq!(
        state.signal_actions_for_context(
            SignalEvent::WindowFocused,
            Some("Finder"),
            Some("Downloads"),
            None,
        ),
        vec!["echo focus".to_string()]
    );
    assert!(
        state
            .signal_actions_for_context(
                SignalEvent::WindowFocused,
                Some("Finder"),
                Some("Scratch"),
                None,
            )
            .is_empty()
    );
    assert_eq!(
        state.signal_actions_for_context(
            SignalEvent::ApplicationLaunched,
            Some("Finder"),
            None,
            None,
        ),
        vec!["echo app".to_string()]
    );
    assert_eq!(
        state.signal_actions_for_context(
            SignalEvent::WindowMinimized,
            Some("Finder"),
            Some("Downloads"),
            Some(true),
        ),
        vec!["echo min".to_string()]
    );
    assert!(
        state
            .signal_actions_for_context(
                SignalEvent::WindowMinimized,
                Some("Finder"),
                Some("Downloads"),
                Some(false),
            )
            .is_empty()
    );
    assert_eq!(
        state.signal_actions_for_context(
            SignalEvent::WindowDeminimized,
            Some("Finder"),
            Some("Downloads"),
            Some(false),
        ),
        vec!["echo demin".to_string()]
    );
    assert_eq!(
        state.signal_actions_for_context(
            SignalEvent::WindowTitleChanged,
            Some("Finder"),
            Some("Renamed"),
            Some(false),
        ),
        vec!["echo title".to_string()]
    );
    assert!(
        state
            .signal_actions_for_context(
                SignalEvent::WindowTitleChanged,
                Some("Finder"),
                Some("Renamed"),
                Some(true),
            )
            .is_empty()
    );
    assert!(
        state
            .signal_actions_for_context(
                SignalEvent::ApplicationLaunched,
                Some("Safari"),
                None,
                None,
            )
            .is_empty()
    );
}

#[test]
fn signal_add_rejects_invalid_regex() {
    let mut state = AppState::new();
    let error = state
        .handle_tokens(&toks(&[
            "signal",
            "--add",
            "event=window_focused",
            "app=(",
            "action=echo nope",
        ]))
        .unwrap_err();
    assert!(error.contains("invalid regex pattern '(' for key 'app'"));
}

#[test]
fn cross_space_mouse_drop() {
    let mut state = AppState::new();
    // Set up two spaces on two displays
    state.displays.insert(
        1,
        DisplayInfo {
            frame: Area::new(0.0, 0.0, 1000.0, 1000.0),
        },
    );
    state.displays.insert(
        2,
        DisplayInfo {
            frame: Area::new(1000.0, 0.0, 1000.0, 1000.0),
        },
    );
    state.space_displays.insert(101, 1);
    state.space_displays.insert(102, 2);

    state.display_active_space.insert(1, 101);
    state.display_active_space.insert(2, 102);

    state.spaces.insert(
        101,
        yabai_core::Tree::new(
            yabai_core::ViewType::Bsp,
            state.config.layout_config(),
            Area::new(0.0, 0.0, 1000.0, 1000.0),
        ),
    );
    state.spaces.insert(
        102,
        yabai_core::Tree::new(
            yabai_core::ViewType::Bsp,
            state.config.layout_config(),
            Area::new(1000.0, 0.0, 1000.0, 1000.0),
        ),
    );

    state.active_space = Some(101);

    // Window on space 101
    state.window_spaces.insert(10, 101);
    state.spaces.get_mut(&101).unwrap().add_window(10, None);
    state.set_focused_window(Some(10));

    // Window on space 102
    state.window_spaces.insert(20, 102);
    state.spaces.get_mut(&102).unwrap().add_window(20, None);

    // Stack-drop window 10 onto display 2 (space 102), over window 20. A
    // non-swap drop only relocates the dragged window; window 20 stays put.
    let point = Point {
        x: 1500.0,
        y: 500.0,
    };
    let result = state.drop_tiled_window_at_point(10, point, MouseDropAction::Stack);

    assert_eq!(
        result,
        DropResult::CrossSpace {
            dragged_id: 10,
            dragged_new_sid: 102,
            swapped_id: None,
            swapped_new_sid: None,
        }
    );
    assert_eq!(state.window_space(10), Some(102));
    assert_eq!(state.window_space(20), Some(102));
    assert!(state.spaces.get(&101).unwrap().capture().is_empty());
    assert_eq!(state.spaces.get(&102).unwrap().capture().len(), 2);
}

#[test]
fn cross_space_mouse_drop_swap_returns_target() {
    let mut state = AppState::new();
    for (did, x) in [(1u32, 0.0), (2u32, 1000.0)] {
        state.displays.insert(
            did,
            DisplayInfo {
                frame: Area::new(x, 0.0, 1000.0, 1000.0),
            },
        );
    }
    state.space_displays.insert(101, 1);
    state.space_displays.insert(102, 2);
    state.display_active_space.insert(1, 101);
    state.display_active_space.insert(2, 102);
    for (sid, x) in [(101u64, 0.0), (102u64, 1000.0)] {
        state.spaces.insert(
            sid,
            yabai_core::Tree::new(
                yabai_core::ViewType::Bsp,
                state.config.layout_config(),
                Area::new(x, 0.0, 1000.0, 1000.0),
            ),
        );
    }
    state.active_space = Some(101);
    state.window_spaces.insert(10, 101);
    state.spaces.get_mut(&101).unwrap().add_window(10, None);
    state.set_focused_window(Some(10));
    state.window_spaces.insert(20, 102);
    state.spaces.get_mut(&102).unwrap().add_window(20, None);

    // A cross-space swap onto window 20 sends 20 back to the dragged window's
    // old space (101) while 10 lands on 102.
    let point = Point {
        x: 1500.0,
        y: 500.0,
    };
    let result = state.drop_tiled_window_at_point(10, point, MouseDropAction::Swap);

    assert_eq!(
        result,
        DropResult::CrossSpace {
            dragged_id: 10,
            dragged_new_sid: 102,
            swapped_id: Some(20),
            swapped_new_sid: Some(101),
        }
    );
    assert_eq!(state.window_space(10), Some(102));
    assert_eq!(state.window_space(20), Some(101));
    assert_eq!(state.spaces.get(&101).unwrap().capture().len(), 1);
    assert_eq!(state.spaces.get(&102).unwrap().capture().len(), 1);
}
