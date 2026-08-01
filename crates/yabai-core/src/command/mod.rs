//! Typed command model for `yabai -m`, ported from the domain handlers in
//! `src/message.c`.
//!
//! The C daemon parses tokens and applies them against live managers in one
//! pass. This module separates the *parse* step into a typed AST so it can be
//! unit-tested without a daemon. Selector resolution and the actual mutation of
//! manager state remain the daemon's responsibility.
//!
//! The Rust client receives already-split argv tokens, so this works on
//! `&[String]` instead of replicating `get_token`'s NUL-delimited walk over a
//! single buffer.

use crate::layout::{Child, InsertDirection, InsertionPolicy, NodeSplit, ViewType};
use crate::parser::{
    KeyValue, Selector, ValueType, parse_auto_balance, parse_insertion_policy, parse_key_value,
    parse_layout, parse_resize_handle, parse_selector, parse_split_type, parse_value_type,
    parse_window_placement,
};
use crate::rule::Layer;
use std::fmt;

/// A top-level message domain. Mirrors the `DOMAIN_*` dispatch in
/// `handle_message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    Config,
    Display,
    Space,
    Window,
    Query,
    Rule,
    Signal,
}

impl Domain {
    pub fn as_str(self) -> &'static str {
        match self {
            Domain::Config => "config",
            Domain::Display => "display",
            Domain::Space => "space",
            Domain::Window => "window",
            Domain::Query => "query",
            Domain::Rule => "rule",
            Domain::Signal => "signal",
        }
    }
}

/// Parse the leading domain token. Matches `handle_message`'s dispatch and its
/// `unknown domain '...'` failure.
pub fn parse_domain(token: &str) -> Result<Domain, ParseError> {
    match token {
        "config" => Ok(Domain::Config),
        "display" => Ok(Domain::Display),
        "space" => Ok(Domain::Space),
        "window" => Ok(Domain::Window),
        "query" => Ok(Domain::Query),
        "rule" => Ok(Domain::Rule),
        "signal" => Ok(Domain::Signal),
        other => Err(ParseError::UnknownDomain(other.to_string())),
    }
}

/// Focus-follows-mouse mode. Mirrors `ffm_mode_str` / `FFM_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfmMode {
    Disabled,
    Autofocus,
    Autoraise,
}

/// The keyboard modifier that arms mouse drag actions (`mouse_modifier`). Mirrors
/// the C `MOUSE_MOD_*` (a single modifier; `fn` is the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseModifier {
    Alt,
    Shift,
    Cmd,
    Ctrl,
    Fn,
}

/// A mouse drag action (`mouse_action1` / `mouse_action2`). Mirrors `MOUSE_MODE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Move,
    Resize,
}

/// What happens when a dragged window is dropped onto another (`mouse_drop_action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseDropAction {
    Swap,
    Stack,
}

/// `display_arrangement_order`: how displays are indexed (C
/// `enum display_arrangement_order`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayArrangementOrder {
    Default,
    Horizontal,
    Vertical,
}

/// `window_origin_display`: which display a new window's origin is chosen from
/// (C `enum window_origin_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowOriginMode {
    Default,
    Focused,
    Cursor,
}

/// `external_bar` mode (C `enum external_bar_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalBarMode {
    Off,
    Main,
    All,
}

/// `external_bar <mode>:<top>:<bottom>`: reserve space at the top/bottom of a
/// display for an external status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalBar {
    pub mode: ExternalBarMode,
    pub top: i32,
    pub bottom: i32,
}

/// Animation easing function names in C `ANIMATION_EASING_TYPE_ENTRY` order;
/// `window_animation_easing` stores the index into this table.
pub const ANIMATION_EASING_NAMES: [&str; 21] = [
    "ease_in_sine",
    "ease_out_sine",
    "ease_in_out_sine",
    "ease_in_quad",
    "ease_out_quad",
    "ease_in_out_quad",
    "ease_in_cubic",
    "ease_out_cubic",
    "ease_in_out_cubic",
    "ease_in_quart",
    "ease_out_quart",
    "ease_in_out_quart",
    "ease_in_quint",
    "ease_out_quint",
    "ease_in_out_quint",
    "ease_in_expo",
    "ease_out_expo",
    "ease_in_out_expo",
    "ease_in_circ",
    "ease_out_circ",
    "ease_in_out_circ",
];

/// A typed config value, resolved according to the setting's expected type.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    Bool(bool),
    Ffm(FfmMode),
    Layout(ViewType),
    SplitType(NodeSplit),
    AutoBalance(NodeSplit),
    Placement(Child),
    InsertionPoint(InsertionPolicy),
    MouseMod(MouseModifier),
    MouseAction(MouseAction),
    MouseDrop(MouseDropAction),
    ArrangementOrder(DisplayArrangementOrder),
    WindowOrigin(WindowOriginMode),
    /// Index into [`ANIMATION_EASING_NAMES`].
    AnimationEasing(u8),
    /// `insert_feedback_color`, a packed `0xAARRGGBB` value.
    Color(u32),
    ExternalBar(ExternalBar),
    Float(f32),
    Int(i32),
}

/// A single config operation: query the current value, or set a new one. A
/// command token with no following value is a `Get`, matching the C handler
/// which prints the current value when `get_token` yields nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigOp {
    Get(String),
    Set(String, ConfigValue),
}

