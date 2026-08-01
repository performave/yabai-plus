//! Install / load / uninstall the scripting addition, and manage the passwordless
//! `--load-sa` sudoers rule. This is the Rust port of the C `src/sa.m`
//! orchestration: it installs the OSAX bundle (writing the build-embedded
//! [`crate::LOADER_BINARY`]/[`crate::PAYLOAD_BINARY`]), normalizes the loader's
//! arm64e PAC ABI to match Dock, code-signs it, then runs the loader — which does
//! the actual `task_for_pid` injection inside the compiled ObjC island.
#![cfg(target_os = "macos")]

use std::ffi::{CString, c_char, c_void};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use yabai_osax_common::{OSAX_ATTRIB_ALL, OSAX_VERSION, sa_socket_path};

use crate::{LOADER_BINARY, PAYLOAD_BINARY, request_handshake};

const OSAX_BASE: &str = "/Library/ScriptingAdditions/yabai.osax";
const DOCK_MACHO: &str = "/System/Library/CoreServices/Dock.app/Contents/MacOS/Dock";
const SUDOERS_PATH: &str = "/private/etc/sudoers.d/yabai";
const SUDOERS_TMP_PATH: &str = "/private/etc/sudoers.d/yabai.tmp";

const CSR_ALLOW_UNRESTRICTED_FS: u32 = 0x02;
const CSR_ALLOW_TASK_FOR_PID: u32 = 0x04;

unsafe extern "C" {
    fn getuid() -> u32;
    fn geteuid() -> u32;
    /// Private libSystem SIP query (C `csr_get_active_config`).
    fn csr_get_active_config(config: *mut u32) -> i32;
    fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *const c_void,
        newlen: usize,
    ) -> i32;
}

/// The filesystem layout of the installed `yabai.osax` bundle (C
/// `scripting_addition_set_path`).
struct Paths {
    info_plist: PathBuf,
    macos: PathBuf,
    resources: PathBuf,
    payload_macos: PathBuf,
    payload_plist: PathBuf,
    bin_loader: PathBuf,
    bin_payload: PathBuf,
}

fn paths() -> Paths {
    let contents = PathBuf::from(OSAX_BASE).join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    let payload_contents = resources.join("payload.bundle").join("Contents");
    let payload_macos = payload_contents.join("MacOS");
    Paths {
        info_plist: contents.join("Info.plist"),
        payload_plist: payload_contents.join("Info.plist"),
        bin_loader: macos.join("loader"),
        bin_payload: payload_macos.join("payload"),
        macos,
        resources,
        payload_macos,
    }
}

fn sa_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
<key>CFBundleDevelopmentRegion</key>
<string>en</string>
<key>CFBundleExecutable</key>
<string>loader</string>
<key>CFBundleIdentifier</key>
<string>com.asmvik.yabai-osax</string>
<key>CFBundleInfoDictionaryVersion</key>
<string>6.0</string>
<key>CFBundleName</key>
<string>yabai</string>
<key>CFBundlePackageType</key>
<string>osax</string>
<key>CFBundleShortVersionString</key>
<string>{OSAX_VERSION}</string>
<key>CFBundleVersion</key>
<string>{OSAX_VERSION}</string>
<key>NSHumanReadableCopyright</key>
<string>Copyright © 2019 Åsmund Vikane. All rights reserved.</string>
<key>OSAXHandlers</key>
<dict>
</dict>
</dict>
</plist>"#
    )
}

fn sa_bundle_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
<key>CFBundleDevelopmentRegion</key>
<string>en</string>
<key>CFBundleExecutable</key>
<string>payload</string>
<key>CFBundleIdentifier</key>
<string>com.asmvik.yabai-sa</string>
<key>CFBundleInfoDictionaryVersion</key>
<string>6.0</string>
<key>CFBundleName</key>
<string>payload</string>
<key>CFBundlePackageType</key>
<string>BNDL</string>
<key>CFBundleShortVersionString</key>
<string>{OSAX_VERSION}</string>
<key>CFBundleVersion</key>
<string>{OSAX_VERSION}</string>
<key>NSHumanReadableCopyright</key>
<string>Copyright © 2019 Åsmund Vikane. All rights reserved.</string>
<key>NSPrincipalClass</key>
<string></string>
</dict>
</plist>"#
    )
}

