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

use crate::layout::{Child, InsertionPolicy, NodeSplit, ViewType};
use crate::parser::{
    KeyValue, Selector, ValueType, parse_auto_balance, parse_insertion_policy, parse_key_value,
    parse_layout, parse_resize_handle, parse_selector, parse_split_type, parse_value_type,
    parse_window_placement,
};
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
        | "window_shadow" => ValueKind::Bool,
        "focus_follows_mouse" => ValueKind::Ffm,
        "layout" => ValueKind::Layout,
        "split_type" => ValueKind::SplitType,
        "auto_balance" => ValueKind::AutoBalance,
        "window_placement" => ValueKind::Placement,
        "window_insertion_point" => ValueKind::InsertionPoint,
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
        ValueKind::Float => value.parse::<f32>().ok().map(ConfigValue::Float),
        ValueKind::Int => value.parse::<i32>().ok().map(ConfigValue::Int),
    }
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

/// A `window` domain action. Covers the structurally-clean subset; richer
/// actions (`--opacity`, `--sub-layer`, `--scratchpad`, `--insert`) are carried
/// as raw argument strings until their effects live in `yabai-core`.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowAction {
    Focus(Option<Selector>),
    Close,
    Minimize,
    Deminimize,
    Raise,
    Lower,
    Swap(Selector),
    Warp(Selector),
    Stack(Selector),
    Display(Selector),
    Space(Selector),
    Move { kind: ValueType, dx: f32, dy: f32 },
    Resize { handle: u8, dw: f32, dh: f32 },
    Ratio { kind: ValueType, ratio: f32 },
    Grid([i32; 6]),
    Toggle(String),
    Raw { command: String, arg: String },
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
            "--close" => WindowAction::Close,
            "--minimize" => WindowAction::Minimize,
            "--deminimize" => WindowAction::Deminimize,
            "--raise" => WindowAction::Raise,
            "--lower" => WindowAction::Lower,
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
            "--toggle" => {
                WindowAction::Toggle(require(iter.next(), command, Domain::Window)?.clone())
            }
            "--opacity" | "--sub-layer" | "--scratchpad" | "--insert" => WindowAction::Raw {
                command: command.clone(),
                arg: require(iter.next(), command, Domain::Window)?.clone(),
            },
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
        Some(tok) if !tok.starts_with("--") => iter
            .next()
            .unwrap()
            .split(',')
            .map(str::to_string)
            .collect(),
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
    Add(Vec<KeyValue>),
    Remove(Option<Selector>),
    Apply(Option<Selector>),
    List,
}

