use super::*;

const SCREEN: Area = Area::new(0.0, 0.0, 1000.0, 1000.0);

fn bsp() -> Tree {
    Tree::new(ViewType::Bsp, LayoutConfig::default(), SCREEN)
}

#[test]
fn first_window_fills_root() {
    let mut tree = bsp();
    let node = tree.add_window(1, None).unwrap();
    assert_eq!(node, tree.root());
    assert_eq!(tree.node(node).window_list, vec![1]);
    assert!(tree.node(node).is_leaf());
    assert_eq!(tree.node(node).area, SCREEN);
}

#[test]
fn second_window_splits_root_vertically() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));

    let root = tree.root();
    assert!(!tree.node(root).is_leaf());
    // 1000-wide root splits vertically (w >= h) at ratio 0.5.
    let left = tree.node(root).left.unwrap();
    let right = tree.node(root).right.unwrap();
    assert_eq!(tree.node(left).area.w as i32, 500);
    assert_eq!(tree.node(right).area.w as i32, 500);
    assert_eq!(tree.node(right).area.x as i32, 500);
    // Default placement keeps the existing window in the first child.
    assert_eq!(tree.node(left).window_list, vec![1]);
    assert_eq!(tree.node(right).window_list, vec![2]);
}

#[test]
fn leaves_and_window_list_are_ordered() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    tree.add_window(3, Some(2));
    assert_eq!(tree.window_list(), vec![1, 2, 3]);
}

#[test]
fn capture_emits_a_frame_per_window() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    let frames = tree.capture();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].window_id, 1);
    assert_eq!(frames[1].window_id, 2);
    // The two halves don't overlap: window 1 ends where window 2 begins.
    assert_eq!(
        (frames[0].area.x + frames[0].area.w) as i32,
        frames[1].area.x as i32
    );
}

#[test]
fn remove_collapses_sibling_into_parent() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    // Removing window 2 leaves window 1 occupying the whole screen again.
    tree.remove_window(2);
    let node = tree.find_window_node(1).unwrap();
    assert!(tree.node(node).is_leaf());
    assert_eq!(tree.node(node).area, SCREEN);
    assert_eq!(tree.window_list(), vec![1]);
}

#[test]
fn remove_last_window_clears_root() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.remove_window(1);
    assert!(tree.window_list().is_empty());
    assert!(tree.node(tree.root()).is_leaf());
}

#[test]
fn stack_layout_keeps_one_node() {
    let mut tree = Tree::new(ViewType::Stack, LayoutConfig::default(), SCREEN);
    tree.add_window(1, None);
    tree.add_window(2, None);
    tree.add_window(3, None);
    let root = tree.root();
    assert!(tree.node(root).is_leaf());
    // window_order tracks most-recent-first; window_list keeps stack order.
    assert_eq!(tree.node(root).window_order[0], 3);
    assert_eq!(tree.node(root).window_list.len(), 3);
}

#[test]
fn rotate_180_swaps_children_and_inverts_ratio() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    let root = tree.root();
    tree.node_mut(root).ratio = 0.3;
    tree.rotate(root, 180);
    assert!((tree.node(root).ratio - 0.7).abs() < 1e-6);
}

#[test]
fn find_node_in_direction_picks_horizontal_neighbor() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    let left = tree.find_window_node(1).unwrap();
    let right = tree.find_window_node(2).unwrap();
    assert_eq!(
        tree.find_node_in_direction(left, Direction::East),
        Some(right)
    );
    assert_eq!(tree.find_node_in_direction(left, Direction::West), None);
    assert_eq!(
        tree.find_node_in_direction(right, Direction::West),
        Some(left)
    );
}

#[test]
fn equalize_resets_ratios_to_default() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    let root = tree.root();
    tree.node_mut(root).ratio = 0.8;
    tree.equalize(root, NodeSplit::Vertical);
    assert!((tree.node(root).ratio - 0.5).abs() < 1e-6);
}

#[test]
fn swap_windows_in_same_leaf_swaps_slots() {
    let mut tree = Tree::new(ViewType::Stack, LayoutConfig::default(), SCREEN);
    tree.add_window(1, None);
    tree.add_window(2, None);
    let root = tree.root();
    assert!(tree.swap_windows(1, 2));
    // The two ids trade places in both lists; the set is unchanged.
    let mut list = tree.node(root).window_list.clone();
    list.sort_unstable();
    assert_eq!(list, vec![1, 2]);
}

#[test]
fn swap_windows_across_leaves_swaps_contents() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    let left = tree.find_window_node(1).unwrap();
    let right = tree.find_window_node(2).unwrap();
    assert!(tree.swap_windows(1, 2));
    // Window 2 now lives in the left leaf and window 1 in the right leaf.
    assert_eq!(tree.node(left).window_list, vec![2]);
    assert_eq!(tree.node(right).window_list, vec![1]);
}

