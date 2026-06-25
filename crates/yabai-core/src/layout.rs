//! Pure-Rust BSP layout tree, ported from `src/view.c`.
//!
//! The C implementation threads parent/left/right raw pointers through
//! heap-allocated `struct window_node`s and reads policy from the global
//! `g_space_manager`. This port keeps the same tree semantics but stores nodes
//! in an arena addressed by [`NodeId`], and lifts every global into an explicit
//! [`LayoutConfig`] so the logic is deterministic and unit-testable.
//!
//! Deliberately out of scope for this module (handled higher up, against live
//! macOS state): zoom persistence (`window_zoom_persist`), insert-feedback
//! windows, and the window-rank tie-break in `view_find_window_node_in_direction`
//! (which needs the space's z-order). Those are noted at their call sites.

use crate::geometry::{Area, Direction, Split};

/// Index of a node within a [`Tree`]'s arena.
pub type NodeId = usize;

/// Resize handle flags, matching `HANDLE_*` in `src/misc/macros.h`. Combine with
/// bitwise-or to describe which edges of a window are being dragged.
pub const HANDLE_TOP: u8 = 0x01;
pub const HANDLE_BOTTOM: u8 = 0x02;
pub const HANDLE_LEFT: u8 = 0x04;
pub const HANDLE_RIGHT: u8 = 0x08;
/// Absolute move/resize handle (`--resize abs:...`); not a fence operation.
pub const HANDLE_ABS: u8 = 0x10;

/// Per-node split orientation. Mirrors `enum window_node_split` in `src/view.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeSplit {
    None,
    /// `SPLIT_Y`: a vertical divider, children placed left/right.
    Vertical,
    /// `SPLIT_X`: a horizontal divider, children placed top/bottom.
    Horizontal,
    /// `SPLIT_AUTO`: choose orientation from the node's aspect ratio.
    Auto,
}

impl NodeSplit {
    fn to_geometry(self) -> Split {
        match self {
            NodeSplit::Horizontal => Split::Horizontal,
            // None/Auto are resolved before this is called; treat as vertical.
            _ => Split::Vertical,
        }
    }
}

/// Which child a freshly split node keeps its existing windows in.
/// Mirrors `enum window_node_child`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Child {
    None,
    Second,
    First,
}

/// Where a new window is inserted when no explicit insertion point is set.
/// Mirrors `enum window_insertion_point`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionPolicy {
    Focused,
    First,
    Last,
}

/// Top-level layout of a space. Mirrors `enum view_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewType {
    Bsp,
    Stack,
    Float,
}

/// Policy that the C code reads from `g_space_manager` (and per-view fields).
#[derive(Debug, Clone, Copy)]
pub struct LayoutConfig {
    /// Forced split orientation, or `Auto`/`None` to derive from aspect ratio.
    pub split_type: NodeSplit,
    /// Default split ratio used when a node's own ratio is out of `[0.1, 0.9]`.
    pub split_ratio: f32,
    /// Which child keeps existing windows when a leaf is split.
    pub window_placement: Child,
    /// Axis flag to auto-balance after insert/remove (`NodeSplit::None` = off).
    pub auto_balance: NodeSplit,
    /// Default leaf selection when no insertion point is set.
    pub insertion_policy: InsertionPolicy,
    /// Gap between sibling areas, in points. `0` disables gaps.
    pub gap: i32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            split_type: NodeSplit::Auto,
            split_ratio: 0.5,
            window_placement: Child::Second,
            auto_balance: NodeSplit::None,
            insertion_policy: InsertionPolicy::Focused,
            gap: 0,
        }
    }
}

/// A node in the BSP tree. Leaves carry windows; intermediates carry children.
/// Mirrors `struct window_node` (minus zoom/feedback fields).
#[derive(Debug, Clone)]
pub struct Node {
    pub area: Area,
    pub parent: Option<NodeId>,
    pub left: Option<NodeId>,
    pub right: Option<NodeId>,
    /// Windows occupying this leaf, in spatial order.
    pub window_list: Vec<u32>,
    /// Same windows in focus/most-recent order; `window_order[0]` is on top.
    pub window_order: Vec<u32>,
    pub ratio: f32,
    pub split: NodeSplit,
    pub child: Child,
    /// Pending insertion direction marker (`0` = none). Used by insert feedback.
    pub insert_dir: i32,
}

