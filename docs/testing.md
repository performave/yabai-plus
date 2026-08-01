# Testing

Two automated layers, plus manual live testing for anything the macOS boundary
touches.

## Unit tests

```bash
cargo test --workspace    # (make test)
```

The unit suites live inside each crate (`crates/*/src/**` and the split-out
`<mod>/tests.rs` files). They are pure and deterministic — they must not depend on
a running window manager, Accessibility permission, a GUI session, the scripting
addition, SIP state, or Mission Control.

CI (`.github/workflows/test.yml`) runs `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets`, `cargo build --release`, and
`cargo test --workspace` on a GitHub-hosted macOS runner.

## Local e2e smoke

```bash
make e2e     # sh scripts/e2e-smoke.sh
```

Builds the release binary, starts it in the foreground with an empty temporary
config, sends real `yabai -m` messages, and verifies basic query, config, rule,
and error-path behavior. Stop the normal service first, or point the script at a
specific binary:

```bash
yabai --stop-service
YABAI_BIN=./target/release/yabai sh scripts/e2e-smoke.sh
yabai --start-service
```

It skips (rather than fails) when local preconditions aren't safe: another yabai
is running, Accessibility isn't granted, "Displays have separate Spaces" is
disabled, or `python3` is unavailable.

## Live testing

Features that touch the macOS boundary (tiling, SA ops, mouse, Mission Control,
multi-display) need a real run: grant Accessibility to the binary, load the
scripting addition (`sudo yabai --load-sa`) for SA-backed features, and verify
against the read-only `--experimental-*` probes and `yabai -m query`. This
rearranges real windows and switches spaces — use a disposable machine/VM.
