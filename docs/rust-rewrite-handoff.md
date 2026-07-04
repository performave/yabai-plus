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
  `first`, and `last` selectors restored from the daemon's minimized-window AX
  registry, and `window --close` is wired through the AX close button.
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
  one-shot removal, regex matching, and the live `manage` effect (`manage=off`
  floats/untiles, `manage=on` retiles); other rule effects are parsed/stored but
  deferred. 169 workspace tests pass. The shipped C `make` flow is unchanged.
- Last updated: 2026-07-03.
- User decisions captured:
  - The Rust rewrite may diverge permanently from upstream yabai. Rebaseability is no
    longer a primary constraint for this track.
  - Clean up edge cases and document breaking changes instead of preserving every
    bug-for-bug behavior.
  - For the scripting addition, use the most reliable engineering path rather than
    forcing literal Rust at the cost of fragile injection behavior.

## Progress log

### 2026-07-03 (session 51) — pure `window --toggle split` + verified live

- Implemented `window --toggle split`, previously an unhandled toggle
  (`window toggle 'split' not yet handled`). It's a pure BSP tree op, so it lives
  in `yabai-core`: `Tree::toggle_window_split(window_id)` mirrors the C
  `space_manager_toggle_window_split` — BSP-only, flips the window's **parent**
  node split axis (`SPLIT_Y`↔`SPLIT_X`), then re-tiles (balances the whole tree
  when auto-balance is on, else recomputes the parent subtree). `dispatch_window`'s
  `Toggle("split")` arm calls it on the focused window; a lone/root window or a
  non-BSP space is a silent no-op, matching C's
  `window_node_is_intermediate` guard.
- Added `window_toggle_split_flips_parent_axis` (flip + restore round-trip) and
  `window_toggle_split_is_noop_on_root_window`.
- **Verified live on the remote (macOS 26.5.1):** a side-by-side tiled pair (903 at
  x769, 902 at x1120, both `split vertical`) → `window --toggle split` re-tiled them
  stacked (903 top y501 h215, 902 bottom y729 h215, both `split horizontal`); a
  second toggle restored `vertical`. The daemon flushed the new frames to screen
  (query frames changed), confirming the pure op rides the existing re-tile path.
