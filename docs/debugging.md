# Debugging & local development

Practical workflow for hacking on yabai-plus: building a throwaway dev binary,
getting traces out of the running window manager, and the scripting-addition
gotchas that bite every time. Paths below use `$(id -un)` for the current user.

## Fast dev loop

```bash
make check                 # fmt-check + clippy + tests (run before committing)
cargo build --release      # -> target/release/yabai   (make release)
codesign -f -s - target/release/yabai   # (make sign) so macOS doesn't SIGKILL a re-signed binary
./target/release/yabai --check-sa       # is the scripting addition live?
```

Run the built binary directly — there is no Homebrew swap. Grant it
Accessibility once (System Settings → Privacy & Security → Accessibility); the
grant tracks the binary's signature, so ad-hoc re-signing may require re-granting.
For a full production run, `./target/release/yabai` with no args starts the
daemon on the real socket (see below).

## Getting traces

The daemon logs to **stdout/stderr** directly (Rust `println!`/`eprintln!`) —
there is no verbosity gate; startup and event lines are always printed.

**Running in the foreground (easiest for dev).** Run the daemon directly and
watch its output; it binds `/tmp/yabai_$(id -un).socket`:

```bash
./target/release/yabai 2>&1 | tee /tmp/yabai-trace.log
# ...reproduce the bug, then Ctrl-C...
```

**Running as the launchd service.** stdout/stderr go to the log files from the
plist (`crates/yabai/src/service.rs`):

| Path | Contents |
|---|---|
| `/tmp/yabai_$(id -un).out.log` | stdout — daemon startup + event lines |
| `/tmp/yabai_$(id -un).err.log` | stderr — warnings/errors |

## Adding temporary trace points

Drop an `eprintln!("...")` wherever you need it. **Strip temporary tracing before
committing** — keep the fix logic, remove the noise. `cargo clippy` denies
`dbg!`/`todo!`, so those can't slip through.

## Scripting addition (SA)

The SA is a payload injected into `Dock.app` for privileged window-server
operations. Check whether it is actually live (talks to the payload directly; no
root, no re-inject):

```bash
yabai --check-sa
# loaded and healthy (payload v2.1.30)   -> exit 0
# NOT loaded / OUTDATED / missing support -> exit non-zero
```

### What actually needs the SA (verified on macOS 15)

Not everything that *feels* like an SA feature is one. Don't infer SA health
from your hotkeys working — verify with `--check-sa` or the table below.

| Operation | Needs SA? |
|---|---|
| `space --focus` (switch active space) | **No** — works without it |
| `window --space` (move window to another space) | **No** — works without it; this fork prefers SA when loaded |
| `space --create` / `--destroy` / `--move` | **Yes** — fails with "error with the scripting-addition" |
| smooth `scripting_addition_move_window` during alt-drag | **Yes** — without it, drags fall back to the blocking AX path (the mid-drag freeze) |

So the cleanest "is the SA really loaded?" test is `yabai -m space --create`
(then destroy the result). Space focus / window-to-space succeeding proves
nothing.

### Sequoia arm64e PAC ABI gotcha

On Sequoia 15.7.x, upstream release builds can fail at the arm64e remote-thread
spawn even when SIP and boot-args are correct:

```
could not spawn remote thread: (os/kern) protection failure
```

The root cause is a Mach-O arm64e PAC ABI capability mismatch: Sequoia's
`Dock.app` is `caps 0x80`, while newer toolchains can emit the yabai loader as
`caps 0x81`. The kernel rejects the injected thread for the mismatched target.
Check with:

```bash
otool -f /System/Library/CoreServices/Dock.app/Contents/MacOS/Dock
otool -f /Library/ScriptingAdditions/yabai.osax/Contents/MacOS/loader
```

`--load-sa` patches that automatically (Rust `yabai-sa::loader::patch_loader_pac_abi`):
before signing/injection, the installed loader's arm64e fat-header and
Mach-O-header capability bytes are rewritten to match Dock. The loader and payload
are still ad-hoc signed; do not Developer-ID/hardened-runtime sign injected OSAX
components.
If SA still fails, first verify `sysctl kern.bootargs` contains
`-arm64e_preview_abi` and `csrutil status` shows Filesystem / Debugging / NVRAM
protections disabled.

## Mission Control / multi-display debugging

Cross-display space and window bugs surface around Mission Control exit and
space/display change events.

Key facts proven by tracing (don't relearn them the hard way):

- A space dragged between displays **keeps its managed space id** — only the
  space→display association flips; `SLSCopyManagedDisplayForSpace(sid)` reflects
  the new display by MC-exit.
- AX can't enumerate windows on non-visible spaces, so a window moved to another
  space must be tracked via the SkyLight inverse mapping
  (`yabai_macos::windows_on_space`), not dropped — see
  `main::window_space_strict` / the reconcile keep-guard.

Landmarks (Rust):

- Reconcile / space tracking — `crates/yabai/src/main.rs`
  (`reconcile_pid`, `managed_space_for_window`, `refresh_live_display_state`).
- BSP layout + per-display frames — `crates/yabai-core/src/layout`,
  `crates/yabai-runtime/src/app_state` (`set_space_frame`).
- Display / space discovery — `crates/yabai-macos/src/{display,space}.rs`.
- SA load / inject / PAC patch — `crates/yabai-sa/src/loader.rs` +
  the ObjC island `crates/yabai-sa/osax/{loader,payload}.m`.

## Useful runtime files

| Path | What |
|---|---|
| `/tmp/yabai_$(id -un).socket` | message socket (`yabai -m ...`) |
| `/tmp/yabai_$(id -un).lock` | single-instance lock |
| `/tmp/yabai-sa_$(id -un).socket` | SA payload socket (present only when injected) |
| `/tmp/yabai_$(id -un).out.log` / `.err.log` | service stdout / stderr |
