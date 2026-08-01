# AGENTS.md

Guidance for agents and contributors.

## What this is

**yabai-plus** — a **Rust** tiling window manager for macOS. It started as a fork
of [koekeishiya/yabai](https://github.com/koekeishiya/yabai) (C) and was
rewritten into Rust; **the C daemon is gone**. Command grammar, IPC wire format,
and behavior track upstream yabai (contract: `docs/rust-rewrite-compat.md`).

The only non-Rust code is the OSAX injection island
(`crates/yabai-sa/osax/*.m`) — the tiny ObjC loader/payload that runs *inside*
Dock (arm64e, Dock-private classes). `crates/yabai-sa/build.rs` compiles and
embeds it into the Rust binary.

## Workspace (`crates/`)

- **yabai** — the binary: CLI + the production WM daemon (`main.rs`, split into
  `probes`/`sa_ops`/`mouse_ctl`/`service`).
- **yabai-core** — pure BSP layout tree, geometry, parser, command model.
- **yabai-runtime** — control plane: `AppState`, `Config`, query serializer,
  rules, signals.
- **yabai-macos** — the macOS boundary (AX, CoreGraphics/SkyLight, observers,
  mouse tap).
- **yabai-ipc** — client/daemon socket framing.
- **yabai-sa** — SA opcode client + `loader` (install/`--load-sa`, ported from
  `sa.m`) + the embedded OSAX island.
- **yabai-osax-common** — shared SA constants.

## Build & test

```bash
make check                # fmt-check + clippy + tests (the gate)
cargo build --release     # -> target/release/yabai   (make release)
make universal VERSION=vX # x86_64+arm64 -> bin/yabai  (release path)
```

A macOS SDK is needed (`build.rs` runs `xcrun clang` on the OSAX island). Live
testing needs Accessibility granted and, for SA features, the SA loaded — it
rearranges real windows, so use a disposable machine/VM.

## Scripting addition

`sudo yabai --load-sa` installs + injects the SA (all Rust). Needs SIP partially
disabled and, on Apple Silicon, the `-arm64e_preview_abi` boot-arg. A fresh
install writes the bundle + restarts Dock; the **next** `--load-sa` injects.
`yabai --check-sa` reports status without root. Don't hardened-runtime sign the
injected loader/payload; the OSAX island is intentionally ObjC.

## Conventions

- Conventional Commits (`fix`/`feat`/`docs`/`build`/`refactor`/…; `!` for breaks).
- Every `unsafe` block needs a `// SAFETY:` comment (clippy denies otherwise).
  Run `cargo fmt`.
- Document intentional divergences from upstream in
  `docs/rust-rewrite-compat.md`.
- Releases: push a `v<upstream>-plus.<n>` tag → `.github/workflows/release.yml`.
