//! The `signal` domain data model, ported from `src/event_signal.{h,c}` and the
//! `handle_domain_signal` validator in `src/message.c`.
//!
//! This layer is pure: it parses and stores signal definitions and resolves
//! which ones fire for an event. Running the action command (the C `fork` +
//! `execvp` of `/usr/bin/env sh -c <action>`) is the daemon's job.

use crate::parser::KeyValue;

/// The set of events a signal can subscribe to, in the same order as
/// `enum signal_type` so `signal --list` groups identically to the C daemon.
/// `as_str`/`from_name` mirror `signal_type_str` / `signal_type_from_string`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalEvent {
    ApplicationLaunched,
    ApplicationTerminated,
    ApplicationFrontSwitched,
    ApplicationActivated,
    ApplicationDeactivated,
    ApplicationVisible,
    ApplicationHidden,
    WindowCreated,
    WindowDestroyed,
    WindowFocused,
    WindowMoved,
    WindowResized,
    WindowMinimized,
    WindowDeminimized,
    WindowTitleChanged,
    SpaceCreated,
    SpaceDestroyed,
    SpaceChanged,
    DisplayAdded,
    DisplayRemoved,
    DisplayMoved,
    DisplayResized,
    DisplayChanged,
    MissionControlEnter,
    MissionControlExit,
    DockDidChangePref,
    DockDidRestart,
    MenuBarHiddenChanged,
    SystemWoke,
}

impl SignalEvent {
    /// All events in `enum signal_type` order, used to group `signal --list`.
    pub const ALL: [SignalEvent; 29] = [
        SignalEvent::ApplicationLaunched,
        SignalEvent::ApplicationTerminated,
        SignalEvent::ApplicationFrontSwitched,
        SignalEvent::ApplicationActivated,
        SignalEvent::ApplicationDeactivated,
        SignalEvent::ApplicationVisible,
        SignalEvent::ApplicationHidden,
        SignalEvent::WindowCreated,
        SignalEvent::WindowDestroyed,
        SignalEvent::WindowFocused,
        SignalEvent::WindowMoved,
        SignalEvent::WindowResized,
        SignalEvent::WindowMinimized,
        SignalEvent::WindowDeminimized,
        SignalEvent::WindowTitleChanged,
        SignalEvent::SpaceCreated,
        SignalEvent::SpaceDestroyed,
        SignalEvent::SpaceChanged,
        SignalEvent::DisplayAdded,
        SignalEvent::DisplayRemoved,
        SignalEvent::DisplayMoved,
        SignalEvent::DisplayResized,
        SignalEvent::DisplayChanged,
        SignalEvent::MissionControlEnter,
        SignalEvent::MissionControlExit,
        SignalEvent::DockDidChangePref,
        SignalEvent::DockDidRestart,
        SignalEvent::MenuBarHiddenChanged,
        SignalEvent::SystemWoke,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SignalEvent::ApplicationLaunched => "application_launched",
            SignalEvent::ApplicationTerminated => "application_terminated",
            SignalEvent::ApplicationFrontSwitched => "application_front_switched",
            SignalEvent::ApplicationActivated => "application_activated",
            SignalEvent::ApplicationDeactivated => "application_deactivated",
            SignalEvent::ApplicationVisible => "application_visible",
            SignalEvent::ApplicationHidden => "application_hidden",
            SignalEvent::WindowCreated => "window_created",
            SignalEvent::WindowDestroyed => "window_destroyed",
            SignalEvent::WindowFocused => "window_focused",
            SignalEvent::WindowMoved => "window_moved",
            SignalEvent::WindowResized => "window_resized",
            SignalEvent::WindowMinimized => "window_minimized",
            SignalEvent::WindowDeminimized => "window_deminimized",
            SignalEvent::WindowTitleChanged => "window_title_changed",
            SignalEvent::SpaceCreated => "space_created",
            SignalEvent::SpaceDestroyed => "space_destroyed",
            SignalEvent::SpaceChanged => "space_changed",
            SignalEvent::DisplayAdded => "display_added",
            SignalEvent::DisplayRemoved => "display_removed",
            SignalEvent::DisplayMoved => "display_moved",
            SignalEvent::DisplayResized => "display_resized",
            SignalEvent::DisplayChanged => "display_changed",
            SignalEvent::MissionControlEnter => "mission_control_enter",
            SignalEvent::MissionControlExit => "mission_control_exit",
            SignalEvent::DockDidChangePref => "dock_did_change_pref",
            SignalEvent::DockDidRestart => "dock_did_restart",
            SignalEvent::MenuBarHiddenChanged => "menu_bar_hidden_changed",
            SignalEvent::SystemWoke => "system_woke",
        }
    }

    pub fn from_name(value: &str) -> Option<SignalEvent> {
        SignalEvent::ALL.into_iter().find(|e| e.as_str() == value)
    }
}

/// A registered signal: an event subscription plus the shell action to run, with
/// optional `label`/`app`/`title` filters. `app`/`title` are stored verbatim with
/// their exclusion flags; regex compilation/matching happens in the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub event: SignalEvent,
    pub action: String,
    pub label: Option<String>,
    pub app: Option<String>,
    pub app_exclude: bool,
    pub title: Option<String>,
    pub title_exclude: bool,
    /// `active=yes|no`: `None` when unspecified (serialized as `null`).
    pub active: Option<bool>,
}

