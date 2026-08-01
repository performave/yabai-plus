# yabai-plus — a Rust tiling window manager for macOS.
#
# The daemon, CLI, IPC, layout engine, scripting-addition loader, and launchd
# service are all Rust (crates/). The only non-Rust piece is the OSAX injection
# island (crates/yabai-sa/osax/*.m) — the tiny loader/payload that runs inside
# Dock; it is compiled + embedded into the binary by crates/yabai-sa/build.rs.
#
# This makefile is a thin convenience wrapper around cargo.

BINARY := target/release/yabai

.PHONY: all build release install universal icon test e2e clippy fmt fmt-check check clean man sign dev

all: release

# Debug build -> target/debug/yabai
build:
	cargo build

# Optimized release build -> target/release/yabai. Codesigning + notarization for
# distribution happen in CI (.github/workflows/release.yml).
release:
	cargo build --release

install: release

# Universal (x86_64 + arm64) release binary at bin/yabai, for distribution.
# Requires both rustup targets. Pass VERSION to stamp `--version` (CI uses the tag).
bin/yabai:
	rustup target add x86_64-apple-darwin aarch64-apple-darwin
	YABAI_VERSION="$(VERSION)" cargo build --release --target x86_64-apple-darwin
	YABAI_VERSION="$(VERSION)" cargo build --release --target aarch64-apple-darwin
	mkdir -p bin
	lipo -create -output bin/yabai \
		target/x86_64-apple-darwin/release/yabai \
		target/aarch64-apple-darwin/release/yabai

universal: bin/yabai

icon: bin/yabai
	python3 scripts/seticon.py assets/icon/2x/icon-512px@2x.png bin/yabai

test:
	cargo test --workspace

# End-to-end smoke test against a foreground daemon (skips when preconditions
# aren't safe). Builds the release binary first.
e2e: release
	sh scripts/e2e-smoke.sh

clippy:
	cargo clippy --workspace --all-targets

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

# Full local gate (mirrors CI).
check: fmt-check clippy test

clean:
	cargo clean

man:
	asciidoctor -b manpage doc/yabai.asciidoc -o doc/yabai.1

# Ad-hoc sign the release binary for local use (the scripting addition does NOT
# require a signed yabai; injection is gated by SIP + root, not code-signing).
sign: release
	codesign -f -s - $(BINARY)

# Local dev loop: build a release binary and ad-hoc sign it. Run it directly
# (./target/release/yabai) or load the scripting addition with
# `sudo ./target/release/yabai --load-sa`.
dev: sign
	@echo "built + signed $(BINARY)"
