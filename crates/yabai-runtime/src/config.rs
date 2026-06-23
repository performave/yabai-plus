//! Mutable daemon configuration, ported from the `config` settings that the C
//! daemon spreads across `g_window_manager` and `g_space_manager`.
//!
//! [`Config`] holds every setting the `yabai-core` command model can parse, and
//! knows how to apply a [`ConfigOp`] and report a setting's current value as the
//! C daemon would print it. The layout-relevant subset is projected into a
//! [`LayoutConfig`] for the per-space trees via [`Config::layout_config`].

use yabai_core::{
    Child, ConfigOp, ConfigValue, FfmMode, InsertionPolicy, LayoutConfig, NodeSplit, ViewType,
};

/// All daemon-configurable settings the command model understands.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub debug_output: bool,
    pub mouse_follows_focus: bool,
    pub window_sublayer_auto: bool,
    pub manage: bool,
    pub window_zoom_persist: bool,
    pub window_shadow: bool,
    pub focus_follows_mouse: FfmMode,
    pub layout: ViewType,
    pub split_type: NodeSplit,
    pub auto_balance: NodeSplit,
    pub window_placement: Child,
    pub window_insertion_point: InsertionPolicy,
    pub split_ratio: f32,
    pub window_opacity_duration: f32,
    pub window_animation_duration: f32,
    pub active_window_opacity: f32,
    pub normal_window_opacity: f32,
    pub menubar_opacity: f32,
    pub top_padding: i32,
    pub bottom_padding: i32,
    pub left_padding: i32,
    pub right_padding: i32,
    pub window_gap: i32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            debug_output: false,
            mouse_follows_focus: false,
            window_sublayer_auto: true,
            manage: true,
            window_zoom_persist: true,
            window_shadow: true,
            focus_follows_mouse: FfmMode::Disabled,
            layout: ViewType::Bsp,
            split_type: NodeSplit::Auto,
            auto_balance: NodeSplit::None,
            window_placement: Child::Second,
            window_insertion_point: InsertionPolicy::Focused,
            split_ratio: 0.5,
            window_opacity_duration: 0.0,
            window_animation_duration: 0.0,
            active_window_opacity: 1.0,
            normal_window_opacity: 1.0,
            menubar_opacity: 1.0,
            top_padding: 0,
            bottom_padding: 0,
            left_padding: 0,
            right_padding: 0,
            window_gap: 0,
        }
    }
}

impl Config {
    /// Project the layout-relevant settings into a [`LayoutConfig`] for trees.
    pub fn layout_config(&self) -> LayoutConfig {
        LayoutConfig {
            split_type: self.split_type,
            split_ratio: self.split_ratio,
            window_placement: self.window_placement,
            auto_balance: self.auto_balance,
            insertion_policy: self.window_insertion_point,
            gap: self.window_gap,
        }
    }

    /// Apply a parsed [`ConfigOp`]. A `Get` returns the current value formatted
    /// as the C daemon prints it; a `Set` mutates state and returns `None`.
    /// Returns `Err` only for a key this struct does not know (the command
    /// parser already rejects unknown keys, so this is defensive).
    pub fn apply(&mut self, op: &ConfigOp) -> Result<Option<String>, String> {
        match op {
            ConfigOp::Get(key) => self.get(key).map(Some),
            ConfigOp::Set(key, value) => {
                self.set(key, value)?;
                Ok(None)
            }
        }
    }

    fn get(&self, key: &str) -> Result<String, String> {
        let out = match key {
            "debug_output" => bool_str(self.debug_output).to_string(),
            "mouse_follows_focus" => bool_str(self.mouse_follows_focus).to_string(),
            "window_sublayer_auto" => bool_str(self.window_sublayer_auto).to_string(),
            "manage" => bool_str(self.manage).to_string(),
            "window_zoom_persist" => bool_str(self.window_zoom_persist).to_string(),
            "window_shadow" => bool_str(self.window_shadow).to_string(),
            "focus_follows_mouse" => ffm_str(self.focus_follows_mouse).to_string(),
            "layout" => layout_str(self.layout).to_string(),
            "split_type" => split_type_str(self.split_type).to_string(),
            "auto_balance" => auto_balance_str(self.auto_balance).to_string(),
            "window_placement" => placement_str(self.window_placement).to_string(),
            "window_insertion_point" => insertion_str(self.window_insertion_point).to_string(),
            "split_ratio" => format!("{:.4}", self.split_ratio),
            "window_opacity_duration" => format!("{:.4}", self.window_opacity_duration),
            "window_animation_duration" => format!("{:.4}", self.window_animation_duration),
            "active_window_opacity" => format!("{:.4}", self.active_window_opacity),
            "normal_window_opacity" => format!("{:.4}", self.normal_window_opacity),
            "menubar_opacity" => format!("{:.4}", self.menubar_opacity),
            "top_padding" => self.top_padding.to_string(),
            "bottom_padding" => self.bottom_padding.to_string(),
            "left_padding" => self.left_padding.to_string(),
            "right_padding" => self.right_padding.to_string(),
            "window_gap" => self.window_gap.to_string(),
            other => return Err(format!("unsupported config key '{other}'")),
        };
        Ok(out)
    }

