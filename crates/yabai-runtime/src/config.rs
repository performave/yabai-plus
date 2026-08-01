//! Mutable daemon configuration, ported from the `config` settings that the C
//! daemon spreads across `g_window_manager` and `g_space_manager`.
//!
//! [`Config`] holds every setting the `yabai-core` command model can parse, and
//! knows how to apply a [`ConfigOp`] and report a setting's current value as the
//! C daemon would print it. The layout-relevant subset is projected into a
//! [`LayoutConfig`] for the per-space trees via [`Config::layout_config`].

use yabai_core::{
    ANIMATION_EASING_NAMES, Child, ConfigOp, ConfigValue, DisplayArrangementOrder, ExternalBar,
    ExternalBarMode, FfmMode, InsertionPolicy, LayoutConfig, MouseAction, MouseDropAction,
    MouseModifier, NodeSplit, ViewType, WindowOriginMode,
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
    /// `window_opacity`: when on, the daemon auto-applies `active_window_opacity`
    /// to the focused window and `normal_window_opacity` to the others.
    pub enable_window_opacity: bool,
    pub focus_follows_mouse: FfmMode,
    pub mouse_modifier: MouseModifier,
    pub mouse_action1: MouseAction,
    pub mouse_action2: MouseAction,
    pub mouse_drop_action: MouseDropAction,
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
    pub display_arrangement_order: DisplayArrangementOrder,
    pub window_origin_display: WindowOriginMode,
    /// Index into `yabai_core::ANIMATION_EASING_NAMES`.
    pub window_animation_easing: u8,
    /// `insert_feedback_color`, packed `0xAARRGGBB`.
    pub insert_feedback_color: u32,
    pub external_bar: ExternalBar,
    pub skip_window_focus_animation: bool,
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
            enable_window_opacity: false,
            focus_follows_mouse: FfmMode::Disabled,
            mouse_modifier: MouseModifier::Fn,
            mouse_action1: MouseAction::Move,
            mouse_action2: MouseAction::Resize,
            mouse_drop_action: MouseDropAction::Swap,
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
            display_arrangement_order: DisplayArrangementOrder::Default,
            window_origin_display: WindowOriginMode::Default,
            // C default `ease_out_circ_type` (index 19 in ANIMATION_EASING_NAMES).
            window_animation_easing: 19,
            // C default `rgba_color_from_hex(0xffd75f5f)`.
            insert_feedback_color: 0xffd7_5f5f,
            external_bar: ExternalBar {
                mode: ExternalBarMode::Off,
                top: 0,
                bottom: 0,
            },
            skip_window_focus_animation: false,
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
            "window_opacity" => bool_str(self.enable_window_opacity).to_string(),
            "focus_follows_mouse" => ffm_str(self.focus_follows_mouse).to_string(),
            "mouse_modifier" => mouse_mod_str(self.mouse_modifier).to_string(),
            "mouse_action1" => mouse_action_str(self.mouse_action1).to_string(),
            "mouse_action2" => mouse_action_str(self.mouse_action2).to_string(),
            "mouse_drop_action" => mouse_drop_str(self.mouse_drop_action).to_string(),
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
            "display_arrangement_order" => {
                arrangement_order_str(self.display_arrangement_order).to_string()
            }
            "window_origin_display" => window_origin_str(self.window_origin_display).to_string(),
            "window_animation_easing" => {
                ANIMATION_EASING_NAMES[self.window_animation_easing as usize].to_string()
            }
            // C prints `0x%x` (no zero-padding).
            "insert_feedback_color" => format!("0x{:x}", self.insert_feedback_color),
            "external_bar" => format!(
                "{}:{}:{}",
                external_bar_mode_str(self.external_bar.mode),
                self.external_bar.top,
                self.external_bar.bottom
            ),
            "skip_window_focus_animation" => bool_str(self.skip_window_focus_animation).to_string(),
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
            ("window_opacity", ConfigValue::Bool(b)) => self.enable_window_opacity = *b,
            ("mouse_modifier", ConfigValue::MouseMod(m)) => self.mouse_modifier = *m,
            ("mouse_action1", ConfigValue::MouseAction(a)) => self.mouse_action1 = *a,
            ("mouse_action2", ConfigValue::MouseAction(a)) => self.mouse_action2 = *a,
            ("mouse_drop_action", ConfigValue::MouseDrop(a)) => self.mouse_drop_action = *a,
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
            ("display_arrangement_order", ConfigValue::ArrangementOrder(o)) => {
                self.display_arrangement_order = *o
            }
            ("window_origin_display", ConfigValue::WindowOrigin(m)) => {
                self.window_origin_display = *m
            }
            ("window_animation_easing", ConfigValue::AnimationEasing(i)) => {
                self.window_animation_easing = *i
            }
            ("insert_feedback_color", ConfigValue::Color(c)) => self.insert_feedback_color = *c,
            ("external_bar", ConfigValue::ExternalBar(b)) => self.external_bar = *b,
            ("skip_window_focus_animation", ConfigValue::Bool(b)) => {
                self.skip_window_focus_animation = *b
            }
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

fn mouse_mod_str(modifier: MouseModifier) -> &'static str {
    match modifier {
        MouseModifier::Alt => "alt",
        MouseModifier::Shift => "shift",
        MouseModifier::Cmd => "cmd",
        MouseModifier::Ctrl => "ctrl",
        MouseModifier::Fn => "fn",
    }
}

