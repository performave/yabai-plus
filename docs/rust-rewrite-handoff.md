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
  plus live CoreGraphics display discovery and AX window diagnostics. 105
  workspace tests pass. The shipped C `make` flow is unchanged.
- Last updated: 2026-06-23.
- User decisions captured:
  - The Rust rewrite may diverge permanently from upstream yabai. Rebaseability is no
    longer a primary constraint for this track.
  - Clean up edge cases and document breaking changes instead of preserving every
    bug-for-bug behavior.
  - For the scripting addition, use the most reliable engineering path rather than
    forcing literal Rust at the cost of fragile injection behavior.

## Progress log

### 2026-06-23 (session 3)

- Started Phase 2 (pure Rust core) by porting the BSP layout tree from
  `src/view.c` into a new `yabai-core::layout` module. The C version threads
  raw parent/left/right pointers through heap nodes and reads policy from the
  global `g_space_manager`; the port stores nodes in an arena addressed by
  `NodeId` and lifts every global into an explicit `LayoutConfig`
  (`split_type`, `split_ratio`, `window_placement`, `auto_balance`,
  `insertion_policy`, `gap`), so the logic is deterministic and unit-testable.
- Ported, faithfully to the C reference: split policy
  (`window_node_get_split` / `_ratio` / `area_make_pair_for_node`), area
  recompute (`window_node_update`), leaf traversal
  (`find_{first,last,prev,next}_leaf`), `view_find_window_node`,
  `view_find_min_depth_leaf_node` (BFS), insertion
  (`view_add_window_node` + `window_node_split` + `view_stack_window_node`),
  removal (`view_remove_window_node`, including the sibling-collapse and
  intermediate-adoption cases), `rotate`, `mirror`, `equalize`,
  `window_node_balance`, and `view_find_window_node_in_direction`.
- Reused the existing `geometry::Area::split` for `area_make_pair`, so the
  C truncation behavior stays shared between the geometry and layout code.
- Added 11 layout tests (insert/split, ordering, sibling collapse, root clear,
  stack, rotate, equalize, auto-balance, directional neighbor). Whole-workspace
  suite is now 15 passing suites; `cargo clippy --workspace --all-targets` is
  clean and `cargo fmt --all` applied.

- Ported `window_node_fence` plus the tree halves of resize and swap:
  `Tree::fence`, `Tree::resize_window` (the `HANDLE_*` fence-and-clamp logic from
  `window_manager_resize_window_relative`), `Tree::swap_window_lists`, and
  `Tree::swap_windows` (the same-space subset of `window_manager_swap_window` —
  same-leaf slot swap, cross-leaf list swap, insertion-point follow). Added
  `HANDLE_{TOP,BOTTOM,LEFT,RIGHT}` constants mirroring `src/misc/macros.h`.
- Added a `yabai-core::parser` module: pure token classifiers ported from the
  `token_equals`/argument-keyword logic in `src/message.c`. `parse_selector`
  classifies the common selector set (`prev`/`next`/`first`/`last`/`recent`/
  `mouse`/`stack`/`stack.N`/directions/indices/labels) into a typed `Selector`;
  `parse_direction`, `parse_layout`, `parse_split_type`, `parse_auto_balance`,
  `parse_window_placement`, and `parse_insertion_policy` map config-argument
  keywords directly onto the layout enums. Selector *resolution* against live
  managers stays the daemon's job.
- `yabai-core` is now 27 tests (geometry + layout + parser); whole-workspace
  `cargo test`, `cargo clippy --workspace --all-targets`, and `cargo fmt --all`
  are clean.

Deliberately deferred (need live macOS/daemon state, not the pure layer):
- Zoom persistence (`window_zoom_persist`) — removal currently assumes no
  persisted zoom.
- Insert-feedback windows (`insert_feedback_show/destroy`).
- The z-order rank tie-break in `view_find_window_node_in_direction`; the pure
  layer breaks distance ties by smaller `NodeId` and documents the difference.
- The cross-space/focus halves of warp and swap (they call into the window and
  space managers and macOS focus APIs).
- The `:NaturalWarp` area heuristic in `window_manager_warp_window`.

- Added a `yabai-core::command` module: the start of the typed command model,
  ported from the domain handlers in `src/message.c`. `parse_domain` mirrors
  `handle_message`'s dispatch (and its `unknown domain '...'` failure);
  `parse_config` parses the `config` domain into a typed `ConfigCommand`
  (optional `--space <selector>` prefix plus a list of `ConfigOp::Get`/`Set`
  with typed `ConfigValue`s). A `ParseError` enum's `Display` reproduces the
  daemon's `unknown domain` / `unknown command` / `unknown value` message text
  verbatim for compatibility.
- The config grammar is faithful to C in a subtle way that has a golden test:
  each command unconditionally consumes the next token as its value, so a bare
  command only becomes a `Get` at end-of-input, and `config layout window_gap`
  fails with "unknown value 'window_gap' ..." exactly like the C handler.