/// A parsed `config` message: an optional `--space` selector followed by one or
/// more operations.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigCommand {
    pub space: Option<Selector>,
    pub ops: Vec<ConfigOp>,
}

/// How a given config key's value token should be interpreted.
#[derive(Clone, Copy)]
enum ValueKind {
    Bool,
    Ffm,
    Layout,
    SplitType,
    AutoBalance,
    Placement,
    InsertionPoint,
    MouseMod,
    MouseAction,
    MouseDrop,
    ArrangementOrder,
    WindowOrigin,
    AnimationEasing,
    Color,
    ExternalBar,
    Float,
    Int,
}

/// Map a config command key to its value kind, or `None` if the key is unknown.
/// Curated to the settings whose grammar is fully determined by `yabai-core`'s
/// own enums plus the numeric/bool settings; richer string settings (colors,
/// easing, mouse modifiers, external bar) are intentionally not modeled yet.
fn config_value_kind(key: &str) -> Option<ValueKind> {
    let kind = match key {
        "debug_output"
        | "mouse_follows_focus"
        | "window_sublayer_auto"
        | "manage"
        | "window_zoom_persist"
        | "window_opacity"
        | "window_shadow" => ValueKind::Bool,
        "focus_follows_mouse" => ValueKind::Ffm,
        "layout" => ValueKind::Layout,
        "split_type" => ValueKind::SplitType,
        "auto_balance" => ValueKind::AutoBalance,
        "window_placement" => ValueKind::Placement,
        "window_insertion_point" => ValueKind::InsertionPoint,
        "mouse_modifier" => ValueKind::MouseMod,
        "mouse_action1" | "mouse_action2" => ValueKind::MouseAction,
        "mouse_drop_action" => ValueKind::MouseDrop,
        "display_arrangement_order" => ValueKind::ArrangementOrder,
        "window_origin_display" => ValueKind::WindowOrigin,
        "window_animation_easing" => ValueKind::AnimationEasing,
        "insert_feedback_color" => ValueKind::Color,
        "external_bar" => ValueKind::ExternalBar,
        "skip_window_focus_animation" => ValueKind::Bool,
        "split_ratio"
        | "window_opacity_duration"
        | "window_animation_duration"
        | "active_window_opacity"
        | "normal_window_opacity"
        | "menubar_opacity" => ValueKind::Float,
        "top_padding" | "bottom_padding" | "left_padding" | "right_padding" | "window_gap" => {
            ValueKind::Int
        }
        _ => return None,
    };
    Some(kind)
}

fn parse_config_value(kind: ValueKind, value: &str) -> Option<ConfigValue> {
    match kind {
        ValueKind::Bool => match value {
            "on" => Some(ConfigValue::Bool(true)),
            "off" => Some(ConfigValue::Bool(false)),
            _ => None,
        },
        ValueKind::Ffm => match value {
            "off" => Some(ConfigValue::Ffm(FfmMode::Disabled)),
            "autofocus" => Some(ConfigValue::Ffm(FfmMode::Autofocus)),
            "autoraise" => Some(ConfigValue::Ffm(FfmMode::Autoraise)),
            _ => None,
        },
        ValueKind::Layout => parse_layout(value).map(ConfigValue::Layout),
        ValueKind::SplitType => parse_split_type(value).map(ConfigValue::SplitType),
        ValueKind::AutoBalance => parse_auto_balance(value).map(ConfigValue::AutoBalance),
        ValueKind::Placement => parse_window_placement(value).map(ConfigValue::Placement),
        ValueKind::InsertionPoint => parse_insertion_policy(value).map(ConfigValue::InsertionPoint),
        ValueKind::MouseMod => match value {
            "alt" => Some(ConfigValue::MouseMod(MouseModifier::Alt)),
            "shift" => Some(ConfigValue::MouseMod(MouseModifier::Shift)),
            "cmd" => Some(ConfigValue::MouseMod(MouseModifier::Cmd)),
            "ctrl" => Some(ConfigValue::MouseMod(MouseModifier::Ctrl)),
            "fn" => Some(ConfigValue::MouseMod(MouseModifier::Fn)),
            _ => None,
        },
        ValueKind::MouseAction => match value {
            "move" => Some(ConfigValue::MouseAction(MouseAction::Move)),
            "resize" => Some(ConfigValue::MouseAction(MouseAction::Resize)),
            _ => None,
        },
        ValueKind::MouseDrop => match value {
            "swap" => Some(ConfigValue::MouseDrop(MouseDropAction::Swap)),
            "stack" => Some(ConfigValue::MouseDrop(MouseDropAction::Stack)),
            _ => None,
        },
        ValueKind::ArrangementOrder => match value {
            "default" => Some(ConfigValue::ArrangementOrder(
                DisplayArrangementOrder::Default,
            )),
            "horizontal" => Some(ConfigValue::ArrangementOrder(
                DisplayArrangementOrder::Horizontal,
            )),
            "vertical" => Some(ConfigValue::ArrangementOrder(
                DisplayArrangementOrder::Vertical,
            )),
            _ => None,
        },
        ValueKind::WindowOrigin => match value {
            "default" => Some(ConfigValue::WindowOrigin(WindowOriginMode::Default)),
            "focused" => Some(ConfigValue::WindowOrigin(WindowOriginMode::Focused)),
            "cursor" => Some(ConfigValue::WindowOrigin(WindowOriginMode::Cursor)),
            _ => None,
        },
        ValueKind::AnimationEasing => ANIMATION_EASING_NAMES
            .iter()
            .position(|name| *name == value)
            .map(|index| ConfigValue::AnimationEasing(index as u8)),
        ValueKind::Color => parse_color_u32(value)
            .filter(|color| *color != 0)
            .map(ConfigValue::Color),
        ValueKind::ExternalBar => parse_external_bar(value).map(ConfigValue::ExternalBar),
        ValueKind::Float => value.parse::<f32>().ok().map(ConfigValue::Float),
        ValueKind::Int => value.parse::<i32>().ok().map(ConfigValue::Int),
    }
}

