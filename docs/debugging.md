# Debugging & local development

Practical workflow for hacking on yabai-plus: building a throwaway dev binary,
getting traces out of the running window manager, and the scripting-addition
gotchas that bite every time. Paths below use `$(id -un)` for the current user.

## Fast dev loop

The makefile has a self-contained "yabai-plus local-dev" block (see the bottom
of `makefile`):

```bash
make dev          # build + Developer-ID sign + swap into /opt/homebrew/bin/yabai + restart service
make dev-restore  # put the Homebrew-managed release binary back
make sa-status    # is the scripting addition's socket present?
```

`make dev` bakes a **canary** version string in so you can tell at a glance that
the swapped-in build is running, not the Homebrew release:

```bash
yabai --version   # e.g. v7.1.25-plus.2-dirty-canary  ("-canary" = local dev build)
```

It also signs with your Developer ID (hardened runtime) so the Accessibility
grant and scripting-addition trust carry across the swap, refreshes a
sha256-pinned passwordless `--load-sa` sudoers rule, and prints `--check-sa` at
the end. The service label is `com.asmvik.yabai`; `--restart-service` falls back
to `--start-service` when it isn't registered yet.

## Getting verbose traces

`debug(...)` (`src/misc/log.h`) writes to **stdout** but only when `g_verbose`
is set. There are two ways to turn it on:

**Running as the launchd service (easiest).** Toggle verbosity at runtime and
tail the service's stdout log:

```bash
yabai -m config debug_output on
tail -f /tmp/yabai_$(id -un).out.log     # debug() + normal stdout land here
yabai -m config debug_output off         # when done
```

Log file locations (from the launchd plist, `src/misc/service.h`):

| Path | Contents |
|---|---|
| `/tmp/yabai_$(id -un).out.log` | stdout — `debug()` / `debug_message()` |
| `/tmp/yabai_$(id -un).err.log` | stderr — `warn()` / `error()` |

**Running in the foreground.** Stop the service and run the binary directly with
`--verbose`. Capture it through a pty so output is line-flushed — a plain
`yabai --verbose > file` block-buffers and drops the tail when you kill it:

```bash
yabai --stop-service
script -t 0 /tmp/yabai-trace.log /opt/homebrew/bin/yabai --verbose
# ...reproduce the bug, then Ctrl-C...
yabai --start-service
```

## Adding temporary trace points

Drop `debug("%s: ...\n", __FUNCTION__, ...);` wherever you need it; it compiles
to nothing hot (early-returns on `!g_verbose`) and prints once verbosity is on.
**Strip these `debug()` calls before committing** — keep the fix logic, remove
only the tracing. Grep your diff for `debug(` before you commit.

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
| `window --space` (move window to another space) | **No** — works without it |
| `space --create` / `--destroy` / `--move` | **Yes** — fails with "error with the scripting-addition" |
| smooth `scripting_addition_move_window` during alt-drag | **Yes** — without it, drags fall back to the blocking AX path (the mid-drag freeze) |

So the cleanest "is the SA really loaded?" test is `yabai -m space --create`
(then destroy the result). Space focus / window-to-space succeeding proves
nothing.

### Known injection failure

On this machine `sudo yabai --load-sa` currently fails at the arm64e thread
spawn:

```
could not spawn remote thread: (os/kern) protection failure
```

What has been ruled out: `-arm64e_preview_abi` **is** live in the running kernel
(`sysctl kern.bootargs` confirms it, not just `nvram boot-args`), and a reboot
does **not** fix it — so the handoff-era "reboot will fix it" theory is wrong.
SIP is in the expected custom config (Filesystem / Debugging / NVRAM
protections disabled). Release CI signs identically to `make dev`, so signing is
not the differentiator. This looks like a deeper macOS-15 injection-path issue
in the upstream loader (`src/osax/loader.m`, arm64e path ~lines 220–240), not
something introduced by this fork's patches. Day-to-day tiling, focus, and
window-to-space all work without it; the cost is the occasional drag freeze and
no `space --create`.

## Mission Control / multi-display debugging

Cross-display space and window bugs surface around Mission Control exit and
space/display change events. When tracing one of these, grep the log for:

```
MISSION_CONTROL_EXIT|SPACE_CHANGED|DISPLAY_CHANGED|SLS_SPACE_(CREATED|DESTROYED)
```

Key facts proven by tracing (don't relearn them the hard way):

- A space dragged between displays **keeps its managed space id** — only the
  space→display association flips. `space_display_id(sid)` reflects the new
  display correctly by MC-exit.
- A pure display change does **not** dirty a view's layout, so a view's cached
  frame (`view->root->area`) is not recomputed unless something explicitly
  invalidates the view. That gap was the cross-display "teleport" bug.

Landmarks:

- `window_manager_validate_and_check_for_windows_on_space` and
  `window_manager_correct_for_mission_control_changes` — `src/window_manager.c`
- `view_update` (sets VALID + DIRTY, frame from `space_display_id`) — `src/view.c`
- `space_display_id` (live `SLSCopyManagedDisplayForSpace`) — `src/space.c`
- SA load/inject — `src/sa.m`, `src/osax/loader.m`

## Useful runtime files

| Path | What |
|---|---|
| `/tmp/yabai_$(id -un).socket` | message socket (`yabai -m ...`) |
| `/tmp/yabai_$(id -un).lock` | single-instance lock |
| `/tmp/yabai-sa_$(id -un).socket` | SA payload socket (present only when injected) |
| `/tmp/yabai_$(id -un).out.log` / `.err.log` | service stdout / stderr |
</content>
</invoke>