- Scope note: `config` keys are curated to those whose grammar is fully
  determined by `yabai-core` enums plus the numeric/bool settings. Richer string
  settings (colors, easing, mouse-modifier strings, external bar) and the
  `display`/`space`/`window`/`query`/`rule`/`signal` domain bodies are not
  modeled yet. The Rust client (`crates/yabai`) still forwards raw tokens to the
  daemon; this typed model is for the future Rust daemon, not client-side
  validation. Verified the client still round-trips `query --spaces`/`--displays`
  against the live C daemon after these changes.
- Extended the typed command model to the `window` and `space` domains
  (`parse_window`/`parse_space`, with `WindowCommand`/`WindowAction` and
  `SpaceCommand`/`SpaceAction`). Both follow the C `[SELECTOR] --cmd [arg] ...`
  grammar: a leading non-`--` token is the target selector, then each `--cmd`
  consumes its argument(s). Modeled the colon-delimited argument formats
  faithfully — `--move type:dx:dy`, `--resize handle:dw:dh` (handles via the new
  `parse_resize_handle`), `--ratio type:r`, `--grid R:C:X:Y:W:H`,
  `--padding type:t:b:l:r`, `--gap type:gap` — plus `--rotate` (90/180/270) and
  the `x-axis`/`y-axis` -> `NodeSplit` mapping for `--mirror`/`--balance`/
  `--equalize` (no axis = both). Added `parse_value_type` and `HANDLE_ABS`.
- Errors stay faithful: `MissingValue` and the shared `UnknownValue`/
  `UnknownCommand` messages carry the right domain string, so e.g.
  `space --rotate 45` reports "unknown value '45' given to command '--rotate'
  for domain 'space'".
- `yabai-core` is now 53 tests (geometry + layout + parser + command for the
  config/window/space domains). Whole-workspace `cargo test`, `cargo clippy
  --workspace --all-targets`, and `cargo fmt --all` are clean; the Rust client
  still round-trips queries against the live C daemon.

- Completed the typed command model for all seven domains. Added
  `parse_display` (`DisplayCommand`/`DisplayAction`), `parse_query`
  (`QueryCommand` with `QueryTarget` + optional comma-separated property list +
  optional `--display`/`--space`/`--window` scope and selector), `parse_rule`
  (`RuleCommand::Add`/`Remove`/`Apply`/`List`), and `parse_signal`
  (`SignalCommand::Add`/`Remove`/`List`). Rule/signal `--add` collect
  `key=value`/`key!=value` pairs via the new `parser::parse_key_value`, which
  replicates the C left-to-right scan (so `a=b!=c` splits on the first `=`).
- Added a unifying `parse_message(tokens)` that dispatches on the leading domain
  token exactly like `handle_message`, returning a `Message` enum. New
  `ParseError` variants `MissingDomain` and `InvalidKeyValue` round out the
  faithful error text.
- `yabai-core` is now 61 tests (geometry + layout + parser + command across all
  seven domains). Whole-workspace `cargo test`, `cargo clippy --workspace
  --all-targets`, and `cargo fmt --all` are clean; the Rust client still
  round-trips queries against the live C daemon.

Scope notes / deferred: query property *names* are kept as raw strings (not yet
validated against `display`/`space`/`window` property tables); rule `--add`
keys/values are not yet validated or type-coerced (regex compile, grid format,
opacity range); selector *resolution* is still the daemon's job.

- Started Phase 4 by wiring the typed `Message` model and the layout `Tree`
  together in `yabai-runtime`:
  - `config::Config` is the mutable settings struct replacing the scattered
    `g_window_manager`/`g_space_manager` config fields. It applies a `ConfigOp`
    (set mutates, get returns the value formatted as the C daemon prints it —
    `on`/`off`, `bsp`, `auto_balance` `off`/`on`, etc.) and projects the
    layout-relevant subset into a `LayoutConfig` via `layout_config()`.
  - `app_state::AppState` owns the `Config`, a `sid -> Tree` map, the active
    space, and the focused window. `handle_tokens` parses with `parse_message`
    and dispatches: config ops mutate `Config` (and re-flow every space's tree
    when a layout-affecting key changes), `window --swap/--resize/--focus` and
    `space --balance/--equalize/--mirror/--rotate/--layout` mutate the active
    space's `Tree`. Domains/actions that need the macOS layers return an explicit
    "not yet handled" error instead of silently succeeding.
  - Selector resolution is limited to `Selector::Index` (concrete ids); anything
    needing live z-order returns a clear error. This keeps `AppState` pure and
    fully unit-testable (10 tests, incl. a gap-change reflow and a swap).
- Whole workspace is now 82 passing tests; `cargo clippy --workspace
  --all-targets` and `cargo fmt --all` are clean; the Rust client still
  round-trips queries against the live C daemon.

