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
    Area, ConfigOp, Message, NodeSplit, QueryCommand, QueryScopeKind, QueryTarget, Selector,
    SpaceAction, Tree, ViewType, WindowAction, WindowFrame, parse_message,
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
    /// A display was discovered, with its full display frame.
    DisplayCreated { display_id: u32, frame: Area },
    /// A display disappeared.
    DisplayRemoved { display_id: u32 },
    /// A new managed window appeared on the active space.
    WindowCreated { window_id: u32 },
    /// A managed window went away.
    WindowDestroyed { window_id: u32 },
    /// Focus moved to a window.
    WindowFocused { window_id: u32 },
    /// A space was discovered, with its usable (already padded or full) frame.
    SpaceCreated { sid: u64, frame: Area },
    /// A space was discovered on a known display.
    SpaceCreatedOnDisplay {
        sid: u64,
        display_id: u32,
        frame: Area,
    },
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

#[derive(Debug, Clone, Copy, PartialEq)]
struct DisplayInfo {
    frame: Area,
}

/// Owns all mutable daemon state.
#[derive(Debug, Default)]
pub struct AppState {
    pub config: Config,
    displays: HashMap<u32, DisplayInfo>,
    space_displays: HashMap<u64, u32>,
    spaces: HashMap<u64, Tree>,
    active_space: Option<u64>,
    focused_window: Option<u32>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a display's frame.
    pub fn add_display(&mut self, display_id: u32, frame: Area) {
        self.displays.insert(display_id, DisplayInfo { frame });
    }

