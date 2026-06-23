//! Daemon-side state and message dispatcher.
//!
//! This is the start of Phase 4: a single owner of mutable state (replacing the
//! C globals `g_window_manager` / `g_space_manager`) plus a dispatcher that
//! applies a parsed [`Message`] to it. It is deliberately pure — no macOS APIs —
//! so the control-plane logic is unit-testable. The macOS-facing layers will
//! later feed this state from real events and flush its layout back to windows.
//!
//! Window selectors are resolved against the active layout tree: numeric ids,
//! `first`/`last`, `next`/`prev`, and cardinal directions. Selectors that need
//! live state the tree does not hold (`recent`, `mouse`, `stack[.N]`, labels)
//! return an explicit error rather than silently guessing.

use std::collections::HashMap;

use yabai_core::{
    Area, ConfigOp, Message, NodeSplit, Selector, SpaceAction, Tree, WindowAction, WindowFrame,
    parse_message,
};

use crate::config::Config;

/// The outcome of dispatching a message: optional text to send back to the
/// client (queries/gets), or an error message (the daemon's `daemon_fail`).
pub type Response = Result<Option<String>, String>;

/// A system event with the payload [`AppState`] needs to update itself.
///
/// This is the typed, data-carrying counterpart to [`crate::Event`] (which is
/// just the name list mirroring `src/event_loop.h`). The macOS layer translates
/// raw AX/SkyLight callbacks into these and feeds them to the actor thread,
/// exactly as `src/event_loop.c` serializes events through one worker queue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StateEvent {
    /// A new managed window appeared on the active space.
    WindowCreated { window_id: u32 },
    /// A managed window went away.
    WindowDestroyed { window_id: u32 },
    /// Focus moved to a window.
    WindowFocused { window_id: u32 },
    /// A space was discovered, with its usable (already padded or full) frame.
    SpaceCreated { sid: u64, frame: Area },
    /// The active space changed.
    SpaceChanged { sid: u64 },
    /// A space's display frame changed (display add/resize/move); re-inset and
    /// re-flow.
    DisplayFrameChanged { sid: u64, frame: Area },
}

/// Sink that applies computed window frames to the real world.
///
/// This is the seam between the pure control plane and the macOS layer: the
/// `yabai-macos` crate will implement it to move/resize windows via AX/SkyLight,
/// while tests implement it by recording frames. Keeping it a trait means
/// [`AppState`] never depends on macOS.
pub trait LayoutSink {
    /// Place `frame.window_id` at `frame.area`.
    fn move_window(&mut self, frame: WindowFrame);
}

/// A [`LayoutSink`] that records every placement, for tests and dry-runs.
#[derive(Debug, Default)]
pub struct RecordingSink {
    pub moves: Vec<WindowFrame>,
}

impl LayoutSink for RecordingSink {
    fn move_window(&mut self, frame: WindowFrame) {
        self.moves.push(frame);
    }
}

/// Owns all mutable daemon state.
#[derive(Debug, Default)]
pub struct AppState {
    pub config: Config,
    spaces: HashMap<u64, Tree>,
    active_space: Option<u64>,
    focused_window: Option<u32>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a space's layout tree with the given area, using
    /// the current config and the config's default layout.
    pub fn add_space(&mut self, sid: u64, area: Area) {
        let tree = Tree::new(self.config.layout, self.config.layout_config(), area);
        self.spaces.insert(sid, tree);
        if self.active_space.is_none() {
            self.active_space = Some(sid);
        }
    }

    pub fn set_active_space(&mut self, sid: u64) {
        self.active_space = Some(sid);
    }

    pub fn set_focused_window(&mut self, window_id: Option<u32>) {
        self.focused_window = window_id;
    }

    pub fn space(&self, sid: u64) -> Option<&Tree> {
        self.spaces.get(&sid)
    }

    pub fn active_tree_mut(&mut self) -> Result<&mut Tree, String> {
        let sid = self
            .active_space
            .ok_or_else(|| "no active space".to_string())?;
        self.spaces
            .get_mut(&sid)
            .ok_or_else(|| "active space has no layout".to_string())
    }

    /// Add a window to the active space (respecting the current focus).
    pub fn add_window(&mut self, window_id: u32) -> Result<(), String> {
        let focused = self.focused_window;
        self.active_tree_mut()?.add_window(window_id, focused);
        self.focused_window = Some(window_id);
        Ok(())
    }

    /// Remove a window from the active space.
    pub fn remove_window(&mut self, window_id: u32) -> Result<(), String> {
        self.active_tree_mut()?.remove_window(window_id);
        if self.focused_window == Some(window_id) {
            self.focused_window = None;
        }
        Ok(())
    }

