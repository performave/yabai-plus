//! Runtime client for the scripting addition.
//!
//! This is the Rust port of the `scripting_addition_*` request functions in
//! `src/sa.m`: it connects to the injected payload's Unix socket
//! (`/tmp/yabai-sa_<user>.socket`) and sends opcode messages that perform the
//! privileged window/space operations the Accessibility API cannot (space
//! create/destroy/move, cross-display moves, opacity/layer/sticky/shadow, etc.).
//!
//! Wire format (matching the C `sa_payload_init`/`pack`/`sa_payload_send`
//! macros): a little-endian `u16` length (the number of bytes that follow it —
//! one opcode byte plus the payload), then the opcode byte, then the payload.
//! Every scalar is packed little-endian, exactly as the C `memcpy`-based `pack`
//! does on Apple targets. The payload replies with a single dummy ack byte.
//!
//! This module is deliberately free of any macOS dependency so it can be unit
//! tested against an in-process mock socket; callers that need live SkyLight
//! state (e.g. skipping already-ordered-in windows) pre-process their inputs.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

use yabai_osax_common::{OSAX_ATTRIB_ALL, OSAX_VERSION, SA_SOCKET_BUFF_LEN, SaOpcode};

use crate::ScriptingAdditionStatus;

/// Builds an SA payload, mirroring the C `pack` macro (native little-endian).
#[derive(Debug, Default, Clone)]
pub struct Payload {
    bytes: Vec<u8>,
}

impl Payload {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u32(mut self, value: u32) -> Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn u64(mut self, value: u64) -> Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn i32(mut self, value: i32) -> Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn f32(mut self, value: f32) -> Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// Pack a C `bool` (a single byte), matching `sizeof(bool) == 1`.
    pub fn bool(mut self, value: bool) -> Self {
        self.bytes.push(u8::from(value));
        self
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Frame and send one opcode message to the SA socket, then read the single ack
/// byte the payload writes back. Mirrors `scripting_addition_send_bytes`.
pub fn send_message(socket_path: &str, opcode: SaOpcode, payload: &[u8]) -> io::Result<()> {
    // length field = opcode byte (1) + payload, as a little-endian u16.
    let frame_len = 1 + payload.len();
    if frame_len + 2 > SA_SOCKET_BUFF_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "scripting-addition payload exceeds socket buffer",
        ));
    }
    let length = u16::try_from(frame_len)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "payload too large"))?;

    let mut frame = Vec::with_capacity(frame_len + 2);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.push(opcode as u8);
    frame.extend_from_slice(payload);

    let mut stream = UnixStream::connect(socket_path)?;
    stream.write_all(&frame)?;
    // The payload replies with one dummy byte; read it best-effort, like the C
    // client's `recv(sockfd, &dummy, 1, 0)`.
    let mut ack = [0u8; 1];
    let _ = stream.read(&mut ack);
    Ok(())
}

/// Send the handshake opcode and parse the reply: a NUL-terminated version
/// string immediately followed by a little-endian `u32` attribute mask. Mirrors
/// `scripting_addition_request_handshake`.
pub fn request_handshake(socket_path: &str) -> io::Result<(String, u32)> {
    let mut stream = UnixStream::connect(socket_path)?;
    // [length=1][opcode=HANDSHAKE]
    let frame = [0x01u8, 0x00, SaOpcode::Handshake as u8];
    stream.write_all(&frame)?;

    let mut response = vec![0u8; SA_SOCKET_BUFF_LEN];
    let read = stream.read(&mut response)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "no handshake response",
        ));
    }
    let response = &response[..read];

    let zero = response.iter().position(|&b| b == 0).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "handshake response missing version terminator",
        )
    })?;
    let version = String::from_utf8_lossy(&response[..zero]).into_owned();
    let attrib = response
        .get(zero + 1..zero + 1 + 4)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "handshake response missing attribute mask",
            )
        })?
        .try_into()
        .map(u32::from_le_bytes)
        .expect("slice length checked above");

    Ok((version, attrib))
}

