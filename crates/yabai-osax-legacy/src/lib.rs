pub const LOADER_SOURCE: &str = "src/osax/loader.m";
pub const PAYLOAD_SOURCE: &str = "src/osax/payload.m";
pub const LOADER_EMBED: &str = "src/osax/loader_bin.c";
pub const PAYLOAD_EMBED: &str = "src/osax/payload_bin.c";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyOsaxArtifact {
    Loader,
    Payload,
}

impl LegacyOsaxArtifact {
    pub const fn source_path(self) -> &'static str {
        match self {
            Self::Loader => LOADER_SOURCE,
            Self::Payload => PAYLOAD_SOURCE,
        }
    }

    pub const fn embedded_c_path(self) -> &'static str {
        match self {
            Self::Loader => LOADER_EMBED,
            Self::Payload => PAYLOAD_EMBED,
        }
    }
}
