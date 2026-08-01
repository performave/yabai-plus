pub use yabai_osax_common as common;

mod client;
pub use client::{Payload, ScriptingAddition, request_handshake, send_message, status};

/// The compiled OSAX loader executable (fat x86_64 + arm64e), built from
/// `osax/loader.m` by `build.rs`. `--load-sa` writes it to disk and runs it to
/// inject the payload into Dock.
#[cfg(target_os = "macos")]
pub const LOADER_BINARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/loader"));

/// The compiled OSAX payload dylib (fat x86_64 + arm64e), built from
/// `osax/payload.m` by `build.rs` — the code that runs inside Dock.
#[cfg(target_os = "macos")]
pub const PAYLOAD_BINARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptingAdditionStatus {
    NotLoaded,
    Outdated { payload_version: String },
    MissingSupport { attributes: u32 },
    Healthy { payload_version: String },
}

pub fn socket_path_for_user(user: &str) -> String {
    common::sa_socket_path(user)
}

pub fn is_healthy(status: &ScriptingAdditionStatus) -> bool {
    matches!(status, ScriptingAdditionStatus::Healthy { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_uses_common_protocol_constant() {
        assert_eq!(socket_path_for_user("eric"), "/tmp/yabai-sa_eric.socket");
    }

    #[test]
    fn health_is_explicit() {
        assert!(is_healthy(&ScriptingAdditionStatus::Healthy {
            payload_version: common::OSAX_VERSION.to_string(),
        }));
        assert!(!is_healthy(&ScriptingAdditionStatus::NotLoaded));
    }
}