impl Signal {
    /// Build a signal from `signal --add` key-values, mirroring the validation in
    /// `handle_domain_signal`: unknown keys, the two required pairs, the `active`
    /// value domain, and the `!` (exclusion) restriction on non-regex keys all
    /// reproduce the C `daemon_fail` text. `app`/`title` accept the `!=` form and
    /// preserve the exclusion flags for runtime regex matching.
    pub fn from_key_values(pairs: &[KeyValue]) -> Result<Signal, String> {
        let mut event = None;
        let mut action = None;
        let mut label = None;
        let mut app = None;
        let mut app_exclude = false;
        let mut title = None;
        let mut title_exclude = false;
        let mut active = None;

        for KeyValue {
            key,
            value,
            exclusion,
        } in pairs
        {
            match key.as_str() {
                "label" => {
                    if *exclusion {
                        return Err(unsupported_exclusion(key));
                    }
                    label = Some(value.clone());
                }
                "app" => {
                    app = Some(value.clone());
                    app_exclude = *exclusion;
                }
                "title" => {
                    title = Some(value.clone());
                    title_exclude = *exclusion;
                }
                "active" => {
                    if *exclusion {
                        return Err(unsupported_exclusion(key));
                    }
                    active = Some(match value.as_str() {
                        "yes" => true,
                        "no" => false,
                        _ => {
                            return Err(format!("invalid value '{value}' for key '{key}'\n"));
                        }
                    });
                }
                "action" => {
                    if *exclusion {
                        return Err(unsupported_exclusion(key));
                    }
                    action = Some(value.clone());
                }
                "event" => {
                    if *exclusion {
                        return Err(unsupported_exclusion(key));
                    }
                    match SignalEvent::from_name(value) {
                        Some(e) => event = Some(e),
                        None => {
                            return Err(format!("invalid value '{value}' for key '{key}'\n"));
                        }
                    }
                }
                _ => return Err(format!("unknown key '{key}'\n")),
            }
        }

        let event = event.ok_or("missing required key-value pair 'event=..'\n".to_string())?;
        let action = action.ok_or("missing required key-value pair 'action=..'\n".to_string())?;

        Ok(Signal {
            event,
            action,
            label,
            app,
            app_exclude,
            title,
            title_exclude,
            active,
        })
    }
}

fn unsupported_exclusion(key: &str) -> String {
    format!("unsupported token '!' (exclusion) given for key '{key}'\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: value.to_string(),
            exclusion: false,
        }
    }

    fn kv_excl(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: value.to_string(),
            exclusion: true,
        }
    }

    #[test]
    fn event_round_trips_through_strings() {
        for event in SignalEvent::ALL {
            assert_eq!(SignalEvent::from_name(event.as_str()), Some(event));
        }
        assert_eq!(SignalEvent::from_name("nope"), None);
    }

    #[test]
    fn from_key_values_builds_a_signal() {
        let signal = Signal::from_key_values(&[
            kv("event", "window_focused"),
            kv("action", "echo hi"),
            kv("label", "greet"),
            kv("app", "^Finder$"),
            kv_excl("title", "^Scratch$"),
            kv("active", "yes"),
        ])
        .unwrap();
        assert_eq!(signal.event, SignalEvent::WindowFocused);
        assert_eq!(signal.action, "echo hi");
        assert_eq!(signal.label.as_deref(), Some("greet"));
        assert_eq!(signal.app.as_deref(), Some("^Finder$"));
        assert!(!signal.app_exclude);
        assert_eq!(signal.title.as_deref(), Some("^Scratch$"));
        assert!(signal.title_exclude);
        assert_eq!(signal.active, Some(true));
    }

    #[test]
    fn from_key_values_reports_faithful_errors() {
        let missing_event = Signal::from_key_values(&[kv("action", "x")]).unwrap_err();
        assert!(missing_event.contains("missing required key-value pair 'event=..'"));

        let missing_action = Signal::from_key_values(&[kv("event", "window_created")]).unwrap_err();
        assert!(missing_action.contains("missing required key-value pair 'action=..'"));

        let unknown = Signal::from_key_values(&[kv("bogus", "x")]).unwrap_err();
        assert!(unknown.contains("unknown key 'bogus'"));

        let bad_event =
            Signal::from_key_values(&[kv("event", "nope"), kv("action", "x")]).unwrap_err();
        assert!(bad_event.contains("invalid value 'nope' for key 'event'"));

        let bad_active = Signal::from_key_values(&[
            kv("event", "window_created"),
            kv("action", "x"),
            kv("active", "maybe"),
        ])
        .unwrap_err();
        assert!(bad_active.contains("invalid value 'maybe' for key 'active'"));

        let excl = Signal::from_key_values(&[
            KeyValue {
                key: "action".to_string(),
                value: "x".to_string(),
                exclusion: true,
            },
            kv("event", "window_created"),
        ])
        .unwrap_err();
        assert!(excl.contains("unsupported token '!' (exclusion) given for key 'action'"));
    }
}