#[test]
fn stack_window_onto_moves_source_into_target_leaf() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    tree.add_window(3, Some(2));

    assert!(tree.stack_window_onto(1, 3));
    let target = tree.find_window_node(3).unwrap();
    assert_eq!(tree.node(target).window_list, vec![3, 1]);
    assert_eq!(tree.node(target).window_order[0], 1);
    assert_eq!(tree.window_list(), vec![2, 3, 1]);
}

#[test]
fn swap_same_or_unmanaged_window_is_noop() {
    let mut tree = bsp();
    tree.add_window(1, None);
    assert!(!tree.swap_windows(1, 1));
    assert!(!tree.swap_windows(1, 99));
}

#[test]
fn warp_window_restructures_next_to_target() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    tree.add_window(3, Some(2));
    // 1 is root's left child; 2 and 3 share the right subtree.
    assert!(tree.warp_window(1, 3));
    let mut list = tree.window_list();
    list.sort_unstable();
    assert_eq!(list, vec![1, 2, 3]);
    // 1 was removed from its old leaf and re-inserted as 3's new sibling.
    let n1 = tree.find_window_node(1).unwrap();
    let n3 = tree.find_window_node(3).unwrap();
    assert_eq!(tree.node(n1).parent, tree.node(n3).parent);
    assert!(tree.node(n1).parent.is_some());
}

#[test]
fn directional_warp_uses_requested_side() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    tree.add_window(3, Some(2));

    assert!(tree.warp_window_directional(1, 3, NodeSplit::Horizontal, Child::First));
    let target = tree.find_window_node(3).unwrap();
    let src = tree.find_window_node(1).unwrap();
    let parent = tree.node(target).parent.unwrap();
    assert_eq!(tree.node(parent).split, NodeSplit::Horizontal);
    assert_eq!(tree.node(parent).left, Some(src));
    assert_eq!(tree.node(parent).right, Some(target));
}

#[test]
fn zoom_fullscreen_overrides_only_its_own_frame() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    let root_area = tree.node(tree.root()).area;

    assert!(tree.toggle_zoom(1, ZoomKind::Fullscreen));
    assert_eq!(tree.zoomed(), Some((1, ZoomKind::Fullscreen)));
    let frames = tree.capture();
    let f1 = frames.iter().find(|f| f.window_id == 1).unwrap();
    let f2 = frames.iter().find(|f| f.window_id == 2).unwrap();
    assert_eq!(f1.area, root_area); // zoomed window fills the space
    assert_ne!(f2.area, root_area); // the other stays tiled

    // Toggling the same kind again un-zooms and restores the tiled frame.
    assert!(tree.toggle_zoom(1, ZoomKind::Fullscreen));
    assert!(tree.zoomed().is_none());
    let f1b = tree
        .capture()
        .into_iter()
        .find(|f| f.window_id == 1)
        .unwrap();
    assert_ne!(f1b.area, root_area);
}

#[test]
fn zoom_unmanaged_window_is_noop() {
    let mut tree = bsp();
    tree.add_window(1, None);
    assert!(!tree.toggle_zoom(99, ZoomKind::Fullscreen));
    assert!(tree.zoomed().is_none());
}

#[test]
fn warp_same_shared_or_non_bsp_is_noop() {
    let mut tree = bsp();
    tree.add_window(1, None);
    assert!(!tree.warp_window(1, 1)); // same window
    assert!(!tree.warp_window(1, 99)); // unmanaged target
    let mut stack = Tree::new(ViewType::Stack, LayoutConfig::default(), SCREEN);
    stack.add_window(1, None);
    stack.add_window(2, None);
    assert!(!stack.warp_window(1, 2)); // warp is BSP-only
}

#[test]
fn fence_finds_dividing_ancestor() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    let left = tree.find_window_node(1).unwrap();
    let right = tree.find_window_node(2).unwrap();
    let root = tree.root();
    // Root splits the screen vertically, so each leaf is fenced east/west.
    assert_eq!(tree.fence(left, Direction::East), Some(root));
    assert_eq!(tree.fence(right, Direction::West), Some(root));
    assert_eq!(tree.fence(left, Direction::North), None);
}

#[test]
fn resize_window_moves_the_divider() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    let root = tree.root();
    let left = tree.find_window_node(1).unwrap();
    // Drag window 1's right edge 100px to the right (area is 1000 wide).
    assert!(tree.resize_window(1, HANDLE_RIGHT, 100.0, 0.0));
    assert!((tree.node(root).ratio - 0.6).abs() < 1e-6);
    // The left leaf grew to ~600px wide.
    assert_eq!(tree.node(left).area.w as i32, 600);
}