/// Probe the SA at `socket_path` and classify it, mirroring the checks in
/// `scripting_addition_status`.
pub fn status(socket_path: &str) -> ScriptingAdditionStatus {
    match request_handshake(socket_path) {
        Err(_) => ScriptingAdditionStatus::NotLoaded,
        Ok((version, attributes)) => {
            if version != OSAX_VERSION {
                ScriptingAdditionStatus::Outdated {
                    payload_version: version,
                }
            } else if attributes & OSAX_ATTRIB_ALL != OSAX_ATTRIB_ALL {
                ScriptingAdditionStatus::MissingSupport { attributes }
            } else {
                ScriptingAdditionStatus::Healthy {
                    payload_version: version,
                }
            }
        }
    }
}

/// A connected (by path) scripting-addition client. Each call opens a short-lived
/// connection, exactly like the C daemon (which reconnects per request).
#[derive(Debug, Clone)]
pub struct ScriptingAddition {
    socket_path: String,
}

impl ScriptingAddition {
    pub fn new(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// Build a client for the given user's SA socket.
    pub fn for_user(user: &str) -> Self {
        Self::new(yabai_osax_common::sa_socket_path(user))
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    fn send(&self, opcode: SaOpcode, payload: Payload) -> io::Result<()> {
        send_message(&self.socket_path, opcode, payload.as_bytes())
    }

    pub fn status(&self) -> ScriptingAdditionStatus {
        status(&self.socket_path)
    }

    // --- spaces -----------------------------------------------------------

    pub fn focus_space(&self, sid: u64) -> io::Result<()> {
        self.send(SaOpcode::SpaceFocus, Payload::new().u64(sid))
    }

    pub fn create_space(&self, sid: u64) -> io::Result<()> {
        self.send(SaOpcode::SpaceCreate, Payload::new().u64(sid))
    }

    pub fn destroy_space(&self, sid: u64) -> io::Result<()> {
        self.send(SaOpcode::SpaceDestroy, Payload::new().u64(sid))
    }

    /// Move `src_sid` onto another display, mirroring
    /// `scripting_addition_move_space_to_display`.
    pub fn move_space_to_display(
        &self,
        src_sid: u64,
        dst_sid: u64,
        src_prev_sid: u64,
        focus: bool,
    ) -> io::Result<()> {
        self.send(
            SaOpcode::SpaceMove,
            Payload::new()
                .u64(src_sid)
                .u64(dst_sid)
                .u64(src_prev_sid)
                .bool(focus),
        )
    }

    /// Reorder `src_sid` after `dst_sid`, mirroring
    /// `scripting_addition_move_space_after_space` (the C packs a zero
    /// `prev_sid` placeholder in this case).
    pub fn move_space_after_space(
        &self,
        src_sid: u64,
        dst_sid: u64,
        focus: bool,
    ) -> io::Result<()> {
        self.send(
            SaOpcode::SpaceMove,
            Payload::new().u64(src_sid).u64(dst_sid).u64(0).bool(focus),
        )
    }

    // --- windows ----------------------------------------------------------

    pub fn move_window(&self, wid: u32, x: i32, y: i32) -> io::Result<()> {
        self.send(SaOpcode::WindowMove, Payload::new().u32(wid).i32(x).i32(y))
    }

    /// Set window opacity; uses the fade opcode when `duration > 0`, matching the
    /// C selector.
    pub fn set_opacity(&self, wid: u32, opacity: f32, duration: f32) -> io::Result<()> {
        let opcode = if duration > 0.0 {
            SaOpcode::WindowOpacityFade
        } else {
            SaOpcode::WindowOpacity
        };
        self.send(opcode, Payload::new().u32(wid).f32(opacity).f32(duration))
    }

    pub fn set_layer(&self, wid: u32, layer: i32) -> io::Result<()> {
        self.send(SaOpcode::WindowLayer, Payload::new().u32(wid).i32(layer))
    }

    pub fn set_sticky(&self, wid: u32, sticky: bool) -> io::Result<()> {
        self.send(SaOpcode::WindowSticky, Payload::new().u32(wid).bool(sticky))
    }

    pub fn set_shadow(&self, wid: u32, shadow: bool) -> io::Result<()> {
        self.send(SaOpcode::WindowShadow, Payload::new().u32(wid).bool(shadow))
    }

    pub fn focus_window(&self, wid: u32) -> io::Result<()> {
        self.send(SaOpcode::WindowFocus, Payload::new().u32(wid))
    }

    pub fn scale_window(&self, wid: u32, x: f32, y: f32, w: f32, h: f32) -> io::Result<()> {
        self.send(
            SaOpcode::WindowScale,
            Payload::new().u32(wid).f32(x).f32(y).f32(w).f32(h),
        )
    }

    /// Order window `a` relative to `b` (`order`: +1 above, -1 below, 0 out).
    pub fn order_window(&self, a_wid: u32, order: i32, b_wid: u32) -> io::Result<()> {
        self.send(
            SaOpcode::WindowOrder,
            Payload::new().u32(a_wid).i32(order).u32(b_wid),
        )
    }

    /// Order a list of windows in. The caller is responsible for substituting a
    /// `0` window id for any already-ordered-in window (the C path consults
    /// `SLSWindowIsOrderedIn`, which this macOS-free crate cannot).
    pub fn order_window_in(&self, window_list: &[u32]) -> io::Result<()> {
        let mut payload = Payload::new().i32(window_list.len() as i32);
        for &wid in window_list {
            payload = payload.u32(wid);
        }
        self.send(SaOpcode::WindowOrderIn, payload)
    }

    pub fn move_window_to_space(&self, sid: u64, wid: u32) -> io::Result<()> {
        self.send(SaOpcode::WindowToSpace, Payload::new().u64(sid).u32(wid))
    }

    pub fn move_window_list_to_space(&self, sid: u64, window_list: &[u32]) -> io::Result<()> {
        let mut payload = Payload::new().u64(sid).i32(window_list.len() as i32);
        for &wid in window_list {
            payload = payload.u32(wid);
        }
        self.send(SaOpcode::WindowListToSpace, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::thread;

    fn temp_socket(name: &str) -> String {
        let mut path = std::env::temp_dir();
        let unique = format!(
            "yabai-sa-test-{name}-{}-{:?}.socket",
            std::process::id(),
            thread::current().id()
        );
        path.push(unique);
        let path = path.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&path);
        path
    }

    /// Accept one connection, capture the received frame, and reply with `ack`.
    fn serve_once(path: &str, ack: Vec<u8>) -> mpsc::Receiver<Vec<u8>> {
        let listener = UnixListener::bind(path).expect("bind mock SA socket");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = vec![0u8; 256];
                let n = stream.read(&mut buf).unwrap_or(0);
                buf.truncate(n);
                let _ = stream.write_all(&ack);
                let _ = tx.send(buf);
            }
        });
        rx
    }