impl Node {
    fn empty() -> Self {
        Self {
            area: Area::new(0.0, 0.0, 0.0, 0.0),
            parent: None,
            left: None,
            right: None,
            window_list: Vec::new(),
            window_order: Vec::new(),
            ratio: 0.0,
            split: NodeSplit::None,
            child: Child::None,
            insert_dir: 0,
        }
    }

    pub fn is_leaf(&self) -> bool {
        self.left.is_none() && self.right.is_none()
    }

    pub fn is_occupied(&self) -> bool {
        !self.window_list.is_empty()
    }

    pub fn contains_window(&self, window_id: u32) -> bool {
        self.window_list.contains(&window_id)
    }
}

#[derive(Debug, Clone, Copy)]
struct BalanceNode {
    y_count: i32,
    x_count: i32,
}

/// A window and the frame it should occupy, produced by [`Tree::capture`].
/// Mirrors `struct window_capture` in `src/view.h`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowFrame {
    pub window_id: u32,
    pub area: Area,
}

/// A temporary, non-structural fullscreen state for one window (`window
/// --toggle zoom-*`). The window keeps its place in the tree; only its captured
/// frame is overridden, so toggling back restores tiling with no reflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomKind {
    /// `zoom-fullscreen`: fill the whole space (root) area.
    Fullscreen,
    /// `zoom-parent`: fill the window's parent node area.
    Parent,
}

/// A BSP/stack/float layout tree for one space.
#[derive(Debug, Clone)]
pub struct Tree {
    nodes: Vec<Node>,
    free: Vec<NodeId>,
    root: NodeId,
    pub layout: ViewType,
    pub config: LayoutConfig,
    /// Window id the next insert prefers to split next to (`None` = unset).
    pub insertion_point: Option<u32>,
    /// The window currently zoomed (and how), if any. Overrides only its own
    /// captured frame; cleared on toggle-off or when the window leaves the tree.
    zoom: Option<(u32, ZoomKind)>,
}

impl Tree {
    /// Create an empty tree whose root area is `area`.
    pub fn new(layout: ViewType, config: LayoutConfig, area: Area) -> Self {
        let mut root = Node::empty();
        root.area = area;
        Self {
            nodes: vec![root],
            free: Vec::new(),
            root: 0,
            layout,
            config,
            insertion_point: None,
            zoom: None,
        }
    }

    /// `window --toggle zoom-fullscreen|zoom-parent`: zoom `window_id` with
    /// `kind`, or un-zoom if it is already zoomed the same way. Returns `false`
    /// if the window is not in the tree. Purely a capture-time override — no tree
    /// restructuring — so toggling off restores the tiled frame exactly.
    pub fn toggle_zoom(&mut self, window_id: u32, kind: ZoomKind) -> bool {
        if self.find_window_node(window_id).is_none() {
            return false;
        }
        self.zoom = if self.zoom == Some((window_id, kind)) {
            None
        } else {
            Some((window_id, kind))
        };
        true
    }

