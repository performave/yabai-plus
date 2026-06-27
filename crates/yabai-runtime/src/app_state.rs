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

use std::collections::{HashMap, HashSet};

use regex_lite::Regex;
use yabai_core::{
    Area, ConfigOp, DisplayAction, Layer, Message, NodeSplit, Point, QueryCommand, QueryScopeKind,
    QueryTarget, Rule, RuleApply, RuleCommand, RuleEffects, Selector, Signal, SignalCommand,
    SignalEvent, SpaceAction, Tree, ViewType, WindowAction, WindowFrame, ZoomKind, parse_message,
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
    /// A managed window belongs to a specific space (new or moved).
    WindowAssignedToSpace { window_id: u32, sid: u64 },
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
    /// A space disappeared.
    SpaceRemoved { sid: u64 },
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
    /// The window/space focused immediately before the current one, for the
    /// `recent` selector (mirrors the C `last_window_id` / `last_space_id`).
    last_focused_window: Option<u32>,
    last_active_space: Option<u64>,
    window_meta: HashMap<u32, WindowMeta>,
    window_spaces: HashMap<u32, u64>,
    /// Windows the user floated (`window --toggle float`): kept out of every tree
    /// so they are never tiled, and skipped by reconcile's space assignment.
    floating: HashSet<u32>,
    /// Each display's currently visible space. Every display tiles its own
    /// current space simultaneously, so the daemon flushes all of these, while
    /// `active_space` (the focused display's space) drives command dispatch.
    display_active_space: HashMap<u32, u64>,
    /// Registered `signal`s in insertion order, with optional app/title regexes
    /// pre-compiled. The daemon runs the matching action commands.
    signals: Vec<CompiledSignal>,
    /// Registered window `rule`s in insertion order, each with its filter
    /// patterns pre-compiled. The daemon evaluates these against new windows.
    rules: Vec<CompiledRule>,
    /// User-assigned space labels (`space --label <name>`), `sid -> label`.
    /// Labels are unique across spaces, so a label selector resolves to one sid.
    space_labels: HashMap<u64, String>,
    /// User-assigned display labels (`display --label <name>`), `did -> label`,
    /// also unique across displays.
    display_labels: HashMap<u32, String>,
    /// Last known cursor location (top-left CG coords), set by the daemon before
    /// dispatching a command so the `mouse` selector can resolve.
    cursor_point: Option<Point>,
}

/// A [`Rule`] with its filter patterns compiled to regexes. Absent filters keep
/// a `None` matcher (they never reject a window, matching the C `RULE_*_VALID`
/// behavior).
#[derive(Debug)]
struct CompiledRule {
    rule: Rule,
    app: Option<Regex>,
    title: Option<Regex>,
    role: Option<Regex>,
    subrole: Option<Regex>,
}

#[derive(Debug)]
struct CompiledSignal {
    signal: Signal,
    app: Option<Regex>,
    title: Option<Regex>,
}

impl CompiledSignal {
    fn matches(
        &self,
        event: SignalEvent,
        app: Option<&str>,
        title: Option<&str>,
        active: Option<bool>,
    ) -> bool {
        match event {
            SignalEvent::ApplicationLaunched
            | SignalEvent::ApplicationActivated
            | SignalEvent::ApplicationDeactivated
            | SignalEvent::ApplicationVisible => self.app_matches(app),
            SignalEvent::ApplicationTerminated
            | SignalEvent::ApplicationHidden
            | SignalEvent::WindowDestroyed => self.app_matches(app) && self.active_matches(active),
            SignalEvent::WindowCreated
            | SignalEvent::WindowFocused
            | SignalEvent::WindowDeminimized => self.app_matches(app) && self.title_matches(title),
            SignalEvent::WindowMoved
            | SignalEvent::WindowResized
            | SignalEvent::WindowMinimized
            | SignalEvent::WindowTitleChanged => {
                self.app_matches(app) && self.title_matches(title) && self.active_matches(active)
            }
            _ => true,
        }
    }

    fn app_matches(&self, value: Option<&str>) -> bool {
        signal_filter_ok(self.app.as_ref(), self.signal.app_exclude, value)
    }

    fn title_matches(&self, value: Option<&str>) -> bool {
        signal_filter_ok(self.title.as_ref(), self.signal.title_exclude, value)
    }

    fn active_matches(&self, active: Option<bool>) -> bool {
        match self.signal.active {
            None => true,
            Some(expected) => active == Some(expected),
        }
    }
}

fn signal_filter_ok(pattern: Option<&Regex>, exclude: bool, value: Option<&str>) -> bool {
    match pattern {
        None => true,
        Some(re) => re.is_match(value.unwrap_or_default()) != exclude,
    }
}

impl CompiledRule {
    /// Whether this rule matches a window, mirroring
    /// `window_manager_rule_matches_window`: every present filter must match
    /// (or, for an `!=` filter, must not match). Role/subrole default to empty
    /// strings until AX role discovery is wired up.
    fn matches(&self, app: &str, title: &str, role: &str, subrole: &str) -> bool {
        filter_ok(self.app.as_ref(), self.rule.app_exclude, app)
            && filter_ok(self.title.as_ref(), self.rule.title_exclude, title)
            && filter_ok(self.role.as_ref(), self.rule.role_exclude, role)
            && filter_ok(self.subrole.as_ref(), self.rule.subrole_exclude, subrole)
    }
}

/// One filter's verdict. An absent pattern never rejects. For a present pattern
/// the C keeps the window iff `is_match == !exclude` (a normal filter requires a
/// match; an `!=` filter requires a non-match), i.e. `is_match != exclude`.
fn filter_ok(pattern: Option<&Regex>, exclude: bool, value: &str) -> bool {
    match pattern {
        None => true,
        Some(re) => re.is_match(value) != exclude,
    }
}

