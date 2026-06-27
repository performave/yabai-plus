pub use yabai_osax_common as common;

mod client;
pub use client::{Payload, ScriptingAddition, request_handshake, send_message, status};

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