- Grew `AppState` toward the macOS boundary:
  - Added `Tree::capture` (`window_node_capture_windows`) returning a
    `WindowFrame { window_id, area }` per managed window, and `AppState::flush`/
    `flush_active` that expose those frames — the data the macOS layer turns
    into window-move operations.
  - Added `AppState::set_space_frame(sid, display_frame)`, which insets the
    display frame by the configured paddings (as the C view does when the space
    manager sets the root area) and re-flows the tree.
  - Replaced the Index-only window selector resolver with one that resolves
    against the active tree: numeric ids, `first`/`last`, `next`/`prev` (tree
    order, no wrap), and cardinal directions (top window of the neighbor node).
    `recent`/`mouse`/`stack[.N]`/labels still return an explicit error.
- Whole workspace is now 86 passing tests; `cargo clippy --workspace
  --all-targets` and `cargo fmt --all` clean; client still talks to the live C
  daemon.

- Added `StateEvent`, the typed, payload-carrying counterpart to the
  name-only `Event` list: `WindowCreated`/`WindowDestroyed`/`WindowFocused`
  (carry a window id), `SpaceCreated`/`DisplayFrameChanged` (carry sid + frame),
  and `SpaceChanged` (sid). `AppState::handle_event` applies them — gating
  `WindowCreated` on `config.manage` like the C daemon — so the macOS layer can
  drive state from real AX/SkyLight callbacks and then call `flush_active` to
  push the new layout. The full create→tile→focus→destroy→reflow loop is now
  exercised by a test with no daemon or macOS APIs.
- Whole workspace is now 89 passing tests; clippy/fmt clean; client still talks
  to the live C daemon.

- Added the `LayoutSink` trait — the seam between the pure control plane and
  macOS. `AppState::flush_active_to(sink)` and `handle_event_and_flush(event,
  sink)` push computed `WindowFrame`s through it; `yabai-macos` will implement
  it with AX/SkyLight moves, while `RecordingSink` (in-crate) records frames for
  tests. `AppState` now has no path that would require a macOS dependency.
- Added `runtime::Runtime<S: LayoutSink>`, which pairs `AppState` with a sink
  and re-flows the active layout after every mutation: `event(...)` applies a
  `StateEvent` then flushes, and `message(tokens)` handles a `-m` token list then
  flushes (even on a command error, so a partially-applied chain still settles
  its windows). This captures the C daemon's "handle then flush the view"
  discipline without threads yet — deterministic and unit-tested with
  `RecordingSink`.
- Added `actor::Actor<S>`, the single-threaded worker mirroring
  `src/event_loop.c`: it owns a `Runtime` on a spawned thread and takes work over
  a channel, so `post_event` (from the macOS layer) and `message` (from the
  socket loop, blocking for the response) are serialized strictly in order
  against one `AppState`, never concurrently. `shutdown()` returns the final
  `Runtime` so tests can inspect the recorded sink moves. Underlying `Runtime`
  stays synchronous.
- Whole workspace is now 95 passing tests; clippy/fmt clean; client still talks
  to the live C daemon.

The pure + concurrent control plane is now complete: `Actor` → `Runtime`
(flush-after-mutate) → `AppState` (Config + per-space `Tree`) → `LayoutSink`.
Everything from a token list or a `StateEvent` to a set of `WindowFrame`s runs
without macOS or a daemon.

- Started Phase 5 (macOS boundary) in `yabai-macos`: added `ax::AxSink`, the
  first real `LayoutSink`. It owns the window-id -> `AXUIElementRef` map the
  control plane lacks (`register`/`unregister` with an RAII `AxWindow` that
  `CFRelease`s on drop) and `move_window` sets `kAXPosition`/`kAXSize` via
  `AXValueCreate` + `AXUIElementSetAttributeValue`, releasing each value — a
  faithful port of `window_manager_move_window`/`_resize_window`. The minimal
  CoreFoundation/ApplicationServices FFI is declared locally (no new deps) and
  the crate links those frameworks; all `unsafe` is confined to this module with
  per-block SAFETY notes. `unsafe impl Send` is justified by the single-actor
  ownership invariant (matches the C daemon touching AX refs only on the worker
  thread). Two tests run without a display (unregistered move is a no-op;
  register/unregister tracking).
- Whole workspace is now 97 passing tests; clippy/fmt clean; builds and links on
  `aarch64-apple-darwin`; client still talks to the live C daemon.

- Added the first Rust query/read path in `yabai-runtime::AppState`. `Message::Query`
  now handles the state the pure runtime actually owns: `query --windows` for
  `id`, `frame`, and `has-focus`, plus `query --spaces` for `id`, `type`,
  `windows`, `first-window`, `last-window`, `has-focus`, and `is-visible`. The
  serializer deliberately emits C-style pretty JSON (`{\n\t...}` objects,
  comma-separated arrays, four-decimal frames) and has golden tests for windows
  and spaces. Unsupported properties such as `app`, and scopes requiring live
  display/window metadata, fail explicitly rather than inventing values.