    pub fn remove_display(&mut self, display_id: u32) {
        self.displays.remove(&display_id);
        self.space_displays.retain(|_, did| *did != display_id);
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

    /// Register a space and associate it with a display.
    pub fn add_space_to_display(&mut self, sid: u64, display_id: u32, area: Area) {
        self.add_space(sid, area);
        self.space_displays.insert(sid, display_id);
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
            StateEvent::DisplayCreated { display_id, frame } => {
                self.add_display(display_id, frame);
            }
            StateEvent::DisplayRemoved { display_id } => self.remove_display(display_id),
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
            StateEvent::SpaceCreatedOnDisplay {
                sid,
                display_id,
                frame,
            } => self.add_space_to_display(sid, display_id, frame),
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
            Message::Query(cmd) => self.dispatch_query(&cmd),
            // Domains whose effects need the macOS layers are accepted but not
            // yet enacted here; report them rather than silently succeeding.
            Message::Display(_) | Message::Rule(_) | Message::Signal(_) => {
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

    fn dispatch_query(&self, cmd: &QueryCommand) -> Response {
        match cmd.target {
            QueryTarget::Displays => self.query_displays(cmd),
            QueryTarget::Spaces => self.query_spaces(cmd),
            QueryTarget::Windows => self.query_windows(cmd),
        }
    }

    fn query_displays(&self, cmd: &QueryCommand) -> Response {
        let properties = query_properties(
            &cmd.properties,
            &["id", "index", "frame", "spaces", "has-focus"],
            "display",
        )?;

        match &cmd.scope {
            Some((QueryScopeKind::Display, selector)) => {
                let display_id = self.resolve_display_selector(selector.as_ref())?;
                let display = self
                    .displays
                    .get(&display_id)
                    .ok_or_else(|| "could not retrieve display details.".to_string())?;
                Ok(Some(format!(
                    "{}\n",
                    self.serialize_display(display_id, display, &properties)
                )))
            }
            None => {
                let mut display_ids = self.displays.keys().copied().collect::<Vec<_>>();
                display_ids.sort_unstable();
                let mut output = String::from("[");
                for (idx, display_id) in display_ids.into_iter().enumerate() {
                    if idx > 0 {
                        output.push(',');
                    }
                    let display = self.displays.get(&display_id).unwrap();
                    output.push_str(&self.serialize_display(display_id, display, &properties));
                }
                output.push_str("]\n");
                Ok(Some(output))
            }
            Some((QueryScopeKind::Space, selector)) => {
                let sid = self.resolve_space_selector(selector.as_ref())?;
                let display_id = self
                    .space_displays
                    .get(&sid)
                    .copied()
                    .ok_or_else(|| "could not retrieve display details.".to_string())?;
                let display = self
                    .displays
                    .get(&display_id)
                    .ok_or_else(|| "could not retrieve display details.".to_string())?;
                Ok(Some(format!(
                    "{}\n",
                    self.serialize_display(display_id, display, &properties)
                )))
            }
            Some((QueryScopeKind::Window, selector)) => {
                let window_id = match selector.as_ref() {
                    Some(selector) => self.resolve_window(selector)?,
                    None => self.require_focused()?,
                };
                let sid = self
                    .window_space(window_id)
                    .ok_or_else(|| "could not retrieve display details.".to_string())?;
                let display_id = self
                    .space_displays
                    .get(&sid)
                    .copied()
                    .ok_or_else(|| "could not retrieve display details.".to_string())?;
                let display = self
                    .displays
                    .get(&display_id)
                    .ok_or_else(|| "could not retrieve display details.".to_string())?;
                Ok(Some(format!(
                    "{}\n",
                    self.serialize_display(display_id, display, &properties)
                )))
            }
        }
    }

    fn query_spaces(&self, cmd: &QueryCommand) -> Response {
        let properties = query_properties(
            &cmd.properties,
            &[
                "id",
                "type",
                "windows",
                "first-window",
                "last-window",
                "has-focus",
                "is-visible",
            ],
            "space",
        )?;

        match &cmd.scope {
            Some((QueryScopeKind::Space, selector)) => {
                let sid = self.resolve_space_selector(selector.as_ref())?;
                let tree = self
                    .spaces
                    .get(&sid)
                    .ok_or_else(|| "could not retrieve space details.".to_string())?;
                Ok(Some(format!(
                    "{}\n",
                    self.serialize_space(sid, tree, &properties)
                )))
            }
            None => {
                let mut spaces = self.spaces.iter().collect::<Vec<_>>();
                spaces.sort_by_key(|(sid, _)| *sid);
                let mut output = String::from("[");
                for (idx, (&sid, tree)) in spaces.into_iter().enumerate() {
                    if idx > 0 {
                        output.push(',');
                    }
                    output.push_str(&self.serialize_space(sid, tree, &properties));
                }
                output.push_str("]\n");
                Ok(Some(output))
            }
            Some((QueryScopeKind::Display, selector)) => {
                let display_id = self.resolve_display_selector(selector.as_ref())?;
                let sids = self.display_spaces(display_id);
                let mut output = String::from("[");
                for (idx, sid) in sids.into_iter().enumerate() {
                    if idx > 0 {
                        output.push(',');
                    }
                    let tree = self
                        .spaces
                        .get(&sid)
                        .ok_or_else(|| "could not retrieve spaces for display.".to_string())?;
                    output.push_str(&self.serialize_space(sid, tree, &properties));
                }
                output.push_str("]\n");
                Ok(Some(output))
            }
            Some((QueryScopeKind::Window, _)) => {
                Err("query --spaces --window needs live window state".to_string())
            }
        }
    }

    fn query_windows(&self, cmd: &QueryCommand) -> Response {
        let properties =
            query_properties(&cmd.properties, &["id", "frame", "has-focus"], "window")?;

        match &cmd.scope {
            Some((QueryScopeKind::Window, selector)) => {
                let window_id = match selector.as_ref() {
                    Some(selector) => self.resolve_window(selector)?,
                    None => self.require_focused()?,
                };
                let frame = self
                    .window_frame(window_id)
                    .ok_or_else(|| "could not retrieve window details.".to_string())?;
                Ok(Some(format!(
                    "{}\n",
                    self.serialize_window(frame, &properties)
                )))
            }
            Some((QueryScopeKind::Space, selector)) => {
                let sid = self.resolve_space_selector(selector.as_ref())?;
                let frames = self
                    .flush(sid)
                    .ok_or_else(|| "could not retrieve windows for space.".to_string())?;
                Ok(Some(self.serialize_window_array(&frames, &properties)))
            }
            None => {
                let mut sids = self.spaces.keys().copied().collect::<Vec<_>>();
                sids.sort_unstable();
                let frames = sids
                    .into_iter()
                    .flat_map(|sid| self.flush(sid).unwrap_or_default())
                    .collect::<Vec<_>>();
                Ok(Some(self.serialize_window_array(&frames, &properties)))
            }
            Some((QueryScopeKind::Display, selector)) => {
                let display_id = self.resolve_display_selector(selector.as_ref())?;
                let frames = self
                    .display_spaces(display_id)
                    .into_iter()
                    .flat_map(|sid| self.flush(sid).unwrap_or_default())
                    .collect::<Vec<_>>();
                Ok(Some(self.serialize_window_array(&frames, &properties)))
            }
        }
    }

    fn serialize_window_array(&self, frames: &[WindowFrame], properties: &[&str]) -> String {
        let mut output = String::from("[");
        for (idx, frame) in frames.iter().enumerate() {
            if idx > 0 {
                output.push(',');
            }
            output.push_str(&self.serialize_window(*frame, properties));
        }
        output.push_str("]\n");
        output
    }

    fn serialize_display(
        &self,
        display_id: u32,
        display: &DisplayInfo,
        properties: &[&str],
    ) -> String {
        let mut fields = Vec::new();
        for property in properties {
            match *property {
                "id" => fields.push(format!("\t\"id\":{display_id}")),
                "index" => fields.push(format!(
                    "\t\"index\":{}",
                    self.display_index(display_id).unwrap_or(0)
                )),
                "frame" => fields.push(format_area("frame", display.frame)),
                "spaces" => fields.push(format!(
                    "\t\"spaces\":[{}]",
                    join_u64s(&self.display_spaces(display_id))
                )),
                "has-focus" => fields.push(format!(
                    "\t\"has-focus\":{}",
                    json_bool(self.active_display() == Some(display_id))
                )),
                _ => unreachable!("query_properties rejects unsupported properties"),
            }
        }
        format!("{{\n{}\n}}", fields.join(",\n"))
    }

    fn serialize_space(&self, sid: u64, tree: &Tree, properties: &[&str]) -> String {
        let windows = tree.window_list();
        let mut fields = Vec::new();
        for property in properties {
            match *property {
                "id" => fields.push(format!("\t\"id\":{sid}")),
                "type" => fields.push(format!("\t\"type\":\"{}\"", view_type_str(tree.layout))),
                "windows" => fields.push(format!("\t\"windows\":[{}]", join_ids(&windows))),
                "first-window" => fields.push(format!(
                    "\t\"first-window\":{}",
                    windows.first().copied().unwrap_or(0)
                )),
                "last-window" => fields.push(format!(
                    "\t\"last-window\":{}",
                    windows.last().copied().unwrap_or(0)
                )),
                "has-focus" => fields.push(format!(
                    "\t\"has-focus\":{}",
                    json_bool(self.active_space == Some(sid))
                )),
                "is-visible" => fields.push(format!(
                    "\t\"is-visible\":{}",
                    json_bool(self.active_space == Some(sid))
                )),
                _ => unreachable!("query_properties rejects unsupported properties"),
            }
        }
        format!("{{\n{}\n}}", fields.join(",\n"))
    }

    fn serialize_window(&self, frame: WindowFrame, properties: &[&str]) -> String {
        let mut fields = Vec::new();
        for property in properties {
            match *property {
                "id" => fields.push(format!("\t\"id\":{}", frame.window_id)),
                "frame" => fields.push(format_area("frame", frame.area)),
                "has-focus" => fields.push(format!(
                    "\t\"has-focus\":{}",
                    json_bool(self.focused_window == Some(frame.window_id))
                )),
                _ => unreachable!("query_properties rejects unsupported properties"),
            }
        }
        format!("{{\n{}\n}}", fields.join(",\n"))
    }

    fn window_frame(&self, window_id: u32) -> Option<WindowFrame> {
        self.spaces
            .values()
            .flat_map(Tree::capture)
            .find(|frame| frame.window_id == window_id)
    }

    fn window_space(&self, window_id: u32) -> Option<u64> {
        self.spaces
            .iter()
            .find_map(|(&sid, tree)| tree.window_list().contains(&window_id).then_some(sid))
    }

    fn display_spaces(&self, display_id: u32) -> Vec<u64> {
        let mut spaces = self
            .space_displays
            .iter()
            .filter_map(|(&sid, &did)| (did == display_id).then_some(sid))
            .collect::<Vec<_>>();
        spaces.sort_unstable();
        spaces
    }

    fn active_display(&self) -> Option<u32> {
        self.active_space
            .and_then(|sid| self.space_displays.get(&sid).copied())
    }

    fn display_index(&self, display_id: u32) -> Option<usize> {
        let mut display_ids = self.displays.keys().copied().collect::<Vec<_>>();
        display_ids.sort_unstable();
        display_ids
            .iter()
            .position(|did| *did == display_id)
            .map(|idx| idx + 1)
    }

    fn resolve_display_selector(&self, selector: Option<&Selector>) -> Result<u32, String> {
        match selector {
            Some(Selector::Index(index)) => {
                let mut display_ids = self.displays.keys().copied().collect::<Vec<_>>();
                display_ids.sort_unstable();
                let index = (*index as usize).checked_sub(1).ok_or_else(|| {
                    "could not locate display with arrangement index '0'.".to_string()
                })?;
                display_ids.get(index).copied().ok_or_else(|| {
                    format!(
                        "could not locate display with arrangement index '{}'.",
                        index + 1
                    )
                })
            }
            Some(selector) => Err(format!(
                "selector {selector:?} cannot be resolved without live display state"
            )),
            None => self
                .active_display()
                .ok_or_else(|| "could not locate the selected display.".to_string()),
        }
    }

    fn resolve_space_selector(&self, selector: Option<&Selector>) -> Result<u64, String> {
        match selector {
            Some(Selector::Index(id)) => Ok(u64::from(*id)),
            Some(selector) => Err(format!(
                "selector {selector:?} cannot be resolved without live space state"
            )),
            None => self
                .active_space
                .ok_or_else(|| "no active space".to_string()),
        }
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

fn query_properties<'a>(
    requested: &'a [String],
    default_properties: &'a [&'a str],
    entity: &str,
) -> Result<Vec<&'a str>, String> {
    if requested.is_empty() {
        return Ok(default_properties.to_vec());
    }

    let mut properties = Vec::with_capacity(requested.len());
    for property in requested {
        let property = property.as_str();
        if default_properties.contains(&property) {
            properties.push(property);
        } else {
            return Err(format!(
                "'{property}' is not available from pure {entity} state"
            ));
        }
    }
    Ok(properties)
}

fn view_type_str(layout: ViewType) -> &'static str {
    match layout {
        ViewType::Bsp => "bsp",
        ViewType::Stack => "stack",
        ViewType::Float => "float",
    }
}

fn join_ids(windows: &[u32]) -> String {
    windows
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn join_u64s(values: &[u64]) -> String {
    values
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_area(name: &str, area: Area) -> String {
    format!(
        "\t\"{name}\":{{\n\t\t\"x\":{:.4},\n\t\t\"y\":{:.4},\n\t\t\"w\":{:.4},\n\t\t\"h\":{:.4}\n\t}}",
        area.x, area.y, area.w, area.h
    )
}

fn json_bool(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
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
        let mut state = state_with_space();
        assert_eq!(
            state.handle_tokens(&toks(&["query", "--windows", "app"])),
            Err("'app' is not available from pure window state".to_string())
        );
    }

    #[test]
    fn unhandled_domain_reports() {
        let mut state = AppState::new();
        assert!(state.handle_tokens(&toks(&["rule", "--list"])).is_err());
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