    /// The window currently zoomed, and how (`None` if no window is zoomed).
    pub fn zoomed(&self) -> Option<(u32, ZoomKind)> {
        self.zoom
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    /// Update the root area and recompute every descendant's area.
    pub fn set_root_area(&mut self, area: Area) {
        self.nodes[self.root].area = area;
        self.update(self.root);
    }

    fn alloc(&mut self, node: Node) -> NodeId {
        if let Some(id) = self.free.pop() {
            self.nodes[id] = node;
            id
        } else {
            self.nodes.push(node);
            self.nodes.len() - 1
        }
    }

    fn is_left_child(&self, id: NodeId) -> bool {
        match self.nodes[id].parent {
            Some(p) => self.nodes[p].left == Some(id),
            None => false,
        }
    }

    fn is_right_child(&self, id: NodeId) -> bool {
        match self.nodes[id].parent {
            Some(p) => self.nodes[p].right == Some(id),
            None => false,
        }
    }

    // --- split policy (window_node_get_split / _ratio / area_make_pair) ---

    fn resolve_split(&self, id: NodeId) -> NodeSplit {
        let node = &self.nodes[id];
        if node.split != NodeSplit::None {
            return node.split;
        }
        if self.config.split_type != NodeSplit::None && self.config.split_type != NodeSplit::Auto {
            return self.config.split_type;
        }
        if node.area.w >= node.area.h {
            NodeSplit::Vertical
        } else {
            NodeSplit::Horizontal
        }
    }

    fn resolve_ratio(&self, id: NodeId) -> f32 {
        let ratio = self.nodes[id].ratio;
        if (0.1..=0.9).contains(&ratio) {
            ratio
        } else {
            self.config.split_ratio
        }
    }

    /// `area_make_pair_for_node`: lay out a node's two children and persist the
    /// resolved split/ratio back onto the node.
    fn make_pair(&mut self, id: NodeId) {
        let split = self.resolve_split(id);
        let ratio = self.resolve_ratio(id);
        let (left_area, right_area) =
            self.nodes[id]
                .area
                .split(split.to_geometry(), self.config.gap, ratio);

        let (left, right) = (self.nodes[id].left, self.nodes[id].right);
        if let Some(l) = left {
            self.nodes[l].area = left_area;
        }
        if let Some(r) = right {
            self.nodes[r].area = right_area;
        }
        self.nodes[id].split = split;
        self.nodes[id].ratio = ratio;
    }

    /// `window_node_update`: recompute areas for `id` and all descendants.
    pub fn update(&mut self, id: NodeId) {
        if self.nodes[id].is_leaf() {
            return;
        }
        self.make_pair(id);
        if let Some(l) = self.nodes[id].left {
            self.update(l);
        }
        if let Some(r) = self.nodes[id].right {
            self.update(r);
        }
    }

    // --- leaf traversal (window_node_find_*_leaf) ---

    pub fn find_first_leaf(&self, start: NodeId) -> NodeId {
        let mut node = start;
        while let Some(l) = self.nodes[node].left {
            node = l;
        }
        node
    }

    pub fn find_last_leaf(&self, start: NodeId) -> NodeId {
        let mut node = start;
        while let Some(r) = self.nodes[node].right {
            node = r;
        }
        node
    }

    pub fn find_prev_leaf(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id].parent?;
        if self.is_left_child(id) {
            return self.find_prev_leaf(parent);
        }
        let left = self.nodes[parent].left.unwrap();
        if self.nodes[left].is_leaf() {
            Some(left)
        } else {
            Some(self.find_last_leaf(self.nodes[left].right.unwrap()))
        }
    }

    pub fn find_next_leaf(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes[id].parent?;
        if self.is_right_child(id) {
            return self.find_next_leaf(parent);
        }
        let right = self.nodes[parent].right.unwrap();
        if self.nodes[right].is_leaf() {
            Some(right)
        } else {
            Some(self.find_first_leaf(self.nodes[right].left.unwrap()))
        }
    }

    /// Iterate leaf node ids from first to last.
    pub fn leaves(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut node = Some(self.find_first_leaf(self.root));
        while let Some(id) = node {
            out.push(id);
            node = self.find_next_leaf(id);
        }
        out
    }

    /// `view_find_window_node`: leaf containing `window_id`, if any.
    pub fn find_window_node(&self, window_id: u32) -> Option<NodeId> {
        self.leaves()
            .into_iter()
            .find(|&id| self.nodes[id].contains_window(window_id))
    }

    /// Flat list of all managed windows, first leaf to last.
    pub fn window_list(&self) -> Vec<u32> {
        self.leaves()
            .into_iter()
            .flat_map(|id| self.nodes[id].window_list.clone())
            .collect()
    }

    /// `window_node_capture_windows`: the target frame for every managed window,
    /// first leaf to last. Each window in a stacked leaf shares the leaf's area.
    /// This is what the macOS layer turns into window-move operations.
    pub fn capture(&self) -> Vec<WindowFrame> {
        // A zoomed window's captured frame is overridden to its zoom area; the
        // rest stay tiled (underneath it).
        let zoom = self.zoom.and_then(|(zid, kind)| {
            let leaf = self.find_window_node(zid)?;
            let area = match kind {
                ZoomKind::Fullscreen => self.nodes[self.root].area,
                ZoomKind::Parent => {
                    let parent = self.nodes[leaf].parent.unwrap_or(self.root);
                    self.nodes[parent].area
                }
            };
            Some((zid, area))
        });

        let mut out = Vec::new();
        for id in self.leaves() {
            let area = self.nodes[id].area;
            for &window_id in &self.nodes[id].window_list {
                let area = match zoom {
                    Some((zid, zoom_area)) if zid == window_id => zoom_area,
                    _ => area,
                };
                out.push(WindowFrame { window_id, area });
            }
        }
        out
    }