/// Parse an `insert_feedback_color` value: a hex `0x…` literal or a decimal, into
/// a packed `u32`. Mirrors the C `token_to_value` accepting `TOKEN_TYPE_U32`.
fn parse_color_u32(value: &str) -> Option<u32> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse::<u32>().ok()
    }
}

/// Parse an `external_bar` value `<mode>:<top>:<bottom>` where `mode` is
/// `off`/`main`/`all` and top/bottom are integers. Mirrors the C
/// `sscanf("%5[^:]:%d:%d")` + mode validation.
fn parse_external_bar(value: &str) -> Option<ExternalBar> {
    let mut parts = value.splitn(3, ':');
    let mode = match parts.next()? {
        "off" => ExternalBarMode::Off,
        "main" => ExternalBarMode::Main,
        "all" => ExternalBarMode::All,
        _ => return None,
    };
    let top = parts.next()?.parse::<i32>().ok()?;
    let bottom = parts.next()?.parse::<i32>().ok()?;
    Some(ExternalBar { mode, top, bottom })
}

/// Parse the tokens following the `config` domain into a [`ConfigCommand`].
/// `tokens` excludes the leading `config` domain token.
pub fn parse_config(tokens: &[String]) -> Result<ConfigCommand, ParseError> {
    let mut iter = tokens.iter().peekable();

    // Optional `--space <selector>` prefix.
    let space = if iter.peek().map(|s| s.as_str()) == Some("--space") {
        iter.next();
        match iter.next() {
            Some(sel) => Some(parse_selector(sel)),
            None => return Err(ParseError::MissingSpaceSelector),
        }
    } else {
        None
    };

    let mut ops = Vec::new();
    while let Some(command) = iter.next() {
        let Some(kind) = config_value_kind(command) else {
            return Err(ParseError::UnknownCommand {
                command: command.clone(),
                domain: Domain::Config,
            });
        };

        // As in the C handler, a command unconditionally consumes the next
        // token as its value. Only the absence of a following token makes it a
        // query (`Get`); a following *command* token is therefore (faithfully)
        // treated as this command's value and will usually be an unknown value.
        match iter.next() {
            None => ops.push(ConfigOp::Get(command.clone())),
            Some(value) => match parse_config_value(kind, value) {
                Some(parsed) => ops.push(ConfigOp::Set(command.clone(), parsed)),
                None => {
                    return Err(ParseError::UnknownValue {
                        value: value.clone(),
                        command: command.clone(),
                        domain: Domain::Config,
                    });
                }
            },
        }
    }

    Ok(ConfigCommand { space, ops })
}

/// A parsed `window` message: an optional leading target selector followed by
/// one or more actions. Mirrors the `window [SELECTOR] --cmd [arg] ...` grammar
/// in `handle_domain_window`.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowCommand {
    pub target: Option<Selector>,
    pub actions: Vec<WindowAction>,
}

/// A `window` domain action. Selector resolution and macOS effects still live in
/// the runtime/daemon layers, but the command grammar is typed here.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowAction {
    Focus(Option<Selector>),
    Close(Option<Selector>),
    Minimize(Option<Selector>),
    Deminimize(Option<Selector>),
    Raise(Option<Selector>),
    Lower(Option<Selector>),
    Swap(Selector),
    Warp(Selector),
    Stack(Selector),
    Display(Selector),
    Space(Selector),
    Move { kind: ValueType, dx: f32, dy: f32 },
    Resize { handle: u8, dw: f32, dh: f32 },
    Ratio { kind: ValueType, ratio: f32 },
    Grid([i32; 6]),
    Opacity(f32),
    SubLayer(Layer),
    Insert(InsertDirection),
    Toggle(String),
    Scratchpad(ScratchpadAction),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScratchpadAction {
    Remove,
    Recover,
    Label(String),
}

