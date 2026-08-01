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
grant tracks the binary's signature, so every `cargo build` (new cdhash) revokes
it and the daemon exits with `Accessibility permission is not granted`. See
[Accessibility that survives rebuilds](#accessibility-that-survives-rebuilds) to
grant it once and stop re-granting. For a full production run,
`./target/release/yabai` with no args starts the daemon on the real socket.

## Running as a launchd service

For a persistent daemon (survives logout, restarts on crash) instead of a
foreground process, use the built-in launchd management. The plist
(`crates/yabai/src/service.rs`) points at the current binary and is bootstrapped
into the **GUI domain** (`gui/$(id -u)`) — this matters: the daemon must run in
your Aqua session or SkyLight space/window APIs (`SLSCopyWindowsWithOptionsAndTags`,
`SLSCopySpacesForWindows`) return nothing and every window resolves to `space 0`.
A plain `nohup ./target/release/yabai &` from a non-GUI shell (e.g. an SSH/agent
session) hits exactly that.

```bash
./target/release/yabai --install-service   # write ~/Library/LaunchAgents/com.asmvik.yabai.plist
./target/release/yabai --start-service      # bootstrap + start (RunAtLoad)
yabai --restart-service                     # after a rebuild: picks up the new binary
yabai --stop-service
yabai --uninstall-service
```

launchd throttles rapid restarts (~10 s); if `--restart-service` leaves it in
`spawn scheduled`, force it: `launchctl kickstart -k gui/$(id -u)/com.asmvik.yabai`.
Check state with `launchctl print gui/$(id -u)/com.asmvik.yabai | grep -E 'state|pid'`.

## Accessibility that survives rebuilds

The System-Settings grant pins the binary's **cdhash**, which changes on every
build — so you re-grant after every `cargo build`. Since the ad-hoc *identifier*
(`yabai-<hash>`) is stable across rebuilds, pin the Accessibility grant to the
identifier instead. One-time (needs SIP's filesystem protection disabled, which
you already have for the SA):

```bash
BIN="$PWD/target/release/yabai"
ID=$(codesign -dv "$BIN" 2>&1 | sed -n 's/^Identifier=//p')   # e.g. yabai-f7b659e4729e6145
printf 'identifier "%s"\n' "$ID" | csreq -r- -b /tmp/yabai_req.bin
TCC="/Library/Application Support/com.apple.TCC/TCC.db"
sudo sqlite3 "$TCC" "INSERT OR REPLACE INTO access \
  (service,client,client_type,auth_value,auth_reason,auth_version,csreq,flags,last_modified) \
  VALUES ('kTCCServiceAccessibility','$BIN',1,2,4,1,readfile('/tmp/yabai_req.bin'),0, \
          CAST(strftime('%s','now') AS INTEGER));"
sudo killall tccd            # flush the TCC cache
yabai --restart-service      # or relaunch the binary
```

`client_type=1` is a path-based entry; `auth_value=2` is *allowed*. After this,
rebuilds keep Accessibility as long as the identifier is unchanged. If the daemon
still logs `Accessibility permission is not granted`, the identifier changed
(re-run the block) or SIP is not sufficiently disabled (`csrutil status`).

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