    /// Set a space's usable frame from its full display frame, insetting by the
    /// configured paddings (as the C view does when the space manager sets the
    /// root area), then re-flow the tree.
    pub fn set_space_frame(&mut self, sid: u64, display_frame: Area) -> Result<(), String> {
        let c = &self.config;
        let area = Area::new(
            display_frame.x + c.left_padding as f32,
            display_frame.y + c.top_padding as f32,
            display_frame.w - (c.left_padding + c.right_padding) as f32,
            display_frame.h - (c.top_padding + c.bottom_padding) as f32,
        );
        let tree = self
            .spaces
            .get_mut(&sid)
            .ok_or_else(|| "space has no layout".to_string())?;
        tree.set_root_area(area);
        Ok(())
    }

    /// The target frame for every managed window in a space — what the macOS
    /// layer turns into window-move operations (`window_node_flush` in C).
    pub fn flush(&self, sid: u64) -> Option<Vec<WindowFrame>> {
        self.spaces.get(&sid).map(Tree::capture)
    }

    /// [`Self::flush`] for the active space.
    pub fn flush_active(&self) -> Option<Vec<WindowFrame>> {
        self.active_space.and_then(|sid| self.flush(sid))
    }

    /// Push the active space's layout through `sink`, returning how many windows
    /// were placed. This is the call the daemon makes after any state change.
    pub fn flush_active_to(&self, sink: &mut impl LayoutSink) -> usize {
        let frames = self.flush_active().unwrap_or_default();
        let count = frames.len();
        for frame in frames {
            sink.move_window(frame);
        }
        count
    }

    /// Apply an event and immediately flush the resulting layout to `sink`.
    pub fn handle_event_and_flush(
        &mut self,
        event: StateEvent,
        sink: &mut impl LayoutSink,
    ) -> Result<usize, String> {
        self.handle_event(event)?;
        Ok(self.flush_active_to(sink))
    }

    /// Apply a system [`StateEvent`], updating state to match the world.
    ///
    /// `WindowCreated` is ignored while `config.manage` is off (yabai is not
    /// tiling), mirroring the C daemon's manage gate. The macOS layer should
    /// call [`Self::flush_active`] afterward to push the new layout to windows.
    pub fn handle_event(&mut self, event: StateEvent) -> Result<(), String> {
        match event {
            StateEvent::WindowCreated { window_id } => {
                if self.config.manage {
                    self.add_window(window_id)?;
                }
            }
            StateEvent::WindowDestroyed { window_id } => {
                // Only act if the window is actually managed somewhere.
                if self.active_space.is_some() {
                    self.remove_window(window_id)?;
                }
            }
            StateEvent::WindowFocused { window_id } => {
                self.focused_window = Some(window_id);
            }
            StateEvent::SpaceCreated { sid, frame } => self.add_space(sid, frame),
            StateEvent::SpaceChanged { sid } => self.active_space = Some(sid),
            StateEvent::DisplayFrameChanged { sid, frame } => self.set_space_frame(sid, frame)?,
        }
        Ok(())
    }

    /// Parse and dispatch a raw `yabai -m` token list.
    pub fn handle_tokens(&mut self, tokens: &[String]) -> Response {
        let message = parse_message(tokens).map_err(|e| e.to_string())?;
        self.dispatch(message)
    }

    /// Dispatch an already-parsed [`Message`].
    pub fn dispatch(&mut self, message: Message) -> Response {
        match message {
            Message::Config(cmd) => self.dispatch_config(&cmd.ops),
            Message::Window(cmd) => self.dispatch_window(&cmd.actions),
            Message::Space(cmd) => self.dispatch_space(&cmd.actions),
            // Domains whose effects need the macOS layers are accepted but not
            // yet enacted here; report them rather than silently succeeding.
            Message::Display(_) | Message::Query(_) | Message::Rule(_) | Message::Signal(_) => {
                Err("domain not yet handled by AppState".to_string())
            }
        }
    }

    fn dispatch_config(&mut self, ops: &[ConfigOp]) -> Response {
        let mut output = String::new();
        let mut layout_dirty = false;
        for op in ops {
            if let Some(text) = self.config.apply(op)? {
                output.push_str(&text);
                output.push('\n');
            } else {
                layout_dirty = true;
            }
        }
        // A config change that affects layout re-flows every known space.
        if layout_dirty {
            let layout_config = self.config.layout_config();
            for tree in self.spaces.values_mut() {
                tree.config = layout_config;
                let root = tree.root();
                tree.update(root);
            }
        }
        Ok((!output.is_empty()).then_some(output))
    }

