//! Compile the OSAX injection island (`osax/*.m`) into the loader executable and
//! the payload dylib, and drop them in `OUT_DIR` so the crate can `include_bytes!`
//! them. This replaces the old makefile `xcrun clang … | xxd -i` step: the
//! injected loader/payload must stay ObjC (they run inside Dock and use arm64e /
//! Dock-private classes), so they are a small build-time-compiled island embedded
//! into the otherwise-Rust binary.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // The OSAX island only builds on macOS; other targets get no embedded bytes.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let osax =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("osax");

    for file in [
        "loader.m",
        "payload.m",
        "arm64_payload.m",
        "x64_payload.m",
        "common.h",
        "hashtable.h",
    ] {
        println!("cargo:rerun-if-changed=osax/{file}");
    }

    let payload_src = osax.join("payload.m");
    let payload_out = out_dir.join("payload");
    // The payload is a fat (x86_64 + arm64e) dylib injected into Dock; it uses the
    // private SkyLight framework plus Foundation/Carbon.
    clang(&[
        payload_src.to_str().unwrap(),
        "-shared",
        "-fPIC",
        "-O3",
        "-mmacosx-version-min=11.0",
        "-arch",
        "x86_64",
        "-arch",
        "arm64e",
        "-o",
        payload_out.to_str().unwrap(),
        "-F/System/Library/PrivateFrameworks",
        "-framework",
        "SkyLight",
        "-framework",
        "Foundation",
        "-framework",
        "Carbon",
    ]);

    let loader_src = osax.join("loader.m");
    let loader_out = out_dir.join("loader");
    // The loader is a fat executable that task_for_pid's Dock and injects the
    // payload; run as a subprocess by `--load-sa`.
    clang(&[
        loader_src.to_str().unwrap(),
        "-O3",
        "-mmacosx-version-min=11.0",
        "-arch",
        "x86_64",
        "-arch",
        "arm64e",
        "-o",
        loader_out.to_str().unwrap(),
        "-framework",
        "Cocoa",
    ]);
}

fn clang(args: &[&str]) {
    let status = Command::new("xcrun")
        .arg("clang")
        .args(args)
        .status()
        .expect("failed to spawn `xcrun clang` for the OSAX island");
    assert!(
        status.success(),
        "OSAX compile failed: xcrun clang {args:?}"
    );
}
