//! launchd service management (`--install-service`/`--start-service`/…), ported
//! from the C `src/misc/service.h`. Writes `~/Library/LaunchAgents/
//! com.asmvik.yabai.plist` pointing at this executable and drives it with
//! `launchctl`.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

const LABEL: &str = "com.asmvik.yabai";
const LAUNCHCTL: &str = "/bin/launchctl";

fn plist_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(format!("Library/LaunchAgents/{LABEL}.plist")))
}

fn plist_contents() -> Option<String> {
    let user = std::env::var("USER").ok().filter(|u| !u.is_empty())?;
    let path_env = std::env::var("PATH").unwrap_or_default();
    let exe = std::env::current_exe().ok()?;
    Some(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>{path_env}</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
        <key>Crashed</key>
        <true/>
    </dict>
    <key>StandardOutPath</key>
    <string>/tmp/yabai_{user}.out.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/yabai_{user}.err.log</string>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>Nice</key>
    <integer>-20</integer>
</dict>
</plist>"#,
        exe = exe.display(),
    ))
}

fn write_plist(path: &std::path::Path) -> bool {
    let Some(contents) = plist_contents() else {
        return false;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, contents).is_ok()
}

fn launchctl(args: &[&str], quiet: bool) -> i32 {
    let mut cmd = Command::new(LAUNCHCTL);
    cmd.args(args);
    if quiet {
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    cmd.status().map(exit_code).unwrap_or(1)
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

fn uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: `getuid` takes no arguments and cannot fail.
    unsafe { getuid() }
}

fn done(ok: bool) -> ExitCode {
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

pub fn install() -> ExitCode {
    let Some(path) = plist_path() else {
        eprintln!("yabai: could not resolve LaunchAgents path (env HOME not set)!");
        return ExitCode::from(1);
    };
    if path.exists() {
        eprintln!(
            "yabai: service file '{}' is already installed!",
            path.display()
        );
        return ExitCode::from(1);
    }
    done(write_plist(&path))
}

pub fn uninstall() -> ExitCode {
    let Some(path) = plist_path() else {
        return ExitCode::from(1);
    };
    if !path.exists() {
        eprintln!("yabai: service file '{}' is not installed!", path.display());
        return ExitCode::from(1);
    }
    done(std::fs::remove_file(&path).is_ok())
}

pub fn start() -> ExitCode {
    let Some(path) = plist_path() else {
        return ExitCode::from(1);
    };
    if !path.exists() && !write_plist(&path) {
        eprintln!(
            "yabai: service file '{}' could not be installed!",
            path.display()
        );
        return ExitCode::from(1);
    }
    let service_target = format!("gui/{}/{LABEL}", uid());
    let domain_target = format!("gui/{}", uid());
    // If not bootstrapped, enable + bootstrap (RunAtLoad starts it); else kickstart.
    if launchctl(&["print", &service_target], true) != 0 {
        launchctl(&["enable", &service_target], false);
        done(
            launchctl(
                &["bootstrap", &domain_target, path.to_str().unwrap()],
                false,
            ) == 0,
        )
    } else {
        done(launchctl(&["kickstart", &service_target], false) == 0)
    }
}

pub fn restart() -> ExitCode {
    let Some(path) = plist_path() else {
        return ExitCode::from(1);
    };
    if !path.exists() {
        eprintln!("yabai: service file '{}' is not installed!", path.display());
        return ExitCode::from(1);
    }
    let service_target = format!("gui/{}/{LABEL}", uid());
    done(launchctl(&["kickstart", "-k", &service_target], false) == 0)
}

pub fn stop() -> ExitCode {
    let Some(path) = plist_path() else {
        return ExitCode::from(1);
    };
    if !path.exists() {
        eprintln!("yabai: service file '{}' is not installed!", path.display());
        return ExitCode::from(1);
    }
    let service_target = format!("gui/{}/{LABEL}", uid());
    let domain_target = format!("gui/{}", uid());
    if launchctl(&["print", &service_target], true) != 0 {
        done(launchctl(&["kill", "SIGTERM", &service_target], false) == 0)
    } else {
        launchctl(&["bootout", &domain_target, path.to_str().unwrap()], false);
        done(launchctl(&["disable", &service_target], false) == 0)
    }
}
