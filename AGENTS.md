# AGENTS.md

Guidance for AI agents and new contributors working in this repository.

## What this repo is

This is **yabai-plus**, a personal fork of [koekeishiya/yabai](https://github.com/koekeishiya/yabai)
(a tiling window manager for macOS). It tracks upstream and carries a small set of
patches on top, distributed as signed/notarized releases.

**Upstream does not accept pull requests.** Patches live here and are rebased onto
new upstream releases rather than contributed back.

### Remotes

- `origin` → `Performave/yabai-plus` (this fork; push here)
- `upstream` → `koekeishiya/yabai` (pull new releases/tags from here)

```bash
git fetch upstream --tags      # get new upstream work
git rebase upstream/master     # rebase the patch branch onto latest
```

## Current patches on top of upstream

- **"Don't warp mouse_follows_focus to/from ineligible windows"** (`src/event_loop.c`,
  `window_did_receive_focus`): only center the cursor when both the focused and
  previously-focused windows are eligible for management. Prevents involuntary
  cursor jumps when auxiliary windows (Picture-in-Picture panels, `AXSystemDialog`)
  steal and return focus on their own.
- **"Fix Mission Control cross-display space-drag teleport"** (`src/window_manager.c`):
  recompute a view's frame for its current display when the view is invalid,
  before the dirty gate, so a space dragged to another display repositions its
  windows onto the new display instead of leaving them behind. See
  [docs/debugging.md](./docs/debugging.md) for the root-cause notes.
- **"Add `yabai --check-sa`"** (`src/yabai.c`, `src/sa.m`, `src/sa.h`): report
  whether the scripting addition is loaded and healthy by talking to the payload
  in Dock directly (no root, no re-inject).
- **"Patch SA loader PAC ABI to match Dock on Sequoia"** (`src/sa.m`): on
  Apple Silicon, normalize the installed loader's arm64e PAC capability to the
  Dock binary before signing/injection. This preserves SA loading on Sequoia
  where Dock is PAC ABI `0x80` but newer toolchains can emit loader binaries as
  `0x81`.
- **Local-dev makefile block** (`makefile`): `make dev` / `dev-restore` /
  `sa-status` — see Building below.

When adding patches, keep each one a focused, well-described commit so it stays
easy to rebase onto upstream.

## Building

```bash
make install   # release build (-O3), universal x86_64 + arm64, into bin/yabai
make           # debug build (-O0 -g)
make man       # build the man page (requires asciidoctor)
make clean     # remove build artifacts
```

The build is a single `xcrun clang` invocation (see `makefile`); there is no
external dependency graph beyond the macOS SDK + frameworks. C standard is C11.

For the local dev loop, `make dev` builds a Developer-ID-signed **canary** binary
(marked in `yabai --version`) and swaps it into the Homebrew path; `make
dev-restore` puts the release binary back. See **[docs/debugging.md](./docs/debugging.md)**
for the full workflow — getting verbose traces, the scripting-addition gotchas
(what actually needs it, how to check it, the known injection failure), and
Mission Control / multi-display debugging notes.

## Architecture (orientation, not exhaustive)

- `src/yabai.c` — entry point, CLI/message dispatch, version macros (`MAJOR`/`MINOR`/`PATCH`).
- `src/event_loop.c` — event handling, focus/mouse behavior. (The mouse-warp patch lives here.)
- `src/window_manager.c`, `src/space_manager.c`, `src/display_manager.c` — core WM logic.
- `src/osax/` — the **scripting addition**: code injected into `Dock.app` for
  privileged window-server operations.
  - `loader.m` injects a payload into Dock via `task_for_pid` + `mach_vm_*` +
    `pthread_create_from_mach_thread`/`dlopen`.
  - `payload.m` / `arm64_payload.m` / `x64_payload.m` run inside Dock.
  - `*_bin.c` are generated (xxd) blobs of the compiled loader/payload, embedded
    into the main binary at build time. **Do not edit `*_bin.c` by hand.**

### Scripting addition: important constraints

- The SA requires the user to **partially disable SIP** and run `yabai --load-sa`
  (as root). This is a user setup step; no code/signing change removes it.
- Injection into Dock is gated by SIP + root — **not** by yabai's own code-signing
  flags. Signing yabai with the hardened runtime does not break the SA.
- On Apple Silicon, `--load-sa` also normalizes the loader's arm64e PAC ABI to
  match Dock before ad-hoc signing it. Do not replace that with Developer-ID or
  hardened-runtime signing for the injected loader/payload.
- **Only `bin/yabai` is codesigned.** The injected loader/payload must not be
  hardened-runtime signed (they run inside Dock). Never add signing of the osax
  payloads.

## Releases & CI

Releases are automated: pushing a `v*` tag runs `.github/workflows/release.yml`,
which builds → Developer ID signs → notarizes → publishes a GitHub Release.

- **How to cut a release:** [docs/releasing.md](./docs/releasing.md)
- **One-time CI/secret setup:** [docs/ci-setup.md](./docs/ci-setup.md)

Versioning: `v<upstream-version>-plus.<n>` (e.g. `v7.1.25-plus.1`). Release
builds compile the pushed tag into `yabai --version`; bump the upstream fallback
version macros in `src/yabai.c` and add a `CHANGELOG.md` entry before tagging.

## Conventions

- Match the surrounding C style (the upstream codebase's idioms, naming, and
  comment style) when patching, so diffs stay minimal and rebases stay clean.
- Keep changes scoped; prefer small commits with clear messages explaining the
  *why* (these become rebase fodder against upstream).
- Don't reformat upstream files wholesale — it makes future rebases painful.