- Verification: `cargo fmt --all`; `cargo test --workspace` (169 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-03 (session 50) — document + de-risk display reconfiguration signals; clippy-clean

- Retroactively documented commit `0036397` ("display reconfiguration signals"),
  which had landed with no handoff entry: it wires
  `CGDisplayRegisterReconfigurationCallback` (`yabai_macos::display`) into a callback
  that fires `display_added` / `display_removed` / `display_moved` / `display_resized`
  (mapped from `kCGDisplay{Add,Remove,Moved,DesktopShapeChanged}Flag`), each with
  `YABAI_DISPLAY_ID` and a `refresh_live_display_state`, mirroring the C
  `display_handler`. The commit also swapped the workspace observer's terminal
  `CFRunLoopRun` for `[NSApp run]` so the WindowServer routes the CG reconfiguration
  callback (the callback is registered on the main thread right before the run loop).
- **De-risked the `CFRunLoopRun` → `[NSApp run]` swap** (the same main-thread run loop
  session 17 fixed for NSWorkspace delivery): cleaned the two dead-code warnings it
  left (the now-orphaned `CFRunLoopRun` extern in `workspace.rs` and the unused
  `CGDisplayRemoveReconfigurationCallback` extern in `display.rs`) and fixed three
  stale `CFRunLoopRun` doc comments. `cargo clippy --workspace --all-targets` is now
  **fully clean (0 warnings)** — previously two dead-code warnings on every build.
- **Verified live on the remote (macOS 26.5.1):** the WM daemon starts with the new
  binary, `observe_display_reconfiguration().unwrap()` does **not** panic (so
  `CGDisplayRegisterReconfigurationCallback` succeeds on macOS 26), and the daemon
  runs stably under `[NSApp run]` while its worker-thread event loop keeps serving
  socket commands (`query --displays` → 2 displays). That proves the regression-risk
  part (callback registration + the run-loop swap). **Not verified:** the actual
  `display_moved`/`display_resized` firing on a real reconfiguration — no
  `displayplacer` on the remote and no safe way to force a resolution/arrangement
  change over SSH without risking the display's state; the CG/NSWorkspace callbacks
  also historically need a GUI session. Left for a physical two-display session.
- Verification: `cargo fmt --all`; `cargo test --workspace` (167 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-03 (session 49) — cross-space / cross-display mouse drops + cleanup + verified live

- Finished and cleaned up the **cross-space / cross-display tiled drag-drop** started
  in the previous commit (`7958ade`). `drop_tiled_window_at_point` now returns a
  `DropResult` (`Ignored` / `SameSpace` / `CrossSpace { dragged, dragged_new_sid,
  swapped, swapped_new_sid }`); when the release point lands on a display whose
  active space differs from the dragged window's, the pure layer reassigns tree
  membership (a swap onto a target window also sends that window back to the source
  space) and the daemon issues the SA `move_window_to_space` opcode(s) to perform the
  real move, then re-tiles both displays.
- **Cleanup of the committed first cut:** extracted a `reassign_window_to_space`
  helper collapsing the three duplicated cross-space branches; removed the
  stream-of-consciousness comments (`// Wait…`, `// can refine later`, `// The C
  behavior for swap is complex`); fixed a doc comment that had been split across
  `managed_space_at_point` / `managed_window_at_point`. `handle_drag` now takes the
  daemon's existing `ScriptingAddition` instead of building a fresh one with a
  hardcoded `"eric"` USER fallback (which would have pointed at the wrong SA socket
  on the remote `student` box). Rewrote the sloppy `cross_space_mouse_drop` test and
  added `cross_space_mouse_drop_swap_returns_target` for the swap-back path. Dropped
  two scratch test files (`test-cross-space-drag.swift`, `test-e2e-cross-space.sh`)
  that had been left staged.
- **Verified live on the remote (two displays, macOS 26.5.1):** display 1 (1470×956,
  space 1) + display 2 (1600×900, space 92), SA loaded (payload v2.1.30). With
  `mouse_drop_action swap`, an `fn`+drag of Finder window `1017` from display 1
  (411,494) onto Finder window `902` on display 2 (2270,465) swapped them across
  displays: raw SkyLight `windows_on_space` went from space 1 `[…,1017,…]` /
  space 92 `[…,902,…]` to space 1 `[…,902,…]` / space 92 `[…,1017,…]`, and the
  daemon's managed `query --windows` confirmed `1017`→`space 92, disp 2` and
  `902`→`space 1, disp 1` (the other three managed Finder windows untouched). Two
  independent authoritative sources agree. (Screenshots were unavailable — the SSH
  session lacked Screen Recording after the re-sign — but the SkyLight + query pair
  is the same proof standard used for the session-35 cross-display swap.)
- **Remote gotchas hit this session (for the runbook):** `/tmp` was cleared (reboot),
  so `/tmp/yabai-c` was gone and the SA wouldn't load — redeploy `bin/yabai` as
  `/tmp/yabai-c`. Every scp'd binary must be re-signed on the box
  (`codesign -f -s - <path>`) or it's SIGKILLed (exit 137). The passwordless
  `--load-sa` sudoers rule is hash-pinned and breaks after any re-sign/redeploy, so
  the SA load now needs one interactive `sudo /tmp/yabai-c --load-sa` (user password).
  AX window discovery and `--experimental-space-probe`/`screencapture` all return
  empty until the display is awake — run `caffeinate -d -u` (a stale daemon socket
  also needs `rm -f` before rebind).
- Verification: `cargo fmt --all`; `cargo test --workspace` (167 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-07-03 (session 48) — pure `window --stack <sel>` runtime action

- Wired `WindowAction::Stack` through `AppState::dispatch_window`, using the
  existing `Tree::stack_window_onto` helper from the mouse-drop work. The focused
  window is moved into the target leaf's stack, matching the pure same-space core
  of the C stack operation; macOS z-order animation remains outside the pure layer.
- Added `window_stack_moves_focused_window_into_target_leaf`, verifying the target
  leaf's `window_list` and `window_order` after a `window --stack` command.
- Verification: `cargo fmt --all`; `cargo test --workspace` (165 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-07-03 (session 47) — pure `query --windows has-shadow`

- Added the `has-shadow` property to the pure `query --windows` serializer, backed
  by the runtime's existing shadow toggle state (`true` by default, `false` after
  `window --toggle shadow` records a disabled shadow). This is state the daemon
  already owns from the SA-backed shadow command, so no live AX/SkyLight read is
  needed for managed windows still in the tree.
- Added `query_windows_serializes_has_shadow` covering the default and
  shadow-disabled states.
- Verification: `cargo fmt --all`; `cargo test --workspace` (164 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-07-03 (session 46) — reject empty query property segments

- Implemented the compatibility cleanup for query property lists: empty segments in
  comma-separated property tokens now fail during `query` parsing (`id,,frame`,
  `,id`, `id,`) instead of falling through as an empty property name. This avoids
  preserving C's incidental in-place token mutation behavior.
- Added `query_empty_property_segment_errors` covering interior, leading, and
  trailing empty segments.
- Verification: `cargo fmt --all`; `cargo test --workspace` (163 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-07-03 (session 45) — pure `query --windows stack-index`

- Added the `stack-index` property to the pure `query --windows` serializer. It
  matches the C `window.c` behavior: 1-based position within a leaf whose
  `window_list` has more than one window, otherwise `0`. This is backed entirely
  by the runtime BSP/stack tree state, so no macOS or SA access is needed.
- Added `AppState::window_stack_index` and a golden runtime test over `layout stack`
  with three windows (`stack-index` 1/2/3).
- Verification: `cargo fmt --all`; `cargo test --workspace` (162 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-07-03 (session 44) — `space_created` / `space_destroyed` signals from topology diff + verified live

- Wired `space_created` and `space_destroyed` signal firing into the existing live
  topology refresh (`refresh_live_display_state`). The daemon snapshots known
  spaces before re-reading displays/spaces, fires `space_created` for new live sids
  with `YABAI_SPACE_ID` + `YABAI_SPACE_INDEX` (using Mission Control order), and
  fires `space_destroyed` for removed sids with `YABAI_SPACE_ID`, mirroring the C
  `event_signal.c` payloads. Destroyed-space signals only fire when the space
  snapshot was complete, matching the existing conservative removal gate.
- **Verified live on the remote (macOS 26.5.1)** with the SA-loaded Rust WM daemon
  on an isolated socket: registered signal actions appending env vars to
  `/tmp/yabai-space-signals.out`, ran `space --create`, detected new sid 83, then
  ran `space 83 --destroy`. Captured output:
  - `created:83:3`
  - `destroyed:83`
- Verification: `cargo fmt --all`; `cargo test --workspace` (161 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-07-03 (session 43) — mouse drag drop actions (swap/stack/edge-warp) + verified live

- Implemented same-space tiled drop actions for drag-to-move release. A tiled move
  now attempts a drop before snapping back: center drops use `mouse_drop_action`
  (`swap` or `stack`), while edge drops use the C triangular target zones to warp
  the dragged window top/right/bottom/left of the target. Floating drag-move and
  tiled drag-resize behavior from sessions 41/42 is unchanged.
- Added pure tree helpers: `Tree::stack_window_onto` (move source into target leaf
  stack) and `Tree::warp_window_directional` (explicit split+child directional
  warp). Added `AppState::drop_tiled_window_at_point`, including the C-style
  center rectangle and edge triangle hit tests. Cross-space/cross-display drop
  bookkeeping and insert-feedback overlays remain deferred.
- **Verified live on the remote (macOS 26.5.1):**
  - Center swap: dragged Finder window 893 onto 891; 893 moved from
    (65,43,692,903) to 891's old frame (768,500,692,446), and 891 moved to 893's
    old frame.
  - Center stack: with `mouse_drop_action stack`, dragged 900 onto 898; both ended
    with the same frame (768,43,692,903), confirming shared stack leaf capture.
  - Top-edge warp: dragged 903 to the top edge of 901; 903 ended above 901 in the
    same column, (768,43,692,446) above 901 at (768,500,692,446).
- Verification: `cargo fmt --all`; `cargo test --workspace` (161 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-07-03 (session 42) — mouse-drag resize (`mouse_action2` + right-drag) via active CGEventTap + verified live

- Extended the active mouse drag tap to listen to right-button down/drag/up in
  addition to left-button events. `MouseDragEvent::Down` now carries the starting
  button, so the daemon selects `config.mouse_action1` for left-drag and
  `config.mouse_action2` for right-drag, matching the C `mouse_handler` model.
- Implemented drag-to-resize. On mouse-down the daemon chooses the resize handle
  from the initial cursor quadrant relative to the target window midpoint (same as
  C). Floating windows are resized directly through `AxSink::set_frame` and keep
  the new frame; tiled windows call the new `AppState::resize_tiled_window` helper
  to resize the BSP tree containing that window, then flush visible spaces so the
  change persists. Tiled drag-to-move still snaps back on release because drop
  actions remain deferred.
- Added `post_right_mouse_drag` and the `--experimental-post-right-mouse-drag <x1
  y1 x2 y2>` probe to synthesize `fn`+right-drag on the remote test box.
- **Verified live on the remote (macOS 26.5.1):**
  - Tiled Finder window 873: `fn`+right-drag from its bottom-right edge (737,926)
    -> (857,926) resized bounds from (65,43,692,903) to (65,43,811,903), and the
    size remained stable on a later readback (persistent BSP resize).
  - Floating Finder window 873 after `--toggle float`: `fn`+right-drag
    (856,926)->(946,996) resized bounds from (65,43,811,903) to
    (65,43,901,913). Height growth was display-bottom-clamped; width and origin
    verified direct AX resize persisted.
- Verification: `cargo fmt --all`; `cargo test --workspace` (159 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-07-03 (session 41) — mouse-drag move (`mouse_modifier` + left-drag) via an active CGEventTap + verified live

- Implemented drag-to-move on top of the session-40 config model. Added a second,
  **active** (input-consuming) `CGEventTap` in `yabai_macos::mouse`
  (`observe_mouse_drag`) on left mouse down/dragged/up: it consumes the click only
  while the armed `mouse_modifier` is held (an `AtomicU8` the daemon sets via
  `set_drag_modifier` from `config.mouse_modifier`, re-armed on config change),
  mirroring the C `mouse_handler`. Kept separate from the listen-only
  `focus_follows_mouse` tap (session 38) so that stays untouched. Events flow as
  `WmWork::Drag(MouseDragEvent::{Down,Dragged,Up})`.
- `handle_drag` (daemon) captures the window under the cursor on down (floating
  windows first — they sit above tiles — by hit-testing their live AX frames, else
  the tiled window on the visible space), moves it live via the new
  `AxSink::set_frame` by the drag delta, and on release: a **floating** window keeps
  its new position; a **tiled** window snaps back (`flush_all_active_to`). Only
  `mouse_action1 = move` is implemented; **resize (`mouse_action2`) and drop actions
  (swap/stack/warp + BSP-grid resize) are deferred** (the C `mouse_drop_action_*` /
  `mouse_drop_try_adjust_bsp_grid`).
- Added test/verification helpers: `post_mouse_drag` (synthesizes a full fn+drag
  sequence with `CGEventSetFlags`), the `--experimental-post-mouse-drag <x1 y1 x2 y2>`
  probe, and `window_bounds` (`SLSGetWindowBounds`) + `--experimental-window-bounds`
  to read any window's on-screen frame (tiled or floating).
- **Verified live on the remote (macOS 26.5.1):**
  - Floating window 834: `fn`+drag (300,300)→(500,450) moved it (65,43)→(265,193)
    (exactly +200,+150, size preserved) and it **persisted**; a second fn+drag moved
    it +100,+50 more.
  - Modifier gating: with `mouse_modifier cmd`, a fn+drag left 834 unchanged; back at
    `fn` it moved again.
  - Tiled window 821: fn+drag left its bounds unchanged (snap-back), as designed.
- Verification: `cargo fmt --all`; `cargo test --workspace` (159 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-07-03 (session 40) — mouse-drag config model (`mouse_modifier`/`mouse_action1`/`mouse_action2`/`mouse_drop_action`)

- Added the config model + parser for the four mouse-drag settings, previously
  unparsed: `mouse_modifier` (`alt`/`shift`/`cmd`/`ctrl`/`fn`, default `fn`),
  `mouse_action1` / `mouse_action2` (`move`/`resize`, defaults `move`/`resize`), and
  `mouse_drop_action` (`swap`/`stack`, default `swap`). New `yabai-core` enums
  `MouseModifier` / `MouseAction` / `MouseDropAction`, `ConfigValue`/`ValueKind`
  variants, key→kind mapping, value parsing (bad values report the C-faithful
  "unknown value … for domain 'config'"), and `Config` fields + get/set + defaults.
  Unit-tested (`config_mouse_settings_parse`).
- **Behavior deferred (next step):** the actual mouse-drag *move* — an
  input-consuming `CGEventTap` (`kCGEventLeftMouseDown/Dragged/Up`, consuming the
  click only while the armed modifier is held, like the C `mouse_handler`), a drag
  state machine (capture the window + frame under the cursor on down, move it live
  via AX on drag, finalize on up), starting with `mouse_action1 = move` and
  deferring resize / drop actions (swap/stack/warp + BSP-grid resize). This is a
  larger, higher-risk piece (an active tap can consume input) and is kept separate
  from the listen-only `focus_follows_mouse` tap (session 38) to avoid regressing it.
  Plan: a second active tap in `yabai_macos::mouse`, an armed-modifier `AtomicU8`
  set by the daemon from `config.mouse_modifier`, `WmWork::MouseDown/Dragged/Up`,
  and verify via synthesized down/drag/up events (`CGEventSetFlags` for the
  modifier) checking the window frame moved.
- Verification: `cargo fmt --all`; `cargo test --workspace` (159 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### Earlier sessions 1–39 (condensed changelog)

One line per session, newest first. Full detail for any entry is recoverable from
this file's git history; the durable facts they established live in "Current
status", "RESUME HERE", "Hard rules / gotchas", and "Faithful-port notes".

- **session 39 (07-03)** — `window_opacity on/off` auto active/normal opacity on focus change via SA; verified live.
- **session 38 (07-03)** — `focus_follows_mouse` (autofocus/autoraise) via a mouse-moved `CGEventTap` in `yabai_macos::mouse`; verified live.
- **session 37 (07-03)** — window `--sub-layer` / `--toggle sticky` / `--toggle shadow` via the SA; verified live.
- **session 36 (07-03)** — `space --switch` via SA `focus_space` (same-display) + content-swap (cross-display); verified live.
- **session 35 (07-03)** — cross-display `space --swap` content swap via the macOS-26-safe `windows_on_space`; verified live.
- **session 34 (07-03)** — fixed the macOS-26 window→space mis-assignment: enumerate `windows_on_space` (`SLSCopyWindowsWithOptionsAndTags`) instead of `SLSCopySpacesForWindows`.
- **session 33 (07-03)** — same-display `space --swap` via `move_space_after_space` (cross-display swap deferred, done in 35); verified live.
- **session 32 (07-03)** — `space --move` (intra-display reorder) via SA; verified live.
- **session 31 (07-03)** — `space --display` (cross-display space move) via SA; verified live on two displays.
- **session 30 (07-03)** — `window --display` via SA `move_window_to_space`; verified live cross-display.
- **session 29 (06-27)** — `space --focus` uses SA `focus_space` first (gesture fallback); this fork's SA installed on the remote.
- **session 28 (06-27)** — `window --opacity <float>` via SA `set_opacity`; verified live (`SLSGetWindowAlpha` readback).
- **session 27 (06-27)** — SA `space --create/--destroy` + `window --space` wired; first real macOS space create/destroy; verified live.
- **session 26 (06-27)** — Phase 7 start: `yabai-sa` runtime client (framing/handshake/opcodes); verified against the real payload.
- **session 25 (06-27)** — `mouse` selector (window/space/display under cursor).
- **session 24 (06-27)** — `display --label`.
- **session 23 (06-27)** — `recent` selector (window + space).
- **session 22 (06-27)** — space labels.
- **session 21b (06-27)** — space `display` query property.
- **session 21 (06-27)** — more `query --windows` properties.
- **session 20 (06-26)** — system/display/dock/menu-bar signals (`system_woke`, `display_changed`, `dock_did_change_pref`, `menu_bar_hidden_changed`).
- **session 19 (06-26)** — `application_front_switched` signal.
- **session 18 (06-26)** — backport audit of C `master` fixes.
- **session 17 (06-26)** — `application_activated/deactivated/hidden/visible` signals; fixed NSWorkspace delivery (`NSApplicationLoad` + AppKit run loop on main thread); verified from a `gui/501` LaunchAgent.
- **session 16 (06-25)** — `window_moved`/`window_resized` signals.
- **session 15 (06-25)** — `window_title_changed` signal.
- **session 14 (06-25)** — `window_minimized`/`window_deminimized` signals.
- **session 13 (06-25)** — window lifecycle signals (`window_created`/`window_destroyed`/`window_focused`).
- **session 12 (06-25)** — signal `app`/`title` regex filters (incl. `!=`).
- **session 11 (06-25)** — `rule` domain `manage` effect (float/untile, retile).
- **session 10 (06-25)** — `mouse_follows_focus` cursor centering on focus.
- **session 9 (06-25)** — `signal --add/--list/--remove` model + execution.
- **session 8 (06-25)** — `window --toggle native-fullscreen` via `AXFullScreen`.
- **session 7 (06-25)** — `window --close` via the AX close button.
- **session 6 (06-25)** — `window --deminimize` (numeric/`first`/`last`) from the minimized-window AX registry.
- **session 5 (06-25)** — cross-display `space --focus` (gesture).
- **session 4 (06-25)** — display hot-plug (poll/reconcile).
- **session 3 (06-25)** — multi-display: tile every display's current space at once.
- **session 2 (06-25)** — `window --minimize` (`AXMinimized`).
- **session (06-25)** — real `window --focus <selector>` enacted on the live window via AX.
- **session (06-24)** — WM daemon tracks the focused window (`AXFocusedWindowChanged` → pure core).
- **session 3 (06-23)** — Phase 2 pure core: ported the BSP layout tree from `src/view.c` into `yabai-core::layout` (arena + explicit `LayoutConfig`), plus resize/swap/fence and the `parser` module.
- **session 2 (06-23)** — wired `yabai-ipc` into a working client (framing, socket).
- **session (06-23)** — Phase 0/1: `docs/rust-rewrite-compat.md` compatibility contract + Cargo workspace skeleton.

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
    `mouse_drop_action`) via an active `CGEventTap`.

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
   (`AxSink::focus_window`), `--close`, `--warp`, `--toggle float`, `--toggle
   zoom-fullscreen`/`zoom-parent`, `--toggle native-fullscreen` (enter on the
   focused window; exit via id/`first`/`last`/single-window bare toggle),
   `--minimize`, `--deminimize` for numeric ids and `first`/`last`; `--swap`
   already worked. `window --opacity <float>` is now wired through the SA
   (`set_opacity` + `config.window_opacity_duration`), verified live via the
   `--experimental-window-alpha` (`SLSGetWindowAlpha`) readback. `window --display`
   (session 30), `--sub-layer below|normal|above|auto` (SA `set_layer`), `--toggle
   sticky` (SA `set_sticky` + untile/re-tile) and `--toggle shadow` (SA `set_shadow`)
   are all wired through the SA and verified live (session 37); the parser already
   produces `--sub-layer` as `WindowAction::Raw`, so no grammar change was needed
   (the C command is `--sub-layer`, not `--layer`). Still to do:
   remaining deminimize/native-fullscreen-exit selectors, and
   scratchpad. Mouse drag-to-**move** (`mouse_modifier` + left-drag),
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
   but unverified (needs `[NSApp run]`; see session 20). Still to do:
   `mission_control_enter`/`exit` (need SLS/private notifications).
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
  --all-targets && cargo test --workspace`. Currently 158 tests, clippy clean.
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