#[test]
fn resize_window_clamps_ratio() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    let root = tree.root();
    // A huge drag is clamped to the 0.9 maximum.
    assert!(tree.resize_window(1, HANDLE_RIGHT, 100_000.0, 0.0));
    assert!((tree.node(root).ratio - 0.9).abs() < 1e-6);
}

#[test]
fn adjust_window_ratio_abs_and_rel() {
    let mut tree = bsp();
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    let root = tree.root();

    // `abs` replaces the parent ratio; `rel` adds to it.
    assert!(tree.adjust_window_ratio(1, false, 0.7));
    assert!((tree.node(root).ratio - 0.7).abs() < 1e-6);
    assert!(tree.adjust_window_ratio(1, true, -0.2));
    assert!((tree.node(root).ratio - 0.5).abs() < 1e-6);

    // Both directions clamp to [0.1, 0.9].
    assert!(tree.adjust_window_ratio(1, true, 100.0));
    assert!((tree.node(root).ratio - 0.9).abs() < 1e-6);
    assert!(tree.adjust_window_ratio(1, false, -5.0));
    assert!((tree.node(root).ratio - 0.1).abs() < 1e-6);
}

#[test]
fn adjust_window_ratio_root_or_unknown_window_fails() {
    let mut tree = bsp();
    tree.add_window(1, None);
    // A lone root window has no parent, and an unknown window isn't in the tree.
    assert!(!tree.adjust_window_ratio(1, false, 0.7));
    assert!(!tree.adjust_window_ratio(99, false, 0.7));
}

#[test]
fn set_window_insertion_places_next_window_directionally() {
    for (dir, east_ish) in [
        (InsertDirection::East, true),
        (InsertDirection::West, false),
    ] {
        let mut tree = bsp();
        tree.add_window(1, None);
        assert!(tree.set_window_insertion(1, dir));
        tree.add_window(2, None);
        let cap = tree.capture();
        let f1 = cap.iter().find(|f| f.window_id == 1).unwrap();
        let f2 = cap.iter().find(|f| f.window_id == 2).unwrap();
        // A vertical split keeps both at the same y; the new window sits on the
        // requested side.
        assert!((f1.area.y - f2.area.y).abs() < 1e-3);
        assert_eq!(f2.area.x > f1.area.x, east_ish, "dir {dir:?}");
    }

    // North puts the new window above (smaller y) in a horizontal split.
    let mut tree = bsp();
    tree.add_window(1, None);
    assert!(tree.set_window_insertion(1, InsertDirection::North));
    tree.add_window(2, None);
    let cap = tree.capture();
    let f1 = cap.iter().find(|f| f.window_id == 1).unwrap();
    let f2 = cap.iter().find(|f| f.window_id == 2).unwrap();
    assert!((f1.area.x - f2.area.x).abs() < 1e-3);
    assert!(f2.area.y < f1.area.y);
}

#[test]
fn set_window_insertion_stack_joins_target_leaf() {
    let mut tree = bsp();
    tree.add_window(1, None);
    assert!(tree.set_window_insertion(1, InsertDirection::Stack));
    tree.add_window(2, None);
    // The new window stacks into window 1's leaf instead of splitting.
    let node = tree.find_window_node(1).unwrap();
    assert_eq!(tree.find_window_node(2), Some(node));
    assert_eq!(tree.node(node).window_list.len(), 2);
}

#[test]
fn set_window_insertion_toggles_off_and_rejects_unknown() {
    let mut tree = bsp();
    tree.add_window(1, None);
    assert!(tree.set_window_insertion(1, InsertDirection::East));
    assert_eq!(tree.insertion_point, Some(1));
    // Re-selecting the same direction clears the marker.
    assert!(tree.set_window_insertion(1, InsertDirection::East));
    assert_eq!(tree.insertion_point, None);
    // An unknown window can't be marked.
    assert!(!tree.set_window_insertion(99, InsertDirection::East));
}

#[test]
fn resize_window_without_fence_fails() {
    let mut tree = bsp();
    tree.add_window(1, None);
    // Single root leaf has no dividing ancestor in any direction.
    assert!(!tree.resize_window(1, HANDLE_RIGHT, 50.0, 0.0));
}

#[test]
fn auto_balance_evens_out_a_chain() {
    // Force vertical splits so the whole chain shares one axis.
    let config = LayoutConfig {
        split_type: NodeSplit::Vertical,
        auto_balance: NodeSplit::Vertical,
        ..LayoutConfig::default()
    };
    let mut tree = Tree::new(ViewType::Bsp, config, SCREEN);
    tree.add_window(1, None);
    tree.add_window(2, Some(1));
    tree.add_window(3, Some(2));
    // Three leaves balanced on the vertical axis: root keeps 1/3 left.
    let root = tree.root();
    assert!((tree.node(root).ratio - 1.0 / 3.0).abs() < 1e-6);
}