    fn set(&mut self, key: &str, value: &ConfigValue) -> Result<(), String> {
        match (key, value) {
            ("debug_output", ConfigValue::Bool(b)) => self.debug_output = *b,
            ("mouse_follows_focus", ConfigValue::Bool(b)) => self.mouse_follows_focus = *b,
            ("window_sublayer_auto", ConfigValue::Bool(b)) => self.window_sublayer_auto = *b,
            ("manage", ConfigValue::Bool(b)) => self.manage = *b,
            ("window_zoom_persist", ConfigValue::Bool(b)) => self.window_zoom_persist = *b,
            ("window_shadow", ConfigValue::Bool(b)) => self.window_shadow = *b,
            ("focus_follows_mouse", ConfigValue::Ffm(m)) => self.focus_follows_mouse = *m,
            ("layout", ConfigValue::Layout(l)) => self.layout = *l,
            ("split_type", ConfigValue::SplitType(s)) => self.split_type = *s,
            ("auto_balance", ConfigValue::AutoBalance(s)) => self.auto_balance = *s,
            ("window_placement", ConfigValue::Placement(c)) => self.window_placement = *c,
            ("window_insertion_point", ConfigValue::InsertionPoint(i)) => {
                self.window_insertion_point = *i
            }
            ("split_ratio", ConfigValue::Float(f)) => self.split_ratio = *f,
            ("window_opacity_duration", ConfigValue::Float(f)) => self.window_opacity_duration = *f,
            ("window_animation_duration", ConfigValue::Float(f)) => {
                self.window_animation_duration = *f
            }
            ("active_window_opacity", ConfigValue::Float(f)) => self.active_window_opacity = *f,
            ("normal_window_opacity", ConfigValue::Float(f)) => self.normal_window_opacity = *f,
            ("menubar_opacity", ConfigValue::Float(f)) => self.menubar_opacity = *f,
            ("top_padding", ConfigValue::Int(i)) => self.top_padding = *i,
            ("bottom_padding", ConfigValue::Int(i)) => self.bottom_padding = *i,
            ("left_padding", ConfigValue::Int(i)) => self.left_padding = *i,
            ("right_padding", ConfigValue::Int(i)) => self.right_padding = *i,
            ("window_gap", ConfigValue::Int(i)) => self.window_gap = *i,
            (other, _) => return Err(format!("unsupported config key '{other}'")),
        }
        Ok(())
    }
}

fn bool_str(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

fn ffm_str(mode: FfmMode) -> &'static str {
    match mode {
        FfmMode::Disabled => "off",
        FfmMode::Autofocus => "autofocus",
        FfmMode::Autoraise => "autoraise",
    }
}

fn layout_str(layout: ViewType) -> &'static str {
    match layout {
        ViewType::Bsp => "bsp",
        ViewType::Stack => "stack",
        ViewType::Float => "float",
    }
}

fn split_type_str(split: NodeSplit) -> &'static str {
    match split {
        NodeSplit::Vertical => "vertical",
        NodeSplit::Horizontal => "horizontal",
        NodeSplit::Auto => "auto",
        NodeSplit::None => "none",
    }
}

/// Mirrors `auto_balance_str` in `src/view.h`.
fn auto_balance_str(split: NodeSplit) -> &'static str {
    match split {
        NodeSplit::None => "off",
        NodeSplit::Vertical => "vertical",
        NodeSplit::Horizontal => "horizontal",
        NodeSplit::Auto => "on",
    }
}

fn placement_str(child: Child) -> &'static str {
    match child {
        Child::First => "first_child",
        Child::Second => "second_child",
        Child::None => "none",
    }
}

fn insertion_str(point: InsertionPolicy) -> &'static str {
    match point {
        InsertionPolicy::Focused => "focused",
        InsertionPolicy::First => "first",
        InsertionPolicy::Last => "last",
    }
}