/// Parse the tokens following the `window` domain (excluding the `window`
/// token).
pub fn parse_window(tokens: &[String]) -> Result<WindowCommand, ParseError> {
    let mut iter = tokens.iter().peekable();

    // A leading token that is not a `--command` is the target selector.
    let target = match iter.peek() {
        Some(tok) if !tok.starts_with("--") => Some(parse_selector(iter.next().unwrap())),
        _ => None,
    };

    let mut actions = Vec::new();
    while let Some(command) = iter.next() {
        let action = match command.as_str() {
            "--close" => match iter.peek() {
                Some(tok) if !tok.starts_with("--") => {
                    WindowAction::Close(Some(parse_selector(iter.next().unwrap())))
                }
                _ => WindowAction::Close(None),
            },
            "--minimize" => match iter.peek() {
                Some(tok) if !tok.starts_with("--") => {
                    WindowAction::Minimize(Some(parse_selector(iter.next().unwrap())))
                }
                _ => WindowAction::Minimize(None),
            },
            "--deminimize" => match iter.peek() {
                Some(tok) if !tok.starts_with("--") => {
                    WindowAction::Deminimize(Some(parse_selector(iter.next().unwrap())))
                }
                _ => WindowAction::Deminimize(None),
            },
            // `--raise`/`--lower` take an optional window selector: the acting
            // window is ordered above/below it (bare = above/below everything).
            // Mirrors the C `parse_window_selector(..., optional=true)`.
            "--raise" => match iter.peek() {
                Some(tok) if !tok.starts_with("--") => {
                    WindowAction::Raise(Some(parse_selector(iter.next().unwrap())))
                }
                _ => WindowAction::Raise(None),
            },
            "--lower" => match iter.peek() {
                Some(tok) if !tok.starts_with("--") => {
                    WindowAction::Lower(Some(parse_selector(iter.next().unwrap())))
                }
                _ => WindowAction::Lower(None),
            },
            "--focus" => {
                // Optional selector argument.
                match iter.peek() {
                    Some(tok) if !tok.starts_with("--") => {
                        WindowAction::Focus(Some(parse_selector(iter.next().unwrap())))
                    }
                    _ => WindowAction::Focus(None),
                }
            }
            "--swap" => WindowAction::Swap(parse_selector(require(
                iter.next(),
                command,
                Domain::Window,
            )?)),
            "--warp" => WindowAction::Warp(parse_selector(require(
                iter.next(),
                command,
                Domain::Window,
            )?)),
            "--stack" => WindowAction::Stack(parse_selector(require(
                iter.next(),
                command,
                Domain::Window,
            )?)),
            "--display" => WindowAction::Display(parse_selector(require(
                iter.next(),
                command,
                Domain::Window,
            )?)),
            "--space" => WindowAction::Space(parse_selector(require(
                iter.next(),
                command,
                Domain::Window,
            )?)),
            "--move" => {
                let (kind, dx, dy) =
                    parse_move_arg(require(iter.next(), command, Domain::Window)?, command)?;
                WindowAction::Move { kind, dx, dy }
            }
            "--resize" => {
                let (handle, dw, dh) =
                    parse_resize_arg(require(iter.next(), command, Domain::Window)?, command)?;
                WindowAction::Resize { handle, dw, dh }
            }
            "--ratio" => {
                let (kind, ratio) =
                    parse_ratio_arg(require(iter.next(), command, Domain::Window)?, command)?;
                WindowAction::Ratio { kind, ratio }
            }
            "--grid" => WindowAction::Grid(parse_grid_arg(
                require(iter.next(), command, Domain::Window)?,
                command,
            )?),
            "--opacity" => WindowAction::Opacity(parse_opacity_arg(
                require(iter.next(), command, Domain::Window)?,
                command,
            )?),
            "--sub-layer" => WindowAction::SubLayer(parse_sub_layer_arg(
                require(iter.next(), command, Domain::Window)?,
                command,
            )?),
            "--insert" => WindowAction::Insert(parse_insert_arg(
                require(iter.next(), command, Domain::Window)?,
                command,
            )?),
            "--toggle" => {
                WindowAction::Toggle(require(iter.next(), command, Domain::Window)?.clone())
            }
            "--scratchpad" => WindowAction::Scratchpad(match iter.peek() {
                Some(tok) if !tok.starts_with("--") => {
                    let arg = iter.next().unwrap();
                    if arg == "recover" {
                        ScratchpadAction::Recover
                    } else {
                        ScratchpadAction::Label(arg.clone())
                    }
                }
                _ => ScratchpadAction::Remove,
            }),
            _ => {
                return Err(ParseError::UnknownCommand {
                    command: command.clone(),
                    domain: Domain::Window,
                });
            }
        };
        actions.push(action);
    }

    Ok(WindowCommand { target, actions })
}

fn require<'a>(
    value: Option<&'a String>,
    command: &str,
    domain: Domain,
) -> Result<&'a String, ParseError> {
    value.ok_or_else(|| ParseError::MissingValue {
        command: command.to_string(),
        domain,
    })
}

fn invalid(value: &str, command: &str, domain: Domain) -> ParseError {
    ParseError::UnknownValue {
        value: value.to_string(),
        command: command.to_string(),
        domain,
    }
}

/// `type:dx:dy` (`ARGUMENT_WINDOW_MOVE`).
fn parse_move_arg(arg: &str, command: &str) -> Result<(ValueType, f32, f32), ParseError> {
    let parts: Vec<&str> = arg.split(':').collect();
    let bad = || invalid(arg, command, Domain::Window);
    if parts.len() != 3 {
        return Err(bad());
    }
    let kind = parse_value_type(parts[0]).ok_or_else(bad)?;
    let dx = parts[1].parse::<f32>().map_err(|_| bad())?;
    let dy = parts[2].parse::<f32>().map_err(|_| bad())?;
    Ok((kind, dx, dy))
}

