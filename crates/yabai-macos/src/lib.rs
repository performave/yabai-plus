#![cfg_attr(not(target_os = "macos"), allow(unused))]

#[cfg(target_os = "macos")]
pub mod ax;

#[cfg(target_os = "macos")]
pub use ax::{AxSink, AxWindow};

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