    fn dispatch_window(&mut self, actions: &[WindowAction]) -> Response {
        for action in actions {
            match action {
                WindowAction::Focus(sel) => {
                    if let Some(sel) = sel {
                        self.focused_window = Some(self.resolve_window(sel)?);
                    }
                }
                WindowAction::Swap(sel) => {
                    let other = self.resolve_window(sel)?;
                    let focused = self.require_focused()?;
                    self.active_tree_mut()?.swap_windows(focused, other);
                }
                WindowAction::Resize { handle, dw, dh } => {
                    let focused = self.require_focused()?;
                    self.active_tree_mut()?
                        .resize_window(focused, *handle, *dw, *dh);
                }
                // Remaining window actions require the macOS layers.
                _ => return Err("window action not yet handled by AppState".to_string()),
            }
        }
        Ok(None)
    }

    fn dispatch_space(&mut self, actions: &[SpaceAction]) -> Response {
        for action in actions {
            let tree = self.active_tree_mut()?;
            let root = tree.root();
            match action {
                SpaceAction::Balance(axis) => {
                    tree.balance(axis.unwrap_or(NodeSplit::Auto));
                    tree.update(root);
                }
                SpaceAction::Equalize(axis) => {
                    tree.equalize(root, axis.unwrap_or(NodeSplit::Auto));
                    tree.update(root);
                }
                SpaceAction::Mirror(axis) => {
                    tree.mirror(root, *axis);
                    tree.update(root);
                }
                SpaceAction::Rotate(degrees) => {
                    tree.rotate(root, *degrees);
                    tree.update(root);
                }
                SpaceAction::Layout(layout) => {
                    tree.layout = *layout;
                }
                // Create/destroy/move/focus/etc. need the macOS layers.
                _ => return Err("space action not yet handled by AppState".to_string()),
            }
        }
        Ok(None)
    }

    fn require_focused(&self) -> Result<u32, String> {
        self.focused_window
            .ok_or_else(|| "no focused window".to_string())
    }

    /// Resolve a window selector against the active space's layout tree.
    ///
    /// Resolved here: a numeric id, `first`/`last` (by tree order), `next`/`prev`
    /// (relative to the focused window in tree order, no wrap), and a cardinal
    /// direction (the top window of the neighboring node). `recent`, `mouse`,
    /// `stack[.N]`, and labels still need live state and are reported.
    fn resolve_window(&self, selector: &Selector) -> Result<u32, String> {
        if let Selector::Index(id) = selector {
            return Ok(*id);
        }

        let sid = self
            .active_space
            .ok_or_else(|| "no active space".to_string())?;
        let tree = self
            .spaces
            .get(&sid)
            .ok_or_else(|| "active space has no layout".to_string())?;
        let windows = tree.window_list();
        let unresolved =
            || format!("selector {selector:?} cannot be resolved without live window state");

        match selector {
            Selector::First => windows
                .first()
                .copied()
                .ok_or_else(|| "no windows".to_string()),
            Selector::Last => windows
                .last()
                .copied()
                .ok_or_else(|| "no windows".to_string()),
            Selector::Next | Selector::Prev => {
                let focused = self.require_focused()?;
                let idx = windows
                    .iter()
                    .position(|&w| w == focused)
                    .ok_or_else(|| "focused window is not on the active space".to_string())?;
                let target = if matches!(selector, Selector::Next) {
                    idx.checked_add(1).filter(|&i| i < windows.len())
                } else {
                    idx.checked_sub(1)
                };
                target
                    .map(|i| windows[i])
                    .ok_or_else(|| "no window in that direction".to_string())
            }
            Selector::Direction(dir) => {
                let focused = self.require_focused()?;
                let node = tree
                    .find_window_node(focused)
                    .ok_or_else(|| "focused window is not on the active space".to_string())?;
                let neighbor = tree
                    .find_node_in_direction(node, *dir)
                    .ok_or_else(|| "no window in that direction".to_string())?;
                tree.node(neighbor)
                    .window_order
                    .first()
                    .copied()
                    .ok_or_else(|| "neighbor node is empty".to_string())
            }
            _ => Err(unresolved()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yabai_core::ViewType;

    fn toks(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn state_with_space() -> AppState {
        let mut state = AppState::new();
        state.add_space(1, Area::new(0.0, 0.0, 1000.0, 1000.0));
        state
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
    fn window_focus_label_selector_still_unsupported() {
        let mut state = state_with_space();
        state.add_window(1).unwrap();
        state.set_focused_window(Some(1));
        let err = state
            .handle_tokens(&toks(&["window", "--focus", "mouse"]))
            .unwrap_err();
        assert!(err.contains("cannot be resolved"));
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
    fn unhandled_domain_reports() {
        let mut state = AppState::new();
        assert!(state.handle_tokens(&toks(&["query", "--windows"])).is_err());
    }

    #[test]
    fn parse_error_surfaces_as_response_error() {
        let mut state = AppState::new();
        assert_eq!(
            state.handle_tokens(&toks(&["bogus"])),
            Err("unknown domain 'bogus'".to_string())
        );
    }
}