    #[test]
    fn send_message_frames_length_opcode_and_payload() {
        let path = temp_socket("frame");
        let rx = serve_once(&path, vec![0x00]);

        // focus_space packs a u64 sid.
        ScriptingAddition::new(path.clone())
            .focus_space(0x0102_0304_0506_0708)
            .expect("send");
        let frame = rx.recv().expect("frame");
        let _ = std::fs::remove_file(&path);

        // [len u16 LE = 1 + 8][opcode SpaceFocus][sid u64 LE]
        assert_eq!(&frame[0..2], &9u16.to_le_bytes());
        assert_eq!(frame[2], SaOpcode::SpaceFocus as u8);
        assert_eq!(&frame[3..11], &0x0102_0304_0506_0708u64.to_le_bytes());
        assert_eq!(frame.len(), 11);
    }

    #[test]
    fn move_window_packs_signed_coordinates() {
        let path = temp_socket("move");
        let rx = serve_once(&path, vec![0x00]);

        ScriptingAddition::new(path.clone())
            .move_window(42, -5, 300)
            .expect("send");
        let frame = rx.recv().expect("frame");
        let _ = std::fs::remove_file(&path);

        assert_eq!(frame[2], SaOpcode::WindowMove as u8);
        assert_eq!(&frame[3..7], &42u32.to_le_bytes());
        assert_eq!(&frame[7..11], &(-5i32).to_le_bytes());
        assert_eq!(&frame[11..15], &300i32.to_le_bytes());
    }

