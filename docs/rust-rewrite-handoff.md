# Rust rewrite handoff

This is the living handoff file for a possible full Rust rewrite of yabai-plus.
Update it at every meaningful checkpoint so another session can resume without
reconstructing context.

## Current status

- Status: Phase 0/1 done; Phase 3 client done; Phase 2 (pure core) largely done;
  Phase 4 (control plane) done; Phase 5 (macOS boundary) started. A
  compatibility contract and non-invasive Cargo workspace exist; the Rust
  `yabai -m` client talks to the live C daemon; the BSP layout tree from
  `src/view.c` and the full `yabai -m` command grammar (all 7 domains) are
  ported into pure-Rust `yabai-core`; `yabai-runtime` has the full control plane
  (`AppState` + `Config` + `Runtime` flush + single-threaded `Actor`) plus the
  first pure query serializer for windows/spaces/displays; and `yabai-macos` has
  the first real `LayoutSink` (`AxSink`) moving windows via the Accessibility API
  plus live CoreGraphics display discovery, display/space topology reconciliation,
  and AX window diagnostics. Live `window --deminimize` works for numeric,
  `first`, and `last` selectors (including C-style trailing selector syntax)
  restored from the daemon's minimized-window AX registry, and `window --close`
  is wired through the AX close button.
  `window --toggle native-fullscreen` enters/exits via the `AXFullScreen`
  attribute with a fullscreen AX registry mirroring minimize. `mouse_action1` /
  `mouse_action2` drags now move or resize windows via an active mouse event tap
  (left/right button respectively), including persistent BSP-grid resize for tiled
  windows and direct AX resize for floating windows. Tiled drag-to-move drops now
  perform same-space swap/stack center drops and edge-zone warps, plus
  cross-space/cross-display drops (the dragged window — and, for a swap, the target
  window — are reassigned in the model and relocated via the SA
  `move_window_to_space` opcode). The `signal` domain
  is modeled and executed: `signal --add/--list/--remove` plus live firing of
  `window_created`, `window_destroyed`, `window_focused`, `window_moved`,
  `window_resized`, `window_minimized`, `window_deminimized`,
  `window_title_changed`, `application_launched/terminated`, `space_changed`,
  `space_created`/`space_destroyed`, `application_activated/deactivated/hidden/visible`,
  `application_front_switched`, `display_changed`, `display_added/removed`, `system_woke`,
  `menu_bar_hidden_changed`, and `dock_did_change_pref` actions, with `app`/`title`
  regex filters honored for the metadata-carrying categories and the `active`
  (front-app) context for the hidden/terminated categories. NSWorkspace
  notifications are now actually delivered (fixed via `NSApplicationLoad` plus
  running the AppKit run loop on the daemon's main thread, with the event loop on a
  worker thread). `mouse_follows_focus` warps the cursor to the focused window on
  focus.
  The `rule` domain is modeled and executed for stored rules, list/remove/apply,
  one-shot removal, regex matching, live pure effects for `manage`
  (`manage=off` floats/untiles, `manage=on` retiles) and `scratchpad=`
  (assigns/floats), plus best-effort SA-backed `sticky`, `sub-layer`, and
  `opacity` effects, AX-backed `grid=` placement, and SA-backed `display=`/`space=`
  moves for new windows and `rule --apply`; remaining rule effects are parsed/stored
  but deferred. Per-space
  `space --gap`/`--padding` (abs/rel) are dispatched and
  survive reconciles; `window --grid` places a floating/unmanaged window on a grid.
  `window --raise`/`--lower` reorder a window's z-stacking (above/below an optional
  reference window) through the SA `order_window` opcode. `window --scratchpad`
  assigns/removes/recovers scratchpads and `window --toggle <label>` hides/shows
  them through the SA z-order opcodes. The whole `display` domain is now wired:
  `--focus` (C `display_manager_focus_display`), `--space` (C
  `display_manager_focus_space`, SA `focus_space`), and `--label`.
  205 workspace tests pass. The shipped C `make` flow is unchanged.
- Last updated: 2026-07-31.
- User decisions captured:
  - The Rust rewrite may diverge permanently from upstream yabai. Rebaseability is no
    longer a primary constraint for this track.
  - Clean up edge cases and document breaking changes instead of preserving every
    bug-for-bug behavior.
  - For the scripting addition, use the most reliable engineering path rather than
    forcing literal Rust at the cost of fragile injection behavior.

## Progress log

The detailed per-session changelog was compacted out on 2026-07-31 — it is
fully preserved in git history (`git log -- docs/rust-rewrite-handoff.md`) and
superseded by the **RESUME HERE** section below, which is the ground truth for
current state. Only recent milestones are kept here going forward.