fn mouse_action_str(action: MouseAction) -> &'static str {
    match action {
        MouseAction::Move => "move",
        MouseAction::Resize => "resize",
    }
}

fn mouse_drop_str(action: MouseDropAction) -> &'static str {
    match action {
        MouseDropAction::Swap => "swap",
        MouseDropAction::Stack => "stack",
    }
}

/// Mirrors `display_arrangement_order_str` in `src/display_manager.h`.
fn arrangement_order_str(order: DisplayArrangementOrder) -> &'static str {
    match order {
        DisplayArrangementOrder::Default => "default",
        DisplayArrangementOrder::Horizontal => "horizontal",
        DisplayArrangementOrder::Vertical => "vertical",
    }
}

/// Mirrors `window_origin_mode_str` in `src/window_manager.h`.
fn window_origin_str(mode: WindowOriginMode) -> &'static str {
    match mode {
        WindowOriginMode::Default => "default",
        WindowOriginMode::Focused => "focused",
        WindowOriginMode::Cursor => "cursor",
    }
}

/// Mirrors `external_bar_mode_str` in `src/display_manager.h`.
fn external_bar_mode_str(mode: ExternalBarMode) -> &'static str {
    match mode {
        ExternalBarMode::Off => "off",
        ExternalBarMode::Main => "main",
        ExternalBarMode::All => "all",
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

#[cfg(test)]
mod tests {
    use super::*;
    use yabai_core::parse_config;

    fn set(config: &mut Config, tokens: &[&str]) {
        let owned: Vec<String> = tokens.iter().map(|s| s.to_string()).collect();
        for op in parse_config(&owned).unwrap().ops {
            config.apply(&op).unwrap();
        }
    }

    fn get(config: &mut Config, key: &str) -> String {
        config
            .apply(&ConfigOp::Get(key.to_string()))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn extended_config_keys_round_trip() {
        let mut config = Config::default();

        // Defaults print the way the C daemon does.
        assert_eq!(get(&mut config, "display_arrangement_order"), "default");
        assert_eq!(get(&mut config, "window_origin_display"), "default");
        assert_eq!(get(&mut config, "window_animation_easing"), "ease_out_circ");
        assert_eq!(get(&mut config, "insert_feedback_color"), "0xffd75f5f");
        assert_eq!(get(&mut config, "external_bar"), "off:0:0");
        assert_eq!(get(&mut config, "skip_window_focus_animation"), "off");

        // Set/get round-trips.
        set(&mut config, &["display_arrangement_order", "horizontal"]);
        assert_eq!(get(&mut config, "display_arrangement_order"), "horizontal");
        set(&mut config, &["window_origin_display", "focused"]);
        assert_eq!(get(&mut config, "window_origin_display"), "focused");
        set(&mut config, &["window_animation_easing", "ease_in_sine"]);
        assert_eq!(get(&mut config, "window_animation_easing"), "ease_in_sine");
        set(&mut config, &["insert_feedback_color", "0x11223344"]);
        assert_eq!(get(&mut config, "insert_feedback_color"), "0x11223344");
        set(&mut config, &["external_bar", "main:28:4"]);
        assert_eq!(get(&mut config, "external_bar"), "main:28:4");
        set(&mut config, &["skip_window_focus_animation", "on"]);
        assert_eq!(get(&mut config, "skip_window_focus_animation"), "on");
    }
}