- Scope note: numeric `--space` selectors currently resolve to the runtime space
  id, not Mission Control index. That is acceptable for the pure state layer but
  must be revisited when live display/space discovery lands.
- Whole workspace is now 100 passing tests; `cargo fmt --all`, `cargo test
  --workspace`, and `cargo clippy --workspace --all-targets` are clean.

- Extended the pure read path with display metadata owned by `AppState`: a
  `display_id -> frame` registry plus `space -> display` associations, fed by
  explicit methods and new `StateEvent::DisplayCreated` / `DisplayRemoved` /
  `SpaceCreatedOnDisplay` variants. `query --displays` now serializes `id`,
  `index`, `frame`, `spaces`, and `has-focus`; `query --spaces --display`,
  `query --windows --display`, and `query --displays --space` are answered from
  the same association map. Numeric display selectors follow C semantics by
  resolving as arrangement indices over the registered displays, while serialized
  `id` remains the display id.
- Scope note: pure display `spaces` currently emits runtime space ids, not
  Mission Control indices, for the same reason as the prior `--space` selector
  note. UUIDs/labels still require live macOS metadata and remain unsupported.
- Whole workspace is now 102 passing tests; `cargo fmt --all`, `cargo test
  --workspace`, and `cargo clippy --workspace --all-targets` are clean.

- Added a safe dry-run Rust daemon entry in `crates/yabai`: the new
  `--experimental-rust-daemon <socket>` flag binds only the caller-provided
  socket path (it never uses `/tmp/yabai_$USER.socket` implicitly), starts an
  `Actor<RecordingSink>`, decodes the existing C-compatible IPC framing, and
  returns actor responses with the same `FAILURE_MARKER` convention as the C
  daemon/client pair. This is intentionally not the real macOS daemon yet: it has
  no observers and no `AxSink`, but it exercises the socket -> actor -> response
  loop behind a non-conflicting flag. Added an in-process Unix-socket test that
  sends `query --windows id` through the daemon path and receives `[]\n`.
- Whole workspace is now 103 passing tests; `cargo fmt --all`, `cargo test
  --workspace`, and `cargo clippy --workspace --all-targets` are clean.

- Added live display discovery to `yabai-macos`: `display::active_displays()`
  wraps `CGGetActiveDisplayList` / `CGDisplayBounds` and returns display ids plus
  `Area` frames. The dry-run Rust daemon seeds `AppState` with these displays on
  startup, so `query --displays id,index,frame` now returns real display geometry
  through the Rust socket even before SkyLight space discovery exists. The
  workspace has a live-safe test that simply verifies the CoreGraphics call
  returns successfully.
- Live verification on this machine: stopped the running `/opt/homebrew/bin/yabai`
  with `yabai --stop-service`, started `target/debug/yabai
  --experimental-rust-daemon /tmp/yabai_rust_live.socket`, then queried it with
  `USER=rust_live target/debug/yabai -m query --displays id,index,frame`. The
  response reported one display (`id: 2`, `2560x1440`) via the Rust daemon. The
  experimental socket was removed afterward.
- Whole workspace is now 104 passing tests; `cargo fmt --all`, `cargo test
  --workspace`, and `cargo clippy --workspace --all-targets` are clean.

- Added AX discovery probes in `yabai-macos::ax`: trust checks with the same
  prompt option pattern as the C daemon, focused-window probing, PID-based
  `AXWindows` enumeration, and diagnostics for focused app / app-window counts /
  `_AXUIElementGetWindow` mapping. `crates/yabai` exposes these behind
  `--experimental-ax-focused-window`, `--experimental-ax-debug`,
  `--experimental-ax-windows-for-pid <pid>`, and
  `--experimental-ax-pid-debug <pid>`. A small build script adds the private
  framework search path for the SkyLight `_AXUIElementGetWindow` symbol.
- Live AX verification on this machine: Accessibility was granted and the probe
  reports `trusted=true`. The system-wide focused app/window attributes returned
  no value in this launch context, but PID diagnostics against Finder succeeded
  at the AX layer: `AXUIElementCreateApplication(527)` worked,
  `AXUIElementGetPid` returned `527`, and `AXWindows` returned 2 windows. The
  current unresolved issue is CG-id mapping: `_AXUIElementGetWindow` returned no
  ids for those Finder AX windows (`window_ids=[]`). This means the next live
  movement test should operate directly on retained AX elements first, then add a
  separate CGWindow/AX matching strategy for stable ids.
- Whole workspace is now 105 passing tests; `cargo fmt --all`, `cargo test
  --workspace`, and `cargo clippy --workspace --all-targets` are clean.
- Commit status: the AX diagnostics slice was committed as
  `feat(rust-port): add AX window diagnostics` (`1224cc9`); the earlier
  `1Password: failed to fill whole buffer` signing error did not recur.

