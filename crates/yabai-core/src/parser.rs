//! Pure token parsers for the `yabai -m` command grammar, ported from the
//! `token_equals`/argument-keyword logic in `src/message.c`.
//!
//! The C grammar resolves each selector against live managers (display, space,
//! window) the moment it parses it. That resolution is the daemon's job. What
//! *is* pure — and what this module covers — is classifying a token into a typed
//! value: the common selectors (`prev`/`next`/`first`/`last`/`recent`/`mouse`/
//! directions/indices/labels) and the config-argument keywords that map directly
//! onto the [`crate::layout`] enums.

use crate::geometry::Direction;
use crate::layout::{
    Child, HANDLE_ABS, HANDLE_BOTTOM, HANDLE_LEFT, HANDLE_RIGHT, HANDLE_TOP, InsertionPolicy,
    NodeSplit, ViewType,
};

/// Coordinate interpretation for `--move`/`--ratio`. Mirrors `TYPE_ABS`/`TYPE_REL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Abs,
    Rel,
}

/// Parse an `abs`/`rel` value type keyword.
pub fn parse_value_type(token: &str) -> Option<ValueType> {
    match token {
        "abs" => Some(ValueType::Abs),
        "rel" => Some(ValueType::Rel),
        _ => None,
    }
}

/// Parse a `--resize` handle keyword into `HANDLE_*` flags, ported from
/// `parse_resize_handle` in `src/message.c`. Returns `None` for an unknown
/// handle (the C code returns `0`).
pub fn parse_resize_handle(token: &str) -> Option<u8> {
    let handle = match token {
        "top" => HANDLE_TOP,
        "bottom" => HANDLE_BOTTOM,
        "left" => HANDLE_LEFT,
        "right" => HANDLE_RIGHT,
        "top_left" => HANDLE_TOP | HANDLE_LEFT,
        "top_right" => HANDLE_TOP | HANDLE_RIGHT,
        "bottom_left" => HANDLE_BOTTOM | HANDLE_LEFT,
        "bottom_right" => HANDLE_BOTTOM | HANDLE_RIGHT,
        "abs" => HANDLE_ABS,
        _ => return None,
    };
    Some(handle)
}

/// A `key=value` / `key!=value` pair from a `rule`/`signal` message.
/// `exclusion` is `true` for the `!=` form. Mirrors `parse_key_value_pair` in
/// `src/message.c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
    pub exclusion: bool,
}

/// Split a `key=value` or `key!=value` token. Returns `None` when there is no
/// separator or the value is empty, matching the C parser which yields a null
/// key/value in those cases.
///
/// Like the C scan, this takes the *earliest* separator: in `a=b!=c` the first
/// `=` wins, giving key `a` and value `b!=c`.
pub fn parse_key_value(token: &str) -> Option<KeyValue> {
    let bytes = token.as_bytes();
    let mut sep = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'!' && bytes.get(i + 1) == Some(&b'=') {
            sep = Some((i, 2, true));
            break;
        }
        if bytes[i] == b'=' {
            sep = Some((i, 1, false));
            break;
        }
        i += 1;
    }

    let (idx, width, exclusion) = sep?;
    let value = &token[idx + width..];
    if value.is_empty() {
        return None;
    }
    Some(KeyValue {
        key: token[..idx].to_string(),
        value: value.to_string(),
        exclusion,
    })
}

/// A common selector token before it is resolved against live state. Mirrors the
/// `ARGUMENT_COMMON_SEL_*` set plus numeric indices and arbitrary labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Direction(Direction),
    Prev,
    Next,
    First,
    Last,
    Recent,
    Mouse,
    /// Bare `stack` selector.
    Stack,
    /// `stack.N` selector carrying the stack index `N`.
    StackIndex(u32),
    /// A bare numeric selector, e.g. a space/display index or window id.
    Index(u32),
    /// Any other token, treated as a user-defined label.
    Label(String),
}

/// Classify a common selector token. Numeric tokens become [`Selector::Index`],
/// `stack.N` becomes [`Selector::StackIndex`], and anything else falls through
/// to [`Selector::Label`] — so this never fails, matching the C behavior where an
/// unrecognized selector is tried as a label.
pub fn parse_selector(token: &str) -> Selector {
    if let Some(dir) = parse_direction(token) {
        return Selector::Direction(dir);
    }
    match token {
        "prev" => return Selector::Prev,
        "next" => return Selector::Next,
        "first" => return Selector::First,
        "last" => return Selector::Last,
        "recent" => return Selector::Recent,
        "mouse" => return Selector::Mouse,
        "stack" => return Selector::Stack,
        _ => {}
    }
    if let Some(rest) = token.strip_prefix("stack.") {
        if let Ok(index) = rest.parse::<u32>() {
            return Selector::StackIndex(index);
        }
    }
    if let Ok(index) = token.parse::<u32>() {
        return Selector::Index(index);
    }
    Selector::Label(token.to_string())
}

/// Parse a cardinal direction keyword (`north`/`east`/`south`/`west`).
pub fn parse_direction(token: &str) -> Option<Direction> {
    match token {
        "north" => Some(Direction::North),
        "east" => Some(Direction::East),
        "south" => Some(Direction::South),
        "west" => Some(Direction::West),
        _ => None,
    }
}

/// Parse a `layout` argument (`bsp`/`stack`/`float`).
pub fn parse_layout(token: &str) -> Option<ViewType> {
    match token {
        "bsp" => Some(ViewType::Bsp),
        "stack" => Some(ViewType::Stack),
        "float" => Some(ViewType::Float),
        _ => None,
    }
}

