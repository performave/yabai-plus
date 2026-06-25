#![cfg_attr(not(target_os = "macos"), allow(unused))]

#[cfg(target_os = "macos")]
pub mod ax;
#[cfg(target_os = "macos")]
pub mod cgwindow;
#[cfg(target_os = "macos")]
pub mod display;
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
    AxDiagnostics, AxPidDiagnostics, AxSink, AxWindow, DiscoveredAxWindow, accessibility_trusted,
    accessibility_trusted_with_prompt, focused_window, focused_window_diagnostics,
    move_focused_window, move_pid_window, tileable_pid_windows, windows_for_pid,
    windows_for_pid_diagnostics,
};
#[cfg(target_os = "macos")]
pub use cgwindow::{CgWindow, application_pids_with_windows, on_screen_windows};
#[cfg(target_os = "macos")]
pub use display::{MacDisplay, active_displays};
#[cfg(target_os = "macos")]
pub use observe::{ObservedEvent, observe_pid};
#[cfg(target_os = "macos")]
pub use screen::{main_visible_frame, visible_frame_for_display};
#[cfg(target_os = "macos")]
pub use space::{
    current_space_for_display, mission_control_spaces, spaces_for_display, spaces_for_window,
    switch_space_by_gesture,
};
#[cfg(target_os = "macos")]
pub use workspace::{WorkspaceEvent, observe_workspace, regular_application_pids};

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