/// `handle:dw:dh` (`ARGUMENT_WINDOW_RESIZE`).
fn parse_resize_arg(arg: &str, command: &str) -> Result<(u8, f32, f32), ParseError> {
    let parts: Vec<&str> = arg.split(':').collect();
    let bad = || invalid(arg, command, Domain::Window);
    if parts.len() != 3 {
        return Err(bad());
    }
    let handle = parse_resize_handle(parts[0]).ok_or_else(bad)?;
    let dw = parts[1].parse::<f32>().map_err(|_| bad())?;
    let dh = parts[2].parse::<f32>().map_err(|_| bad())?;
    Ok((handle, dw, dh))
}

/// `type:ratio` (`ARGUMENT_WINDOW_RATIO`).
fn parse_ratio_arg(arg: &str, command: &str) -> Result<(ValueType, f32), ParseError> {
    let parts: Vec<&str> = arg.split(':').collect();
    let bad = || invalid(arg, command, Domain::Window);
    if parts.len() != 2 {
        return Err(bad());
    }
    let kind = parse_value_type(parts[0]).ok_or_else(bad)?;
    let ratio = parts[1].parse::<f32>().map_err(|_| bad())?;
    Ok((kind, ratio))
}

/// `R:C:X:Y:W:H` (`ARGUMENT_WINDOW_GRID`).
fn parse_grid_arg(arg: &str, command: &str) -> Result<[i32; 6], ParseError> {
    let parts: Vec<&str> = arg.split(':').collect();
    let bad = || invalid(arg, command, Domain::Window);
    if parts.len() != 6 {
        return Err(bad());
    }
    let mut out = [0i32; 6];
    for (slot, part) in out.iter_mut().zip(parts) {
        *slot = part.parse::<i32>().map_err(|_| bad())?;
    }
    Ok(out)
}

fn parse_opacity_arg(arg: &str, command: &str) -> Result<f32, ParseError> {
    let opacity = arg
        .parse::<f32>()
        .map_err(|_| invalid(arg, command, Domain::Window))?;
    if (0.0..=1.0).contains(&opacity) {
        Ok(opacity)
    } else {
        Err(invalid(arg, command, Domain::Window))
    }
}

fn parse_sub_layer_arg(arg: &str, command: &str) -> Result<Layer, ParseError> {
    match arg {
        "below" => Ok(Layer::Below),
        "normal" => Ok(Layer::Normal),
        "above" => Ok(Layer::Above),
        "auto" => Ok(Layer::Auto),
        _ => Err(invalid(arg, command, Domain::Window)),
    }
}

fn parse_insert_arg(arg: &str, command: &str) -> Result<InsertDirection, ParseError> {
    match arg {
        "north" => Ok(InsertDirection::North),
        "east" => Ok(InsertDirection::East),
        "south" => Ok(InsertDirection::South),
        "west" => Ok(InsertDirection::West),
        "stack" => Ok(InsertDirection::Stack),
        _ => Err(invalid(arg, command, Domain::Window)),
    }
}

/// A parsed `space` message: an optional leading target selector followed by
/// one or more actions. Mirrors the `space [SELECTOR] --cmd [arg] ...` grammar
/// in `handle_domain_space`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceCommand {
    pub target: Option<Selector>,
    pub actions: Vec<SpaceAction>,
}

/// A `space` domain action. Layout-transform actions carry the axis as a
/// [`NodeSplit`] consistent with `yabai-core::layout` (`x-axis` =
/// `NodeSplit::Horizontal`, `y-axis` = `NodeSplit::Vertical`); `None` means both
/// axes, as when `--balance`/`--equalize` are given without an argument.
#[derive(Debug, Clone, PartialEq)]
pub enum SpaceAction {
    Focus(Option<Selector>),
    Switch(Selector),
    Create,
    Destroy(Option<Selector>),
    Move(Selector),
    Swap(Selector),
    Display(Selector),
    Equalize(Option<NodeSplit>),
    Balance(Option<NodeSplit>),
    Mirror(NodeSplit),
    Rotate(i32),
    Padding {
        kind: ValueType,
        top: i32,
        bottom: i32,
        left: i32,
        right: i32,
    },
    Gap {
        kind: ValueType,
        gap: i32,
    },
    Toggle(String),
    Layout(ViewType),
    Label(String),
}

/// `x-axis`/`y-axis` -> layout axis. `x-axis` is `SPLIT_X` (`Horizontal`),
/// `y-axis` is `SPLIT_Y` (`Vertical`), matching `src/message.c`.
fn parse_axis(token: &str) -> Option<NodeSplit> {
    match token {
        "x-axis" => Some(NodeSplit::Horizontal),
        "y-axis" => Some(NodeSplit::Vertical),
        _ => None,
    }
}

