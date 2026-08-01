#![cfg_attr(not(target_os = "macos"), allow(unused))]

#[cfg(target_os = "macos")]
pub mod ax;
#[cfg(target_os = "macos")]
pub mod cgwindow;
#[cfg(target_os = "macos")]
pub mod coredock;
#[cfg(target_os = "macos")]
pub mod display;
#[cfg(target_os = "macos")]
pub mod mouse;
#[cfg(target_os = "macos")]
pub mod objc;
#[cfg(target_os = "macos")]
pub mod observe;
#[cfg(target_os = "macos")]
pub mod screen;
#[cfg(target_os = "macos")]
pub mod space;
#[cfg(target_os = "macos")]
pub mod workspace;

#[cfg(target_os = "macos")]
pub use ax::{
    AxDiagnostics, AxPidDiagnostics, AxSink, AxWindow, AxWindowInfo, DiscoveredAxWindow,
    accessibility_trusted, accessibility_trusted_with_prompt, focused_window,
    focused_window_diagnostics, move_focused_window, move_pid_window, pid_window_infos,
    tileable_pid_windows, windows_for_pid, windows_for_pid_diagnostics,
};
#[cfg(target_os = "macos")]
pub use cgwindow::{CgWindow, application_pids_with_windows, on_screen_windows};
#[cfg(target_os = "macos")]
pub use display::{
    MacDisplay, active_displays, cursor_display_id, cursor_location, main_display_id,
    observe_display_reconfiguration, set_active_display, warp_cursor_to_display_center,
    warp_cursor_to_point,
};
#[cfg(target_os = "macos")]
pub use mouse::{
    MOUSE_MOD_ALT, MOUSE_MOD_CMD, MOUSE_MOD_CTRL, MOUSE_MOD_FN, MOUSE_MOD_SHIFT, MouseDragButton,
    MouseDragEvent, observe_mouse_drag, observe_mouse_moved, post_mouse_drag, post_mouse_moved,
    post_right_mouse_drag, set_drag_modifier,
};
#[cfg(target_os = "macos")]
pub use observe::{MissionControlEvent, ObservedEvent, observe_mission_control, observe_pid};
#[cfg(target_os = "macos")]
pub use screen::{main_visible_frame, visible_frame_for_display};
#[cfg(target_os = "macos")]
pub use space::{
    current_space_for_display, display_for_space, mission_control_spaces, spaces_for_display,
    spaces_for_window, switch_space_by_gesture, window_alpha, window_bounds, window_is_ordered_in,
    window_level, window_transform, windows_on_space,
};
#[cfg(target_os = "macos")]
pub use workspace::{
    WorkspaceEvent, dock_pid, ns_application_load, observe_workspace, regular_application_pids,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpaceId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pid(pub i32);

pub mod private_api {
    pub const SKYLIGHT_FRAMEWORK_PATH: &str =
        "/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight";
}