    #[test]
    fn set_opacity_selects_fade_opcode_by_duration() {
        let path = temp_socket("opacity");
        let rx = serve_once(&path, vec![0x00]);
        ScriptingAddition::new(path.clone())
            .set_opacity(7, 0.5, 0.0)
            .unwrap();
        assert_eq!(rx.recv().unwrap()[2], SaOpcode::WindowOpacity as u8);
        let _ = std::fs::remove_file(&path);

        let path = temp_socket("opacity-fade");
        let rx = serve_once(&path, vec![0x00]);
        ScriptingAddition::new(path.clone())
            .set_opacity(7, 0.5, 0.25)
            .unwrap();
        assert_eq!(rx.recv().unwrap()[2], SaOpcode::WindowOpacityFade as u8);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn handshake_parses_version_and_attrib() {
        let path = temp_socket("handshake");
        // Reply: "2.1.30\0" + attrib u32 LE.
        let mut ack = Vec::new();
        ack.extend_from_slice(OSAX_VERSION.as_bytes());
        ack.push(0);
        ack.extend_from_slice(&OSAX_ATTRIB_ALL.to_le_bytes());
        let rx = serve_once(&path, ack);

        let (version, attrib) = request_handshake(&path).expect("handshake");
        let frame = rx.recv().expect("frame");
        let _ = std::fs::remove_file(&path);

        assert_eq!(version, OSAX_VERSION);
        assert_eq!(attrib, OSAX_ATTRIB_ALL);
        // The handshake request frame is [0x01, 0x00, HANDSHAKE].
        assert_eq!(frame, vec![0x01, 0x00, SaOpcode::Handshake as u8]);
    }

    #[test]
    fn status_classifies_handshake_results() {
        // Healthy.
        let path = temp_socket("status-ok");
        let mut ack = Vec::new();
        ack.extend_from_slice(OSAX_VERSION.as_bytes());
        ack.push(0);
        ack.extend_from_slice(&OSAX_ATTRIB_ALL.to_le_bytes());
        serve_once(&path, ack);
        assert_eq!(
            status(&path),
            ScriptingAdditionStatus::Healthy {
                payload_version: OSAX_VERSION.to_string()
            }
        );
        let _ = std::fs::remove_file(&path);

        // Outdated version.
        let path = temp_socket("status-old");
        let mut ack = Vec::new();
        ack.extend_from_slice(b"0.0.1");
        ack.push(0);
        ack.extend_from_slice(&OSAX_ATTRIB_ALL.to_le_bytes());
        serve_once(&path, ack);
        assert_eq!(
            status(&path),
            ScriptingAdditionStatus::Outdated {
                payload_version: "0.0.1".to_string()
            }
        );
        let _ = std::fs::remove_file(&path);

        // Missing macOS support (incomplete attribute mask).
        let path = temp_socket("status-attr");
        let mut ack = Vec::new();
        ack.extend_from_slice(OSAX_VERSION.as_bytes());
        ack.push(0);
        ack.extend_from_slice(&1u32.to_le_bytes());
        serve_once(&path, ack);
        assert_eq!(
            status(&path),
            ScriptingAdditionStatus::MissingSupport { attributes: 1 }
        );
        let _ = std::fs::remove_file(&path);

        // Not loaded (no socket).
        assert_eq!(
            status("/tmp/yabai-sa-test-does-not-exist.socket"),
            ScriptingAdditionStatus::NotLoaded
        );
    }
}
