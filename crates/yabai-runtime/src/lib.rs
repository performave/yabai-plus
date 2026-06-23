pub mod actor;
pub mod app_state;
pub mod config;
pub mod runtime;

pub use actor::Actor;
pub use app_state::{AppState, LayoutSink, RecordingSink, Response, StateEvent};
pub use config::Config;
pub use runtime::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    ApplicationLaunched,
    ApplicationTerminated,
    ApplicationFrontSwitched,
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
    SlsWindowOrdered,
    SlsWindowDestroyed,
    SlsSpaceCreated,
    SlsSpaceDestroyed,
    SpaceChanged,
    DisplayAdded,
    DisplayRemoved,
    DisplayMoved,
    DisplayResized,
    DisplayChanged,
    MouseDown,
    MouseUp,
    MouseDragged,
    MouseMoved,
    MissionControlShowAllWindows,
    MissionControlShowFrontWindows,
    MissionControlShowDesktop,
    MissionControlEnter,
    MissionControlCheckForExit,
    MissionControlExit,
    DockDidRestart,
    MenuOpened,
    MenuClosed,
    MenuBarHiddenChanged,
    DockDidChangePref,
    SystemWoke,
    DaemonMessage,
}

impl Event {
    pub const fn c_name(self) -> &'static str {
        match self {
            Self::ApplicationLaunched => "APPLICATION_LAUNCHED",
            Self::ApplicationTerminated => "APPLICATION_TERMINATED",
            Self::ApplicationFrontSwitched => "APPLICATION_FRONT_SWITCHED",
            Self::ApplicationVisible => "APPLICATION_VISIBLE",
            Self::ApplicationHidden => "APPLICATION_HIDDEN",
            Self::WindowCreated => "WINDOW_CREATED",
            Self::WindowDestroyed => "WINDOW_DESTROYED",
            Self::WindowFocused => "WINDOW_FOCUSED",
            Self::WindowMoved => "WINDOW_MOVED",
            Self::WindowResized => "WINDOW_RESIZED",
            Self::WindowMinimized => "WINDOW_MINIMIZED",
            Self::WindowDeminimized => "WINDOW_DEMINIMIZED",
            Self::WindowTitleChanged => "WINDOW_TITLE_CHANGED",
            Self::SlsWindowOrdered => "SLS_WINDOW_ORDERED",
            Self::SlsWindowDestroyed => "SLS_WINDOW_DESTROYED",
            Self::SlsSpaceCreated => "SLS_SPACE_CREATED",
            Self::SlsSpaceDestroyed => "SLS_SPACE_DESTROYED",
            Self::SpaceChanged => "SPACE_CHANGED",
            Self::DisplayAdded => "DISPLAY_ADDED",
            Self::DisplayRemoved => "DISPLAY_REMOVED",
            Self::DisplayMoved => "DISPLAY_MOVED",
            Self::DisplayResized => "DISPLAY_RESIZED",
            Self::DisplayChanged => "DISPLAY_CHANGED",
            Self::MouseDown => "MOUSE_DOWN",
            Self::MouseUp => "MOUSE_UP",
            Self::MouseDragged => "MOUSE_DRAGGED",
            Self::MouseMoved => "MOUSE_MOVED",
            Self::MissionControlShowAllWindows => "MISSION_CONTROL_SHOW_ALL_WINDOWS",
            Self::MissionControlShowFrontWindows => "MISSION_CONTROL_SHOW_FRONT_WINDOWS",
            Self::MissionControlShowDesktop => "MISSION_CONTROL_SHOW_DESKTOP",
            Self::MissionControlEnter => "MISSION_CONTROL_ENTER",
            Self::MissionControlCheckForExit => "MISSION_CONTROL_CHECK_FOR_EXIT",
            Self::MissionControlExit => "MISSION_CONTROL_EXIT",
            Self::DockDidRestart => "DOCK_DID_RESTART",
            Self::MenuOpened => "MENU_OPENED",
            Self::MenuClosed => "MENU_CLOSED",
            Self::MenuBarHiddenChanged => "MENU_BAR_HIDDEN_CHANGED",
            Self::DockDidChangePref => "DOCK_DID_CHANGE_PREF",
            Self::SystemWoke => "SYSTEM_WOKE",
            Self::DaemonMessage => "DAEMON_MESSAGE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_match_c_event_type_list() {
        assert_eq!(Event::ApplicationLaunched.c_name(), "APPLICATION_LAUNCHED");
        assert_eq!(Event::DaemonMessage.c_name(), "DAEMON_MESSAGE");
    }
}
