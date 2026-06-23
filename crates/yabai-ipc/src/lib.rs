use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

pub const DAEMON_SOCKET_PATH_PREFIX: &str = "/tmp/yabai_";
pub const DAEMON_SOCKET_PATH_SUFFIX: &str = ".socket";

/// First byte the daemon prepends to a response when the command failed.
///
/// Mirrors `FAILURE_MESSAGE "\x07"` from `src/misc/macros.h`.
pub const FAILURE_MARKER: u8 = 0x07;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    PayloadTooLarge(usize),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge(size) => write!(formatter, "IPC payload too large: {size} bytes"),
        }
    }
}

impl std::error::Error for EncodeError {}

pub fn daemon_socket_path(user: &str) -> String {
    format!("{DAEMON_SOCKET_PATH_PREFIX}{user}{DAEMON_SOCKET_PATH_SUFFIX}")
}

pub fn encode_client_message<I, S>(tokens: I) -> Result<Vec<u8>, EncodeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut payload = Vec::new();
    for token in tokens {
        payload.extend_from_slice(token.as_ref().as_bytes());
        payload.push(0);
    }
    payload.push(0);

    let payload_len =
        i32::try_from(payload.len()).map_err(|_| EncodeError::PayloadTooLarge(payload.len()))?;

    let mut message = Vec::with_capacity(size_of::<i32>() + payload.len());
    message.extend_from_slice(&payload_len.to_ne_bytes());
    message.extend_from_slice(&payload);
    Ok(message)
}

pub fn decode_client_payload(payload: &[u8]) -> Option<Vec<&str>> {
    if !payload.ends_with(&[0, 0]) {
        return None;
    }

    let body = &payload[..payload.len() - 2];
    if body.is_empty() {
        return Some(Vec::new());
    }

    let mut tokens = Vec::new();
    for token in body.split(|byte| *byte == 0) {
        if token.is_empty() {
            return None;
        }

        let token = std::str::from_utf8(token).ok()?;
        tokens.push(token);
    }

    Some(tokens)
}

/// Sends one client message to the daemon listening at `socket_path` and
/// streams the response.
///
/// Returns `Ok(true)` when the command succeeded and `Ok(false)` when the
/// daemon reported a failure, mirroring the C client in `src/yabai.c`: the
/// success branch is written to `out`, and a failure response (one whose first
/// byte is [`FAILURE_MARKER`]) has that marker stripped and the remainder
/// written to `err`.
pub fn send_message<I, S>(
    socket_path: &str,
    tokens: I,
    out: &mut impl Write,
    err: &mut impl Write,
) -> io::Result<bool>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let message = encode_client_message(tokens)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    let mut stream = UnixStream::connect(socket_path)?;
    stream.write_all(&message)?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut buffer = [0u8; 8192];
    let mut first = true;
    let mut success = true;
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        let mut chunk = &buffer[..read];
        if first {
            first = false;
            if chunk[0] == FAILURE_MARKER {
                success = false;
                chunk = &chunk[1..];
            }
        }

        if success {
            out.write_all(chunk)?;
        } else {
            err.write_all(chunk)?;
        }
    }

    Ok(success)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_matches_c_format() {
        assert_eq!(daemon_socket_path("eric"), "/tmp/yabai_eric.socket");
    }

    #[test]
    fn encode_client_message_matches_c_wire_shape() {
        let message = encode_client_message(["query", "--displays"]).unwrap();
        let payload_len = i32::from_ne_bytes(message[..4].try_into().unwrap());

        assert_eq!(payload_len as usize, b"query\0--displays\0\0".len());
        assert_eq!(&message[4..], b"query\0--displays\0\0");
    }

    #[test]
    fn decode_client_payload_rejects_missing_final_nul() {
        assert_eq!(decode_client_payload(b"query\0--displays\0"), None);
    }

    #[test]
    fn decode_client_payload_round_trips_tokens() {
        let message = encode_client_message(["config", "debug_output", "on"]).unwrap();
        let tokens = decode_client_payload(&message[4..]).unwrap();

        assert_eq!(tokens, ["config", "debug_output", "on"]);
    }

    use std::os::unix::net::UnixListener;
    use std::thread;

    /// Spawns a one-shot mock daemon that decodes the framed request, hands the
    /// tokens to `respond`, and writes the returned bytes back to the client.
    fn mock_daemon<F>(respond: F) -> (String, thread::JoinHandle<Vec<String>>)
    where
        F: FnOnce(&[String]) -> Vec<u8> + Send + 'static,
    {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir();
        let path = dir
            .join(format!(
                "yabai-ipc-test-{}-{unique}.socket",
                std::process::id()
            ))
            .to_string_lossy()
            .into_owned();
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path).unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).unwrap();

            let payload = &request[size_of::<i32>()..];
            let tokens: Vec<String> = decode_client_payload(payload)
                .unwrap()
                .into_iter()
                .map(str::to_owned)
                .collect();

            let response = respond(&tokens);
            stream.write_all(&response).unwrap();
            tokens
        });

        (path, handle)
    }

    #[test]
    fn send_message_streams_success_response() {
        let (path, handle) = mock_daemon(|_tokens| b"ok".to_vec());

        let mut out = Vec::new();
        let mut err = Vec::new();
        let success = send_message(&path, ["query", "--displays"], &mut out, &mut err).unwrap();

        let tokens = handle.join().unwrap();
        assert_eq!(tokens, ["query", "--displays"]);
        assert!(success);
        assert_eq!(out, b"ok");
        assert!(err.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn send_message_routes_failure_marker_to_err() {
        let (path, handle) = mock_daemon(|_tokens| {
            let mut response = vec![FAILURE_MARKER];
            response.extend_from_slice(b"unknown domain 'bogus'\n");
            response
        });

        let mut out = Vec::new();
        let mut err = Vec::new();
        let success = send_message(&path, ["bogus"], &mut out, &mut err).unwrap();

        handle.join().unwrap();
        assert!(!success);
        assert!(out.is_empty());
        assert_eq!(err, b"unknown domain 'bogus'\n");

        let _ = std::fs::remove_file(&path);
    }
}
