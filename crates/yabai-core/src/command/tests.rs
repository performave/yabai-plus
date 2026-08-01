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
fn config_extended_keys_parse_typed_values() {
    let cases: &[(&[&str], ConfigValue)] = &[
        (
            &["display_arrangement_order", "vertical"],
            ConfigValue::ArrangementOrder(DisplayArrangementOrder::Vertical),
        ),
        (
            &["window_origin_display", "cursor"],
            ConfigValue::WindowOrigin(WindowOriginMode::Cursor),
        ),
        (
            &["window_animation_easing", "ease_out_circ"],
            ConfigValue::AnimationEasing(19),
        ),
        (
            &["insert_feedback_color", "0xffd75f5f"],
            ConfigValue::Color(0xffd7_5f5f),
        ),
        (
            &["external_bar", "all:32:0"],
            ConfigValue::ExternalBar(ExternalBar {
                mode: ExternalBarMode::All,
                top: 32,
                bottom: 0,
            }),
        ),
        (
            &["skip_window_focus_animation", "on"],
            ConfigValue::Bool(true),
        ),
    ];
    for (tokens, expected) in cases {
        let cmd = parse_config(&toks(tokens)).unwrap();
        assert_eq!(
            cmd.ops,
            vec![ConfigOp::Set(tokens[0].to_string(), expected.clone())],
            "parsing {tokens:?}"
        );
    }

    // Invalid values are rejected (unknown easing, zero color, bad bar mode).
    for bad in [
        vec!["window_animation_easing", "bogus"],
        vec!["insert_feedback_color", "0x0"],
        vec!["external_bar", "sometimes:1:2"],
        vec!["display_arrangement_order", "diagonal"],
    ] {
        assert!(
            parse_config(&toks(&bad)).is_err(),
            "expected error for {bad:?}"
        );
    }
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
fn config_mouse_settings_parse() {
    let cmd = parse_config(&toks(&["mouse_modifier", "cmd"])).unwrap();
    assert_eq!(
        cmd.ops,
        vec![ConfigOp::Set(
            "mouse_modifier".to_string(),
            ConfigValue::MouseMod(MouseModifier::Cmd)
        )]
    );
    let cmd = parse_config(&toks(&["mouse_action1", "move"])).unwrap();
    assert_eq!(
        cmd.ops,
        vec![ConfigOp::Set(
            "mouse_action1".to_string(),
            ConfigValue::MouseAction(MouseAction::Move)
        )]
    );
    let cmd = parse_config(&toks(&["mouse_drop_action", "stack"])).unwrap();
    assert_eq!(
        cmd.ops,
        vec![ConfigOp::Set(
            "mouse_drop_action".to_string(),
            ConfigValue::MouseDrop(MouseDropAction::Stack)
        )]
    );
    // A bad value reports the C-faithful error.
    let err = parse_config(&toks(&["mouse_modifier", "hyper"])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown value 'hyper' given to command 'mouse_modifier' for domain 'config'"
    );
}

#[test]
fn window_target_and_simple_actions() {
    let cmd = parse_window(&toks(&["--close"])).unwrap();
    assert_eq!(cmd.target, None);
    assert_eq!(cmd.actions, vec![WindowAction::Close(None)]);

    let cmd = parse_window(&toks(&["--close", "first"])).unwrap();
    assert_eq!(
        cmd.actions,
        vec![WindowAction::Close(Some(Selector::First))]
    );

    let cmd = parse_window(&toks(&["5", "--minimize"])).unwrap();
    assert_eq!(cmd.target, Some(Selector::Index(5)));
    assert_eq!(cmd.actions, vec![WindowAction::Minimize(None)]);

    let cmd = parse_window(&toks(&["--minimize", "last"])).unwrap();
    assert_eq!(
        cmd.actions,
        vec![WindowAction::Minimize(Some(Selector::Last))]
    );

    let cmd = parse_window(&toks(&["--deminimize"])).unwrap();
    assert_eq!(cmd.actions, vec![WindowAction::Deminimize(None)]);

    let cmd = parse_window(&toks(&["9", "--deminimize"])).unwrap();
    assert_eq!(cmd.target, Some(Selector::Index(9)));
    assert_eq!(cmd.actions, vec![WindowAction::Deminimize(None)]);

    let cmd = parse_window(&toks(&["--deminimize", "first"])).unwrap();
    assert_eq!(
        cmd.actions,
        vec![WindowAction::Deminimize(Some(Selector::First))]
    );
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

    let cmd = parse_window(&toks(&["--opacity", "0.75"])).unwrap();
    assert_eq!(cmd.actions, vec![WindowAction::Opacity(0.75)]);

    let cmd = parse_window(&toks(&["--sub-layer", "above"])).unwrap();
    assert_eq!(cmd.actions, vec![WindowAction::SubLayer(Layer::Above)]);

    let cmd = parse_window(&toks(&["--insert", "stack"])).unwrap();
    assert_eq!(
        cmd.actions,
        vec![WindowAction::Insert(InsertDirection::Stack)]
    );
}

#[test]
fn window_bad_resize_handle_is_unknown_value() {
    let err = parse_window(&toks(&["--resize", "middle:1:1"])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown value 'middle:1:1' given to command '--resize' for domain 'window'"
    );

    let err = parse_window(&toks(&["--opacity", "1.5"])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown value '1.5' given to command '--opacity' for domain 'window'"
    );

    let err = parse_window(&toks(&["--sub-layer", "front"])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown value 'front' given to command '--sub-layer' for domain 'window'"
    );

    let err = parse_window(&toks(&["--insert", "sideways"])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown value 'sideways' given to command '--insert' for domain 'window'"
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
fn window_raise_lower_optional_selector() {
    // Bare `--raise`/`--lower` carry no reference selector.
    let cmd = parse_window(&toks(&["--raise"])).unwrap();
    assert_eq!(cmd.actions, vec![WindowAction::Raise(None)]);
    let cmd = parse_window(&toks(&["--lower"])).unwrap();
    assert_eq!(cmd.actions, vec![WindowAction::Lower(None)]);

    // A trailing non-`--` token is the reference window selector.
    let cmd = parse_window(&toks(&["--raise", "42"])).unwrap();
    assert_eq!(
        cmd.actions,
        vec![WindowAction::Raise(Some(Selector::Index(42)))]
    );

    // A following `--command` is not consumed as the selector.
    let cmd = parse_window(&toks(&["--lower", "--focus"])).unwrap();
    assert_eq!(
        cmd.actions,
        vec![WindowAction::Lower(None), WindowAction::Focus(None)]
    );
}

#[test]
fn window_scratchpad_allows_bare_remove() {
    let cmd = parse_window(&toks(&["--scratchpad"])).unwrap();
    assert_eq!(
        cmd.actions,
        vec![WindowAction::Scratchpad(ScratchpadAction::Remove)]
    );

    let cmd = parse_window(&toks(&["--scratchpad", "notes"])).unwrap();
    assert_eq!(
        cmd.actions,
        vec![WindowAction::Scratchpad(ScratchpadAction::Label(
            "notes".to_string()
        ))]
    );

    let cmd = parse_window(&toks(&["--scratchpad", "recover"])).unwrap();
    assert_eq!(
        cmd.actions,
        vec![WindowAction::Scratchpad(ScratchpadAction::Recover)]
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
fn query_empty_property_segment_errors() {
    for properties in ["id,,frame", ",id", "id,"] {
        let err = parse_query(&toks(&["--windows", properties])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown value '' given to command '--windows' for domain 'query'"
        );
    }
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
        RuleCommand::Add {
            one_shot: false,
            pairs: vec![
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
            ],
        }
    );

    // `--one-shot` is consumed as a flag, not a key-value.
    assert_eq!(
        parse_rule(&toks(&["--add", "--one-shot", "app=Safari"])).unwrap(),
        RuleCommand::Add {
            one_shot: true,
            pairs: vec![KeyValue {
                key: "app".to_string(),
                value: "Safari".to_string(),
                exclusion: false,
            }],
        }
    );

    assert_eq!(parse_rule(&toks(&["--list"])).unwrap(), RuleCommand::List);
    assert_eq!(
        parse_rule(&toks(&["--remove", "myrule"])).unwrap(),
        RuleCommand::Remove(Some(Selector::Label("myrule".to_string())))
    );
    assert_eq!(
        parse_rule(&toks(&["--apply"])).unwrap(),
        RuleCommand::Apply(RuleApply::All)
    );
    assert_eq!(
        parse_rule(&toks(&["--apply", "myrule"])).unwrap(),
        RuleCommand::Apply(RuleApply::Selector(Selector::Label("myrule".to_string())))
    );
    assert_eq!(
        parse_rule(&toks(&["--apply", "app=Safari", "manage=off"])).unwrap(),
        RuleCommand::Apply(RuleApply::AdHoc {
            label: "app=Safari".to_string(),
            pairs: vec![
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
            ],
        })
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