- **2026-08-01 (session 76)** — SA loaded locally (SIP off + `-arm64e_preview_abi`,
  payload v2.1.30) enabling full live testing, which caught **3 real parity bugs
  (all fixed + verified live)**: (1) numeric space selectors resolved as raw sids
  instead of mission-control indices (`window --space 2`/`space 2 --destroy`
  no-op'd) — fixed via a daemon-pushed `AppState::mission_control_order`; (2)
  `reconcile_pid` dropped windows moved to non-visible spaces (AX can't enumerate
  them) — now reassigns via `managed_space_for_window` instead of dropping, plus
  `window --space` updates the model immediately (`finish_window_to_space`); (3)
  moving the focused window followed the space to it — now re-focus a source-space
  window first (`keep_source_space_focused`, C `send_window_to_space`). Live-verified
  SA window ops (opacity/sub-layer/sticky/raise/lower), space create/focus, and
  cross-space moves. Lesson: unit tests passed on all three; only live testing
  caught them — keep testing live (SA stays loaded). Follow-up live pass found
  no new bugs across `space --rotate/--mirror/--balance`, `window --warp/--stack`,
  sticky (shows on all spaces), scratchpad (assign/hide/show/recover), and `rule
  manage=off` on new windows — all confirmed correct. Third pass: fixed a
  phantom-window bug (the keep-guard's `spaces_for_window` fallback retained
  destroyed/transient windows during native-fullscreen transitions — now uses
  strict `window_space_strict`); live-verified `--toggle zoom-fullscreen/
  zoom-parent/pip/native-fullscreen/windowed-fullscreen(float)` and
  `focus_follows_mouse autofocus`. Also verified `window --insert <dir>` and
  `--toggle expose`. `--toggle windowed-fullscreen` on a *managed* window
  re-tiles back — NOT a real parity bug: C's `WINDOW_WINDOWED` flag is never
  checked in the tiling path either, so C also re-tiles on the next reflow (the
  daemon just reflows more eagerly; fully works when the window is floating).
  Essentially all window/space/config features are now live-verified; the one
  remaining functional gap is `query --windows` field completeness.
- **2026-07-31 (session 75)** — installed the Rust toolchain locally (rustup
  stable) and drove the port to functional completeness (details in git log):
  - **All 7 command domains fully handled**: added `display --focus`/`--space`
    (daemon `try_display`), `space --toggle padding/gap/mission-control/
    show-desktop`, and completed the `config` domain (last 6 keys parse/store/
    round-trip with C-faithful print formats).
  - **Config effects enacted**: `display_arrangement_order` (center-x/y display
    ordering), `external_bar` (screen-space reservation per all/main/off, tracks
    `CGMainDisplayID`, immediate re-inset on config change), `window_origin_
    display` (new-window routing to physical/focused/cursor space). Still inert:
    the cosmetic/animation-only keys (`window_animation_easing`,
    `insert_feedback_color`, `skip_window_focus_animation`) — need animation/
    overlay infra, low value.
  - **Signal domain complete**: `mission_control_enter`/`exit` via a Dock
    `AXExpose*` observer (`observe_mission_control` + live-verified `dock_pid`;
    firing is GUI-only, unverified).
  - **`query --windows`**: added pure `is-floating`/`is-sticky`; the remaining C
    fields + off-tree windows need daemon live-read augmentation (documented in
    the compat breaking-changes ledger) — the main remaining functional gap.
  - **Organization**: `app_state`/`command`/`layout` are now `mod.rs`+`tests.rs`
    directory modules; the `yabai` binary is split into `probes`/`sa_ops`/
    `mouse_ctl` child modules of `main` (via `use super::*` ↔ `use <mod>::*`;
    only parent-called items need `pub(crate)`). main.rs 4703 → 3493 lines.
  - **Live-verified locally** (macOS 15.6.1 arm64, SA v2.1.30 loaded on this box
    now — SIP disabled + `-arm64e_preview_abi`): tiling, `--grid`/`--move`/
    `--swap`/`--ratio`/`--toggle split`, `space --toggle padding/gap`,
    `external_bar` (window y +40 on `all:40:0`), the new `config` keys,
    `display --focus`/`--space` error paths, and the SA ops
    `--opacity`/`--sub-layer`/`--toggle sticky`/`--raise`/`--lower`,
    `space --create`/`--focus`.
  - **BUG found live + FIXED:** numeric space selectors resolved as raw sids, so
    `window --space <n>` / `space <n> --destroy` targeted sid n instead of the
    nth mission-control space (silent no-op; `space --focus` was unaffected as it
    uses the live SkyLight order). Fixed with a daemon-pushed
    `AppState::mission_control_order` (seeded at startup + refreshed on topology
    change); numeric selectors now map `order[n-1]` like C. Verified live:
    `window --space 2` moves onto the created space, `space 2 --destroy`
    destroys it.
  205 workspace tests, clippy clean.
- Sessions 40–74 (2026-07-03/04) — window `--grid`/`--move`/`--resize`/`--ratio`/
  `--raise`/`--lower`/`--insert`/`--scratchpad`, all `--toggle` variants
  (split/windowed-fullscreen/pip/expose/native-fullscreen), per-space
  `--gap`/`--padding`, mouse drag move/resize/drop (incl. cross-space), signals,
  and SA-backed `rule` effects (manage/scratchpad/sticky/sub-layer/opacity/grid/
  display/space). See git history for the blow-by-blow.

## Recommendation on "entirely Rust"

Do not force the injected Dock loader/payload into Rust in the first rewrite pass.
The daemon, CLI, IPC, config/rule parsing, layout engine, event runtime, and SA
install/check/load manager should be Rust. The injected OSAX loader/payload should
stay as a small isolated ObjC/C/assembly island until a dedicated feasibility spike
proves Rust can reliably produce the required artifacts.

Reasoning:

- The injected code depends on `arm64e`, PAC behavior, remote thread creation,
  handwritten shellcode, Dock-private classes, and per-macOS pattern scanning.
- Rust improves safety in the daemon and layout/control-plane code, but adds little
  value to the tiny architecture/ABI-sensitive injection stub.
- A failed or subtly wrong Rust payload would break the privileged operations that
  are hardest to diagnose locally.

Target state for the first production Rust release:

- Rust daemon and Rust command/client path.
- Rust ownership boundaries around AX, CoreFoundation, Cocoa, Carbon, CoreGraphics,
  SkyLight, CoreDock, Mach, and launchd interactions.
- Existing OSAX loader/payload embedded as generated binary assets or built by a
  separate legacy target.
- A tracked follow-up spike decides whether to replace that legacy island with Rust,
  assembly, or keep it permanently documented as an exception.

## Compatibility policy

Preserve compatibility where it matters to users:

- Keep the binary name, launchd service behavior, socket path conventions, and
  basic `yabai -m ...` workflow.
- Keep documented commands unless there is a clear reason to remove or rename them.
- Keep JSON output machine-friendly and stable once the Rust version ships.
- Keep scripting-addition user setup semantics: partial SIP requirement, root for
  `--load-sa`, ad-hoc signing for injected components, and no hardened runtime on
  OSAX payloads.

Allow intentional breaking changes:

- Normalize inconsistent parser edge cases from `src/message.c`.
- Replace ambiguous or misleading error messages.
- Stop preserving behavior that only exists because of C memory layout, temp
  allocator lifetime, or single-translation-unit ordering.
- Document each break in `CHANGELOG.md`, release notes, and a migration section in
  the Rust rewrite docs.

## Existing architecture notes

- `src/manifest.m` includes the whole program as one translation unit. Many helpers
  are `static`, depend on include order, and share globals from `src/yabai.c`.
- `src/yabai.c` owns CLI parsing, global managers, socket/lock paths, version,
  startup sequencing, and service/SA command dispatch.
- `src/message.c` is the public command grammar for `config`, `display`, `space`,
  `window`, `query`, `rule`, and `signal`.
- `src/event_loop.c` serializes AX, SkyLight, mouse, Mission Control, and socket
  events through one worker queue. Preserve this actor-like behavior initially.
- `src/view.c`, `src/space_manager.c`, and `src/window_manager.c` contain the core
  layout and state transitions.
- `src/workspace.m`, `src/application.c`, `src/window.c`, `src/mouse_handler.c`,
  and `src/mission_control.c` are macOS integration-heavy.
- `src/sa.m` manages OSAX install/load/check/sudoers behavior.
- `src/osax/loader.m`, `src/osax/payload.m`, `src/osax/x64_payload.m`, and
  `src/osax/arm64_payload.m` are the highest-risk rewrite targets.
- `src/osax/payload_bin.c` and `src/osax/loader_bin.c` are generated assets. Do not
  hand-edit them.

## Proposed Rust workspace

- `yabai`: binary crate for CLI, daemon startup, launchd/service commands, and user
  entry points.
- `yabai-core`: pure Rust geometry, layout tree, command model, config state,
  rule/signal data, and deterministic policy logic.
- `yabai-ipc`: client/server socket framing compatible with the current daemon.
- `yabai-runtime`: event enum, event queue/actor, signal execution, and state
  orchestration.
- `yabai-macos`: unsafe wrappers for AX, CoreFoundation, Cocoa/AppKit,
  CoreGraphics, Carbon, SkyLight, CoreDock, Mach, and private symbol lookup.
- `yabai-sa`: scripting-addition install/uninstall/check/load client logic.
- `yabai-osax-common`: shared SA socket packet/opcode definitions from
  `src/osax/common.h`.
- `yabai-osax-legacy`: optional build/embed boundary for the current ObjC/C OSAX
  loader and payload until replaced or permanently accepted as an exception.

## Phased plan

### Phase 0: Baseline and contracts

- Record current CLI options from `src/yabai.c`.
- Record message grammar and error behavior from `src/message.c`.
- Snapshot JSON fields from `src/display.h`, `src/view.h`, and `src/window.h`.
- Snapshot SA opcodes and packet framing from `src/osax/common.h` and `src/sa.m`.
- Add golden tests around parser behavior, JSON shape, IPC framing, and command
  errors before changing implementation language.

Exit criteria:

- Current C binary has a reproducible behavior baseline.
- `make test` and `make e2e` remain the compatibility floor.

### Phase 1: Rust build skeleton

- Add Cargo workspace beside the current C build.
- Preserve universal release output: `x86_64-apple-darwin` plus
  `aarch64-apple-darwin`, combined with `lipo` if needed.
- Preserve minimum macOS 11.0, Info.plist embedding, release signing,
  notarization, and canary/dev version behavior.
- Keep current `make`, `make install`, `make test`, `make e2e`, and `make dev`
  aliases as the user-facing build surface.

Exit criteria:

- A Rust placeholder binary builds, signs in the dev flow, and does not disturb the
  existing C binary path until intentionally selected.

### Phase 2: Pure Rust core

- Port geometry and area helpers first.
- Port BSP/stack/float layout tree operations.
- Port rule parsing/effect merging where it can be isolated from AX/SkyLight.
- Port command tokenization into a typed parser with intentional cleanup of edge
  cases.
- Use property tests for layout invariants and golden tests for command parsing.

Exit criteria:

- Rust unit tests cover the deterministic logic better than the current
  `tests/src/area.c` coverage.

### Phase 3: IPC and CLI compatibility

- Implement current client message framing: 32-bit byte length followed by
  NUL-delimited argv tokens.
- Implement server socket behavior under `/tmp/yabai_$USER.socket`.
- Keep the Rust client able to talk to the C daemon during migration.
- Decide which parser oddities become documented breaking changes.

Exit criteria:

- Rust `yabai -m ...` can communicate with the C daemon.
- Existing `scripts/e2e-smoke.sh` can be adapted to run against the Rust client.

### Phase 4: Runtime and state ownership

- Replace global C managers with a Rust `AppState`.
- Preserve the serialized event-processing model from `src/event_loop.c`.
- Convert callback inputs into a Rust `Event` enum quickly, then process everything
  on the actor thread.
- Keep unsafe callback bodies as small as possible.

Exit criteria:

- A Rust daemon skeleton starts, accepts messages, and handles mocked event streams.

### Phase 5: macOS integration wrappers

- Wrap CoreFoundation and ObjC objects in RAII types.
- Wrap AX observer lifecycle and AXUIElement references.
- Wrap CGEventTap and display callbacks.
- Wrap private SkyLight/CoreDock calls declared in `src/misc/extern.h`.
- Keep private symbol lookup isolated and explicit.

Exit criteria:

- Business logic never calls raw FFI directly.
- Unsafe blocks are small, named, and documented by module-level invariants.

### Phase 6: Manager migration

- Port display discovery/query/focus logic.
- Port space discovery/query/move/focus logic.
- Port window discovery/query/move/resize/focus/layer/opacity logic.
- Port process and workspace notifications.
- Port mouse handling and Mission Control handling.
- Rework edge cases intentionally instead of copying incidental C behavior.

Exit criteria:

- Rust daemon can manage windows without the C core.
- `make e2e` equivalent passes against the Rust daemon.

### Phase 7: Scripting-addition manager

- Port `src/sa.m` install/uninstall/check/load/sudoers behavior to Rust.
- Use `include_bytes!` or a dedicated artifact step for loader/payload bytes.
- Preserve PAC ABI patching for the loader on Apple Silicon.
- Preserve ad-hoc signing and avoid hardened runtime for injected OSAX artifacts.

Exit criteria:

- Rust daemon can install, load, and check the existing OSAX payload.

### Phase 8: OSAX feasibility spike

- Test whether Rust can reliably build the loader/payload for the required macOS
  and architecture matrix, especially `arm64e` and PAC-sensitive behavior.
- Prototype only one narrow path first, likely the loader.
- If Rust output is fragile, keep `yabai-osax-legacy` and document the exception.
- If Rust output is reliable, port payload opcodes incrementally behind the same SA
  protocol and compare behavior with the legacy payload.

Exit criteria:

- One of these decisions is documented:
  - Keep legacy OSAX permanently as a small non-Rust ABI island.
  - Replace loader only.
  - Replace loader and payload with Rust plus any required assembly.

### Phase 9: Cutover and cleanup

- Switch release builds to the Rust daemon once parity is sufficient.
- Remove C daemon code after the Rust binary is the only shipped daemon.
- Keep or remove legacy OSAX according to Phase 8.
- Update `README.md`, `AGENTS.md`, `docs/debugging.md`, `docs/testing.md`,
  `docs/releasing.md`, CI, and release notes.

Exit criteria:

- Released yabai-plus uses the Rust daemon.
- Remaining non-Rust code, if any, is explicitly limited to the OSAX ABI island.

## Verification matrix

Automated checks:

- `cargo test --workspace`
- Rust parser golden tests
- Rust layout property tests
- Rust IPC compatibility tests
- JSON snapshot tests
- Current `make test` while C code still exists
- Current or adapted `make e2e`

Manual checks:

- Launch, terminate, hide, unhide, and front-switch applications.
- Window create, destroy, focus, move, resize, minimize, deminimize, close.
- BSP, stack, float, swap, warp, insert, balance, equalize, rotate, mirror.
- Native fullscreen, windowed fullscreen, zoom parent, zoom fullscreen.
- Scratchpads, sticky windows, PiP/system dialogs, ineligible windows.
- Mouse modifier drag move/resize/swap/stack behavior.
- Focus-follows-mouse and mouse-follows-focus behavior.
- Mission Control enter/exit, show desktop, space create/destroy/move.
- Display add/remove/move/resize, multi-display space moves, menu bar/Dock changes.
- SA unloaded fallback path and SA loaded fast path.
- `--check-sa`, Dock restart, root/SIP failure modes.

Platform checks:

- Intel Mac.
- Apple Silicon Mac.
- Single display.
- Multi-display with separate Spaces enabled.
- Supported macOS versions still targeted by the release.

## Known high-risk areas

- Private SkyLight/CoreDock ABI drift.
- Dock pattern scanning and macOS-version-specific offsets.
- AX notification timing and object lifetime.
- Mission Control transitions and multi-display space mapping.
- Focus restoration and sticky/ineligible window selection.
- Window animation/proxy behavior.
- `arm64e` loader PAC ABI patching.
- Signing/notarization differences between daemon and injected artifacts.

## RESUME HERE (current map for the next session)

Read this section first; it is the ground truth. Older sections above are a
chronological log and may describe earlier states.

### What exists and where (all pure Rust, no macOS except `yabai-macos`)

- `crates/yabai-core` (62 tests) — pure, deterministic, no deps:
  - `geometry.rs`: `Area`/`Point`/`Direction`/`Split`, area split + truncation,
    `is_in_direction`/`distance_in_direction` (ported from `src/view.c`).
  - `layout.rs`: the BSP `Tree` (arena of `Node` by `NodeId`) ported from
    `src/view.c`. split/insert/remove/rotate/mirror/equalize/balance, fence +
    `resize_window` (`HANDLE_*`), swap, `find_node_in_direction`, `capture()` ->
    `WindowFrame`s, `set_root_area`. Globals are lifted into `LayoutConfig`.
  - `parser.rs`: token classifiers from `src/message.c` — `Selector`,
    directions, layout/split/balance/placement/insertion args, resize handles,
    `ValueType`, `parse_key_value` (faithful left-to-right `=`/`!=` scan).
  - `command.rs`: typed model for ALL 7 domains (`parse_config`/`display`/
    `space`/`window`/`query`/`rule`/`signal`) + `parse_message` dispatcher ->
    `Message`. `ParseError` `Display` text matches the C `daemon_fail` strings.
  - `signal.rs`: `SignalEvent` (all `enum signal_type` variants in order) +
    `Signal` + `Signal::from_key_values` (faithful `handle_domain_signal`
    validation, including `app!=`/`title!=` exclusion flags). The runtime
    stores/serializes/fires these and applies `app`/`title` regex filters for the
    event categories that carry that metadata.
- `crates/yabai-ipc` (6 tests) — client wire framing + `send_message`; the
  `crates/yabai` binary `-m` path uses it and talks to the live C daemon.
- `crates/yabai-runtime` (31 tests) — the control plane, depends on `yabai-core`:
  - `config.rs`: `Config` (all settable keys) + get/set + `layout_config()`.
  - `app_state.rs`: `AppState` (config, `sid -> Tree`, active space, focused
    window). `handle_tokens`/`dispatch` apply messages; `handle_event` applies a
    typed `StateEvent`; `WindowAssignedToSpace` routes new/moved windows into a
    specific tree; `SpaceRemoved` drops vanished trees; `flush`/`flush_active`/
    `flush_active_to`; `LayoutSink` trait + `RecordingSink`. Window selectors
    resolve against the active tree (id/first/last/next/prev/direction).
  - `runtime.rs`: `Runtime<S: LayoutSink>` = state + sink, flushes after every
    mutation.
  - `actor.rs`: `Actor<S>` = a thread owning a `Runtime`, fed serialized work
    (`post_event`, blocking `message`, `shutdown` returns the `Runtime`).
- `crates/yabai-macos` (5 tests) — Phase 5 boundary, depends on runtime+core.
  Modules: `ax.rs`, `screen.rs`, `objc.rs`, `workspace.rs`, `cgwindow.rs`,
  `observe.rs`, `display.rs`, `space.rs`:
  - `ax.rs`: `AxSink` impl of `LayoutSink` moving windows via AX
    (`kAXPosition`/`kAXSize`); `AxWindow` RAII over `AXUIElementRef`; local
    CF/ApplicationServices FFI. Builds/links on macOS. Also AX diagnostics
    probes, the direct movers `move_focused_window` / `move_pid_window`, and
    `tileable_pid_windows` (settable-position discovery, CG-id-independent) —
    used by `--experimental-ax-tile-pid` to BSP-tile a real app's windows
    through `Runtime -> AppState -> AxSink` (verified live tiling 3 Finder
    windows). Plus `set_window_frame` / `read_window_frame` helpers.
  - `screen.rs`: `main_visible_frame()` (`NSScreen.visibleFrame`, menu bar +
    Dock excluded, flipped to top-left CG coords) via shared objc FFI.
  - `objc.rs`: shared Objective-C glue (`class`/`sel`, generic
    `msg0`/`msg1`/`msg4`).
  - `workspace.rs`: `regular_application_pids()` (`NSWorkspace`; NOTE: does not
    refresh without a pumped run loop — see below) and `observe_workspace()` on a
    dedicated run loop for active-space, app-launched, and app-terminated
    notifications.
  - `cgwindow.rs`: `on_screen_windows()` / `application_pids_with_windows()`
    (`CGWindowListCopyWindowInfo`) — live app/window discovery that DOES refresh
    without a run loop; no Screen Recording perm (reads pid/number/layer only).
  - `observe.rs`: `observe_pid(pid, tx)` wraps `AXObserver` → typed
    `ObservedEvent`s over a channel (`WindowCreated`/`Destroyed`/
    `FocusedWindowChanged`). NOTE: `AXUIElementDestroyed` is unreliable; use set
    reconciliation, not the notification.
  - `mouse.rs`: `observe_mouse_moved(tx)` — a listen-only `CGEventTap` on
    `kCGEventMouseMoved` (pumped on a dedicated run-loop thread) reporting cursor
    points for `focus_follows_mouse`; `observe_mouse_drag(tx)` — a second, *active*
    tap on left/right down/dragged/up that consumes the click while the armed
    `mouse_modifier` is held (`set_drag_modifier`), reporting `MouseDragEvent`s for
    drag-to-move/resize; plus `post_mouse_moved` / `post_mouse_drag` /
    `post_right_mouse_drag` (synthesize events for testing).
  - `space.rs`: read-only SkyLight discovery for `current_space_for_display()`
    (`SLSManagedDisplayGetCurrentSpace`), `spaces_for_display()`
    (`SLSCopyManagedDisplaySpaces` + `id64` extraction), and
    `spaces_for_window()` (`SLSCopySpacesForWindows(..., 0x7, ...)` with the C
    fallback to the window display's current space — unreliable on macOS 26, see
    below), and `windows_on_space()` (`SLSCopyWindowsWithOptionsAndTags`, the
    inverse mapping used as the macOS-26-correct window→space resolver).
- `crates/yabai-osax-common`, `-osax-legacy`, `-sa` — still scaffolding/constants.

THE LIVE WM DAEMON (in `crates/yabai/src/main.rs`):
`--experimental-rust-wm-daemon <socket> <pid|all> [gap] [padding]` is a working
dynamic tiling WM. A single-threaded event loop on the main thread owns
`Runtime<AxSink>` (so all sink registration stays single-threaded) and consumes a
unified `WmWork` channel fed by (a) one `observe_pid` thread per app, (b) a 3s
self-heal `Tick`, (c) a socket-acceptor thread, (d) an NSWorkspace observer
(active-space/app launch/app termination). `reconcile_pid` re-discovers an
app's tileable windows on each event, registers newcomers / drops vanished ones
(robust to the unreliable AX destroy), sets `app`/`title`/`pid` metadata, and
re-flows. Verified live: auto-tile on open, auto-reconcile on close, new-app
pickup via CGWindowList, real active-space id discovery at startup, per-space
trees for the first display's discovered spaces, window-to-space assignment
routing during reconciliation, first-display space add/remove reconciliation,
active-space notification handling with SkyLight re-read, direct app pickup in
`all` mode, immediate app-termination cleanup, debounced `window_moved` /
`window_resized` signals, `space --rotate`/`--balance` over the socket, and
`query --windows id,app,title` returning real values.

Other experimental flags in `main.rs`: `--experimental-ax-{focused-window,debug,
windows-for-pid,pid-debug,move-focused,move-pid,tile-pid,observe-pid}`,
`--experimental-cursor-location` (prints the live cursor point),
`--experimental-window-alpha <wid>` (read-only `SLSGetWindowAlpha` opacity
readback — verifies the SA opacity opcode),
`--experimental-window-transform <wid>` (read-only `SLSGetWindowTransform` dump —
verifies the SA `scale_window`/pip opcode, invisible to the AX/CG frame),
`--experimental-windows-on-space <sid>` (read-only `SLSCopyWindowsWithOptionsAndTags`
dump — verifies the macOS-26 window→space resolver), `--experimental-sa-{status,opacity,
create-space,destroy-space,window-to-space,focus-space}` (direct SA opcode
probes — do NOT run the mutating ones against the user's live machine),
`--experimental-rust-{daemon,tile-daemon}` (the tile-daemon is the older
snapshot-only `Actor<AxSink>` version; the wm-daemon supersedes it).

End-to-end today: a real dynamic tiling WM across **all displays**, driven
entirely by the pure core. It seeds real space ids for every display (each in its
own usable frame), tiles each display's current space simultaneously, and routes
discovered windows to the display/space they're physically on. Active-space
changes are notified through NSWorkspace; app launch/termination are notified
too; space add/remove is refreshed by polling before daemon work. Window ops:
focus (raise), close, swap, warp, minimize/deminimize, toggle
float/zoom/native-fullscreen, sticky, and shadow; opacity, sub-layer, move-to-space,
and move-to-display (all via the SA); space focus (SA `focus_space`, gesture fallback), switch (SA focus /
cross-display content swap), cross-display space move (SA `move_space_to_display`),
intra-display space reorder + same/cross-display swap (SA `move_space_after_space` /
`move_window_list_to_space`), and rotate/balance/mirror/layout;
`signal` add/list/remove with live firing on focus/app/space/move/resize/minimize/
deminimize/title-change events and app/title filters for metadata-carrying events;
`mouse_follows_focus` cursor centering on focus; `focus_follows_mouse`
    (autofocus/autoraise) via a mouse-moved `CGEventTap`; `window_opacity` auto
    active/normal opacity on focus change; mouse drag-to-move/resize/drop
    (`mouse_modifier` + left/right drag, `mouse_action1`/`mouse_action2`,
    `mouse_drop_action`) via an active `CGEventTap`; `rule` effects for
    `manage=`, `scratchpad=`, `sticky=`, `sub-layer=`, `opacity=`, `grid=`,
    `display=`, and `space=` on new windows and `rule --apply`.

### Do these next, in order (Phase 5/6 breadth — the big remaining work)

1. Multi-space + Mission Control: space discovery, startup per-space trees, and
   window-to-space assignment/routing now exist for the first display; space
   add/remove is refreshed by polling and active-space changes are notified.
   `space --focus <sel>` uses the SA `focus_space` opcode whenever the SA is
   loaded (SA-first, unconditional), falling back to the dock-swipe gesture only
   on SA error (incl. SA absent), with cross-display cursor warp/display
   activation; `--create`/`--destroy` (session 27), `--display` (cross-display
   space move, session 31), `--move` (intra-display reorder, session 32), and
   `--swap` (same-display, session 33) all work via the SA. Still to do:
   `--switch` (session 36 — SA `focus_space` same-display, content-swap
   cross-display, verified live); the **cross-display** `--swap` (window-content
   swap) is done and verified live (session 35, `space_swap_cross_display`); and,
   later, SLS create/destroy notifications. The whole `space` domain's SA ops are
   now wired: create/destroy/focus/switch/move/swap/display.
   **macOS-26 window→space bug — FIXED (session 34).** `SLSCopySpacesForWindows`
   only reports the *current* space on macOS 26, so the daemon used to mis-assign
   windows on non-current spaces to the active space. Fixed by enumerating the
   inverse mapping via `yabai_macos::space::windows_on_space`
   (`SLSCopyWindowsWithOptionsAndTags`, the C `space_window_list` primitive);
   `managed_space_for_window` now resolves a window's true space and per-space trees
   for non-visible spaces are correct. `spaces_for_window` remains only as an
   older-macOS fallback.
2. Multi-display: done — the daemon tiles every display's current space at once,
   each in its own usable frame, routing windows to the display they're on.
   Display hot-plug is handled by polling/reconcile before daemon work and on the
   3s tick (physically verified unplug/replug). `window --display <sel>` is now
   wired through the SA (session 30): it moves the acting window to the target
   display's active space via `move_window_to_space`, **verified live with a real
   two-display setup** (Finder window moved from `space 1, display 1` to `space 64,
   display 2`, confirmed by both `query` and a `screencapture -D 2`).
   `space --display <sel>` (cross-display *space* move) is also wired through the SA
   (session 31): it moves the acting/active space to the target display's active
   space via `move_space_to_display`, with the C validation order and the
   live-mission-control `prev_space` for the focus case; verified live on two
   displays (non-active + active-space moves, round-trip, and both error strings).
   `space --move <sel>` (intra-display reorder) is also wired through the SA
   (session 32): the C's three `move_space_after_space` branches keyed on
   first-on-display (global mission-control order), verified live (reorder +
   reverse + both error strings). `space --swap <sel>` (same-display) is wired too
   (session 33): the C's five swap branches via `move_space_after_space`, verified
   live (swap + swap-back + errors). The **cross-display** `space --swap` content
   swap is also done and verified live (session 35), using the macOS-26-safe
   `windows_on_space` inverse mapping.
3. App launch/termination are now observed directly through NSWorkspace; the 3s
   tick remains a backstop for missed AX/window changes and CGWindowList pickup.
4. More window ops needing live state: done — `window --focus` with-raise
   (`AxSink::focus_window`), `--close` (including trailing selector parsing),
   `--warp`, `--toggle float`, `--toggle zoom-fullscreen`/`zoom-parent`,
   `--toggle native-fullscreen` (enter on the
   focused window; exit via id/`first`/`last`/single-window bare toggle),
   `--minimize` (including trailing selector parsing), `--deminimize` for numeric
   ids and `first`/`last`; `--swap`
   already worked. `window --opacity <float>` is now a typed parser action wired
   through the SA (`set_opacity` + `config.window_opacity_duration`), verified live
   via the `--experimental-window-alpha` (`SLSGetWindowAlpha`) readback.
   `window --display`
   (session 30), `--sub-layer below|normal|above|auto` (typed parser action + SA
   `set_layer`), `--toggle
   sticky` (SA `set_sticky` + untile/re-tile) and `--toggle shadow` (SA `set_shadow`)
   are all wired through the SA and verified live (sessions 37/67; the C command
   is `--sub-layer`, not `--layer`). `window --insert <dir>` is now a typed
   parser action feeding the pure BSP insertion marker. `window --grid r:c:x:y:w:h`
   places a floating/unmanaged window on a grid over its display's usable area
   (pure `grid_frame` + `try_window_grid` glue, session 56, verified live; a managed
   window is rejected). `window --move abs|rel:dx:dy` repositions a floating/unmanaged
   window (leaving its size unchanged; managed windows rejected) via `try_window_move`
   → `AxSink::set_frame` (session 57, verified live). `window --resize handle:dw:dh`
   resizes both kinds (session 58, verified live): a **managed** window's directional
   handle nudges its own-space fence(s) in the pure core (abs rejected); an
   **unmanaged** window is AX-resized by `try_window_resize` (`abs` sets the size
   origin-fixed, a directional handle grows/shrinks from the dragged edge per C
   `window_manager_resize_window_relative_internal`). `window --raise [sel]`/`--lower
   [sel]` reorder a window's z-stacking above/below an optional reference window (bare =
   above/below everything) via the SA `order_window` opcode (session 59, verified live).
   `window --toggle windowed-fullscreen` saves the window's frame and fills its display's
   usable bounds (no yabai padding), restoring the saved frame on toggle-off — a
   daemon-side `try_window_windowed_fullscreen` + `windowed_frames` map, applied to any
   window like C (session 60, verified live). `window --toggle pip` scales the window
   into/out of a picture-in-picture miniature via the SA `scale_window` opcode
   (stateless — the opcode self-toggles the transform), targeting the window's display
   usable bounds inset by the active-space padding (`window_toggle_pip_via_sa`, session
   61, verified live via the new `--experimental-window-transform` `SLSGetWindowTransform`
   readback). `window --toggle expose` focuses the window with a raise then fires the
   CoreDock `com.apple.expose.front.awake` App-Exposé notification
   (`yabai_macos::coredock` + `try_window_expose`, session 61 — dispatches cleanly but
   the transient Mission Control animation isn't SSH-verifiable). `window --scratchpad`
   assign/remove/recover is typed and `window --toggle <label>` hide/show is done and
   verified live (sessions 62/69). Deminimize/native-fullscreen exit support numeric/`first`/`last`
   in both leading-target and C-style trailing-selector forms (sessions 63-64;
   deminimize is parser-backed); broader selector breadth is still deferred. Mouse
   drag-to-**move** (`mouse_modifier` + left-drag),
   drag-to-**resize** (`mouse_action2` + right-drag), and same-space tiled drop
   actions (`swap`/`stack` center drops plus edge-zone warps) are done and verified
   live (sessions 41-43). Cross-space/cross-display drops are also done and verified
   live (session 49): the dragged window — and, for a swap, the target window — are
   reassigned in the model and relocated via the SA `move_window_to_space` opcode.
   Still deferred: insertion-feedback overlays and slot-accurate cross-space
   placement (the moved window is appended, then the destination re-tile lays it out).
   `mouse_follows_focus` is done (cursor warps to the
   focused window's center on focus, with the contained-skip); `focus_follows_mouse`
   (`autofocus`/`autoraise`) is done too (session 38) via a `CGEventTap` on
   mouse-moved (`yabai_macos::mouse`), with `AxSink::focus_window_without_raise` for
   `autofocus`, verified live. The C occlusion / gesture-debounce / mission-control
   refinements are not modeled.
   Signals: mostly done — `signal --add/--list/--remove`, app/title regex filters
   (including `!=` exclusion), and live firing of `window_created`,
   `window_destroyed`, `window_focused`, `application_launched/terminated`,
   `space_changed`, `space_created`, `space_destroyed`, `window_moved`,
   `window_resized`, `window_minimized`, `window_deminimized`, `window_title_changed`, `application_activated`,
   `application_deactivated`, `application_hidden`, `application_visible`, and
   `application_front_switched` (with `YABAI_*` env vars, incl.
   `YABAI_RECENT_PROCESS_ID`), plus the context-free `space_changed`,
   `display_changed`, `system_woke`, `menu_bar_hidden_changed`, and
   `dock_did_change_pref`, and `display_added`/`display_removed`/`display_moved`/
   `display_resized` — the latter four now via a `CGDisplayRegisterReconfiguration`
   callback (commit `0036397`, documented in session 50); the callback registration
   and the `CFRunLoopRun`→`[NSApp run]` swap are verified live on macOS 26, but the
   actual moved/resized firing on a real reconfiguration is not yet verified (needs a
   physical two-display resolution/arrangement change). `dock_did_restart` is wired
   but unverified (needs `[NSApp run]`; see session 20).
   `mission_control_enter`/`exit` are now wired via a Dock `AXExpose*` observer
   (`observe_mission_control` + `dock_pid`, session 75); `dock_pid` is
   live-verified, the enter/exit firing is not (GUI-only transition). The signal
   domain is now feature-complete.
   The NSWorkspace-driven application signals (launch/terminate/activate/
   deactivate/hide/visible) and app filters are now verified live from a
   `gui/501` LaunchAgent daemon — see session 17, which also fixed the long-
   standing reason NSWorkspace notifications never fired (`NSApplicationLoad` +
   running the AppKit run loop on the main thread).
5. Then Phases 7-9: scripting addition (`yabai-sa`, currently empty — required
   for space management / cross-space moves on modern macOS), OSAX spike, and
   production packaging (wire the Rust binary into `make`, signing, notarization,
   launchd, cutover). None started.

### Hard rules / gotchas (do not violate)

- Do NOT point Homebrew, launchd, `make dev`, or `/usr/local/bin/yabai` at
  `target/debug/yabai`. The Rust binary is a client + experimental daemons only;
  it is not wired into `make`/signing/launchd and must not replace the C daemon.
- The user runs the C yabai live. Read-only `query` via the Rust client is safe;
  never bind `/tmp/yabai_$USER.socket` from Rust.
- **Do all live/mutating macOS testing on the REMOTE box, never on the user's
  local machine.** Anything that changes on-screen state (space focus/create/
  destroy, window moves/opacity/tiling) flips the user's live session — they
  reacted strongly to local space-switching. Read-only checks (cargo, `query`)
  are fine locally. The remote (`ssh student@student`, uid 501; see
  `REMOTE_TESTING.local.md`) now has **this fork's SA installed** (session 29):
  C binary `/tmp/yabai-c` (v7.1.25-plus, SA v2.1.30, macOS-26 capable) with a
  passwordless `--load-sa` sudoers rule — reload after a Dock restart via `ssh
  student@student 'sudo -n /tmp/yabai-c --load-sa'`, verify with `/tmp/yabai-rust
  --experimental-sa-status`. Only the SA is loaded there (no C daemon), so it
  won't fight the Rust WM daemon. Re-grant Accessibility in the USER TCC db after
  every re-sign of `/tmp/yabai-rust` (else AX reads fail with an "Accessibility
  Access" prompt and window discovery returns nothing).
- Workspace lints deny `clippy::undocumented_unsafe_blocks` — every `unsafe`
  block needs a `// SAFETY:` comment. `cargo fmt` reorders `use` lists
  (types/fns interleaved alphabetically); let it, then match its output.
- Verify each step with `cargo fmt --all && cargo clippy --workspace
  --all-targets && cargo test --workspace`. Currently 205 tests, clippy clean.
  The toolchain is rustup stable (installed locally 2026-07-31); `cargo` builds
  and tests the workspace directly on this machine.
- The live WM daemon binds only a caller-supplied socket; to message it use a
  socket named `/tmp/yabai_<name>.socket` and query with `USER=<name>`. Always
  `pkill -f experimental-rust-wm-daemon` to stop it (each shell call is a fresh
  process — a `$DPID` from a previous Bash call is gone).
- Do not edit generated `src/osax/*_bin.c`; do not sign injected OSAX with
  hardened runtime; defer the OSAX payload rewrite (Phase 8).
- Treat the C code as the reference implementation, not an upstream constraint;
  document intentional behavior changes (see Compatibility policy above).

### Faithful-port notes worth keeping

- `config` commands consume the next token as their value unconditionally; a
  bare command is a `Get` only at end-of-input (so `config layout window_gap`
  fails with "unknown value 'window_gap'").
- `x-axis` = `SPLIT_X` = `NodeSplit::Horizontal`; `y-axis` = `SPLIT_Y` =
  `NodeSplit::Vertical`. `auto_balance` `on`=both axes, `off`=none.
- `rotate 180` swaps children, so leaf/`window_list` order flips — expected.
- Deferred in the pure layer (need live state): zoom persistence, insert
  feedback, the z-order rank tie-break in `find_node_in_direction`, cross-space
  warp/swap and the `:NaturalWarp` heuristic (single-space `warp_window` is
  done), and `stack[.N]` selector resolution. DONE since: `recent` (window +
  space, via `last_focused_window`/`last_active_space`), `label` selectors (space
  + display), and `mouse` (window/space/display, via `cursor_point`).