    // --- min-depth leaf (view_find_min_depth_leaf_node), BFS ---

    fn find_min_depth_leaf(&self, start: NodeId) -> NodeId {
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(id) = queue.pop_front() {
            if self.nodes[id].is_leaf() {
                return id;
            }
            if let Some(l) = self.nodes[id].left {
                queue.push_back(l);
            }
            if let Some(r) = self.nodes[id].right {
                queue.push_back(r);
            }
        }
        start
    }

    // --- insertion (view_add_window_node / window_node_split / stack) ---

    /// `view_stack_window_node`: add `window_id` to an existing leaf's stack.
    fn stack_window(&mut self, id: NodeId, window_id: u32) {
        let node = &mut self.nodes[id];
        let top = node.window_order.first().copied();
        let insert_index = top
            .and_then(|t| node.window_list.iter().position(|&w| w == t).map(|i| i + 1))
            .unwrap_or(node.window_list.len());
        node.window_list.insert(insert_index, window_id);
        node.window_order.insert(0, window_id);
    }

    /// `window_node_split`: turn leaf `id` into an intermediate with two leaves.
    fn split_node(&mut self, id: NodeId, window_id: u32) {
        let child = if self.nodes[id].child != Child::None {
            self.nodes[id].child
        } else {
            self.config.window_placement
        };

        let existing_list = std::mem::take(&mut self.nodes[id].window_list);
        let existing_order = std::mem::take(&mut self.nodes[id].window_order);

        let mut left = Node::empty();
        let mut right = Node::empty();

        if child == Child::Second {
            left.window_list = existing_list;
            left.window_order = existing_order;
            right.window_list = vec![window_id];
            right.window_order = vec![window_id];
        } else {
            right.window_list = existing_list;
            right.window_order = existing_order;
            left.window_list = vec![window_id];
            left.window_order = vec![window_id];
        }

        left.parent = Some(id);
        right.parent = Some(id);

        let left_id = self.alloc(left);
        let right_id = self.alloc(right);

        let node = &mut self.nodes[id];
        node.left = Some(left_id);
        node.right = Some(right_id);

        self.make_pair(id);
    }

    /// Select the leaf to split for a new window, honoring an explicit insertion
    /// point, then the configured [`InsertionPolicy`]. `focused` is the id of the
    /// currently focused window (used by [`InsertionPolicy::Focused`]).
    fn pick_insertion_leaf(&mut self, focused: Option<u32>) -> NodeId {
        if let Some(point) = self.insertion_point {
            if let Some(leaf) = self.find_window_node(point) {
                self.nodes[leaf].insert_dir = 0;
                return leaf;
            }
        }

        let by_policy = match self.config.insertion_policy {
            InsertionPolicy::Focused => focused.and_then(|w| self.find_window_node(w)),
            InsertionPolicy::First => Some(self.find_first_leaf(self.root)),
            InsertionPolicy::Last => Some(self.find_last_leaf(self.root)),
        };

        by_policy.unwrap_or_else(|| self.find_min_depth_leaf(self.root))
    }

    /// `view_add_window_node`: insert `window_id`, returning the affected node.
    /// For BSP this is the split leaf (or the root after an auto-balance).
    pub fn add_window(&mut self, window_id: u32, focused: Option<u32>) -> Option<NodeId> {
        let root = self.root;
        if !self.nodes[root].is_occupied() && self.nodes[root].is_leaf() {
            let node = &mut self.nodes[root];
            node.window_list = vec![window_id];
            node.window_order = vec![window_id];
            return Some(root);
        }

        match self.layout {
            ViewType::Bsp => {
                let leaf = self.pick_insertion_leaf(focused);
                self.split_node(leaf, window_id);
                if self.config.auto_balance != NodeSplit::None {
                    self.balance(self.config.auto_balance);
                    self.update(self.root);
                    Some(self.root)
                } else {
                    Some(leaf)
                }
            }
            ViewType::Stack => {
                self.stack_window(root, window_id);
                Some(root)
            }
            ViewType::Float => None,
        }
    }

    // --- removal (view_remove_window_node) ---

