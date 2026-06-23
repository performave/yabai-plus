use std::io::{self, Write};
use std::process::ExitCode;

use yabai_ipc::{daemon_socket_path, send_message};

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

fn print_help() {
    println!(
        "Usage: yabai-rust [option]\n\
         Options:\n\
             --message, -m <msg>    Send message to a running yabai instance.\n\
             --version, -v          Print Rust skeleton version to stdout and exit.\n\
             --help, -h             Print options to stdout and exit."
    );
}