fn is_root() -> bool {
    // SAFETY: `getuid`/`geteuid` take no arguments and cannot fail.
    unsafe { getuid() == 0 || geteuid() == 0 }
}

/// C `scripting_addition_is_sip_friendly`: filesystem protections and
/// task-for-pid debugging restrictions must be disabled.
fn is_sip_friendly() -> bool {
    let mut config: u32 = 0;
    // SAFETY: `config` is a valid out pointer for the private SIP query.
    unsafe {
        csr_get_active_config(&mut config);
    }
    (config & CSR_ALLOW_UNRESTRICTED_FS) != 0 && (config & CSR_ALLOW_TASK_FOR_PID) != 0
}

/// Whether the `-arm64e_preview_abi` boot-arg is set (needed to inject arm64e
/// code into Dock). C `scripting_addition_is_arm64e_enabled`.
fn is_arm64e_enabled() -> bool {
    let name = CString::new("kern.bootargs").unwrap();
    let mut buffer = [0u8; 2048];
    let mut len = buffer.len();
    // SAFETY: standard sysctlbyname read into a stack buffer with its length.
    let err = unsafe {
        sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null(),
            0,
        )
    };
    if err != 0 {
        return false;
    }
    let bootargs = String::from_utf8_lossy(&buffer[..len.min(buffer.len())]);
    bootargs.contains("-arm64e_preview_abi")
}