    /// `view_remove_window_node`: remove `window_id`. Returns the node that
    /// should be reflowed (parent, or root after auto-balance), if any.
    pub fn remove_window(&mut self, window_id: u32) -> Option<NodeId> {
        let id = self.find_window_node(window_id)?;

        // Window shares a leaf with others: just drop it from the lists.
        if self.nodes[id].window_list.len() > 1 {
            let node = &mut self.nodes[id];
            node.window_list.retain(|&w| w != window_id);
            node.window_order.retain(|&w| w != window_id);
            if self.insertion_point == Some(window_id) {
                self.insertion_point = self.nodes[id].window_order.first().copied();
            }
            return None;
        }

        // Sole window in the root leaf: clear the whole tree.
        if id == self.root {
            self.insertion_point = None;
            let node = &mut self.nodes[id];
            node.window_list.clear();
            node.window_order.clear();
            node.insert_dir = 0;
            self.update(self.root);
            return None;
        }

        // Otherwise the sibling's contents collapse into the parent.
        let parent = self.nodes[id].parent.unwrap();
        let sibling = if self.is_right_child(id) {
            self.nodes[parent].left.unwrap()
        } else {
            self.nodes[parent].right.unwrap()
        };

        let sib_list = std::mem::take(&mut self.nodes[sibling].window_list);
        let sib_order = std::mem::take(&mut self.nodes[sibling].window_order);
        let sib_left = self.nodes[sibling].left;
        let sib_right = self.nodes[sibling].right;
        let sib_insert_dir = self.nodes[sibling].insert_dir;
        let sib_split = self.nodes[sibling].split;
        let sib_child = self.nodes[sibling].child;
        let sibling_is_leaf = self.nodes[sibling].is_leaf();

        {
            let p = &mut self.nodes[parent];
            p.window_list = sib_list;
            p.window_order = sib_order;
            p.left = None;
            p.right = None;
            if sib_insert_dir != 0 {
                p.insert_dir = sib_insert_dir;
                p.split = sib_split;
                p.child = sib_child;
            }
        }

        if !sibling_is_leaf {
            // Sibling was intermediate: adopt its children into the parent.
            self.nodes[parent].left = sib_left;
            self.nodes[parent].right = sib_right;
            if let Some(l) = sib_left {
                self.nodes[l].parent = Some(parent);
            }
            if let Some(r) = sib_right {
                self.nodes[r].parent = Some(parent);
            }
            self.update(parent);
        }

        self.free.push(sibling);
        self.free.push(id);

        if self.config.auto_balance != NodeSplit::None {
            self.balance(self.config.auto_balance);
            self.update(self.root);
            return Some(self.root);
        }

        Some(parent)
    }

    // --- transforms (rotate / mirror / equalize / balance) ---

    /// `window_node_rotate`: rotate the subtree at `id` by 90/180/270 degrees.
    pub fn rotate(&mut self, id: NodeId, degrees: i32) {
        let split = self.nodes[id].split;
        if (degrees == 90 && split == NodeSplit::Vertical)
            || (degrees == 270 && split == NodeSplit::Horizontal)
            || degrees == 180
        {
            let node = &mut self.nodes[id];
            std::mem::swap(&mut node.left, &mut node.right);
            node.ratio = 1.0 - node.ratio;
        }

        if degrees != 180 {
            let node = &mut self.nodes[id];
            node.split = match node.split {
                NodeSplit::Horizontal => NodeSplit::Vertical,
                NodeSplit::Vertical => NodeSplit::Horizontal,
                other => other,
            };
        }

        if !self.nodes[id].is_leaf() {
            let (l, r) = (self.nodes[id].left, self.nodes[id].right);
            if let Some(l) = l {
                self.rotate(l, degrees);
            }
            if let Some(r) = r {
                self.rotate(r, degrees);
            }
        }
    }

    /// `window_node_mirror`: flip children across `axis` throughout the subtree.
    pub fn mirror(&mut self, id: NodeId, axis: NodeSplit) {
        if self.nodes[id].is_leaf() {
            return;
        }
        let (l, r) = (self.nodes[id].left.unwrap(), self.nodes[id].right.unwrap());
        self.mirror(l, axis);
        self.mirror(r, axis);
        if self.nodes[id].split == axis {
            self.nodes[id].left = Some(r);
            self.nodes[id].right = Some(l);
        }
    }