/// Parse the tokens following the `rule` domain (excluding the `rule` token).
pub fn parse_rule(tokens: &[String]) -> Result<RuleCommand, ParseError> {
    let mut iter = tokens.iter().peekable();
    let command = require(iter.next(), "rule", Domain::Rule)?;
    match command.as_str() {
        "--add" => Ok(RuleCommand::Add(collect_key_values(iter)?)),
        "--remove" => Ok(RuleCommand::Remove(take_optional_selector(&mut iter))),
        "--apply" => Ok(RuleCommand::Apply(take_optional_selector(&mut iter))),
        "--list" => Ok(RuleCommand::List),
        other => Err(ParseError::UnknownCommand {
            command: other.to_string(),
            domain: Domain::Rule,
        }),
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
    let mut pairs = Vec::new();
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
mod tests {
    use super::*;

    fn toks(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn domains_dispatch() {
        assert_eq!(parse_domain("window"), Ok(Domain::Window));
        assert_eq!(parse_domain("query"), Ok(Domain::Query));
        assert_eq!(
            parse_domain("bogus"),
            Err(ParseError::UnknownDomain("bogus".to_string()))
        );
    }

    #[test]
    fn unknown_domain_message_matches_c() {
        let err = parse_domain("bogus").unwrap_err();
        assert_eq!(err.to_string(), "unknown domain 'bogus'");
    }

    #[test]
    fn config_set_typed_values() {
        let cmd = parse_config(&toks(&["layout", "bsp"])).unwrap();
        assert_eq!(cmd.space, None);
        assert_eq!(
            cmd.ops,
            vec![ConfigOp::Set(
                "layout".to_string(),
                ConfigValue::Layout(ViewType::Bsp)
            )]
        );

        let cmd = parse_config(&toks(&["split_ratio", "0.3"])).unwrap();
        assert_eq!(
            cmd.ops,
            vec![ConfigOp::Set(
                "split_ratio".to_string(),
                ConfigValue::Float(0.3)
            )]
        );

        let cmd = parse_config(&toks(&["window_gap", "8"])).unwrap();
        assert_eq!(
            cmd.ops,
            vec![ConfigOp::Set("window_gap".to_string(), ConfigValue::Int(8))]
        );
    }

    #[test]
    fn config_bare_command_is_a_get() {
        let cmd = parse_config(&toks(&["layout"])).unwrap();
        assert_eq!(cmd.ops, vec![ConfigOp::Get("layout".to_string())]);
    }

    #[test]
    fn config_chains_multiple_sets() {
        let cmd = parse_config(&toks(&["window_gap", "8", "top_padding", "12"])).unwrap();
        assert_eq!(
            cmd.ops,
            vec![
                ConfigOp::Set("window_gap".to_string(), ConfigValue::Int(8)),
                ConfigOp::Set("top_padding".to_string(), ConfigValue::Int(12)),
            ]
        );
    }

    #[test]
    fn config_trailing_command_without_value_is_a_get() {
        // `window_gap 8 layout` -> set gap, then layout has no value -> Get.
        let cmd = parse_config(&toks(&["window_gap", "8", "layout"])).unwrap();
        assert_eq!(
            cmd.ops,
            vec![
                ConfigOp::Set("window_gap".to_string(), ConfigValue::Int(8)),
                ConfigOp::Get("layout".to_string()),
            ]
        );
    }

    #[test]
    fn config_command_consumes_next_token_as_value_like_c() {
        // Faithful to C: `layout window_gap` reads "window_gap" as layout's
        // value, which is not a valid layout -> unknown value.
        let err = parse_config(&toks(&["layout", "window_gap"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown value 'window_gap' given to command 'layout' for domain 'config'"
        );
    }

    #[test]
    fn config_space_selector() {
        let cmd = parse_config(&toks(&["--space", "2", "layout", "stack"])).unwrap();
        assert_eq!(cmd.space, Some(Selector::Index(2)));
        assert_eq!(
            cmd.ops,
            vec![ConfigOp::Set(
                "layout".to_string(),
                ConfigValue::Layout(ViewType::Stack)
            )]
        );
    }

    #[test]
    fn config_missing_space_selector_errors() {
        assert_eq!(
            parse_config(&toks(&["--space"])),
            Err(ParseError::MissingSpaceSelector)
        );
    }

    #[test]
    fn config_unknown_command_errors_like_c() {
        let err = parse_config(&toks(&["nonsense", "on"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown command 'nonsense' for domain 'config'"
        );
    }

    #[test]
    fn config_unknown_value_errors_like_c() {
        let err = parse_config(&toks(&["layout", "grid"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown value 'grid' given to command 'layout' for domain 'config'"
        );
    }

    #[test]
    fn window_target_and_simple_actions() {
        let cmd = parse_window(&toks(&["--close"])).unwrap();
        assert_eq!(cmd.target, None);
        assert_eq!(cmd.actions, vec![WindowAction::Close]);

        let cmd = parse_window(&toks(&["5", "--minimize"])).unwrap();
        assert_eq!(cmd.target, Some(Selector::Index(5)));
        assert_eq!(cmd.actions, vec![WindowAction::Minimize]);
    }

    #[test]
    fn window_focus_optional_selector() {
        let cmd = parse_window(&toks(&["--focus", "west"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![WindowAction::Focus(Some(Selector::Direction(
                crate::geometry::Direction::West
            )))]
        );

        // --focus with no following selector is a bare focus.
        let cmd = parse_window(&toks(&["--focus"])).unwrap();
        assert_eq!(cmd.actions, vec![WindowAction::Focus(None)]);
    }

    #[test]
    fn window_swap_and_warp_take_selectors() {
        let cmd = parse_window(&toks(&["--swap", "next", "--warp", "east"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![
                WindowAction::Swap(Selector::Next),
                WindowAction::Warp(Selector::Direction(crate::geometry::Direction::East)),
            ]
        );
    }

    #[test]
    fn window_resize_move_ratio_grid_args() {
        let cmd = parse_window(&toks(&["--resize", "bottom_right:20:-10"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![WindowAction::Resize {
                handle: crate::layout::HANDLE_BOTTOM | crate::layout::HANDLE_RIGHT,
                dw: 20.0,
                dh: -10.0,
            }]
        );

        let cmd = parse_window(&toks(&["--move", "rel:10:5"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![WindowAction::Move {
                kind: ValueType::Rel,
                dx: 10.0,
                dy: 5.0,
            }]
        );

        let cmd = parse_window(&toks(&["--ratio", "abs:0.5"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![WindowAction::Ratio {
                kind: ValueType::Abs,
                ratio: 0.5,
            }]
        );

        let cmd = parse_window(&toks(&["--grid", "2:2:0:0:1:1"])).unwrap();
        assert_eq!(cmd.actions, vec![WindowAction::Grid([2, 2, 0, 0, 1, 1])]);
    }

    #[test]
    fn window_bad_resize_handle_is_unknown_value() {
        let err = parse_window(&toks(&["--resize", "middle:1:1"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown value 'middle:1:1' given to command '--resize' for domain 'window'"
        );
    }

    #[test]
    fn window_missing_required_value_errors() {
        let err = parse_window(&toks(&["--swap"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "value for '--swap' is missing for domain 'window'"
        );
    }

    #[test]
    fn window_unknown_command_errors() {
        let err = parse_window(&toks(&["--teleport"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown command '--teleport' for domain 'window'"
        );
    }

    #[test]
    fn space_target_and_focus() {
        let cmd = parse_space(&toks(&["--focus", "next"])).unwrap();
        assert_eq!(cmd.target, None);
        assert_eq!(cmd.actions, vec![SpaceAction::Focus(Some(Selector::Next))]);

        let cmd = parse_space(&toks(&["3", "--focus"])).unwrap();
        assert_eq!(cmd.target, Some(Selector::Index(3)));
        assert_eq!(cmd.actions, vec![SpaceAction::Focus(None)]);
    }

    #[test]
    fn space_balance_equalize_axes() {
        // No argument -> both axes (None).
        let cmd = parse_space(&toks(&["--balance"])).unwrap();
        assert_eq!(cmd.actions, vec![SpaceAction::Balance(None)]);

        // x-axis -> Horizontal, y-axis -> Vertical.
        let cmd = parse_space(&toks(&["--equalize", "x-axis"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![SpaceAction::Equalize(Some(NodeSplit::Horizontal))]
        );
        let cmd = parse_space(&toks(&["--balance", "y-axis"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![SpaceAction::Balance(Some(NodeSplit::Vertical))]
        );
    }

    #[test]
    fn space_mirror_rotate_layout() {
        let cmd = parse_space(&toks(&["--mirror", "y-axis", "--rotate", "270"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![
                SpaceAction::Mirror(NodeSplit::Vertical),
                SpaceAction::Rotate(270),
            ]
        );

        let cmd = parse_space(&toks(&["--layout", "bsp"])).unwrap();
        assert_eq!(cmd.actions, vec![SpaceAction::Layout(ViewType::Bsp)]);
    }

    #[test]
    fn space_padding_and_gap_args() {
        let cmd = parse_space(&toks(&["--padding", "abs:10:10:5:5"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![SpaceAction::Padding {
                kind: ValueType::Abs,
                top: 10,
                bottom: 10,
                left: 5,
                right: 5,
            }]
        );

        let cmd = parse_space(&toks(&["--gap", "rel:4"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![SpaceAction::Gap {
                kind: ValueType::Rel,
                gap: 4,
            }]
        );
    }

    #[test]
    fn space_bad_rotate_and_mirror_error() {
        let err = parse_space(&toks(&["--rotate", "45"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown value '45' given to command '--rotate' for domain 'space'"
        );
        let err = parse_space(&toks(&["--mirror", "z-axis"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown value 'z-axis' given to command '--mirror' for domain 'space'"
        );
    }

    #[test]
    fn space_create_and_label() {
        let cmd = parse_space(&toks(&["--create", "--label", "web"])).unwrap();
        assert_eq!(
            cmd.actions,
            vec![SpaceAction::Create, SpaceAction::Label("web".to_string())]
        );
    }

    #[test]
    fn display_actions() {
        let cmd = parse_display(&toks(&["--focus", "2"])).unwrap();
        assert_eq!(cmd.target, None);
        assert_eq!(
            cmd.actions,
            vec![DisplayAction::Focus(Some(Selector::Index(2)))]
        );

        let cmd = parse_display(&toks(&["1", "--label", "main"])).unwrap();
        assert_eq!(cmd.target, Some(Selector::Index(1)));
        assert_eq!(cmd.actions, vec![DisplayAction::Label("main".to_string())]);
    }

    #[test]
    fn query_target_properties_and_scope() {
        let cmd = parse_query(&toks(&["--windows"])).unwrap();
        assert_eq!(cmd.target, QueryTarget::Windows);
        assert!(cmd.properties.is_empty());
        assert_eq!(cmd.scope, None);

        let cmd = parse_query(&toks(&["--spaces", "index,label", "--display", "2"])).unwrap();
        assert_eq!(cmd.target, QueryTarget::Spaces);
        assert_eq!(
            cmd.properties,
            vec!["index".to_string(), "label".to_string()]
        );
        assert_eq!(
            cmd.scope,
            Some((QueryScopeKind::Display, Some(Selector::Index(2))))
        );

        // Scope qualifier with no selector (acts on the active entity).
        let cmd = parse_query(&toks(&["--windows", "--space"])).unwrap();
        assert_eq!(cmd.scope, Some((QueryScopeKind::Space, None)));
    }

    #[test]
    fn query_unknown_target_errors() {
        let err = parse_query(&toks(&["--monitors"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown command '--monitors' for domain 'query'"
        );
    }

    #[test]
    fn rule_add_and_remove() {
        let cmd = parse_rule(&toks(&["--add", "app=Safari", "manage=off"])).unwrap();
        assert_eq!(
            cmd,
            RuleCommand::Add(vec![
                KeyValue {
                    key: "app".to_string(),
                    value: "Safari".to_string(),
                    exclusion: false,
                },
                KeyValue {
                    key: "manage".to_string(),
                    value: "off".to_string(),
                    exclusion: false,
                },
            ])
        );

        assert_eq!(parse_rule(&toks(&["--list"])).unwrap(), RuleCommand::List);
        assert_eq!(
            parse_rule(&toks(&["--remove", "myrule"])).unwrap(),
            RuleCommand::Remove(Some(Selector::Label("myrule".to_string())))
        );
    }

    #[test]
    fn rule_malformed_pair_errors() {
        let err = parse_rule(&toks(&["--add", "appSafari"])).unwrap_err();
        assert_eq!(err.to_string(), "invalid key-value pair 'appSafari'");
    }

    #[test]
    fn signal_add_with_event() {
        let cmd = parse_signal(&toks(&[
            "--add",
            "event=window_focused",
            "action=echo hi",
            "label=l1",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            SignalCommand::Add(vec![
                KeyValue {
                    key: "event".to_string(),
                    value: "window_focused".to_string(),
                    exclusion: false,
                },
                KeyValue {
                    key: "action".to_string(),
                    value: "echo hi".to_string(),
                    exclusion: false,
                },
                KeyValue {
                    key: "label".to_string(),
                    value: "l1".to_string(),
                    exclusion: false,
                },
            ])
        );
    }

    #[test]
    fn parse_message_dispatches_by_domain() {
        assert!(matches!(
            parse_message(&toks(&["query", "--windows"])).unwrap(),
            Message::Query(_)
        ));
        assert!(matches!(
            parse_message(&toks(&["window", "--focus", "west"])).unwrap(),
            Message::Window(_)
        ));
        assert!(matches!(
            parse_message(&toks(&["config", "layout", "bsp"])).unwrap(),
            Message::Config(_)
        ));
        assert_eq!(
            parse_message(&toks(&["bogus"])).unwrap_err().to_string(),
            "unknown domain 'bogus'"
        );
        assert_eq!(parse_message(&[]).unwrap_err(), ParseError::MissingDomain);
    }

    #[test]
    fn config_bool_and_ffm() {
        let cmd = parse_config(&toks(&[
            "mouse_follows_focus",
            "on",
            "focus_follows_mouse",
            "autoraise",
        ]))
        .unwrap();
        assert_eq!(
            cmd.ops,
            vec![
                ConfigOp::Set("mouse_follows_focus".to_string(), ConfigValue::Bool(true)),
                ConfigOp::Set(
                    "focus_follows_mouse".to_string(),
                    ConfigValue::Ffm(FfmMode::Autoraise)
                ),
            ]
        );
    }
}