fn run(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Restart Dock so it re-loads the (re)installed scripting addition.
fn restart_dock() {
    let _ = run("/usr/bin/killall", &["Dock"]);
}

fn remove() -> bool {
    fs::remove_dir_all(OSAX_BASE)
        .map(|()| true)
        .unwrap_or_else(|e| e.kind() == std::io::ErrorKind::NotFound)
}

fn is_installed() -> bool {
    PathBuf::from(OSAX_BASE).is_dir()
}

/// Whether the installed payload bundle's `CFBundleVersion` matches this build
/// (C `scripting_addition_check`). Reads the plist we wrote at install time.
fn installed_version_current() -> bool {
    let plist = paths().payload_plist;
    let Ok(text) = fs::read_to_string(&plist) else {
        return false;
    };
    plist_string_value(&text, "CFBundleVersion").as_deref() == Some(OSAX_VERSION)
}

/// Minimal `<key>NAME</key><string>VALUE</string>` extractor for our own plists.
fn plist_string_value(plist: &str, key: &str) -> Option<String> {
    let key_tag = format!("<key>{key}</key>");
    let after_key = &plist[plist.find(&key_tag)? + key_tag.len()..];
    let start = after_key.find("<string>")? + "<string>".len();
    let end = after_key[start..].find("</string>")?;
    Some(after_key[start..start + end].to_string())
}

fn set_executable(path: &std::path::Path) -> std::io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

// ---------------------------------------------------------------------------
// arm64e PAC ABI patch (C macho_* / scripting_addition_patch_loader_pac_abi):
// normalize the loader Mach-O's arm64e capability byte(s) to match Dock, so the
// injected thread's PAC ABI is accepted on Sequoia.
// ---------------------------------------------------------------------------

const MACHO_CPU_TYPE_ARM64: u32 = 16_777_228;
const MACHO_CPU_SUBTYPE_ARM64E: u32 = 2;
const MACHO_CPU_SUBTYPE_MASK: u32 = 0x00FF_FFFF;
const MACHO_FAT_MAGIC: u32 = 0xCAFE_BABE;
const MACHO_MH_MAGIC_64: u32 = 0xFEED_FACF;

fn u32_be(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_be_bytes(s.try_into().unwrap()))
}
fn u32_le(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

/// Locate the arm64e capability byte(s) in a Mach-O: returns
/// `(caps, fat_caps_offset, mach_caps_offset)`. Handles a 32-bit fat wrapper and a
/// thin arm64e slice. Mirrors C `macho_find_arm64e_caps`.
fn find_arm64e_caps(bytes: &[u8]) -> Option<(u8, Option<usize>, Option<usize>)> {
    if u32_be(bytes, 0)? == MACHO_FAT_MAGIC {
        let arch_count = u32_be(bytes, 4)?;
        for i in 0..arch_count as usize {
            let arch = 8 + i * 20;
            let cputype = u32_be(bytes, arch)?;
            let cpusubtype = u32_be(bytes, arch + 4)?;
            let slice = u32_be(bytes, arch + 8)? as usize;
            if cputype != MACHO_CPU_TYPE_ARM64 {
                continue;
            }
            if cpusubtype & MACHO_CPU_SUBTYPE_MASK != MACHO_CPU_SUBTYPE_ARM64E {
                continue;
            }
            if u32_le(bytes, slice)? != MACHO_MH_MAGIC_64 {
                return None;
            }
            if u32_le(bytes, slice + 4)? != MACHO_CPU_TYPE_ARM64 {
                return None;
            }
            if u32_le(bytes, slice + 8)? & MACHO_CPU_SUBTYPE_MASK != MACHO_CPU_SUBTYPE_ARM64E {
                return None;
            }
            // caps = high byte of the big-endian fat cpusubtype (offset arch+4);
            // the mach-header caps byte is the high byte of the LE slice
            // cpusubtype (slice+8+3).
            return Some(((cpusubtype >> 24) as u8, Some(arch + 4), Some(slice + 11)));
        }
        return None;
    }

    if u32_le(bytes, 0)? == MACHO_MH_MAGIC_64 {
        let cputype = u32_le(bytes, 4)?;
        let cpusubtype = u32_le(bytes, 8)?;
        if cputype != MACHO_CPU_TYPE_ARM64 {
            return None;
        }
        if cpusubtype & MACHO_CPU_SUBTYPE_MASK != MACHO_CPU_SUBTYPE_ARM64E {
            return None;
        }
        return Some(((cpusubtype >> 24) as u8, None, Some(11)));
    }

    None
}

/// Patch the installed loader's arm64e caps byte(s) to match Dock's, if needed.
/// Returns `Ok(true)` when a patch was written. C
/// `scripting_addition_patch_loader_pac_abi`.
fn patch_loader_pac_abi() -> std::io::Result<bool> {
    let dock = fs::read(DOCK_MACHO)?;
    let Some((dock_caps, ..)) = find_arm64e_caps(&dock) else {
        return Ok(false);
    };

    let loader_path = paths().bin_loader;
    let mut loader = fs::read(&loader_path)?;
    let Some((_caps, fat_off, mach_off)) = find_arm64e_caps(&loader) else {
        return Ok(false);
    };

    let mut needs_patch = false;
    if let Some(off) = fat_off {
        if loader.get(off).copied() != Some(dock_caps) {
            needs_patch = true;
        }
    }
    if let Some(off) = mach_off {
        if loader.get(off).copied() != Some(dock_caps) {
            needs_patch = true;
        }
    }
    if !needs_patch {
        return Ok(false);
    }

    if let Some(off) = mach_off {
        loader[off] = dock_caps;
    }
    if let Some(off) = fat_off {
        loader[off] = dock_caps;
    }
    fs::write(&loader_path, &loader)?;
    Ok(true)
}

/// chmod + arm64e PAC patch + ad-hoc codesign the installed binaries (C
/// `scripting_addition_prepare_binaries`).
fn prepare_binaries() {
    let p = paths();
    let _ = set_executable(&p.bin_loader);
    if cfg!(target_arch = "aarch64") {
        if let Err(error) = patch_loader_pac_abi() {
            eprintln!(
                "yabai: scripting-addition failed to normalize loader arm64e PAC ABI: {error}"
            );
        }
    }
    codesign(&p.bin_loader);
    let _ = set_executable(&p.bin_payload);
    codesign(&p.bin_payload);
}

fn codesign(path: &std::path::Path) {
    let _ = run(
        "/usr/bin/codesign",
        &["-f", "-s", "-", path.to_str().unwrap()],
    );
}

/// Install the OSAX bundle: write the plists and the embedded loader/payload
/// binaries, prepare (patch/sign) them, and restart Dock. C
/// `scripting_addition_install`. Returns 0 on success.
fn install() -> i32 {
    let p = paths();
    if is_installed() && !remove() {
        return 1;
    }
    // `create_dir_all` makes intermediate parents, so the leaf dirs suffice.
    for dir in [&p.macos, &p.resources, &p.payload_macos] {
        if fs::create_dir_all(dir).is_err() {
            let _ = remove();
            return 2;
        }
    }
    let writes: [(&PathBuf, Vec<u8>); 4] = [
        (&p.info_plist, sa_plist().into_bytes()),
        (&p.payload_plist, sa_bundle_plist().into_bytes()),
        (&p.bin_loader, LOADER_BINARY.to_vec()),
        (&p.bin_payload, PAYLOAD_BINARY.to_vec()),
    ];
    for (path, bytes) in writes {
        if fs::write(path, bytes).is_err() {
            let _ = remove();
            return 2;
        }
    }
    prepare_binaries();
    restart_dock();
    0
}

/// Run the installed loader executable to inject the payload into Dock (C
/// `mach_loader_inject_payload`). The actual `task_for_pid` injection lives in the
/// compiled loader binary.
fn inject() -> bool {
    run(paths().bin_loader.to_str().unwrap(), &[])
}

/// Resolve the SA socket path for the invoking (sudo) user.
fn invoking_user_socket() -> Option<String> {
    let user = std::env::var("SUDO_USER").ok().filter(|u| !u.is_empty())?;
    Some(sa_socket_path(&user))
}

/// The SA handshake against the invoking user's socket: `Some((version, attrib))`.
fn handshake() -> Option<(String, u32)> {
    request_handshake(&invoking_user_socket()?).ok()
}

fn validate() -> i32 {
    match handshake() {
        Some((version, attrib)) if version == OSAX_VERSION => {
            if attrib & OSAX_ATTRIB_ALL == OSAX_ATTRIB_ALL {
                0
            } else {
                eprintln!(
                    "yabai: scripting-addition payload (0x{attrib:X}) doesn't support this macOS version!"
                );
                1
            }
        }
        Some(_) | None => {
            // Outdated running payload but the latest is installed: a Dock restart
            // reloads it. Mirrors C `scripting_addition_perform_validation`.
            if installed_version_current() {
                restart_dock();
                0
            } else {
                1
            }
        }
    }
}

/// `yabai --load-sa`: install if needed, then inject into Dock (C
/// `scripting_addition_load`). Must run as root with SIP relaxed.
pub fn load() -> i32 {
    if !is_root() {
        eprintln!("yabai: scripting-addition must be loaded as root!");
        return 1;
    }
    if !is_sip_friendly() {
        eprintln!(
            "yabai: System Integrity Protection: Filesystem Protections and Debugging Restrictions must be disabled!"
        );
        return 1;
    }

    // Not installed / outdated on disk -> (re)install and stop (install restarts
    // Dock, which loads it).
    if !is_installed() || !installed_version_current() {
        return install();
    }

    if cfg!(target_arch = "aarch64") {
        if !is_arm64e_enabled() {
            eprintln!("yabai: missing required nvram boot-arg '-arm64e_preview_abi'!");
            return 1;
        }
        match patch_loader_pac_abi() {
            Ok(true) => codesign(&paths().bin_loader),
            Ok(false) => {}
            Err(_) => eprintln!("yabai: scripting-addition failed to check loader arm64e PAC ABI!"),
        }
    }

    if !inject() {
        eprintln!("yabai: scripting-addition failed to inject payload into Dock.app!");
        return 1;
    }
    validate()
}

/// `yabai --uninstall-sa`. Must run as root with SIP relaxed.
pub fn uninstall() -> i32 {
    if !is_sip_friendly() {
        eprintln!(
            "yabai: System Integrity Protection: Filesystem Protections and Debugging Restrictions must be disabled!"
        );
        return 1;
    }
    if !is_root() {
        eprintln!("yabai: scripting-addition must be uninstalled as root!");
        return 1;
    }
    if !is_installed() {
        return 0;
    }
    if remove() { 0 } else { -1 }
}

/// `yabai --install-sudoers`: pin a sha256 passwordless `--load-sa` rule for the
/// invoking user, validated with `visudo -c`. C `scripting_addition_install_sudoers`.
pub fn install_sudoers() -> i32 {
    if !is_root() {
        eprintln!(
            "yabai: sudoers rule must be installed as root! run 'sudo yabai --install-sudoers'"
        );
        return 1;
    }
    let Some(user) = std::env::var("SUDO_USER").ok().filter(|u| !u.is_empty()) else {
        eprintln!("yabai: cannot determine invoking user (env SUDO_USER not set)!");
        return 1;
    };
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("yabai: unable to retrieve path of executable!");
        return 1;
    };
    let Some(sha) = sha256_hex(&exe) else {
        eprintln!("yabai: unable to compute sha256 of '{}'!", exe.display());
        return 1;
    };
    let rule = format!(
        "{user} ALL=(root) NOPASSWD: sha256:{sha} {} --load-sa\n",
        exe.display()
    );
    if fs::write(SUDOERS_TMP_PATH, &rule).is_err() {
        eprintln!("yabai: failed to write '{SUDOERS_TMP_PATH}'!");
        return 1;
    }
    if fs::set_permissions(SUDOERS_TMP_PATH, fs::Permissions::from_mode(0o440)).is_err() {
        eprintln!("yabai: failed to set permissions on '{SUDOERS_TMP_PATH}'!");
        let _ = fs::remove_file(SUDOERS_TMP_PATH);
        return 1;
    }
    if !run("/usr/sbin/visudo", &["-cf", SUDOERS_TMP_PATH]) {
        eprintln!("yabai: generated sudoers rule failed 'visudo -c' validation; not installing!");
        let _ = fs::remove_file(SUDOERS_TMP_PATH);
        return 1;
    }
    if fs::rename(SUDOERS_TMP_PATH, SUDOERS_PATH).is_err() {
        eprintln!("yabai: failed to move sudoers rule into place at '{SUDOERS_PATH}'!");
        let _ = fs::remove_file(SUDOERS_TMP_PATH);
        return 1;
    }
    println!(
        "yabai: installed passwordless '--load-sa' sudoers rule at '{SUDOERS_PATH}' for user '{user}'"
    );
    0
}

/// `yabai --uninstall-sudoers`. C `scripting_addition_uninstall_sudoers`.
pub fn uninstall_sudoers() -> i32 {
    if !is_root() {
        eprintln!(
            "yabai: sudoers rule must be uninstalled as root! run 'sudo yabai --uninstall-sudoers'"
        );
        return 1;
    }
    match fs::remove_file(SUDOERS_PATH) {
        Ok(()) => {
            println!("yabai: removed sudoers rule at '{SUDOERS_PATH}'");
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("yabai: no sudoers rule installed at '{SUDOERS_PATH}'");
            0
        }
        Err(_) => {
            eprintln!("yabai: failed to remove '{SUDOERS_PATH}'!");
            1
        }
    }
}

/// sha256 of a file as lowercase hex, via `/usr/bin/shasum -a 256` (matches the C
/// CommonCrypto digest without pulling in a crypto crate).
fn sha256_hex(path: &std::path::Path) -> Option<String> {
    let out = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let hex = text.split_whitespace().next()?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hex.to_string())
}