    /// `window_node_equalize`: reset matching splits to the default ratio.
    pub fn equalize(&mut self, id: NodeId, axis_flag: NodeSplit) {
        if let Some(l) = self.nodes[id].left {
            self.equalize(l, axis_flag);
        }
        if let Some(r) = self.nodes[id].right {
            self.equalize(r, axis_flag);
        }
        let node = &mut self.nodes[id];
        let matches = (axis_flag == NodeSplit::Vertical && node.split == NodeSplit::Vertical)
            || (axis_flag == NodeSplit::Horizontal && node.split == NodeSplit::Horizontal);
        if matches {
            node.ratio = self.config.split_ratio;
        }
    }

    /// `window_node_balance` over the whole tree for the given axis flag.
    pub fn balance(&mut self, axis_flag: NodeSplit) {
        self.balance_node(self.root, axis_flag);
    }

    fn balance_node(&mut self, id: NodeId, axis_flag: NodeSplit) -> BalanceNode {
        if self.nodes[id].is_leaf() {
            let parent_split = self.nodes[id].parent.map(|p| self.nodes[p].split);
            return BalanceNode {
                y_count: i32::from(parent_split == Some(NodeSplit::Vertical)),
                x_count: i32::from(parent_split == Some(NodeSplit::Horizontal)),
            };
        }

        let left = self.balance_node(self.nodes[id].left.unwrap(), axis_flag);
        let right = self.balance_node(self.nodes[id].right.unwrap(), axis_flag);
        let mut total = BalanceNode {
            y_count: left.y_count + right.y_count,
            x_count: left.x_count + right.x_count,
        };

        let split = self.nodes[id].split;
        if axis_flag == NodeSplit::Vertical && split == NodeSplit::Vertical {
            self.nodes[id].ratio = left.y_count as f32 / total.y_count as f32;
            total.y_count -= 1;
        }
        if axis_flag == NodeSplit::Horizontal && split == NodeSplit::Horizontal {
            self.nodes[id].ratio = left.x_count as f32 / total.x_count as f32;
            total.x_count -= 1;
        }

        if let Some(parent) = self.nodes[id].parent {
            let parent_split = self.nodes[parent].split;
            total.y_count += i32::from(parent_split == NodeSplit::Vertical);
            total.x_count += i32::from(parent_split == NodeSplit::Horizontal);
        }

        total
    }

    // --- fence + resize (window_node_fence / resize_window_relative) ---

    /// `window_node_fence`: nearest ancestor whose split divides `id` along
    /// `direction`. This is the divider that resizing `id` toward `direction`
    /// should move.
    pub fn fence(&self, id: NodeId, direction: Direction) -> Option<NodeId> {
        let area = self.nodes[id].area;
        let mut current = self.nodes[id].parent;
        while let Some(parent) = current {
            let p = &self.nodes[parent];
            let matches = match direction {
                Direction::North => p.split == NodeSplit::Horizontal && p.area.y < area.y,
                Direction::West => p.split == NodeSplit::Vertical && p.area.x < area.x,
                Direction::South => {
                    p.split == NodeSplit::Horizontal && (p.area.y + p.area.h) > (area.y + area.h)
                }
                Direction::East => {
                    p.split == NodeSplit::Vertical && (p.area.x + p.area.w) > (area.x + area.w)
                }
            };
            if matches {
                return Some(parent);
            }
            current = p.parent;
        }
        None
    }

    /// `window_manager_resize_window_relative`'s tree half: nudge the dividers
    /// around the leaf holding `window_id` by `dx`/`dy` points, per the active
    /// resize `handle` (a bitwise-or of [`HANDLE_TOP`]/[`HANDLE_BOTTOM`]/
    /// [`HANDLE_LEFT`]/[`HANDLE_RIGHT`]). Returns false if no divider can move.
    pub fn resize_window(&mut self, window_id: u32, handle: u8, dx: f32, dy: f32) -> bool {
        let Some(node) = self.find_window_node(window_id) else {
            return false;
        };

        let x_fence = if handle & HANDLE_TOP != 0 {
            self.fence(node, Direction::North)
        } else if handle & HANDLE_BOTTOM != 0 {
            self.fence(node, Direction::South)
        } else {
            None
        };
        let y_fence = if handle & HANDLE_LEFT != 0 {
            self.fence(node, Direction::West)
        } else if handle & HANDLE_RIGHT != 0 {
            self.fence(node, Direction::East)
        } else {
            None
        };

        if x_fence.is_none() && y_fence.is_none() {
            return false;
        }

        if let Some(f) = y_fence {
            let area_w = self.nodes[f].area.w;
            let sr = self.nodes[f].ratio + dx / area_w;
            self.nodes[f].ratio = sr.clamp(0.1, 0.9);
        }
        if let Some(f) = x_fence {
            let area_h = self.nodes[f].area.h;
            let sr = self.nodes[f].ratio + dy / area_h;
            self.nodes[f].ratio = sr.clamp(0.1, 0.9);
        }

        self.update(self.root);
        true
    }