/// Parse the tokens following the `space` domain (excluding the `space` token).
pub fn parse_space(tokens: &[String]) -> Result<SpaceCommand, ParseError> {
    use Domain::Space as D;
    let mut iter = tokens.iter().peekable();

    let target = match iter.peek() {
        Some(tok) if !tok.starts_with("--") => Some(parse_selector(iter.next().unwrap())),
        _ => None,
    };

    // Consume an optional `x-axis`/`y-axis` argument for balance/equalize.
    let optional_axis = |iter: &mut std::iter::Peekable<std::slice::Iter<String>>,
                         command: &str|
     -> Result<Option<NodeSplit>, ParseError> {
        match iter.peek() {
            Some(tok) if !tok.starts_with("--") => {
                let tok = iter.next().unwrap();
                parse_axis(tok)
                    .map(Some)
                    .ok_or_else(|| invalid(tok, command, D))
            }
            _ => Ok(None),
        }
    };

    let mut actions = Vec::new();
    while let Some(command) = iter.next() {
        let action = match command.as_str() {
            "--create" => SpaceAction::Create,
            "--focus" => match iter.peek() {
                Some(tok) if !tok.starts_with("--") => {
                    SpaceAction::Focus(Some(parse_selector(iter.next().unwrap())))
                }
                _ => SpaceAction::Focus(None),
            },
            "--destroy" => match iter.peek() {
                Some(tok) if !tok.starts_with("--") => {
                    SpaceAction::Destroy(Some(parse_selector(iter.next().unwrap())))
                }
                _ => SpaceAction::Destroy(None),
            },
            "--switch" => SpaceAction::Switch(parse_selector(require(iter.next(), command, D)?)),
            "--move" => SpaceAction::Move(parse_selector(require(iter.next(), command, D)?)),
            "--swap" => SpaceAction::Swap(parse_selector(require(iter.next(), command, D)?)),
            "--display" => SpaceAction::Display(parse_selector(require(iter.next(), command, D)?)),
            "--equalize" => SpaceAction::Equalize(optional_axis(&mut iter, command)?),
            "--balance" => SpaceAction::Balance(optional_axis(&mut iter, command)?),
            "--mirror" => {
                let arg = require(iter.next(), command, D)?;
                SpaceAction::Mirror(parse_axis(arg).ok_or_else(|| invalid(arg, command, D))?)
            }
            "--rotate" => {
                let arg = require(iter.next(), command, D)?;
                match arg.as_str() {
                    "90" => SpaceAction::Rotate(90),
                    "180" => SpaceAction::Rotate(180),
                    "270" => SpaceAction::Rotate(270),
                    _ => return Err(invalid(arg, command, D)),
                }
            }
            "--layout" => {
                let arg = require(iter.next(), command, D)?;
                SpaceAction::Layout(parse_layout(arg).ok_or_else(|| invalid(arg, command, D))?)
            }
            "--padding" => {
                let arg = require(iter.next(), command, D)?;
                parse_padding_arg(arg, command)?
            }
            "--gap" => {
                let arg = require(iter.next(), command, D)?;
                parse_gap_arg(arg, command)?
            }
            "--label" => SpaceAction::Label(require(iter.next(), command, D)?.clone()),
            "--toggle" => SpaceAction::Toggle(require(iter.next(), command, D)?.clone()),
            _ => {
                return Err(ParseError::UnknownCommand {
                    command: command.clone(),
                    domain: D,
                });
            }
        };
        actions.push(action);
    }

    Ok(SpaceCommand { target, actions })
}

/// `type:t:b:l:r` (`ARGUMENT_SPACE_PADDING`).
fn parse_padding_arg(arg: &str, command: &str) -> Result<SpaceAction, ParseError> {
    let parts: Vec<&str> = arg.split(':').collect();
    let bad = || invalid(arg, command, Domain::Space);
    if parts.len() != 5 {
        return Err(bad());
    }
    let kind = parse_value_type(parts[0]).ok_or_else(bad)?;
    let top = parts[1].parse::<i32>().map_err(|_| bad())?;
    let bottom = parts[2].parse::<i32>().map_err(|_| bad())?;
    let left = parts[3].parse::<i32>().map_err(|_| bad())?;
    let right = parts[4].parse::<i32>().map_err(|_| bad())?;
    Ok(SpaceAction::Padding {
        kind,
        top,
        bottom,
        left,
        right,
    })
}

/// `type:gap` (`ARGUMENT_SPACE_GAP`).
fn parse_gap_arg(arg: &str, command: &str) -> Result<SpaceAction, ParseError> {
    let parts: Vec<&str> = arg.split(':').collect();
    let bad = || invalid(arg, command, Domain::Space);
    if parts.len() != 2 {
        return Err(bad());
    }
    let kind = parse_value_type(parts[0]).ok_or_else(bad)?;
    let gap = parts[1].parse::<i32>().map_err(|_| bad())?;
    Ok(SpaceAction::Gap { kind, gap })
}

/// A parsed `display` message. Mirrors the `display [SELECTOR] --cmd [arg]`
/// grammar in `handle_domain_display`.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayCommand {
    pub target: Option<Selector>,
    pub actions: Vec<DisplayAction>,
}

/// A `display` domain action.
#[derive(Debug, Clone, PartialEq)]
pub enum DisplayAction {
    Focus(Option<Selector>),
    Space(Selector),
    Label(String),
}