/// Parse a `split_type` argument (`vertical`/`horizontal`/`auto`).
pub fn parse_split_type(token: &str) -> Option<NodeSplit> {
    match token {
        "vertical" => Some(NodeSplit::Vertical),
        "horizontal" => Some(NodeSplit::Horizontal),
        "auto" => Some(NodeSplit::Auto),
        _ => None,
    }
}

/// Parse an `auto_balance` argument (`off`/`vertical`/`horizontal`/`on`).
/// Mirrors `auto_balance_str` in `src/view.h`, where `on` means both axes
/// (`SPLIT_AUTO`) and `off` means neither (`SPLIT_NONE`).
pub fn parse_auto_balance(token: &str) -> Option<NodeSplit> {
    match token {
        "off" => Some(NodeSplit::None),
        "vertical" => Some(NodeSplit::Vertical),
        "horizontal" => Some(NodeSplit::Horizontal),
        "on" => Some(NodeSplit::Auto),
        _ => None,
    }
}

/// Parse a `window_placement` argument (`first_child`/`second_child`).
pub fn parse_window_placement(token: &str) -> Option<Child> {
    match token {
        "first_child" => Some(Child::First),
        "second_child" => Some(Child::Second),
        _ => None,
    }
}

/// Parse a `window_insertion_point` argument (`focused`/`first`/`last`).
pub fn parse_insertion_policy(token: &str) -> Option<InsertionPolicy> {
    match token {
        "focused" => Some(InsertionPolicy::Focused),
        "first" => Some(InsertionPolicy::First),
        "last" => Some(InsertionPolicy::Last),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_parse() {
        assert_eq!(parse_direction("north"), Some(Direction::North));
        assert_eq!(parse_direction("east"), Some(Direction::East));
        assert_eq!(parse_direction("south"), Some(Direction::South));
        assert_eq!(parse_direction("west"), Some(Direction::West));
        assert_eq!(parse_direction("nope"), None);
    }

    #[test]
    fn selector_classifies_keywords() {
        assert_eq!(parse_selector("prev"), Selector::Prev);
        assert_eq!(parse_selector("next"), Selector::Next);
        assert_eq!(parse_selector("first"), Selector::First);
        assert_eq!(parse_selector("last"), Selector::Last);
        assert_eq!(parse_selector("recent"), Selector::Recent);
        assert_eq!(parse_selector("mouse"), Selector::Mouse);
        assert_eq!(parse_selector("stack"), Selector::Stack);
        assert_eq!(parse_selector("west"), Selector::Direction(Direction::West));
    }

    #[test]
    fn selector_classifies_indices_and_stack_indices() {
        assert_eq!(parse_selector("3"), Selector::Index(3));
        assert_eq!(parse_selector("stack.2"), Selector::StackIndex(2));
        // A non-numeric stack suffix is not a stack index; it falls to a label.
        assert_eq!(
            parse_selector("stack.foo"),
            Selector::Label("stack.foo".to_string())
        );
    }

    #[test]
    fn selector_falls_through_to_label() {
        assert_eq!(
            parse_selector("my-space"),
            Selector::Label("my-space".to_string())
        );
    }

    #[test]
    fn key_value_pairs_parse() {
        assert_eq!(
            parse_key_value("app=Safari"),
            Some(KeyValue {
                key: "app".to_string(),
                value: "Safari".to_string(),
                exclusion: false,
            })
        );
        assert_eq!(
            parse_key_value("title!=scratch"),
            Some(KeyValue {
                key: "title".to_string(),
                value: "scratch".to_string(),
                exclusion: true,
            })
        );
        // Earliest separator wins: the first `=` splits before the later `!=`.
        assert_eq!(
            parse_key_value("a=b!=c"),
            Some(KeyValue {
                key: "a".to_string(),
                value: "b!=c".to_string(),
                exclusion: false,
            })
        );
        assert_eq!(parse_key_value("noseparator"), None);
        assert_eq!(parse_key_value("emptyvalue="), None);
    }

    #[test]
    fn value_type_and_resize_handles_parse() {
        assert_eq!(parse_value_type("abs"), Some(ValueType::Abs));
        assert_eq!(parse_value_type("rel"), Some(ValueType::Rel));
        assert_eq!(parse_value_type("nope"), None);

        assert_eq!(parse_resize_handle("top"), Some(HANDLE_TOP));
        assert_eq!(
            parse_resize_handle("bottom_right"),
            Some(HANDLE_BOTTOM | HANDLE_RIGHT)
        );
        assert_eq!(parse_resize_handle("abs"), Some(HANDLE_ABS));
        assert_eq!(parse_resize_handle("middle"), None);
    }

    #[test]
    fn config_arguments_map_onto_layout_enums() {
        assert_eq!(parse_layout("bsp"), Some(ViewType::Bsp));
        assert_eq!(parse_layout("stack"), Some(ViewType::Stack));
        assert_eq!(parse_layout("float"), Some(ViewType::Float));
        assert_eq!(parse_layout("grid"), None);

        assert_eq!(parse_split_type("vertical"), Some(NodeSplit::Vertical));
        assert_eq!(parse_split_type("horizontal"), Some(NodeSplit::Horizontal));
        assert_eq!(parse_split_type("auto"), Some(NodeSplit::Auto));

        assert_eq!(parse_auto_balance("off"), Some(NodeSplit::None));
        assert_eq!(parse_auto_balance("on"), Some(NodeSplit::Auto));
        assert_eq!(parse_auto_balance("vertical"), Some(NodeSplit::Vertical));

        assert_eq!(parse_window_placement("first_child"), Some(Child::First));
        assert_eq!(parse_window_placement("second_child"), Some(Child::Second));

        assert_eq!(
            parse_insertion_policy("focused"),
            Some(InsertionPolicy::Focused)
        );
        assert_eq!(parse_insertion_policy("last"), Some(InsertionPolicy::Last));
    }
}