    // --- swap (window_node_swap_window_list / window_manager_swap_window) ---

    /// `window_node_swap_window_list`: exchange the window contents of two leaves.
    pub fn swap_window_lists(&mut self, a: NodeId, b: NodeId) {
        let a_list = std::mem::take(&mut self.nodes[a].window_list);
        let a_order = std::mem::take(&mut self.nodes[a].window_order);
        self.nodes[a].window_list = std::mem::take(&mut self.nodes[b].window_list);
        self.nodes[a].window_order = std::mem::take(&mut self.nodes[b].window_order);
        self.nodes[b].window_list = a_list;
        self.nodes[b].window_order = a_order;
    }

    /// Same-space subset of `window_manager_swap_window`: swap the positions of
    /// `a` and `b`. When they share a leaf, swap their slots in place; otherwise
    /// swap the two leaves' contents. Cross-space moves and focus changes are
    /// the daemon's job and are not handled here. Returns false if either window
    /// is not managed.
    pub fn swap_windows(&mut self, a: u32, b: u32) -> bool {
        if a == b {
            return false;
        }
        let (Some(a_node), Some(b_node)) = (self.find_window_node(a), self.find_window_node(b))
        else {
            return false;
        };

        if a_node == b_node {
            let node = &mut self.nodes[a_node];
            for slot in node.window_list.iter_mut() {
                *slot = if *slot == a {
                    b
                } else if *slot == b {
                    a
                } else {
                    *slot
                };
            }
            for slot in node.window_order.iter_mut() {
                *slot = if *slot == a {
                    b
                } else if *slot == b {
                    a
                } else {
                    *slot
                };
            }
            return true;
        }

        // The insertion point follows whichever window left its leaf.
        if let Some(p) = self.insertion_point {
            if self.nodes[a_node].contains_window(p) {
                self.insertion_point = Some(b);
            } else if self.nodes[b_node].contains_window(p) {
                self.insertion_point = Some(a);
            }
        }

        self.swap_window_lists(a_node, b_node);
        true
    }

    /// `window_manager_warp_window` (single-space BSP core): move `src` to
    /// `target`'s position by removing it and re-inserting it at the target's
    /// node, restructuring the tree (unlike [`Self::swap_windows`], which only
    /// exchanges leaf contents). Insertion follows the tree's normal split policy;
    /// the C daemon's `:NaturalWarp` child-distance heuristic and cross-space warp
    /// are deferred (they need live geometry / multi-view state). Returns `false`
    /// if either window is missing, they are the same, they share a leaf, or the
    /// view is not BSP.
    pub fn warp_window(&mut self, src: u32, target: u32) -> bool {
        if src == target || self.layout != ViewType::Bsp {
            return false;
        }
        let (Some(src_node), Some(target_node)) =
            (self.find_window_node(src), self.find_window_node(target))
        else {
            return false;
        };
        if src_node == target_node {
            return false;
        }

        self.remove_window(src);
        self.add_window(src, Some(target));
        self.update(self.root);
        true
    }

    // --- directional neighbor (view_find_window_node_in_direction) ---

    /// `view_find_window_node_in_direction`: closest leaf to `source` in
    /// `direction`. Ties are broken by smaller [`NodeId`] here; the C daemon
    /// breaks them by window z-order rank, which this pure layer cannot see.
    pub fn find_node_in_direction(&self, source: NodeId, direction: Direction) -> Option<NodeId> {
        let source_area = self.nodes[source].area;
        let mut best: Option<NodeId> = None;
        let mut best_distance = i32::MAX;

        for target in self.leaves() {
            if target == source {
                continue;
            }
            let target_area = self.nodes[target].area;
            if source_area.is_in_direction(target_area, direction) {
                let distance = source_area.distance_in_direction(target_area, direction);
                if distance < best_distance {
                    best = Some(target);
                    best_distance = distance;
                }
            }
        }

        best
    }
}

#[cfg(test)]
mod tests {
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
}
