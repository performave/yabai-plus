pub const SA_SOCKET_PATH_PREFIX: &str = "/tmp/yabai-sa_";
pub const SA_SOCKET_PATH_SUFFIX: &str = ".socket";
pub const SA_SOCKET_BUFF_LEN: usize = 0x1000;

pub const OSAX_VERSION: &str = "2.1.30";

pub const OSAX_ATTRIB_DOCK_SPACES: u32 = 0x01;
pub const OSAX_ATTRIB_DPPM: u32 = 0x02;
pub const OSAX_ATTRIB_ADD_SPACE: u32 = 0x04;
pub const OSAX_ATTRIB_REM_SPACE: u32 = 0x08;
pub const OSAX_ATTRIB_MOV_SPACE: u32 = 0x10;
pub const OSAX_ATTRIB_SET_WINDOW: u32 = 0x20;
pub const OSAX_ATTRIB_ANIM_TIME: u32 = 0x40;
pub const OSAX_ATTRIB_ALL: u32 = OSAX_ATTRIB_DOCK_SPACES
    | OSAX_ATTRIB_DPPM
    | OSAX_ATTRIB_ADD_SPACE
    | OSAX_ATTRIB_REM_SPACE
    | OSAX_ATTRIB_MOV_SPACE
    | OSAX_ATTRIB_SET_WINDOW
    | OSAX_ATTRIB_ANIM_TIME;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SaOpcode {
    Handshake = 0x01,
    SpaceFocus = 0x02,
    SpaceCreate = 0x03,
    SpaceDestroy = 0x04,
    SpaceMove = 0x05,
    WindowMove = 0x06,
    WindowOpacity = 0x07,
    WindowOpacityFade = 0x08,
    WindowLayer = 0x09,
    WindowSticky = 0x0A,
    WindowShadow = 0x0B,
    WindowFocus = 0x0C,
    WindowScale = 0x0D,
    WindowSwapProxyIn = 0x0E,
    WindowSwapProxyOut = 0x0F,
    WindowOrder = 0x10,
    WindowOrderIn = 0x11,
    WindowListToSpace = 0x12,
    WindowToSpace = 0x13,
}

impl TryFrom<u8> for SaOpcode {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        let opcode = match value {
            0x01 => Self::Handshake,
            0x02 => Self::SpaceFocus,
            0x03 => Self::SpaceCreate,
            0x04 => Self::SpaceDestroy,
            0x05 => Self::SpaceMove,
            0x06 => Self::WindowMove,
            0x07 => Self::WindowOpacity,
            0x08 => Self::WindowOpacityFade,
            0x09 => Self::WindowLayer,
            0x0A => Self::WindowSticky,
            0x0B => Self::WindowShadow,
            0x0C => Self::WindowFocus,
            0x0D => Self::WindowScale,
            0x0E => Self::WindowSwapProxyIn,
            0x0F => Self::WindowSwapProxyOut,
            0x10 => Self::WindowOrder,
            0x11 => Self::WindowOrderIn,
            0x12 => Self::WindowListToSpace,
            0x13 => Self::WindowToSpace,
            _ => return Err(()),
        };

        Ok(opcode)
    }
}

pub fn sa_socket_path(user: &str) -> String {
    format!("{SA_SOCKET_PATH_PREFIX}{user}{SA_SOCKET_PATH_SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_matches_c_format() {
        assert_eq!(sa_socket_path("eric"), "/tmp/yabai-sa_eric.socket");
    }

    #[test]
    fn all_attributes_matches_c_mask() {
        assert_eq!(OSAX_ATTRIB_ALL, 0x7f);
    }

    #[test]
    fn opcode_values_match_c_enum() {
        assert_eq!(SaOpcode::Handshake as u8, 0x01);
        assert_eq!(SaOpcode::WindowToSpace as u8, 0x13);
        assert_eq!(SaOpcode::try_from(0x13), Ok(SaOpcode::WindowToSpace));
        assert_eq!(SaOpcode::try_from(0x14), Err(()));
    }
}