fn compile_signal(signal: Signal) -> Result<CompiledSignal, String> {
    let app = compile_signal_regex(signal.app.as_deref(), "app")?;
    let title = compile_signal_regex(signal.title.as_deref(), "title")?;
    Ok(CompiledSignal { signal, app, title })
}

fn compile_signal_regex(pattern: Option<&str>, key: &str) -> Result<Option<Regex>, String> {
    pattern
        .map(|pattern| {
            Regex::new(pattern)
                .map_err(|_| format!("invalid regex pattern '{pattern}' for key '{key}'\n"))
        })
        .transpose()
}

/// Lightweight per-window metadata the macOS layer supplies for queries
/// (`app`/`title`/`pid`). The pure layer only stores and serializes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowMeta {
    pub app: String,
    pub title: String,
    pub pid: i32,
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
        self.display_active_space.remove(&display_id);
        self.space_displays.retain(|_, did| *did != display_id);
    }

    pub fn display_ids(&self) -> Vec<u32> {
        let mut display_ids = self.displays.keys().copied().collect::<Vec<_>>();
        display_ids.sort_unstable();
        display_ids
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
        if !self.spaces.contains_key(&sid) {
            self.add_space(sid, area);
        }
        self.space_displays.insert(sid, display_id);
    }

    pub fn remove_space(&mut self, sid: u64) {
        if let Some(tree) = self.spaces.remove(&sid) {
            for window_id in tree.window_list() {
                self.window_meta.remove(&window_id);
                if self.focused_window == Some(window_id) {
                    self.focused_window = None;
                }
            }
        }
        self.space_displays.remove(&sid);
        self.display_active_space
            .retain(|_, active_sid| *active_sid != sid);
        if self.active_space == Some(sid) {
            self.active_space = self.spaces.keys().copied().min();
        }
    }

    pub fn space_ids(&self) -> Vec<u64> {
        let mut space_ids = self.spaces.keys().copied().collect::<Vec<_>>();
        space_ids.sort_unstable();
        space_ids
    }

    pub fn space_ids_for_display(&self, display_id: u32) -> Vec<u64> {
        self.display_spaces(display_id)
    }

    /// Public space-selector resolution for daemon-side interception (e.g. the
    /// scripting-addition `space --destroy/--move` paths). `None` resolves to the
    /// active space.
    pub fn resolve_space(&self, selector: Option<&Selector>) -> Result<u64, String> {
        self.resolve_space_selector(selector)
    }

    /// Public window-selector resolution for daemon-side interception (e.g. the
    /// scripting-addition `window --space/--display` paths). `None` resolves to the
    /// focused window.
    pub fn resolve_window_selector(&self, selector: Option<&Selector>) -> Result<u32, String> {
        match selector {
            Some(selector) => self.resolve_window(selector),
            None => self.require_focused(),
        }
    }

    pub fn window_space_id(&self, window_id: u32) -> Option<u64> {
        self.window_space(window_id)
    }

    pub fn set_active_space(&mut self, sid: u64) {
        if self.active_space != Some(sid) {
            if let Some(prev) = self.active_space {
                self.last_active_space = Some(prev);
            }
        }
        self.active_space = Some(sid);
    }

    pub fn active_space_id(&self) -> Option<u64> {
        self.active_space
    }

    /// Record the live cursor location so the `mouse` selector can resolve. The
    /// daemon sets this before dispatching each command.
    pub fn set_cursor_point(&mut self, point: Point) {
        self.cursor_point = Some(point);
    }

    /// The display whose frame contains `point` (the `mouse` display).
    fn display_at_point(&self, point: Point) -> Option<u32> {
        self.displays
            .iter()
            .find_map(|(&did, info)| info.frame.contains_point(point).then_some(did))
    }

    /// The managed window whose tiled frame contains `point` (the `mouse` window).
    fn window_at_point(&self, point: Point) -> Option<u32> {
        self.spaces.values().find_map(|tree| {
            tree.capture()
                .into_iter()
                .find_map(|frame| frame.area.contains_point(point).then_some(frame.window_id))
        })
    }

    /// Record `display_id`'s currently visible space (for multi-display flush).
    pub fn set_display_active_space(&mut self, display_id: u32, sid: u64) {
        self.display_active_space.insert(display_id, sid);
    }

    /// The currently visible space on `display_id`, if known.
    pub fn display_active_space_id(&self, display_id: u32) -> Option<u64> {
        self.display_active_space.get(&display_id).copied()
    }

    pub fn set_focused_window(&mut self, window_id: Option<u32>) {
        if window_id.is_some() && window_id != self.focused_window {
            if let Some(prev) = self.focused_window {
                self.last_focused_window = Some(prev);
            }
        }
        self.focused_window = window_id;
    }

    pub fn focused_window_id(&self) -> Option<u32> {
        self.focused_window
    }

    /// The current tiled area of a managed window, from any space's capture.
    /// Used by the daemon's `mouse_follows_focus` to center the cursor on focus.
    pub fn window_area(&self, window_id: u32) -> Option<Area> {
        self.window_frame(window_id).map(|frame| frame.area)
    }

    /// Every window with known metadata, as `(id, app, title, space)`, for the
    /// daemon to re-evaluate rules against on `rule --apply`. Floating windows
    /// report their last-known space; a window with no known space is skipped.
    pub fn windows_with_meta(&self) -> Vec<(u32, String, String, u64)> {
        let mut out = Vec::new();
        for (&id, meta) in &self.window_meta {
            let sid = self
                .window_space(id)
                .or_else(|| self.window_spaces.get(&id).copied());
            if let Some(sid) = sid {
                out.push((id, meta.app.clone(), meta.title.clone(), sid));
            }
        }
        out.sort_unstable_by_key(|(id, ..)| *id);
        out
    }

    /// The owning process id for a window, from its stored metadata.
    pub fn window_pid(&self, window_id: u32) -> Option<i32> {
        self.window_meta.get(&window_id).map(|meta| meta.pid)
    }

    pub fn window_meta(&self, window_id: u32) -> Option<&WindowMeta> {
        self.window_meta.get(&window_id)
    }

    /// Record (or replace) a window's macOS metadata for queries.
    pub fn set_window_meta(&mut self, window_id: u32, meta: WindowMeta) {
        self.window_meta.insert(window_id, meta);
    }

    /// Forget a window's metadata (e.g. when it is destroyed).
    pub fn remove_window_meta(&mut self, window_id: u32) {
        self.window_meta.remove(&window_id);
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
        let sid = self
            .active_space
            .ok_or_else(|| "no active space".to_string())?;
        self.assign_window_to_space(window_id, sid)
    }

    /// Assign a window to `sid`, removing it from any previous tree first.
    pub fn assign_window_to_space(&mut self, window_id: u32, sid: u64) -> Result<(), String> {
        self.window_spaces.insert(window_id, sid);
        // Floating windows are never tiled; this no-op is what keeps reconcile
        // from re-adding them to a tree on every tick.
        if self.floating.contains(&window_id) {
            return Ok(());
        }
        let previous_sid = self.window_space(window_id);
        if previous_sid == Some(sid) {
            return Ok(());
        }
        if !self.spaces.contains_key(&sid) {
            return Err("space has no layout".to_string());
        }

        for tree in self.spaces.values_mut() {
            tree.remove_window(window_id);
        }

        let focused = self.focused_window.filter(|&focused| {
            self.spaces
                .get(&sid)
                .is_some_and(|tree| tree.find_window_node(focused).is_some())
        });
        self.spaces
            .get_mut(&sid)
            .expect("space existence checked above")
            .add_window(window_id, focused);
        if previous_sid.is_none() {
            self.focused_window = Some(window_id);
        }
        Ok(())
    }

    /// Remove a window from whichever space currently owns it (e.g. on destroy),
    /// also clearing any floating mark.
    pub fn remove_window(&mut self, window_id: u32) -> Result<(), String> {
        self.floating.remove(&window_id);
        self.window_spaces.remove(&window_id);
        for tree in self.spaces.values_mut() {
            tree.remove_window(window_id);
        }
        if self.focused_window == Some(window_id) {
            self.focused_window = None;
        }
        Ok(())
    }

    /// Whether `window_id` is currently floating (untiled).
    pub fn is_floating(&self, window_id: u32) -> bool {
        self.floating.contains(&window_id)
    }

    /// Float or unfloat a window (used by the daemon to apply a `manage` rule to a
    /// freshly-discovered window). Floating drops it from every tree and marks it
    /// so reconcile never re-tiles it; unfloating clears the mark and tiles it
    /// back into `sid`.
    pub fn set_window_floating(&mut self, window_id: u32, floating: bool, sid: u64) {
        if floating {
            self.floating.insert(window_id);
            for tree in self.spaces.values_mut() {
                tree.remove_window(window_id);
            }
        } else if self.floating.remove(&window_id) {
            let _ = self.assign_window_to_space(window_id, sid);
        }
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

    /// The target frames for every display's currently visible space — what a
    /// multi-display daemon flushes so all screens tile at once.
    pub fn flush_all_active(&self) -> Vec<WindowFrame> {
        let mut frames = Vec::new();
        for sid in self.display_active_space.values() {
            if let Some(tree) = self.spaces.get(sid) {
                frames.extend(tree.capture());
            }
        }
        frames
    }

    /// Push every display's visible-space layout through `sink`. Falls back to the
    /// single active space when no per-display spaces are recorded (so a
    /// single-display caller that never calls [`Self::set_display_active_space`]
    /// keeps working).
    pub fn flush_all_active_to(&self, sink: &mut impl LayoutSink) -> usize {
        if self.display_active_space.is_empty() {
            return self.flush_active_to(sink);
        }
        let frames = self.flush_all_active();
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
            StateEvent::WindowAssignedToSpace { window_id, sid } => {
                if self.config.manage {
                    self.assign_window_to_space(window_id, sid)?;
                }
            }
            StateEvent::WindowDestroyed { window_id } => {
                // Only act if the window is actually managed somewhere.
                if self.active_space.is_some() {
                    self.remove_window(window_id)?;
                }
            }
            StateEvent::WindowFocused { window_id } => {
                // Through the setter so the `recent` selector tracks live focus.
                self.set_focused_window(Some(window_id));
            }
            StateEvent::SpaceCreated { sid, frame } => self.add_space(sid, frame),
            StateEvent::SpaceCreatedOnDisplay {
                sid,
                display_id,
                frame,
            } => self.add_space_to_display(sid, display_id, frame),
            StateEvent::SpaceRemoved { sid } => self.remove_space(sid),
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
            Message::Window(cmd) => self.dispatch_window(cmd.target.as_ref(), &cmd.actions),
            Message::Space(cmd) => self.dispatch_space(cmd.target.as_ref(), &cmd.actions),
            Message::Query(cmd) => self.dispatch_query(&cmd),
            Message::Signal(cmd) => self.dispatch_signal(cmd),
            Message::Rule(cmd) => self.dispatch_rule(cmd),
            // Domains whose effects need the macOS layers are accepted but not
            // yet enacted here; report them rather than silently succeeding.
            Message::Display(cmd) => self.dispatch_display(cmd.target.as_ref(), &cmd.actions),
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

    fn dispatch_window(&mut self, target: Option<&Selector>, actions: &[WindowAction]) -> Response {
        if let Some(target) = target {
            let resolved = self.resolve_window(target)?;
            self.set_focused_window(Some(resolved));
        }
        for action in actions {
            match action {
                WindowAction::Focus(sel) => {
                    if let Some(sel) = sel {
                        let resolved = self.resolve_window(sel)?;
                        self.set_focused_window(Some(resolved));
                    }
                }
                WindowAction::Swap(sel) => {
                    let other = self.resolve_window(sel)?;
                    let focused = self.require_focused()?;
                    self.active_tree_mut()?.swap_windows(focused, other);
                }
                WindowAction::Warp(sel) => {
                    let target = self.resolve_window(sel)?;
                    let focused = self.require_focused()?;
                    self.active_tree_mut()?.warp_window(focused, target);
                }
                WindowAction::Minimize => {
                    // Validate a window is focused; the macOS layer (daemon) sets
                    // AXMinimized and reconcile drops it from the tree.
                    self.require_focused()?;
                }
                WindowAction::Close => {
                    // Validate a window is focused; the macOS layer (daemon)
                    // presses the AX close button and later reconciliation drops it.
                    self.require_focused()?;
                }
                WindowAction::Toggle(name) => match name.as_str() {
                    "zoom-fullscreen" => {
                        let focused = self.require_focused()?;
                        self.active_tree_mut()?
                            .toggle_zoom(focused, ZoomKind::Fullscreen);
                    }
                    "zoom-parent" => {
                        let focused = self.require_focused()?;
                        self.active_tree_mut()?
                            .toggle_zoom(focused, ZoomKind::Parent);
                    }
                    "float" => {
                        let focused = self.require_focused()?;
                        if self.floating.remove(&focused) {
                            // Un-float: tile it back into the active space.
                            let sid = self
                                .active_space
                                .ok_or_else(|| "no active space".to_string())?;
                            self.assign_window_to_space(focused, sid)?;
                        } else {
                            // Float: drop it from its tree (others re-tile) but keep
                            // it focused and where it is — never moved again.
                            self.floating.insert(focused);
                            for tree in self.spaces.values_mut() {
                                tree.remove_window(focused);
                            }
                        }
                    }
                    "native-fullscreen" => {
                        // Validated no-op: the macOS layer (daemon) toggles
                        // AXFullscreen and reconcile drops the window from the tree
                        // as it leaves for its own fullscreen space (re-added when
                        // toggled back off). Mirrors how minimize is handled.
                        self.require_focused()?;
                    }
                    // Other toggles (sticky/split/shadow) need state the pure /
                    // macOS layers don't model yet.
                    _ => return Err(format!("window toggle '{name}' not yet handled")),
                },
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

    fn dispatch_space(&mut self, target: Option<&Selector>, actions: &[SpaceAction]) -> Response {
        for action in actions {
            // `--label` acts on the selected (or active) space, not the tree.
            if let SpaceAction::Label(label) = action {
                let sid = self.resolve_space_selector(target)?;
                self.set_space_label(sid, label)?;
                continue;
            }
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

    /// Set or clear (empty `label`) a space's label, mirroring the C
    /// `parse_label` rules: numeric labels and the reserved selector keywords are
    /// rejected, an empty label clears it, and labels are unique across spaces
    /// (assigning a name first removes it from any other space).
    fn set_space_label(&mut self, sid: u64, label: &str) -> Result<(), String> {
        if label.is_empty() {
            self.space_labels.remove(&sid);
            return Ok(());
        }
        if label.parse::<f64>().is_ok() {
            return Err(format!("'{label}' cannot be used as a label.\n"));
        }
        const RESERVED: [&str; 6] = ["prev", "next", "first", "last", "recent", "mouse"];
        if RESERVED.contains(&label) {
            return Err(format!(
                "'{label}' is a reserved keyword and cannot be used as a label.\n"
            ));
        }
        self.space_labels.retain(|_, existing| existing != label);
        self.space_labels.insert(sid, label.to_string());
        Ok(())
    }

    fn dispatch_display(
        &mut self,
        target: Option<&Selector>,
        actions: &[DisplayAction],
    ) -> Response {
        for action in actions {
            match action {
                DisplayAction::Label(label) => {
                    let did = self.resolve_display_selector(target)?;
                    self.set_display_label(did, label)?;
                }
                // Focus/move/etc. need the macOS layers (gesture / scripting addition).
                _ => return Err("display action not yet handled by AppState".to_string()),
            }
        }
        Ok(None)
    }

    /// Set or clear (empty `label`) a display's label, mirroring the C
    /// `parse_label` rules for displays: numeric labels and the reserved
    /// direction/selector keywords are rejected, an empty label clears it, and
    /// labels are unique across displays.
    fn set_display_label(&mut self, did: u32, label: &str) -> Result<(), String> {
        if label.is_empty() {
            self.display_labels.remove(&did);
            return Ok(());
        }
        if label.parse::<f64>().is_ok() {
            return Err(format!("'{label}' cannot be used as a label.\n"));
        }
        const RESERVED: [&str; 10] = [
            "north", "east", "south", "west", "prev", "next", "first", "last", "recent", "mouse",
        ];
        if RESERVED.contains(&label) {
            return Err(format!(
                "'{label}' is a reserved keyword and cannot be used as a label.\n"
            ));
        }
        self.display_labels.retain(|_, existing| existing != label);
        self.display_labels.insert(did, label.to_string());
        Ok(())
    }

    fn dispatch_query(&self, cmd: &QueryCommand) -> Response {
        match cmd.target {
            QueryTarget::Displays => self.query_displays(cmd),
            QueryTarget::Spaces => self.query_spaces(cmd),
            QueryTarget::Windows => self.query_windows(cmd),
        }
    }

    fn dispatch_signal(&mut self, cmd: SignalCommand) -> Response {
        match cmd {
            SignalCommand::Add(pairs) => {
                let signal = Signal::from_key_values(&pairs)?;
                let compiled = compile_signal(signal)?;
                // `event_signal_add` drops any prior signal with the same label.
                if let Some(label) = &compiled.signal.label {
                    self.signals
                        .retain(|s| s.signal.label.as_ref() != Some(label));
                }
                self.signals.push(compiled);
                Ok(None)
            }
            SignalCommand::Remove(selector) => self.remove_signal(selector.as_ref()),
            SignalCommand::List => Ok(Some(self.serialize_signals())),
        }
    }

    /// Remove a signal by index (global, in event-grouped order, like
    /// `event_signal_remove_by_index`) or by label. Mirrors the C error text.
    fn remove_signal(&mut self, selector: Option<&Selector>) -> Response {
        match selector {
            Some(Selector::Index(index)) => {
                let order = self.signal_order();
                match order.get(*index as usize) {
                    Some(&pos) => {
                        self.signals.remove(pos);
                        Ok(None)
                    }
                    None => Err(format!("signal with index '{index}' not found.\n")),
                }
            }
            Some(Selector::Label(label)) => {
                match self
                    .signals
                    .iter()
                    .position(|s| s.signal.label.as_deref() == Some(label))
                {
                    Some(pos) => {
                        self.signals.remove(pos);
                        Ok(None)
                    }
                    None => Err(format!("signal with label '{label}' not found.\n")),
                }
            }
            _ => Err("a valid signal selector (index or label) is required.\n".to_string()),
        }
    }

    /// Indices into `self.signals` grouped by event in `SignalEvent::ALL` order,
    /// matching the global ordering the C daemon assigns in `event_signal_list` /
    /// `event_signal_remove_by_index`.
    fn signal_order(&self) -> Vec<usize> {
        let mut order = Vec::with_capacity(self.signals.len());
        for event in SignalEvent::ALL {
            for (pos, signal) in self.signals.iter().enumerate() {
                if signal.signal.event == event {
                    order.push(pos);
                }
            }
        }
        order
    }

    /// Serialize all signals as the C `event_signal_list` does: event-grouped,
    /// globally indexed, with `active` as `null`/`true`/`false`.
    fn serialize_signals(&self) -> String {
        let order = self.signal_order();
        let mut out = String::from("[");
        for (index, &pos) in order.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let s = &self.signals[pos].signal;
            let active = match s.active {
                None => "null",
                Some(true) => "true",
                Some(false) => "false",
            };
            out.push_str(&format!(
                "{{\n\t\"index\":{index},\n\t\"label\":\"{}\",\n\t\"app\":\"{}\",\n\t\"title\":\"{}\",\n\t\"active\":{active},\n\t\"event\":\"{}\",\n\t\"action\":\"{}\"\n}}",
                json_escape(s.label.as_deref().unwrap_or("")),
                json_escape(s.app.as_deref().unwrap_or("")),
                json_escape(s.title.as_deref().unwrap_or("")),
                s.event.as_str(),
                json_escape(&s.action),
            ));
        }
        out.push_str("]\n");
        out
    }

    /// The action commands of every signal subscribed to `event`, in registration
    /// order. This no-context form is correct for events whose C signal filter
    /// ignores app/title/active fields (e.g. `space_changed`).
    pub fn signal_actions_for(&self, event: SignalEvent) -> Vec<String> {
        self.signal_actions_for_context(event, None, None, None)
    }

    /// The action commands of every signal subscribed to `event` whose filters
    /// match the supplied event context. Event categories mirror
    /// `event_signal_filter` in the C daemon.
    pub fn signal_actions_for_context(
        &self,
        event: SignalEvent,
        app: Option<&str>,
        title: Option<&str>,
        active: Option<bool>,
    ) -> Vec<String> {
        self.signals
            .iter()
            .filter(|s| s.signal.event == event && s.matches(event, app, title, active))
            .map(|s| s.signal.action.clone())
            .collect()
    }

    fn dispatch_rule(&mut self, cmd: RuleCommand) -> Response {
        match cmd {
            RuleCommand::Add { one_shot, pairs } => {
                let rule = Rule::from_key_values(&pairs, one_shot)?;
                let compiled = compile_rule(rule)?;
                // `rule_add` drops any prior rule with the same label.
                if let Some(label) = &compiled.rule.label {
                    self.rules.retain(|r| r.rule.label.as_ref() != Some(label));
                }
                self.rules.push(compiled);
                Ok(None)
            }
            RuleCommand::Remove(selector) => self.remove_rule(selector.as_ref()),
            RuleCommand::List => Ok(Some(self.serialize_rules())),
            RuleCommand::Apply(apply) => self.apply_rule(apply),
        }
    }

    fn apply_rule(&mut self, apply: RuleApply) -> Response {
        match apply {
            RuleApply::All => {
                self.apply_all_non_one_shot_rules_to_known_windows();
                Ok(None)
            }
            RuleApply::AdHoc { label, pairs } => {
                if let Some(index) = self.rule_index_by_label(&label) {
                    return self.apply_rule_index(index);
                }
                let rule = Rule::from_key_values(&pairs, false)?;
                let compiled = compile_rule(rule)?;
                self.apply_compiled_rule_to_known_windows(&compiled);
                Ok(None)
            }
            RuleApply::Selector(selector) => self.apply_rule_selector(&selector),
        }
    }

    fn remove_rule(&mut self, selector: Option<&Selector>) -> Response {
        match selector {
            Some(Selector::Index(index)) => {
                if (*index as usize) < self.rules.len() {
                    self.rules.remove(*index as usize);
                    Ok(None)
                } else {
                    Err(format!("rule with index '{index}' not found.\n"))
                }
            }
            Some(Selector::Label(label)) => {
                match self
                    .rules
                    .iter()
                    .position(|r| r.rule.label.as_deref() == Some(label))
                {
                    Some(pos) => {
                        self.rules.remove(pos);
                        Ok(None)
                    }
                    None => Err(format!("rule with label '{label}' not found.\n")),
                }
            }
            _ => Err("a valid rule selector (index or label) is required.\n".to_string()),
        }
    }

    /// The combined effects of every rule matching a window, applied in
    /// registration order (later rules win per field), mirroring
    /// `rule_combine_effects`. Role/subrole default to empty until AX role
    /// discovery exists.
    pub fn rule_effects_for_window(
        &self,
        app: &str,
        title: &str,
        role: &str,
        subrole: &str,
    ) -> RuleEffects {
        self.combined_rule_effects_for_window(app, title, role, subrole, true)
    }

    /// Apply rules to a newly discovered live window. One-shot rules participate
    /// in this pass and are removed after matching, like the C daemon's
    /// `RULE_ONE_SHOT_REMOVE` cleanup after window creation.
    pub fn apply_new_window_rules(
        &mut self,
        window_id: u32,
        app: &str,
        title: &str,
        role: &str,
        subrole: &str,
        sid: u64,
    ) -> RuleEffects {
        let mut result = RuleEffects::default();
        let mut remove = Vec::new();
        for (index, compiled) in self.rules.iter().enumerate() {
            if compiled.matches(app, title, role, subrole) {
                combine_effects(&compiled.rule.effects, &mut result);
                if compiled.rule.one_shot {
                    remove.push(index);
                }
            }
        }
        for index in remove.into_iter().rev() {
            self.rules.remove(index);
        }
        self.apply_manage_effects_to_window(window_id, sid, &result);
        result
    }

    fn combined_rule_effects_for_window(
        &self,
        app: &str,
        title: &str,
        role: &str,
        subrole: &str,
        include_one_shot: bool,
    ) -> RuleEffects {
        let mut result = RuleEffects::default();
        for compiled in &self.rules {
            if (include_one_shot || !compiled.rule.one_shot)
                && compiled.matches(app, title, role, subrole)
            {
                combine_effects(&compiled.rule.effects, &mut result);
            }
        }
        result
    }

    fn apply_all_non_one_shot_rules_to_known_windows(&mut self) {
        for (window_id, app, title, sid) in self.windows_with_meta() {
            let effects = self.combined_rule_effects_for_window(&app, &title, "", "", false);
            self.apply_manage_effects_to_window(window_id, sid, &effects);
        }
    }

    fn apply_compiled_rule_to_known_windows(&mut self, compiled: &CompiledRule) {
        let matches = self
            .windows_with_meta()
            .into_iter()
            .filter_map(|(window_id, app, title, sid)| {
                compiled
                    .matches(&app, &title, "", "")
                    .then_some((window_id, sid))
            })
            .collect::<Vec<_>>();
        for (window_id, sid) in matches {
            self.apply_manage_effects_to_window(window_id, sid, &compiled.rule.effects);
        }
    }

    fn apply_rule_selector(&mut self, selector: &Selector) -> Response {
        let Some(index) = self.resolve_rule_selector(selector)? else {
            return Ok(None);
        };
        self.apply_rule_index(index)
    }

    fn apply_rule_index(&mut self, index: usize) -> Response {
        let matches = {
            let compiled = &self.rules[index];
            if compiled.rule.one_shot {
                return Ok(None);
            }
            self.windows_with_meta()
                .into_iter()
                .filter_map(|(window_id, app, title, sid)| {
                    compiled
                        .matches(&app, &title, "", "")
                        .then_some((window_id, sid))
                })
                .collect::<Vec<_>>()
        };
        let effects = self.rules[index].rule.effects.clone();
        for (window_id, sid) in matches {
            self.apply_manage_effects_to_window(window_id, sid, &effects);
        }
        Ok(None)
    }

    fn resolve_rule_selector(&self, selector: &Selector) -> Result<Option<usize>, String> {
        match selector {
            Selector::Index(index) => {
                if (*index as usize) < self.rules.len() {
                    Ok(Some(*index as usize))
                } else {
                    Err(format!("rule with index '{index}' not found.\n"))
                }
            }
            Selector::Label(label) => match self.rule_index_by_label(label) {
                Some(pos) => Ok(Some(pos)),
                None => Err(format!("rule with label '{label}' not found.\n")),
            },
            _ => Err("a valid rule selector (index or label) is required.\n".to_string()),
        }
    }

    fn rule_index_by_label(&self, label: &str) -> Option<usize> {
        self.rules
            .iter()
            .position(|r| r.rule.label.as_deref() == Some(label))
    }

    fn apply_manage_effects_to_window(&mut self, window_id: u32, sid: u64, effects: &RuleEffects) {
        if let Some(manage) = effects.manage {
            // manage=off -> floating (untiled); manage=on -> tiled.
            self.set_window_floating(window_id, !manage, sid);
        }
    }

    fn serialize_rules(&self) -> String {
        let mut out = String::from("[");
        for (index, compiled) in self.rules.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&serialize_rule(&compiled.rule, index));
        }
        out.push_str("]\n");
        out
    }

    fn query_displays(&self, cmd: &QueryCommand) -> Response {
        let properties = query_properties(
            &cmd.properties,
            &["id", "label", "index", "frame", "spaces", "has-focus"],
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
                "label",
                "type",
                "windows",
                "first-window",
                "last-window",
                "has-focus",
                "is-visible",
                "display",
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
        let properties = query_properties(
            &cmd.properties,
            &[
                "id",
                "pid",
                "app",
                "title",
                "frame",
                "has-focus",
                "space",
                "display",
                "is-visible",
                "split-type",
                "split-child",
                "has-fullscreen-zoom",
                "has-parent-zoom",
            ],
            "window",
        )?;

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
                "label" => fields.push(format!(
                    "\t\"label\":\"{}\"",
                    json_escape(
                        self.display_labels
                            .get(&display_id)
                            .map(String::as_str)
                            .unwrap_or("")
                    )
                )),
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
                "label" => fields.push(format!(
                    "\t\"label\":\"{}\"",
                    json_escape(
                        self.space_labels
                            .get(&sid)
                            .map(String::as_str)
                            .unwrap_or("")
                    )
                )),
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
                    json_bool(self.space_is_visible(sid))
                )),
                "display" => fields.push(format!(
                    "\t\"display\":{}",
                    self.space_displays
                        .get(&sid)
                        .copied()
                        .and_then(|did| self.display_index(did))
                        .unwrap_or(0)
                )),
                _ => unreachable!("query_properties rejects unsupported properties"),
            }
        }
        format!("{{\n{}\n}}", fields.join(",\n"))
    }

    fn serialize_window(&self, frame: WindowFrame, properties: &[&str]) -> String {
        let mut fields = Vec::new();
        for property in properties {
            let meta = self.window_meta.get(&frame.window_id);
            match *property {
                "id" => fields.push(format!("\t\"id\":{}", frame.window_id)),
                "pid" => fields.push(format!("\t\"pid\":{}", meta.map(|m| m.pid).unwrap_or(0))),
                "app" => fields.push(format!(
                    "\t\"app\":\"{}\"",
                    json_escape(meta.map(|m| m.app.as_str()).unwrap_or(""))
                )),
                "title" => fields.push(format!(
                    "\t\"title\":\"{}\"",
                    json_escape(meta.map(|m| m.title.as_str()).unwrap_or(""))
                )),
                "frame" => fields.push(format_area("frame", frame.area)),
                "has-focus" => fields.push(format!(
                    "\t\"has-focus\":{}",
                    json_bool(self.focused_window == Some(frame.window_id))
                )),
                "space" => fields.push(format!(
                    "\t\"space\":{}",
                    self.window_space(frame.window_id).unwrap_or(0)
                )),
                "display" => fields.push(format!(
                    "\t\"display\":{}",
                    self.window_space(frame.window_id)
                        .and_then(|sid| self.space_displays.get(&sid).copied())
                        .and_then(|did| self.display_index(did))
                        .unwrap_or(0)
                )),
                "is-visible" => fields.push(format!(
                    "\t\"is-visible\":{}",
                    json_bool(
                        self.window_space(frame.window_id)
                            .is_some_and(|sid| self.space_is_visible(sid))
                    )
                )),
                "split-type" => fields.push(format!(
                    "\t\"split-type\":\"{}\"",
                    self.window_split_info(frame.window_id).0
                )),
                "split-child" => fields.push(format!(
                    "\t\"split-child\":\"{}\"",
                    self.window_split_info(frame.window_id).1
                )),
                "has-fullscreen-zoom" => fields.push(format!(
                    "\t\"has-fullscreen-zoom\":{}",
                    json_bool(self.window_zoom(frame.window_id) == Some(ZoomKind::Fullscreen))
                )),
                "has-parent-zoom" => fields.push(format!(
                    "\t\"has-parent-zoom\":{}",
                    json_bool(self.window_zoom(frame.window_id) == Some(ZoomKind::Parent))
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

    /// Whether a space is currently visible (the active space on its display, or
    /// the global active space when no per-display active space is recorded).
    /// Mirrors the C `space_is_visible`.
    fn space_is_visible(&self, sid: u64) -> bool {
        if let Some(did) = self.space_displays.get(&sid) {
            if let Some(&active) = self.display_active_space.get(did) {
                return active == sid;
            }
        }
        self.active_space == Some(sid)
    }

    /// The `split-type` / `split-child` strings for a window, mirroring the C
    /// `window.c` logic: split-type is the parent node's split (`none` at the
    /// root), and split-child is `first_child` iff the node is its parent's left
    /// child, else `second_child` (so a lone root window is `second_child`).
    fn window_split_info(&self, window_id: u32) -> (&'static str, &'static str) {
        for tree in self.spaces.values() {
            if let Some(node_id) = tree.find_window_node(window_id) {
                let (split, is_left) = match tree.node(node_id).parent {
                    Some(parent_id) => {
                        let parent = tree.node(parent_id);
                        (parent.split, parent.left == Some(node_id))
                    }
                    None => (NodeSplit::None, false),
                };
                return (
                    split.as_str(),
                    if is_left {
                        "first_child"
                    } else {
                        "second_child"
                    },
                );
            }
        }
        ("none", "none")
    }

    /// The zoom state of a window, if any (used for `has-fullscreen-zoom` /
    /// `has-parent-zoom`).
    fn window_zoom(&self, window_id: u32) -> Option<ZoomKind> {
        self.spaces.values().find_map(|tree| match tree.zoomed() {
            Some((zid, kind)) if zid == window_id => Some(kind),
            _ => None,
        })
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

    /// The 1-based arrangement index of a display (displays sorted by id),
    /// matching `query --displays` `index` and the C `YABAI_DISPLAY_INDEX`.
    pub fn display_index(&self, display_id: u32) -> Option<usize> {
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
            Some(Selector::Label(label)) => self
                .display_labels
                .iter()
                .find_map(|(&did, existing)| (existing == label).then_some(did))
                .ok_or_else(|| format!("could not locate display with label '{label}'.\n")),
            Some(Selector::Mouse) => self
                .cursor_point
                .and_then(|point| self.display_at_point(point))
                .ok_or_else(|| "could not locate the display under the cursor.\n".to_string()),
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
            Some(Selector::Label(label)) => self
                .space_labels
                .iter()
                .find_map(|(&sid, existing)| (existing == label).then_some(sid))
                .ok_or_else(|| format!("could not locate space with label '{label}'.\n")),
            Some(Selector::Recent) => self
                .last_active_space
                .ok_or_else(|| "could not locate the recent space.\n".to_string()),
            Some(Selector::Mouse) => self
                .cursor_point
                .and_then(|point| self.display_at_point(point))
                .and_then(|did| self.display_active_space.get(&did).copied())
                .ok_or_else(|| "could not locate the space under the cursor.\n".to_string()),
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
    /// Resolved here: a numeric id, `recent` (the previously focused window, which
    /// may be on another space), `first`/`last` (by tree order), `next`/`prev`
    /// (relative to the focused window in tree order, no wrap), and a cardinal
    /// direction (the top window of the neighboring node). `mouse`, `stack[.N]`,
    /// and labels still need live state and are reported.
    fn resolve_window(&self, selector: &Selector) -> Result<u32, String> {
        if let Selector::Index(id) = selector {
            return Ok(*id);
        }
        if let Selector::Recent = selector {
            return self
                .last_focused_window
                .ok_or_else(|| "could not locate the recent window.\n".to_string());
        }
        if let Selector::Mouse = selector {
            return self
                .cursor_point
                .and_then(|point| self.window_at_point(point))
                .ok_or_else(|| "could not locate the window under the cursor.\n".to_string());
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

/// Compile a rule's filter patterns, mapping a bad pattern to the C
/// `daemon_fail` text for `regcomp` failure.
fn compile_rule(rule: Rule) -> Result<CompiledRule, String> {
    fn compile(pattern: &Option<String>, key: &str) -> Result<Option<Regex>, String> {
        match pattern {
            None => Ok(None),
            Some(p) => Regex::new(p)
                .map(Some)
                .map_err(|_| format!("invalid regex pattern '{p}' for key '{key}'\n")),
        }
    }
    let app = compile(&rule.app, "app")?;
    let title = compile(&rule.title, "title")?;
    let role = compile(&rule.role, "role")?;
    let subrole = compile(&rule.subrole, "subrole")?;
    Ok(CompiledRule {
        rule,
        app,
        title,
        role,
        subrole,
    })
}

/// Merge one rule's effects into the accumulating result (later rules win per
/// field), mirroring `rule_combine_effects`.
fn combine_effects(effects: &RuleEffects, result: &mut RuleEffects) {
    if effects.manage.is_some() {
        result.manage = effects.manage;
    }
    if effects.sticky.is_some() {
        result.sticky = effects.sticky;
    }
    if effects.mff.is_some() {
        result.mff = effects.mff;
    }
    if effects.fullscreen.is_some() {
        result.fullscreen = effects.fullscreen;
    }
    if effects.opacity.is_some() {
        result.opacity = effects.opacity;
    }
    if effects.layer.is_some() {
        result.layer = effects.layer;
    }
    if effects.grid.is_some() {
        result.grid = effects.grid;
    }
    if effects.scratchpad.is_some() {
        result.scratchpad = effects.scratchpad.clone();
    }
    if effects.display.is_some() {
        result.display = effects.display.clone();
        result.follow_space = effects.follow_space;
    }
    if effects.space.is_some() {
        result.space = effects.space.clone();
        result.follow_space = effects.follow_space;
    }
}

/// Serialize a rule as `rule_serialize` does. `display`/`space` resolve to live
/// arrangement/Mission-Control indices in the C daemon; that resolution is
/// deferred here, so both serialize as `0`.
fn serialize_rule(rule: &Rule, index: usize) -> String {
    let effects = &rule.effects;
    // flags hex: (effects.flags << 16) | rule.flags, bit values from rule.h.
    let mut rule_flags: u32 = 0;
    if rule.app.is_some() {
        rule_flags |= 0x001;
    }
    if rule.title.is_some() {
        rule_flags |= 0x002;
    }
    if rule.role.is_some() {
        rule_flags |= 0x004;
    }
    if rule.subrole.is_some() {
        rule_flags |= 0x008;
    }
    if rule.app_exclude {
        rule_flags |= 0x010;
    }
    if rule.title_exclude {
        rule_flags |= 0x020;
    }
    if rule.role_exclude {
        rule_flags |= 0x040;
    }
    if rule.subrole_exclude {
        rule_flags |= 0x080;
    }
    if rule.one_shot {
        rule_flags |= 0x100;
    }
    let mut effect_flags: u32 = 0;
    if effects.follow_space {
        effect_flags |= 0x01;
    }
    if effects.opacity.is_some() {
        effect_flags |= 0x02;
    }
    if effects.layer.is_some() {
        effect_flags |= 0x04;
    }
    let flags = (effect_flags << 16) | rule_flags;

    let grid = effects.grid.unwrap_or([0; 6]);
    format!(
        "{{\n\t\"index\":{index},\n\t\"label\":\"{}\",\n\t\"app\":\"{}\",\n\t\"title\":\"{}\",\n\t\"role\":\"{}\",\n\t\"subrole\":\"{}\",\n\t\"display\":0,\n\t\"space\":0,\n\t\"follow_space\":{},\n\t\"opacity\":{:.4},\n\t\"manage\":{},\n\t\"sticky\":{},\n\t\"mouse_follows_focus\":{},\n\t\"sub-layer\":\"{}\",\n\t\"native-fullscreen\":{},\n\t\"grid\":\"{}:{}:{}:{}:{}:{}\",\n\t\"scratchpad\":\"{}\",\n\t\"one-shot\":{},\n\t\"flags\":\"0x{:08x}\"\n}}",
        json_escape(rule.label.as_deref().unwrap_or("")),
        json_escape(rule.app.as_deref().unwrap_or("")),
        json_escape(rule.title.as_deref().unwrap_or("")),
        json_escape(rule.role.as_deref().unwrap_or("")),
        json_escape(rule.subrole.as_deref().unwrap_or("")),
        json_bool(effects.follow_space),
        effects.opacity.unwrap_or(0.0),
        optional_bool(effects.manage),
        optional_bool(effects.sticky),
        optional_bool(effects.mff),
        effects.layer.map(Layer::as_str).unwrap_or(""),
        optional_bool(effects.fullscreen),
        grid[0],
        grid[1],
        grid[2],
        grid[3],
        grid[4],
        grid[5],
        json_escape(effects.scratchpad.as_deref().unwrap_or("")),
        json_bool(rule.one_shot),
        flags,
    )
}

/// `json_optional_bool`: unset -> `null`, on -> `true`, off -> `false`.
fn optional_bool(value: Option<bool>) -> &'static str {
    match value {
        None => "null",
        Some(true) => "true",
        Some(false) => "false",
    }
}

/// Escape a string for embedding in a JSON string literal (quotes, backslashes,
/// and control characters) — app names and titles are arbitrary user text.
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
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
        let mut state = state_with_space();
        assert_eq!(
            state.handle_tokens(&toks(&["query", "--windows", "role"])),
            Err("'role' is not available from pure window state".to_string())
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
}