- Proved the live window-move path end-to-end (Phase 5 boundary). Factored the
  `AxSink` position/size set into `ax::set_window_frame` and added
  `ax::read_window_frame` (reads `kAXPosition`/`kAXSize` back via the new
  `AXValueGetValue` FFI) so a move can be verified by reading the OS-accepted
  frame. Two new public movers operate **directly on retained AX elements**,
  deliberately bypassing the still-flaky `_AXUIElementGetWindow` CG-id mapping:
  - `ax::move_focused_window(area)` — resolves the system-wide `AXFocusedWindow`
    (then the focused app's `AXFocusedWindow`) and applies a frame.
  - `ax::move_pid_window(pid, index, area)` — retains the `index`-th `AXWindows`
    entry of an app and applies a frame regardless of CG-id resolvability.
  Exposed behind `--experimental-ax-move-focused <x> <y> <w> <h>` and
  `--experimental-ax-move-pid <pid> <index> <x> <y> <w> <h>`.
- Live verification on this machine (yabai service stopped, so nothing re-tiled):
  - `--experimental-ax-move-focused` failed with "no focused AX window could be
    resolved" — the same launch-context limitation noted for
    `--experimental-ax-debug` (system-wide focused attrs return nothing when the
    Rust binary is run from a non-GUI terminal context). Kept it for when the
    Rust daemon owns a real focus context.
  - `--experimental-ax-move-pid 527 0 200 150 800 500` (Finder) **succeeded**:
    the read-back reported exactly `x=200 y=150 w=800 h=500`, confirming the OS
    accepted the AX move. Index 1 is Finder's non-movable desktop window and
    stayed full-screen, as expected. Notably Finder's `window_ids` now resolved
    one CG id (`3582`) where the prior session saw `[]`.
- Whole workspace is still 105 passing tests; `cargo fmt --all`, `cargo test
  --workspace`, and `cargo clippy --workspace --all-targets` are clean. (The new
  movers need a live window, so they have no unit tests; the existing `AxSink`
  no-op/tracking tests still cover the sink.)

- **Tiled real windows with the full pure control plane** — the first time
  `Runtime -> AppState (BSP Tree) -> AxSink` ran end-to-end against live windows.
  Added `ax::tileable_pid_windows(pid)`: it discovers an app's windows and keeps
  only the ones whose `kAXPosition` is settable (new `AXUIElementIsAttributeSettable`
  FFI via `ax::is_position_settable`) with a readable frame — the precise "can the
  layout engine move this" test. This deliberately does **not** depend on the
  flaky `_AXUIElementGetWindow` CG-id mapping: ids are the CG id when it resolves,
  else a stable synthetic id (`0xF000_0000 | index`) so the sink and the tree
  agree on a handle. Exposed behind `--experimental-ax-tile-pid <pid> [gap]`,
  which builds a `Runtime<AxSink>`, seeds the main display + one space, sets a
  window gap, feeds each window as `StateEvent::WindowCreated`, and lets the
  flush-after-mutate discipline place them.
- Live verification on this machine: opened two extra Finder windows, then
  `--experimental-ax-tile-pid 527 16` tiled **3** real Finder windows in a correct
  BSP split with exact 16px gaps:
  `3826 -> (0,0,1272,1440)`, `3825 -> (1288,0,1272,712)`,
  `3582 -> (1288,728,1272,712)` where `1272 = (2560-16)/2` and `712 = (1440-16)/2`.
  Every placement decision was pure Rust (`yabai-core` layout); only discovery and
  the moves touched macOS. Note the AX window set is volatile (a stale probe saw
  only the non-settable desktop window); discover-and-tile in one shot is reliable.
- Made the tiling demo respect the usable display region. Added
  `screen::main_visible_frame()` (local `NSScreen`/`libobjc` FFI, AppKit linked)
  which returns the main display's `visibleFrame` — menu bar and Dock excluded —
  flipped from AppKit's bottom-left origin into top-left CG coordinates. Without
  it, windows tucked under the menu bar (macOS even clamps the top to y≈25) and
  behind the Dock. `--experimental-ax-tile-pid` now tiles inside that frame and
  also takes an optional `[padding]` arg: `window_gap` is the between-window gap
  only, the four `*_padding` settings are the outer screen-edge margin (same
  split as yabai). It sets all five via a `config` message and then
  `set_space_frame` to inset the root area by the paddings.
- Live verification: `--experimental-ax-tile-pid 527 5 5` tiled 5 Finder windows
  with a 5px outer margin and 5px inter-window gaps — first window at
  `x=61 y=30` (56px Dock + 5, 25px menu bar + 5) and the right edge at 2555
  (2560 − 5). Confirmed visually via screenshot; no menu-bar/Dock overlap.
- **Wired an `Actor<AxSink>` into a persistent Rust tiling daemon** (item 2 of
  the next-steps list below — partially done). New
  `--experimental-rust-tile-daemon <socket> <pid> [gap] [padding]`: it binds the
  caller-provided socket *first* (never `/tmp/yabai_$USER.socket`), discovers the
  display + visible frame + the app's tileable windows, registers each window's
  AX element in an `AxSink`, seeds an `AppState` (display, space, gap/padding,
  padding inset), spawns `Actor<AxSink>`, posts a `WindowCreated` per window to
  drive the initial tile, then serves the socket forever. `serve_one` is now
  generic over `Actor<S: LayoutSink>`, so the same socket loop backs both the
  dry-run (`RecordingSink`) and live (`AxSink`) daemons. `tile_config_tokens`
  is shared with `--experimental-ax-tile-pid`.
- Live verification on this machine (yabai service stopped): started the daemon
  on `/tmp/yabai_rustwm.socket` for Finder (pid 527); it tiled 5 windows and
  `USER=rustwm yabai -m query --windows id,frame,has-focus` returned the live
  tree state over the socket. Then `USER=rustwm yabai -m space --rotate 90`
  visibly rotated the real windows (left-column + right-split → top/bottom),
  `space --balance` re-balanced them, and `query --spaces id,windows,first-window`
  reflected the reordered list. The daemon shut down cleanly and removed nothing
  it shouldn't. This is the first persistent Rust daemon that serves live `-m`
  commands which move real windows.
- **Multi-app discovery** (item 2 below — done). Added
  `workspace::regular_application_pids()` (local `NSWorkspace`/`libobjc` FFI):
  the pids of regular, Dock-visible apps, filtered by
  `NSApplicationActivationPolicyRegular == 0`. `--experimental-rust-tile-daemon`
  now accepts `all` in place of a pid, collecting tileable windows across every
  regular app into one BSP tree (per-app discovery failures are skipped so one
  bad app can't abort the rest). Synthetic window ids now fold the pid in
  (`0x8000_0000 | (pid & 0x7FFF) << 16 | idx`) so they stay unique across apps.
- Live verification: `--experimental-rust-tile-daemon <sock> all 10 10` tiled
  **8** windows across Finder + RustRover into a single BSP layout with 10px
  gaps/padding inside the visible frame (no menu-bar/Dock overlap), confirmed via
  screenshot; `query --windows id` over the socket reported 8. Clean shutdown.
- Consolidated the duplicated objc FFI (`screen.rs`, `workspace.rs`) into a
  shared `objc.rs`: `class`/`sel` plus generic `msg0::<R>` / `msg1::<A,R>` that
  reinterpret `objc_msgSend` with the concrete method ABI (correct on arm64 for
  by-value struct returns like `NSRect`, no `_stret` needed).
- **AX observers** (item 1 below — started, the callback half). New `observe.rs`
  wraps `AXObserver` (create + add-notification + run-loop source) and turns each
  callback into a typed `ObservedEvent` (`WindowCreated`/`WindowDestroyed`/
  `FocusedWindowChanged`, each carrying pid + optional CG id). `observe_pid(pid,
  tx)` registers app-level created/focus + per-window destroyed, then blocks in
  `CFRunLoopRun`, sending events over an mpsc channel. A C `refcon` carries a
  shared `ObserverCtx` so the callback can register a destroy-watch on each newly
  created window. Diagnostic CLI `--experimental-ax-observe-pid <pid>`.
- Live verification (Finder): opening windows produced `WindowCreated` +
  `FocusedWindowChanged` reliably with resolved CG ids (e.g. 4253, 4254);
  focusing produced `FocusedWindowChanged`. IMPORTANT FINDING:
  `AXUIElementDestroyed` registered cleanly (rc=0) but macOS never delivered it on
  a Finder window close — pure-AX destroys are unreliable (yabai uses private
  SkyLight events for these). Documented in `observe.rs`; the daemon integration
  must reconcile the live `AXWindows` set on each event to catch closes.
- **Dynamic tiling WM daemon** (`--experimental-rust-wm-daemon <socket>
  <pid|all> [gap] [padding]`) — the capstone that closes the "snapshot" gap. A
  single-threaded event loop on the main thread owns `Runtime<AxSink>` (so all
  sink registration stays single-threaded — no `Actor` needed here) and consumes
  a unified `WmWork` channel fed by: (a) one `observe_pid` AX-observer thread per
  app, and (b) a socket-acceptor thread. On every observer event it calls
  `reconcile_pid`: re-discovers the app's tileable windows, registers newcomers
  in the sink + tree (`WindowCreated`), drops any that vanished (`WindowDestroyed`
  + `unregister`), and re-flows. Reconciliation makes closes robust *despite* the
  unreliable AX destroy notification — a `FocusedWindowChanged` (which does fire
  on close) triggers the reconcile that notices the missing window.
- Live verification (Finder): daemon started tiling 6 windows; opening a new
  Finder window auto-tiled it (`query --windows` count 6 → 7) with no manual
  command; closing a window auto-reconciled it out (7 → 6); `space --rotate 90`
  over the socket still rearranged windows (exit 0). Clean shutdown. This is the
  first Rust build that behaves like a real tiling WM: tile on startup, track the
  world live, and serve `-m` commands, all driven by the pure core.
- Whole workspace is still 105 passing tests; `cargo fmt --all`, `cargo test
  --workspace`, and `cargo clippy --workspace --all-targets` are clean.

- Added a periodic self-heal `Tick` (every 3s) to the WM daemon: it
  re-reconciles every observed app, a backstop for any window change an observer
  missed (notably the unreliable AX destroy). Factored observer spawning into
  `spawn_observer`. IMPORTANT FINDING: discovering apps *launched after startup*
  via `NSWorkspace.runningApplications` does **not** work from this event loop —
  the list is frozen at process-launch because it only refreshes with a pumped
  main `CFRunLoop`, which the channel-blocked event loop deliberately lacks.
  Verified live: launching Calculator under `all` never appeared (stayed 10 apps
  across 3 ticks). The fix is a `CGWindowListCopyWindowInfo`-based pid scan (it
  refreshes without a run loop) — documented in the `Tick` handler and deferred.
- **`CGWindowList` discovery** (`cgwindow.rs`) — fixes the new-app gap above.
  `CGWindowListCopyWindowInfo` reflects current on-screen windows on every call
  (no run loop needed) and reads only owner pid / window number / layer (never
  `kCGWindowName`, so no Screen Recording permission). `on_screen_windows()` ->
  `CgWindow { window_id, pid }` for normal (layer 0) windows;
  `application_pids_with_windows()` -> distinct owner pids. The WM daemon now uses
  it for `all`-mode initial discovery *and* in the tick, so apps launched after
  startup are observed and tiled.
- Live verification: started the WM daemon in `all` mode (3 apps with on-screen
  windows, 8 windows), launched Calculator, and within one 3s tick the count rose
  to 9 with Calculator tiled (its window moved to the computed slot, clamped to
  its fixed 198×350 size). Confirms live app pickup works.
- **Window metadata in queries** (`app`/`title`/`pid`). `AppState` gained a
  `window_meta: HashMap<u32, WindowMeta>` (pure: just stored + serialized);
  `query --windows` now accepts `pid`/`app`/`title` and emits them (with a new
  `json_escape` for arbitrary title text). `yabai-macos` reads them via a new
  `ax_string_attribute` (`CFStringGetCString`): `DiscoveredAxWindow` now carries
  `pid`/`app`/`title` (app = the app element's `AXTitle`, title = the window's
  `AXTitle`). The WM daemon calls `set_window_meta` during `reconcile_pid` (and
  `remove_window_meta` on destroy), refreshing every pass so titles stay current.
- Live verification: `query --windows id,app,title` against the WM daemon
  returned real values — `app:"Finder"`, `title:"DIRECT-67-HP LaserJet 250"` /
  `"Downloads"` per window. Added a `WindowMeta` serializer golden test.
- Whole workspace is now 106 passing tests; `cargo fmt`/`test`/`clippy` clean.

Next (rest of Phase 5): (1) Multi-display: the daemon currently tiles only the
first display's visible frame into one space. (2) Observe app *termination* to
drop a whole app's windows promptly (the tick reconcile already catches it within
3s as a backstop). (3) `space`/`window` selectors and focus operations that need
live z-order/SkyLight (`recent`, `mouse`, `stack[.N]`, label selectors).

### 2026-06-23 (session 2)

- Wired `yabai-ipc` into a working client. `send_message` connects to the daemon
  socket, frames the request with `encode_client_message`, shuts down the write
  side, and streams the response. The first response byte is checked against
  `FAILURE_MARKER` (`0x07`, from `src/misc/macros.h`); a failure response has the
  marker stripped and the remainder routed to stderr with a non-zero exit, exactly
  like the C client in `src/yabai.c`.
- Added two integration tests that exercise `send_message` against an in-process
  mock daemon (success stream and failure-marker routing).
- Replaced the `crates/yabai` `-m` stub with a real client path: it resolves
  `env USER`, builds the socket path, and delegates to `send_message`.
- Verified the Rust client against the live C daemon:
  - `yabai -m query --displays` streams JSON to stdout and exits 0.
  - `yabai -m bogus foo` prints `unknown domain 'bogus'` to stderr and exits 1.
- Reviewed the GPT-authored skeleton against the C reference and confirmed it is
  faithful: `yabai-core` geometry (`area_make_pair`, `area_is_in_direction`,
  `area_distance_in_direction`, `area_max_point`) matches `src/view.c`; the
  `yabai-runtime` `Event` list matches `src/event_loop.h` order-for-order; the SA
  opcodes/attributes match `docs/rust-rewrite-compat.md`.

Verified: `cargo fmt --all`, `cargo clippy --workspace --all-targets` (clean), and
`cargo test --workspace` all pass on `aarch64-apple-darwin`.

Next: expand `yabai-core` from geometry into the first layout tree model (Phase 2),
and add parser/JSON golden tests captured from the C daemon.

### 2026-06-23

- Added `docs/rust-rewrite-compat.md` as the starting compatibility contract for
  CLI, daemon IPC framing, message domains, query fields, rules, signals, and the
  scripting-addition socket protocol.
- Added a root Cargo workspace with these crates:
  - `crates/yabai`
  - `crates/yabai-core`
  - `crates/yabai-ipc`
  - `crates/yabai-macos`
  - `crates/yabai-osax-common`
  - `crates/yabai-osax-legacy`
  - `crates/yabai-runtime`
  - `crates/yabai-sa`
- Ported the first deterministic geometry helpers into `yabai-core`, including
  tests that mirror the existing C area tests.
- Added `yabai-ipc` helpers for the current `yabai -m` wire framing: native-endian
  32-bit length plus NUL-delimited tokens and final double-NUL terminator.
- Added `yabai-osax-common` constants for OSAX version, attributes, opcodes, and SA
  socket path formatting.
- Added `yabai-runtime::Event` names that mirror `src/event_loop.h`.
- Added `yabai-osax-legacy` as the explicit placeholder for the current ObjC/C OSAX
  artifact island.
- Updated `.gitignore` to ignore Cargo's `/target` directory.

Verified:

- `cargo fmt --all && cargo test --workspace` passes on host `aarch64-apple-darwin`.
- `cargo check --workspace --target x86_64-apple-darwin` passes after installing the
  target with `rustup target add x86_64-apple-darwin`.
- `make test` still passes for the existing C harness. It emits only the existing
  macOS 15 CoreVideo deprecation warnings.

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
  - `space.rs`: read-only SkyLight discovery for `current_space_for_display()`
    (`SLSManagedDisplayGetCurrentSpace`), `spaces_for_display()`
    (`SLSCopyManagedDisplaySpaces` + `id64` extraction), and
    `spaces_for_window()` (`SLSCopySpacesForWindows(..., 0x7, ...)` with the C
    fallback to the window display's current space).
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
`all` mode, immediate app-termination cleanup, `space --rotate`/`--balance` over
the socket, and `query --windows id,app,title` returning real values.

Other experimental flags in `main.rs`: `--experimental-ax-{focused-window,debug,
windows-for-pid,pid-debug,move-focused,move-pid,tile-pid,observe-pid}`,
`--experimental-rust-{daemon,tile-daemon}` (the tile-daemon is the older
snapshot-only `Actor<AxSink>` version; the wm-daemon supersedes it).

End-to-end today: a real dynamic tiling WM for the first display / active space,
driven entirely by the pure core. It seeds real space ids for the first display
and routes discovered windows to their reported spaces. Active-space changes are
notified through NSWorkspace; app launch/termination are notified too;
first-display space add/remove is still refreshed by polling before daemon work.
Single-display, active-space tiling only.

### Do these next, in order (Phase 5/6 breadth — the big remaining work)

1. Multi-space + Mission Control: space discovery, startup per-space trees, and
   window-to-space assignment/routing now exist for the first display; space
   add/remove is refreshed by polling and active-space changes are notified.
   Next implement `--space` focus/move/create/destroy commands and, later, SLS
   create/destroy notifications.
2. Multi-display: the WM daemon tiles only `active_displays().next()` today; add
   per-display spaces and route windows to the display they're on.
3. App launch/termination are now observed directly through NSWorkspace; the 3s
   tick remains a backstop for missed AX/window changes and CGWindowList pickup.
4. More window ops needing live state: focus (raise/without-raise), minimize/
   fullscreen/sticky/scratchpad, opacity/layer; mouse drag move/resize/swap;
   rules + signals execution.
5. Then Phases 7-9: scripting addition (`yabai-sa`, currently empty — required
   for space management / cross-space moves on modern macOS), OSAX spike, and
   production packaging (wire the Rust binary into `make`, signing, notarization,
   launchd, cutover). None started.

### Hard rules / gotchas (do not violate)

- Do NOT point Homebrew, launchd, `make dev`, or `/usr/local/bin/yabai` at
  `target/debug/yabai`. The Rust binary is a client + experimental daemons only;
  it is not wired into `make`/signing/launchd and must not replace the C daemon.
- The user runs the C yabai live. Read-only `query` via the Rust client is safe;
  never bind `/tmp/yabai_$USER.socket` from Rust. The WM-daemon live tests run
  with the C service stopped so nothing fights the tiling.
- Workspace lints deny `clippy::undocumented_unsafe_blocks` — every `unsafe`
  block needs a `// SAFETY:` comment. `cargo fmt` reorders `use` lists
  (types/fns interleaved alphabetically); let it, then match its output.
- Verify each step with `cargo fmt --all && cargo clippy --workspace
  --all-targets && cargo test --workspace`. Currently 110 tests, clippy clean.
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
  warp/swap, the `:NaturalWarp` heuristic, and `recent`/`mouse`/`stack[.N]`/
  label selector resolution.