/// Parse the tokens following the `display` domain (excluding the `display`
/// token).
pub fn parse_display(tokens: &[String]) -> Result<DisplayCommand, ParseError> {
    use Domain::Display as D;
    let mut iter = tokens.iter().peekable();

    let target = match iter.peek() {
        Some(tok) if !tok.starts_with("--") => Some(parse_selector(iter.next().unwrap())),
        _ => None,
    };

    let mut actions = Vec::new();
    while let Some(command) = iter.next() {
        let action = match command.as_str() {
            "--focus" => match iter.peek() {
                Some(tok) if !tok.starts_with("--") => {
                    DisplayAction::Focus(Some(parse_selector(iter.next().unwrap())))
                }
                _ => DisplayAction::Focus(None),
            },
            "--space" => DisplayAction::Space(parse_selector(require(iter.next(), command, D)?)),
            "--label" => DisplayAction::Label(require(iter.next(), command, D)?.clone()),
            _ => {
                return Err(ParseError::UnknownCommand {
                    command: command.clone(),
                    domain: D,
                });
            }
        };
        actions.push(action);
    }

    Ok(DisplayCommand { target, actions })
}

/// What kind of entity a `query` reports on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryTarget {
    Displays,
    Spaces,
    Windows,
}

/// Optional scope qualifier on a `query` (`--display`/`--space`/`--window`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryScopeKind {
    Display,
    Space,
    Window,
}

/// A parsed `query` message. Mirrors
/// `query --displays|--spaces|--windows [PROPERTIES] [SCOPE [SELECTOR]]`.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryCommand {
    pub target: QueryTarget,
    /// Requested property names (the comma-separated list), empty for "all".
    pub properties: Vec<String>,
    /// Optional scope qualifier plus its (optional) selector.
    pub scope: Option<(QueryScopeKind, Option<Selector>)>,
}

/// Parse the tokens following the `query` domain (excluding the `query` token).
pub fn parse_query(tokens: &[String]) -> Result<QueryCommand, ParseError> {
    let mut iter = tokens.iter().peekable();

    let target = match iter.next().map(String::as_str) {
        Some("--displays") => QueryTarget::Displays,
        Some("--spaces") => QueryTarget::Spaces,
        Some("--windows") => QueryTarget::Windows,
        Some(other) => {
            return Err(ParseError::UnknownCommand {
                command: other.to_string(),
                domain: Domain::Query,
            });
        }
        None => {
            return Err(ParseError::MissingValue {
                command: "query".to_string(),
                domain: Domain::Query,
            });
        }
    };

    // An optional bare (non-`--`) token is the comma-separated property list.
    let properties = match iter.peek() {
        Some(tok) if !tok.starts_with("--") => {
            let token = iter.next().unwrap();
            let mut properties = Vec::new();
            for property in token.split(',') {
                if property.is_empty() {
                    return Err(ParseError::UnknownValue {
                        value: property.to_string(),
                        command: query_target_str(target).to_string(),
                        domain: Domain::Query,
                    });
                }
                properties.push(property.to_string());
            }
            properties
        }
        _ => Vec::new(),
    };

    // Optional scope qualifier with an optional selector.
    let scope = match iter.next().map(String::as_str) {
        Some("--display") => Some((QueryScopeKind::Display, take_optional_selector(&mut iter))),
        Some("--space") => Some((QueryScopeKind::Space, take_optional_selector(&mut iter))),
        Some("--window") => Some((QueryScopeKind::Window, take_optional_selector(&mut iter))),
        Some(other) => {
            return Err(ParseError::UnknownValue {
                value: other.to_string(),
                command: query_target_str(target).to_string(),
                domain: Domain::Query,
            });
        }
        None => None,
    };

    Ok(QueryCommand {
        target,
        properties,
        scope,
    })
}

fn query_target_str(target: QueryTarget) -> &'static str {
    match target {
        QueryTarget::Displays => "--displays",
        QueryTarget::Spaces => "--spaces",
        QueryTarget::Windows => "--windows",
    }
}

fn take_optional_selector(
    iter: &mut std::iter::Peekable<std::slice::Iter<String>>,
) -> Option<Selector> {
    match iter.peek() {
        Some(tok) if !tok.starts_with("--") => Some(parse_selector(iter.next().unwrap())),
        _ => None,
    }
}

/// A parsed `rule` message. Mirrors `handle_domain_rule`'s subcommands.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleCommand {
    Add {
        one_shot: bool,
        pairs: Vec<KeyValue>,
    },
    Remove(Option<Selector>),
    Apply(RuleApply),
    List,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuleApply {
    All,
    Selector(Selector),
    AdHoc { label: String, pairs: Vec<KeyValue> },
}

/// Parse the tokens following the `rule` domain (excluding the `rule` token).
pub fn parse_rule(tokens: &[String]) -> Result<RuleCommand, ParseError> {
    let mut iter = tokens.iter().peekable();
    let command = require(iter.next(), "rule", Domain::Rule)?;
    match command.as_str() {
        "--add" => {
            // A leading `--one-shot` flag precedes the key-value filters/effects.
            let one_shot = iter.peek().is_some_and(|tok| tok.as_str() == "--one-shot");
            if one_shot {
                iter.next();
            }
            Ok(RuleCommand::Add {
                one_shot,
                pairs: collect_key_values(iter)?,
            })
        }
        "--remove" => Ok(RuleCommand::Remove(take_optional_selector(&mut iter))),
        "--apply" => parse_rule_apply(iter),
        "--list" => Ok(RuleCommand::List),
        other => Err(ParseError::UnknownCommand {
            command: other.to_string(),
            domain: Domain::Rule,
        }),
    }
}

