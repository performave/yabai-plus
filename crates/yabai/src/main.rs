use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::ExitCode;

use yabai_ipc::{FAILURE_MARKER, daemon_socket_path, decode_client_payload, send_message};
use yabai_macos::active_displays;
use yabai_runtime::{Actor, AppState, RecordingSink, Runtime};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("--version") | Some("-v") => {
            println!("yabai-rust-{}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--message") | Some("-m") => run_message(&args[1..]),
        Some("--experimental-rust-daemon") => run_experimental_daemon(&args[1..]),
        _ => {
            eprintln!("yabai-rust: daemon skeleton is not implemented yet");
            ExitCode::from(64)
        }
    }
}

fn run_message(tokens: &[String]) -> ExitCode {
    if tokens.is_empty() {
        eprintln!("yabai-rust: no arguments given to --message");
        return ExitCode::from(1);
    }

    let user = match std::env::var("USER") {
        Ok(user) if !user.is_empty() => user,
        _ => {
            eprintln!("yabai-rust: 'env USER' not set! abort..");
            return ExitCode::from(1);
        }
    };

    let socket_path = daemon_socket_path(&user);
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();

    match send_message(&socket_path, tokens, &mut out, &mut err) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            let _ = writeln!(
                err,
                "yabai-rust: failed to message daemon at {socket_path}: {error}"
            );
            ExitCode::from(1)
        }
    }
}

fn run_experimental_daemon(args: &[String]) -> ExitCode {
    let Some(socket_path) = args.first() else {
        eprintln!("yabai-rust: --experimental-rust-daemon requires a socket path");
        return ExitCode::from(64);
    };

    match bind_experimental_daemon(socket_path) {
        Ok(listener) => {
            let mut state = AppState::new();
            seed_live_displays(&mut state);
            let actor = Actor::spawn(Runtime::new(state, RecordingSink::default()));
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => serve_one(stream, &actor),
                    Err(error) => eprintln!("yabai-rust: failed to accept client: {error}"),
                }
            }
            actor.shutdown();
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("yabai-rust: failed to bind daemon socket at {socket_path}: {error}");
            ExitCode::from(1)
        }
    }
}

fn seed_live_displays(state: &mut AppState) {
    match active_displays() {
        Ok(displays) => {
            for display in displays {
                state.add_display(display.id, display.frame);
            }
        }
        Err(error) => eprintln!("yabai-rust: failed to discover displays: {error}"),
    }
}

fn bind_experimental_daemon(socket_path: &str) -> io::Result<UnixListener> {
    UnixListener::bind(socket_path)
}

fn serve_one(mut stream: UnixStream, actor: &Actor<RecordingSink>) {
    let mut header = [0u8; size_of::<i32>()];
    let response = match stream.read_exact(&mut header) {
        Ok(()) => {
            let size = i32::from_ne_bytes(header);
            if size < 0 {
                Err("negative IPC payload size".to_string())
            } else {
                let mut payload = vec![0; size as usize];
                match stream.read_exact(&mut payload) {
                    Ok(()) => match decode_client_payload(&payload) {
                        Some(tokens) => {
                            let tokens = tokens.into_iter().map(str::to_owned).collect::<Vec<_>>();
                            actor.message(tokens)
                        }
                        None => Err("invalid IPC payload".to_string()),
                    },
                    Err(error) => Err(format!("failed to read IPC payload: {error}")),
                }
            }
        }
        Err(error) => Err(format!("failed to read IPC header: {error}")),
    };

    match response {
        Ok(Some(output)) => {
            let _ = stream.write_all(output.as_bytes());
        }
        Ok(None) => {}
        Err(error) => {
            let _ = stream.write_all(&[FAILURE_MARKER]);
            let _ = writeln!(stream, "{error}");
        }
    }
}

fn print_help() {
    println!(
        "Usage: yabai-rust [option]\n\
         Options:\n\
             --message, -m <msg>    Send message to a running yabai instance.\n\
             --experimental-rust-daemon <socket>\n\
                                     Run dry-run Rust daemon on an explicit socket.\n\
             --version, -v          Print Rust skeleton version to stdout and exit.\n\
             --help, -h             Print options to stdout and exit."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use yabai_ipc::encode_client_message;

    #[test]
    fn experimental_daemon_serves_one_message() {
        let dir = std::env::temp_dir();
        let path = dir
            .join(format!(
                "yabai-rust-daemon-test-{}.socket",
                std::process::id()
            ))
            .to_string_lossy()
            .into_owned();
        let _ = std::fs::remove_file(&path);
        let listener = bind_experimental_daemon(&path).unwrap();
        let actor = Actor::spawn(Runtime::new(AppState::new(), RecordingSink::default()));

        thread::scope(|scope| {
            scope.spawn(|| {
                let (stream, _) = listener.accept().unwrap();
                serve_one(stream, &actor);
            });

            let mut client = UnixStream::connect(&path).unwrap();
            client
                .write_all(&encode_client_message(["query", "--windows", "id"]).unwrap())
                .unwrap();
            client.shutdown(std::net::Shutdown::Write).unwrap();

            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            assert_eq!(response, "[]\n");
        });

        actor.shutdown();
        let _ = std::fs::remove_file(&path);
    }
}