fn parse_rule_apply(
    mut iter: std::iter::Peekable<std::slice::Iter<String>>,
) -> Result<RuleCommand, ParseError> {
    let Some(first) = iter.next() else {
        return Ok(RuleCommand::Apply(RuleApply::All));
    };

    match parse_key_value(first) {
        Some(kv) => Ok(RuleCommand::Apply(RuleApply::AdHoc {
            label: first.clone(),
            pairs: collect_key_values_with_first(kv, iter)?,
        })),
        None if iter.peek().is_none() => Ok(RuleCommand::Apply(RuleApply::Selector(
            parse_selector(first),
        ))),
        None => Err(ParseError::InvalidKeyValue(first.clone())),
    }
}

/// A parsed `signal` message. Mirrors `handle_domain_signal`'s subcommands.
#[derive(Debug, Clone, PartialEq)]
pub enum SignalCommand {
    Add(Vec<KeyValue>),
    Remove(Option<Selector>),
    List,
}

/// Parse the tokens following the `signal` domain (excluding the `signal`
/// token).
pub fn parse_signal(tokens: &[String]) -> Result<SignalCommand, ParseError> {
    let mut iter = tokens.iter().peekable();
    let command = require(iter.next(), "signal", Domain::Signal)?;
    match command.as_str() {
        "--add" => Ok(SignalCommand::Add(collect_key_values(iter)?)),
        "--remove" => Ok(SignalCommand::Remove(take_optional_selector(&mut iter))),
        "--list" => Ok(SignalCommand::List),
        other => Err(ParseError::UnknownCommand {
            command: other.to_string(),
            domain: Domain::Signal,
        }),
    }
}

/// Collect the remaining `key=value` tokens, erroring on a malformed pair.
fn collect_key_values(
    iter: std::iter::Peekable<std::slice::Iter<String>>,
) -> Result<Vec<KeyValue>, ParseError> {
    collect_key_values_with_firsts(None, iter)
}

fn collect_key_values_with_first(
    first: KeyValue,
    iter: std::iter::Peekable<std::slice::Iter<String>>,
) -> Result<Vec<KeyValue>, ParseError> {
    collect_key_values_with_firsts(Some(first), iter)
}

fn collect_key_values_with_firsts(
    first: Option<KeyValue>,
    iter: std::iter::Peekable<std::slice::Iter<String>>,
) -> Result<Vec<KeyValue>, ParseError> {
    let mut pairs = Vec::new();
    if let Some(first) = first {
        pairs.push(first);
    }
    for token in iter {
        match parse_key_value(token) {
            Some(kv) => pairs.push(kv),
            None => return Err(ParseError::InvalidKeyValue(token.clone())),
        }
    }
    Ok(pairs)
}

/// A fully parsed `yabai -m` message, dispatched by domain.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Config(ConfigCommand),
    Display(DisplayCommand),
    Space(SpaceCommand),
    Window(WindowCommand),
    Query(QueryCommand),
    Rule(RuleCommand),
    Signal(SignalCommand),
}

/// Parse a full message (`[domain, ...args]`), dispatching on the leading domain
/// token exactly like `handle_message`.
pub fn parse_message(tokens: &[String]) -> Result<Message, ParseError> {
    let (domain, rest) = tokens.split_first().ok_or(ParseError::MissingDomain)?;
    Ok(match parse_domain(domain)? {
        Domain::Config => Message::Config(parse_config(rest)?),
        Domain::Display => Message::Display(parse_display(rest)?),
        Domain::Space => Message::Space(parse_space(rest)?),
        Domain::Window => Message::Window(parse_window(rest)?),
        Domain::Query => Message::Query(parse_query(rest)?),
        Domain::Rule => Message::Rule(parse_rule(rest)?),
        Domain::Signal => Message::Signal(parse_signal(rest)?),
    })
}

/// Command parse failures, with `Display` impls matching the daemon's
/// `daemon_fail` message text in `src/message.c`.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    MissingDomain,
    UnknownDomain(String),
    MissingSpaceSelector,
    UnknownCommand {
        command: String,
        domain: Domain,
    },
    UnknownValue {
        value: String,
        command: String,
        domain: Domain,
    },
    MissingValue {
        command: String,
        domain: Domain,
    },
    InvalidKeyValue(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::MissingDomain => write!(f, "no domain given"),
            ParseError::UnknownDomain(domain) => {
                write!(f, "unknown domain '{domain}'")
            }
            ParseError::MissingSpaceSelector => {
                write!(f, "value for '--space' selector is missing")
            }
            ParseError::UnknownCommand { command, domain } => {
                write!(
                    f,
                    "unknown command '{command}' for domain '{}'",
                    domain.as_str()
                )
            }
            ParseError::UnknownValue {
                value,
                command,
                domain,
            } => write!(
                f,
                "unknown value '{value}' given to command '{command}' for domain '{}'",
                domain.as_str()
            ),
            ParseError::MissingValue { command, domain } => write!(
                f,
                "value for '{command}' is missing for domain '{}'",
                domain.as_str()
            ),
            ParseError::InvalidKeyValue(token) => {
                write!(f, "invalid key-value pair '{token}'")
            }
        }
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests;
