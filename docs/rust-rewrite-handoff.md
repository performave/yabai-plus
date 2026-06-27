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
  attribute with a fullscreen AX registry mirroring minimize. The `signal` domain
  is modeled and executed: `signal --add/--list/--remove` plus live firing of
  `window_created`, `window_destroyed`, `window_focused`, `window_moved`,
  `window_resized`, `window_minimized`, `window_deminimized`,
  `window_title_changed`, `application_launched/terminated`, `space_changed`,
  `application_activated/deactivated/hidden/visible`, `application_front_switched`,
  `display_changed`, `display_added/removed`, `system_woke`,
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
  deferred. 149 workspace tests pass. The shipped C `make` flow is unchanged.
- Last updated: 2026-06-26.
- User decisions captured:
  - The Rust rewrite may diverge permanently from upstream yabai. Rebaseability is no
    longer a primary constraint for this track.
  - Clean up edge cases and document breaking changes instead of preserving every
    bug-for-bug behavior.
  - For the scripting addition, use the most reliable engineering path rather than
    forcing literal Rust at the cost of fragile injection behavior.

## Progress log

### 2026-06-27 (session 22) — space labels

- Implemented `space --label <name>` end-to-end in the pure runtime. `AppState`
  gained a `space_labels: HashMap<sid, String>`; `dispatch_space` intercepts
  `--label` and acts on the selected (or active) space. `set_space_label` mirrors
  the C `parse_label` rules: numeric labels and the reserved selector keywords
  (`prev`/`next`/`first`/`last`/`recent`/`mouse`) are rejected with the C error
  text, an empty label clears it, and labels are unique across spaces (assigning a
  name removes it from any other space). `resolve_space_selector` now resolves a
  `Selector::Label`, so `--space <label>` works for any space command/query. Added
  the `label` property to `query --spaces`.
- Live-verified through the WM daemon: `space --label coding` labeled the active
  space (sid 18); `query --spaces id,label` showed it; `query --spaces id --space
  coding` resolved the label selector to sid 18. Added a pure golden/behavior test
  (set, label query, uniqueness move, clear, numeric/reserved rejection).
- Verification: `cargo fmt --all`; `cargo test --workspace` (149 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-27 (session 21b) — space `display` query property

- Added the `display` property to `query --spaces` (arrangement index of the
  space's display, via `display_index`), and switched `query --spaces`
  `is-visible` to the per-display `space_is_visible` helper (a space can be visible
  on its own display without being the globally focused space). Added a golden test
  over a two-display state. 148 tests; fmt/clippy clean; release builds.

### 2026-06-27 (session 21) — more `query --windows` properties

- Extended the pure `query --windows` serializer with properties the `AppState`
  already owns: `space` (runtime sid), `display` (1-based arrangement index, via
  the new `pub AppState::display_index`), `is-visible` (new `space_is_visible`,
  mirroring the C `space_is_visible`), `split-type`/`split-child` (from the
  window's BSP node — parent split orientation and left/right child, matching the
  C `window.c` strings incl. the lone-root → `second_child` quirk), and
  `has-fullscreen-zoom`/`has-parent-zoom` (from `Tree::zoomed`). Added
  `NodeSplit::as_str` (`window_node_split_str`) in `yabai-core`.
- Scope notes / deliberate divergences (consistent with the rest of the Rust query
  layer): `space` emits the runtime sid, not the Mission Control index (same as
  numeric `--space` selectors); `display` emits the arrangement index, matching C.
  Not added: `is-floating`/`is-minimized`/`is-native-fullscreen` (floating,
  minimized, and native-fullscreen windows leave the BSP trees in the Rust model,
  so they never appear in `query --windows` — emitting an always-`false` value
  would imply tracking that does not exist); `role`/`subrole`/`level`/`layer`/
  `opacity`/`can-move`/etc. need live AX/SkyLight state.
- Added a pure golden test (`query_windows_serializes_space_display_and_tree_properties`)
  and live-verified against the WM daemon: a lone Finder window reported
  `space:18, display:1, is-visible:true, split-type:"none", split-child:"second_child"`.
- Verification: `cargo fmt --all`; `cargo test --workspace` (147 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-26 (session 20) — system/display/dock/menu-bar signals

- Wired the remaining notification-driven signal categories that mirror the C
  `workspace_context` setup, all context-free (no `YABAI_*` env vars, unfiltered —
  the C `event_signal_filter` never rejects them):
  - `display_changed` (`NSWorkspaceActiveDisplayDidChangeNotification`, also
    refreshes live display state) and `system_woke` (`NSWorkspaceDidWakeNotification`)
    on the NSWorkspace notification center;
  - `dock_did_restart` (`NSApplicationDockDidRestartNotification`) on the default
    `NSNotificationCenter`;
  - `menu_bar_hidden_changed` (`AppleInterfaceMenuBarHidingChangedNotification`)
    and `dock_did_change_pref` (`com.apple.dock.prefchanged`) on
    `NSDistributedNotificationCenter`.
  Added five `WorkspaceEvent` variants + callbacks + registrations in
  `yabai-macos::workspace`, and the daemon arms that fire each signal.
- Live verification (GUI `gui/501` LaunchAgent daemon): toggling "automatically
  hide menu bar" on/off via System Events fired `menu_bar_hidden_changed` exactly
  twice — confirming the **distributed** notification-center path (so
  `dock_did_change_pref`, same center, is covered by the same mechanism).
  `display_changed`/`system_woke` use the already-verified NSWorkspace center.
- Also fired `display_added` / `display_removed` from the existing display-topology
  poll in `refresh_live_display_state`: it now snapshots the known display ids
  before registering/removing, then fires `display_added` (with `YABAI_DISPLAY_ID`
  + `YABAI_DISPLAY_INDEX`) for newcomers and `display_removed` (`YABAI_DISPLAY_ID`)
  for vanished displays, mirroring `event_signal.c`. Exposed
  `AppState::display_index` (1-based arrangement index, matching `query --displays`).
  The diff fires exactly once per topology change (prior set is read fresh each
  poll, which already reflects the previous poll's registrations). Not yet
  live-verified — needs a physical monitor hot-plug (the tiling side of hot-plug
  was verified in session 4). `display_moved`/`display_resized` still need a
  CGDisplayReconfiguration callback (deferred).
- KNOWN GAP: `dock_did_restart` did **not** fire on `killall Dock`. It is an
  AppKit-internal notification posted to the local default center, and detecting
  the Dock restart appears to require the full `[NSApp run]` AppKit event loop;
  the Rust daemon runs a plain `CFRunLoopRun` on the main thread (which is enough
  for NSWorkspace + distributed notifications but not this AppKit-internal one).
  Left wired but documented as unverified — switching to `[NSApp run]` is risky
  (could disturb the now-working NSWorkspace delivery) and `dock_did_restart` is
  low value, so deferred.
- Verification: `cargo fmt --all`; `cargo test --workspace` (146 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-26 (session 19) — application_front_switched signal

- The Rust WM daemon now fires `application_front_switched` when the frontmost app
  changes, driven by the NSWorkspace activate notification. It exports
  `YABAI_PROCESS_ID` (the new front pid) and `YABAI_RECENT_PROCESS_ID` (the
  previous front pid), mirroring the C process manager's
  `g_process_manager.front_pid` / `last_front_pid`. The signal is unfiltered (the
  C `event_signal_filter` falls to its `default: return false`, i.e. never filters
  it out), matching the runtime's `_ => true` arm. Fired only on an actual front
  change (`front_pid != Some(pid)`), just before `application_activated`.
- Live verification on `ssh student@student` (GUI `gui/501` LaunchAgent daemon):
  activating Calculator → TextEdit → Finder produced
  `fsw:<calc>:recent:<calc>` (cold start, recent == self),
  `fsw:<te>:recent:<calc>`, `fsw:<finder>:recent:<te>`, with the recent pid
  correctly chaining each switch.
- Verification: `cargo fmt --all`; `cargo test --workspace` (146 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-26 (session 18) — backport audit of C `master` fixes

Audited the 9 commits on `master` (the C codebase) that postdate the rust-port
branch point (`53f53f8`) and assessed each against the Rust port:

- `1cd51ce fix: ignore zero-area windows when managing` — **PORTED.** The Rust
  `tileable_pid_windows` had the same bug (it tiled 0x0 AXStandardWindows, e.g.
  zoom.us's invisible window, reserving a phantom BSP slot). Added a
  `frame.w > 0 && frame.h > 0` gate in discovery, mirroring the C
  `window_manager_should_manage_window` zero-area check.
- `817edb0 fix(manage): respect disabled management at startup` — **N/A yet.** The
  C fix adds `read_config_manage_setting` to read `config manage off` from
  `yabairc` at startup. The experimental Rust daemon does not read a config file
  at all (gap/padding come from CLI args; `config` is set at runtime over the
  socket). Revisit when config-file loading lands. The same commit's
  `--toggle float` make-managed/make-floating split is superseded by `fda359b`.
- `658f356 fix(manage): keep managed windows tiled when moved between spaces` and
  `fda359b fix(manage): mark on-demand-tiled windows so layout switches keep them`
  — **N/A structurally.** These fix a C-specific bug: under `config manage off`, a
  window tiled on demand via `--toggle float` lacked `WINDOW_RULE_MANAGED`, so any
  path that *re-derives* tree membership through `window_manager_should_manage_window`
  (bsp<->stack layout switch, cross-space re-home) dropped it. The Rust port does
  not re-derive membership from a manage gate on rebuild — a window stays in its
  `Tree` until explicitly floated/removed/destroyed (`floating: HashSet`), and the
  config-change path only re-flows geometry. Cross-space moves aren't implemented
  yet (need the scripting addition). So the orphaning bug is not reachable in the
  current Rust model. Re-verify if/when membership ever becomes gate-derived on
  rebuild or manage-off on-demand tiling + layout switch lands.
- `a7f15a9 build(dev): swap canary in place to preserve Accessibility grant` —
  **build-flow only**, C `make dev`. Not Rust code, but the underlying lesson
  (replacing a binary changes its cdhash and invalidates a cdhash-bound TCC grant)
  is exactly the remote-testing friction hit in session 17; the Rust remote runbook
  now uses a fixed-identifier ad-hoc signature so the grant survives rebuilds.
- `d10e2bf`, `f80d7e9`, `0cb5d50` (release chores) and `00ba964` (docs) — no code.

- Verification: `cargo fmt --all`; `cargo test --workspace` (146 tests);
  `cargo clippy --workspace --all-targets`.

### 2026-06-26 (session 17) — application activated/deactivated/hidden/visible signals

- The Rust WM daemon now fires `application_activated`, `application_deactivated`,
  `application_hidden`, and `application_visible` signals. `yabai-macos::workspace`
  gained four new `WorkspaceEvent` variants and registers the matching NSWorkspace
  notifications (`NSWorkspaceDidActivate/Deactivate/Hide/UnhideApplicationNotification`)
  on the existing observer object/thread; each carries the pid and localized app
  name. The daemon fires the signal with `YABAI_PROCESS_ID` and app context,
  mirroring the C `event_signal.c` categories.
- Tracked the front (active) app pid from `application_activated` so the
  `application_hidden` category (and now `application_terminated`) can supply the
  `active` filter context (`front_pid == pid`), as the C event populates
  `es->active`. Previously terminated passed no `active`, so an `active=`-filtered
  terminate signal could never fire; the common (unfiltered) case is unchanged.
  The pure runtime already categorized all of these (activated/deactivated/visible
  = app filter only; hidden/terminated = app + active), so no runtime changes were
  needed — only the live firing.
- **Root-caused and fixed why NSWorkspace notifications never fired live** (this
  had been an open caveat since session 12 — application launch/terminate and
  active-space signals had *never* been verified live). Two missing pieces, both
  from the C daemon's `main`:
  1. `NSApplicationLoad()` — a non-bundled command-line tool must call this to
     establish the AppKit/Cocoa machinery; without it the NSWorkspace notification
     connection is never set up and notifications are silently dropped. Added
     `yabai-macos::workspace::ns_application_load()` (links AppKit) and call it once
     at WM-daemon startup on the main thread.
  2. The AppKit/CF run loop must run on the **main thread**. The C daemon runs
     `[NSApp run]` on main while its event loop runs on a worker pthread; the Rust
     daemon had it inverted (event loop on main, no run loop). Restructured
     `run_rust_wm_daemon`: the unified event-processing loop (owning
     `Runtime<AxSink>`) now runs on a dedicated worker thread, and the main thread
     runs `observe_workspace` (which blocks in `CFRunLoopRun`). `AxSink` ops stay
     single-threaded on that one worker thread — and AX-from-worker matches the C
     daemon, which also touches AX on its worker pthread, not main. Replaced
     `spawn_workspace_observer` with `start_workspace_bridge` (returns the sender;
     the caller runs `observe_workspace` on main).
- Live verification on `ssh student@student` (macOS 26.5.1), WM daemon launched as
  a `gui/501` LaunchAgent (required: NSWorkspace notifications are aqua-session
  scoped and never reach an SSH-launched daemon; aqua-session Accessibility is read
  from the root-owned **system** TCC.db). Registered all four new signals plus
  `application_launched`/`application_terminated` and an `app='^Chess$'`-filtered
  activation. Focus-switching Calculator/TextEdit wrote `deactivated`/`activated`
  pairs with correct pids; hiding/unhiding Calculator wrote `hidden`/`visible`;
  launching+quitting Chess wrote `launched`/`terminated`; the app filter wrote
  `chess-activated` only for Chess (not the Calculator/TextEdit activations). This
  is the first live confirmation of the entire NSWorkspace observer subsystem.
- TCC note for the remote runbook: aqua-session (GUI LaunchAgent) Accessibility
  needs a grant in `/Library/.../TCC.db` (root, sudo) whose `csreq` matches the
  binary. To avoid re-granting on every rebuild, ad-hoc sign with a fixed
  identifier (`codesign -fs - --identifier com.test.yabai`) and grant a
  `csreq` keyed to `identifier "com.test.yabai"`; then rebuilds only need
  re-signing, no sudo.
- Verification: `cargo fmt --all`; `cargo test --workspace` (146 tests, no new
  unit test — the change is in the macOS/daemon FFI + thread-structure layer, which
  is not unit-tested; the runtime filter categories were already covered);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 16) — moved/resized signals

- The Rust WM daemon now fires live `window_moved` and `window_resized` signals.
  The AX observer registers `AXMoved` and `AXResized` on existing windows and
  newly-created windows, forwarding typed `ObservedEvent` variants into the main
  daemon loop.
- Geometry signals are debounced against the runtime's expected tiled frame before
  reconciliation, using the C daemon's `AX_DIFF` threshold (`>= 1.5` points). This
  suppresses signals caused by the daemon's own corrective AX flushes while still
  reporting external user/app movement or resizing. Signal context includes
  `YABAI_WINDOW_ID`, app/title metadata, and active-window state for filter
  matching.
- Live remote verification on `ssh student@student` with the WM daemon on socket
  `/tmp/yabai_geom.socket`: registered Finder-filtered `window_moved` and
  `window_resized` actions. An AppleScript Finder bounds change fired
  `window_resized` for window `766`; a direct AX move by Finder pid/index fired
  `window_moved` for the same window. The test daemon, socket, logs, and deployed
  `/tmp/yabai-geom-1782414658` binary were cleaned up.
- Verification: `cargo fmt --all`; `cargo test --workspace` (146 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 15) — title-change signals

- The Rust WM daemon now fires live `window_title_changed` signals. The AX
  observer registers `AXTitleChanged` on existing windows and newly-created
  windows, forwarding a typed `ObservedEvent::WindowTitleChanged`; the existing
  reconciliation path then diffs the full AX window metadata and fires
  `SIGNAL_WINDOW_TITLE_CHANGED` only when an already-known window's title changes.
  New-window initial titles still fire only `window_created`, avoiding a false
  title-change signal during first discovery.
- Signal context matches the C event category for title changes: actions receive
  `YABAI_WINDOW_ID`, and `app`/`title`/`active=yes|no` filters evaluate against
  the updated title and the current focused-window state. Added a pure runtime
  regression covering `window_title_changed` app/title/active filtering.
- Live remote verification on `ssh student@student` with the WM daemon on socket
  `/tmp/yabai_title.socket`: registered a Finder-filtered
  `window_title_changed` signal with `title='^Documents$'`, created a temporary
  Finder window, changed its target to `Documents`, and `/tmp/title.log` recorded
  `title:1051`. The temporary window, daemon, socket, log files, and deployed
  `/tmp/yabai-title-1782413837` binary were cleaned up.
- Verification: `cargo fmt --all`; `cargo test --workspace` (145 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 14) — minimize/deminimize signals

- The Rust WM daemon now fires live `window_minimized` and `window_deminimized`
  signals when the corresponding command paths succeed. `window --minimize`
  caches the acting window's metadata before reconcile removes it from the tiled
  tree, then fires `SIGNAL_WINDOW_MINIMIZED` with `YABAI_WINDOW_ID`, app/title,
  and active context. `window <sel> --deminimize` fires after restoring the AX
  element and reconciling the app, using refreshed runtime metadata or the
  lifecycle signal registry as fallback context.
- Added a pure runtime regression in `signal_filters_match_c_event_categories` so
  `window_minimized` honors `active=yes|no` while `window_deminimized` follows the
  C category that filters by app/title only.
- Live remote verification on `ssh student@student` with the WM daemon on socket
  `/tmp/yabai_minsig.socket`: registered Finder-filtered minimize/deminimize
  signals, focused Finder window `1040`, ran `window --minimize` and confirmed it
  disappeared from `query --windows`, then ran `window 1040 --deminimize` and
  confirmed it returned. `/tmp/minsig.log` contained exactly `min:1040` and
  `demin:1040`. The test daemon and temp files were cleaned up, and the test
  Finder window was closed.
- Verification: `cargo fmt --all`; `cargo test --workspace` (145 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 13) — window lifecycle signals

- The Rust WM daemon now fires live `window_created` and `window_destroyed`
  signals. The important correctness change is a separate full-AX-window tracker:
  `yabai-macos::ax::pid_window_infos(pid)` discovers broad window identity and
  metadata without retaining elements for movement, and the daemon diffs that set
  separately from the tileable `managed` set. This avoids false lifecycle signals
  when a window leaves/re-enters the tiled layout because of minimize/deminimize
  or native fullscreen.
- `pid_window_infos` includes windows with a real CG id, settable position, or
  minimized state, and skips non-window AX entries such as Finder's desktop. It
  uses the same pid+AXWindows-index synthetic id fallback as tiling for windows
  whose CG id cannot be resolved. The layout path is unchanged: only
  `tileable_pid_windows` registers elements in `AxSink` and BSP trees.
- `reconcile_pid` now calls `sync_window_lifecycle_signals` before tileable
  reconciliation. New full-set ids fire `window_created` with
  `YABAI_WINDOW_ID` and app/title context; vanished ids fire `window_destroyed`
  with `YABAI_WINDOW_ID`, app context, and active-window context. `drop_pid` also
  emits `window_destroyed` for any tracked windows when an observed app exits.
- Live remote verification on `ssh student@student` with the WM daemon on socket
  `/tmp/yabai_life.socket`: after registering Finder-filtered lifecycle signals,
  creating a Finder window wrote `created:1028`; `window 1028 --minimize` and
  `window 1028 --deminimize` wrote no additional lifecycle lines; `window 1028
  --close` wrote `destroyed:1028`. Cleanup closed an older stray test window and
  wrote `destroyed:1024`, confirming the destroy path as well.
- Verification: `cargo fmt --all`; `cargo test --workspace` (145 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 12) — signal app/title filters

- The Rust `signal` runtime now honors `app`/`title` regex filters instead of only
  storing them. `yabai-core::Signal` preserves `app!=` / `title!=` exclusion
  flags; `yabai-runtime` compiles signal regexes at `signal --add` time with the
  C-style error text (`invalid regex pattern '...' for key '...'`) and evaluates
  them by the same event categories as `event_signal_filter` in
  `src/event_signal.c`. Events without app/title/active metadata still use the
  no-context path.
- The WM daemon passes focused-window metadata into `window_focused` signal
  dispatch, so live `signal --add event=window_focused app=... title=...` filters
  work against the `WindowMeta` captured during reconcile. `WorkspaceEvent` now
  carries the `NSRunningApplication.localizedName` for launch/terminate events so
  app-filtered application signals have the needed context when those
  notifications arrive.
- Live remote verification on `ssh student@student` with the WM daemon on isolated
  socket `/tmp/yabai_rsig.socket`: after the remote was unlocked, the daemon saw
  10 Finder windows. A focused-window signal with `app='^Finder$' title='MacBook
  Air'` fired on `window 766 --focus`; negative filters `app='^Safari$'` and
  `title='NoSuchTitle'` did not; exclusion filters `app!='^Safari$'
  title!='NoSuchTitle'` fired, while `app!='^Finder$'` and `title!='MacBook Air'`
  did not. An `env>/tmp/rsig-env` action confirmed `YABAI_WINDOW_ID=766` was
  exported. Application-launch filter verification was attempted with Calculator
  and TextEdit, but the SSH-launched daemon did not receive NSWorkspace launch
  notifications in that run even though TextEdit started; treat that as an
  observer/environment caveat, not a filter failure.
- Verification: `cargo fmt --all`; `cargo test --workspace` (145 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 11) — rule domain manage effects

- The Rust WM now handles the `rule` domain in `AppState`: `rule --add`,
  `--list`, `--remove`, and `--apply` are parsed and dispatched. `--add` supports
  `--one-shot`; `--apply` supports all rules, index/label selectors, and ad-hoc
  `key=value` filters/effects (`rule --apply app=... manage=...`). Rules compile
  their `app`/`title`/`role`/`subrole` filters with `regex-lite`, preserve C-style
  exclusion semantics, replace prior rules with the same label, and serialize in
  the C `rule --list` JSON shape including flags.
- The first live rule effect is wired: `manage=off` marks a matching window
  floating and removes it from BSP trees; `manage=on` clears that floating mark
  and reassigns it to its last-known space. `AppState` now tracks last-known
  window space separately from tree membership so `rule --apply ... manage=on`
  can retile a previously floated window without guessing from active space.
  Reconcile keeps floating windows out of trees on later ticks.
- One-shot rules now participate only in new-window application, then remove
  themselves after a match, mirroring the C `RULE_ONE_SHOT_REMOVE` cleanup. The
  live daemon applies rules only when a window is first seen; `rule --apply`
  re-evaluates known windows through the pure state layer. Unsupported live rule
  effects (`sticky`, `opacity`, `sub-layer`, `grid`, `display`/`space`,
  `native-fullscreen`, `scratchpad`, and per-window `mouse_follows_focus`) are
  parsed/stored/listed but not enacted yet.
- Live remote verification on macOS 26.2 with the WM daemon over Finder on
  isolated socket `/tmp/yabai_rtest.socket`: initial tiled window count was 10;
  `rule --add app='^Finder$' manage=off label=fin` then `rule --apply fin`
  reduced `query --windows id` count to 0; `rule --apply app='^Finder$'
  manage=on` restored count to 10. A one-shot `manage=off` rule floated only the
  next newly-created Finder window (count stayed 10) and `rule --list` returned
  `[]`; creating a second Finder window then increased the tiled count to 11,
  proving the one-shot had been removed. The two test Finder windows were closed
  afterward. Follow-up with the rebuilt binary verified the C label/ad-hoc
  precedence edge: a rule labeled `app=^Finder$` was preferred by `rule --apply
  app=^Finder$` (count 10 -> 0), then removing it allowed ad-hoc `rule --apply
  app=^Finder$ manage=on` to restore count 10.
- Verification: `cargo fmt --all`; `cargo test --workspace` (143 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 10) — mouse_follows_focus

- `mouse_follows_focus` now works in the Rust WM daemon, mirroring
  `window_manager_center_mouse`. New `center_mouse_on_focus` checks
  `config.mouse_follows_focus`, reads the live cursor, skips when the cursor is
  already inside the focused window's frame, and otherwise warps to the frame
  center. It runs on both focus paths (the AX observer and the command
  `window <sel> --focus` post-step), next to the focus signal/raise.
- macOS primitives added to `yabai-macos::display`: `cursor_location()`
  (`SLSGetCurrentCursorLocation` -> `Point`) and `warp_cursor_to_point(Point)`
  (`CGWarpMouseCursorPosition`); the existing FFI was reused. `AppState` exposes
  `window_area(window_id) -> Option<Area>` (the captured tiled frame) for the
  daemon. A diagnostic `--experimental-cursor-location` prints the cursor.
- `config mouse_follows_focus on|off` already round-tripped through the pure
  `Config`; this session just wires the effect. The config was already modeled.
- Live remote verification on macOS 26.2 (WM daemon over Finder, 11 windows, 2
  displays): with `mouse_follows_focus on`, `window 186 --focus` warped the cursor
  to `(3202, 732)` and `window 795 --focus` to `(2290, 618)` — both exact frame
  centers. Focusing a window the cursor was already inside left the cursor put
  (the contained-skip), and with `mouse_follows_focus off` focus no longer moved
  the cursor. Cursor read back via `--experimental-cursor-location`.
- Gotcha worth keeping: do NOT `scp` over the daemon's running binary path — macOS
  SIGKILLs (`Killed: 9`) subsequent execs when a running ad-hoc-signed file is
  overwritten. Deploy to a fresh path (or stop the daemon first).
- Verification: `cargo fmt --all`; `cargo test --workspace` (134 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 9) — signals

- The `signal` domain is now modeled and executed, ported from
  `src/event_signal.{h,c}` and `handle_domain_signal`. New pure
  `yabai-core::signal` holds `SignalEvent` (all `enum signal_type` variants, in
  order, with `as_str`/`from_name` mirroring `signal_type_str` /
  `signal_type_from_string`) and `Signal` + `Signal::from_key_values`, which
  reproduces the C `daemon_fail` validation text (unknown key, missing
  `event=..`/`action=..`, `active` value domain, `!` exclusion restriction).
- `AppState` owns a `Vec<Signal>` registry and `dispatch_signal`: `--add`
  (replacing a prior same-label signal, like `event_signal_add`), `--remove` by
  index (global, event-grouped order, matching `event_signal_remove_by_index`) or
  label, and `--list` serialized exactly like `event_signal_list` (event-grouped,
  globally indexed, `active` as `null`/`true`/`false`). `signal_actions_for(event)`
  returns the action commands the daemon should run.
- The WM daemon fires signals with the matching `YABAI_*` env vars via a new
  `fire_signals` (`/usr/bin/env sh -c <action>`, fire-and-forget, mirroring the C
  `fork`/`execvp`): `window_focused` (`YABAI_WINDOW_ID`),
  `application_launched`/`application_terminated` (`YABAI_PROCESS_ID`), and
  `space_changed` (`YABAI_SPACE_ID`). `window_focused` fires from both the AX
  observer and the command focus path, de-duplicated via a `last_focus_signal`
  tracker so a single real focus change fires once.
- Fixed a latent focus bug found while testing: `is_window_focus` only matched the
  `window --focus <sel>` form, so `window <id> --focus` (selector as the leading
  target) never enacted the real AX raise (or the focus signal). It now treats a
  window command with a `Focus` action plus either a target or a focus selector as
  a focus request, covering both grammar forms.
- Deferred (documented): `app`/`title` regex filters on signals (the C compiles
  POSIX `REG_EXTENDED`; adding a regex engine is a separate decision — fields are
  stored but not matched), and `window_created`/`window_destroyed` and the other
  events, because reconcile-based detection cannot yet distinguish a real
  create/destroy from minimize/fullscreen/space-move churn.
- Live remote verification on macOS 26.2: WM daemon over Finder on isolated
  socket; `signal --add event=window_focused action='echo focused:$YABAI_WINDOW_ID
  >> /tmp/sigfire.log'`, then `window <id> --focus` for three windows wrote
  `focused:767`, `focused:766`, `focused:54` — one line per focus change with the
  correct id exported. `signal --list`/`--remove` (label and index) plus the
  validation errors were verified locally through the dry-run daemon.
- Verification: `cargo fmt --all`; `cargo test --workspace` (133 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 8) — native fullscreen

- `window --toggle native-fullscreen` now works in the Rust WM daemon, mirroring
  `window_manager_toggle_window_native_fullscreen`. `AxSink` gained a `fullscreen`
  AX-element registry (parallel to the minimized one) plus `enter_native_fullscreen`
  / `exit_native_fullscreen`, which set the window's `AXFullScreen` attribute
  (string `"AXFullScreen"`, per `src/window.h`). Entering moves the element into the
  fullscreen registry so the reconcile that drops the now-untileable window from its
  tree does not release it; exiting moves it back and reconciles it into the layout.
- The toggle is split across the two layers like minimize/deminimize:
  - Enter: the pure core treats `--toggle native-fullscreen` as a validated no-op
    (`require_focused`); a daemon post-step focuses the window (the attribute is only
    honored on the key window, per C), sets `AXFullScreen`, records the pid, and
    reconciles so the window leaves the tiled layout for its own fullscreen space.
  - Exit: a fullscreen window has left every tree, so the pure core cannot act on
    it. `try_window_native_fullscreen_exit` intercepts the toggle, resolves the
    target against the fullscreen registry (numeric id / `first` / `last` / a bare
    toggle when exactly one window is fullscreen), clears `AXFullScreen`, and
    reconciles. A `was_fullscreen_exit` flag stops the enter post-step from
    re-firing on the same tokens. App termination drops any fullscreen-registry
    entries for that pid (like minimized).
- Deferred (need live state the pure/registry layers don't have): `next`/`prev`/
  direction/`recent`/`mouse`/label selectors for the exit half, and a bare-toggle
  exit when several windows are fullscreen (kept as an enter to stay unambiguous).
- Live remote verification on macOS 26.2: WM daemon on isolated socket
  `/tmp/yabai_rtest.socket` over Finder (10 windows, 2 displays). `window 780
  --focus` then `window --toggle native-fullscreen` removed id 780 from
  `query --windows` (10 → 9) and a `-D 1` screenshot showed a single Finder window
  filling the whole display with no menu bar or tiled siblings. `window 780
  --toggle native-fullscreen` returned 780 to the tiled query; toggling again
  re-entered fullscreen (correct toggle semantics), and a final toggle restored all
  10 windows.
- Verification: `cargo fmt --all`; `cargo test --workspace` (129 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 7) — window close

- `window --close` is now wired in the Rust WM daemon. `AppState` treats close as
  a validated no-op, like minimize, and the daemon then calls the new
  `AxSink::close_window`, which presses the window's `AXCloseButton` via
  `AXPress`, matching `window_manager_close_window` in the C daemon. After a
  successful close press the daemon reconciles the app immediately; the existing
  3s tick remains the backstop for delayed AX destroy notifications.
- Fixed a command-routing gap found while adding close: the pure runtime now
  applies a leading `window <selector>` as the acting window before dispatching
  actions. This makes `window 1 --close`, `window 1 --minimize`, and
  `window 1 --focus` target the selected window instead of ignoring the leading
  selector.
- Verification: `cargo fmt --all`; `cargo test --workspace` (127 tests);
  `cargo clippy --workspace --all-targets`.

### 2026-06-25 (session 4) — display hot-plug

- The Rust WM daemon no longer treats `display_frames` as a startup-only snapshot.
  New `refresh_live_display_state(&mut Runtime<AxSink>, &mut Vec<(u32, Area)>)`
  re-enumerates `active_displays`, recomputes per-display usable frames,
  registers/removes displays in `AppState`, refreshes every display's current
  space, and rebuilds the daemon's `display_frames` cache. The event loop calls it
  before every AX-observed event, active-space/app-launch workspace event, periodic
  tick, and socket message, so topology changes are picked up on the next daemon
  work item (at worst the 3s tick).
- Space refresh is now global instead of per-display destructive. The reconcile
  gathers every `spaces_for_display(did)` result into one live-space set before
  removing missing spaces, so a space that keeps the same sid but is rehomed onto
  another display preserves its BSP tree and just updates `space_displays` plus its
  frame. Transient `spaces_for_display` failures are treated as incomplete
  snapshots and do **not** delete existing spaces.
- `AppState::add_space_to_display` now preserves an existing space tree instead of
  replacing it; `remove_display` clears `display_active_space`; `remove_space`
  clears any display-active entry pointing at the removed sid. Added
  `space_ids()`/`display_ids()` helpers and tests for rehomed-space tree
  preservation plus display-removal cleanup.
- Live remote verification on macOS 26.2: deployed a release binary, probed two
  active displays (external 1920x1080 with spaces `[1, 12]`, built-in Retina with
  one display-local space), then ran `--experimental-rust-wm-daemon` in `all` mode
  on an isolated socket. `query --displays` returned both displays / all three
  spaces, and `query --windows` showed three Finder windows tiled across both
  displays. User-assisted physical hot-plug was also verified: with the daemon
  running, unplugging the external changed both the probe and daemon query from two
  displays to one (`display 1`, spaces `[1, 12]`), and replugging changed both back
  to two displays with the newly-created display-local space sid.
- Verification: `cargo fmt --all`; `cargo test --workspace` (122 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 6) — window deminimize

- `window <sel> --deminimize` now works in the Rust WM daemon for numeric ids plus
  `first`/`last` over the minimized-window registry. Minimized windows
  intentionally leave the pure BSP trees, so `AxSink` now keeps a separate
  minimized AX-element registry when `window --minimize` succeeds instead of
  dropping the element during reconcile. The daemon also records the minimized
  window's pid so deminimize can clear `AXMinimized` and reconcile that app back
  into the tiled layout.
- The pure layout model is unchanged: minimized windows are absent from
  `query --windows` and space window lists until AX reports them tileable again.
  Other deminimize selectors (`next`/`prev`/directions/recent/mouse/labels) are
  still deferred because minimized windows do not have enough live query state
  outside the sink registry yet.
- Live remote verification on macOS 26.2: started `--experimental-rust-wm-daemon`
  on isolated socket `/tmp/yabai_rtest.socket`, focused Finder window id 186 via
  the daemon, ran `window --minimize` and confirmed id 186 disappeared from
  `query --windows`, then ran `window 186 --deminimize` and confirmed id 186
  returned to the tiled query with focus. A repeated deminimize correctly reports
  `window with id '186' is not minimized.`
- Follow-up remote verification: minimized two Finder windows (`781`, `186`), ran
  `window first --deminimize` and confirmed `186` returned, then ran
  `window last --deminimize` and confirmed `781` returned.
- Verification: `cargo fmt --all`; `cargo test --workspace` (123 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 5) — cross-display space focus

- `space --focus <sel>` now handles cross-display focus in the Rust path. Added
  `yabai-macos` display primitives for cursor display lookup, cursor warp to a
  display center, and active-menu-bar display activation, plus `display_for_space`
  (`SLSCopyManagedDisplayForSpace` + `CGDisplayGetDisplayIDFromUUID`). The daemon
  and `--experimental-space-probe --focus` share `focus_space_by_gesture`, which
  mirrors the C fallback more closely: compute swipe steps from the target
  display's current space, warp the cursor before posting the dock-swipe gesture
  when the target is on another display, and activate that display afterward.
- Fixed two active-space bookkeeping issues found during live testing: the probe
  now uses the cursor display as its current space instead of always marking the
  first display, and the WM daemon seeds command-active space from the cursor
  display at startup (falling back to first display when the cursor cannot be
  resolved). After a daemon-handled `space --focus`, `AppState.active_space` is set
  to the target so later commands/query `has-focus` match the focused display.
- Live remote verification on two displays: `--experimental-space-probe --focus 1`
  and `--focus 3` moved current focus between the external display's sid 1 and the
  built-in display-local sid 71 with 0 swipe steps (display focus only). Through
  `--experimental-rust-wm-daemon`, query `--spaces id,has-focus,is-visible`
  started on sid 71 when the cursor was on the built-in display, then flipped to
  sid 1 after `space --focus 1`, then back to sid 71 after `space --focus 3`.
- Verification: `cargo fmt --all`; `cargo test --workspace` (122 tests);
  `cargo clippy --workspace --all-targets`; `cargo build --release -p yabai`.

### 2026-06-25 (session 3) — multi-display

- The WM daemon now tiles **every display at once**, not just the first. New
  `screen::visible_frame_for_display` resolves each display's usable frame
  (per-display menu bar/Dock insets) by walking `[NSScreen screens]` and matching
  `NSScreenNumber`. `AppState` gained `display_active_space: HashMap<did,sid>` and
  `flush_all_active(_to)` (flush every display's visible space; falls back to the
  single active space when unset, so the tile daemon still works). The daemon
  seeds spaces for all displays, refreshes/flushes all displays on every event
  (`refresh_all_display_spaces` / `refresh_all_active_spaces`), and routes windows
  to the display they're physically on via the existing `spaces_for_window`
  lookup. Focus moving to another display repoints the command-active space at the
  focused window's space.
- Verified live on two displays (external 1080p main + built-in Retina): each
  display tiled its own current space simultaneously — two windows split across
  the main display, one window filling the built-in display's usable frame (which
  correctly excluded that display's menu bar). Screenshots per display via
  `screencapture -D`.
- Known limitation: moving a window across displays *while the daemon runs* fights
  the daemon (it re-tiles before macOS reassigns the window's space); routing is
  correct when a window's space is discovered at startup/reconcile. A real
  `window --display` / `space --display` move needs the scripting addition
  (Phase 8). At this point display hot-plug was still deferred (fixed in session
  4). The `--experimental-space-probe` now prints every display's
  bounds/usable/current-space.

### 2026-06-25 (session 2)

- `window --minimize`: `AxSink::set_minimized` toggles `AXMinimized`; the daemon
  minimizes the focused window then `reconcile_pid`s its app. Key fix:
  `tileable_pid_windows` now also excludes minimized windows (`is_minimized` via
  `AXMinimized`) — on modern macOS a minimized window still reports `AXPosition`
  as settable, so without this it kept a phantom tree slot. `AppState::window_pid`
  getter added; the pure `Minimize` dispatch just validates a focused window.
  Verified live: minimizing a window sent it to the Dock and re-tiled the rest.
  `--deminimize` was deferred here; session 6 added the minimized AX registry for
  numeric-id restore. 120 workspace tests, clippy clean.
- `window --toggle float`: new `AppState.floating: HashSet<u32>`. Floating drops
  the window from its tree (others re-tile) and keeps it focused and put;
  unfloating re-tiles it into the active space. The key correctness piece is that
  `assign_window_to_space` no-ops for floating windows, so the daemon's
  reconcile/tick never re-adds them; `remove_window` clears the mark on destroy.
  Floating windows are absent from the tree and thus from `query --windows`
  (a deliberate simplification — the C daemon tracks them separately). Verified
  live: floating a window pulled it from the tree, re-tiled the rest, and it
  stayed floating across a reconcile tick; unfloating restored a clean 3-window
  BSP tile. 119 workspace tests, clippy clean.
- `window --toggle zoom-fullscreen` / `zoom-parent`: new `Tree::toggle_zoom` +
  `ZoomKind`. Zoom is a capture-time frame override (the window keeps its tree
  slot; `capture()` returns the root area for fullscreen, the parent-node area for
  zoom-parent, others stay tiled underneath), so toggling off restores the tiled
  frame with no reflow. Persists across flushes/reconciles until toggled off or
  the window leaves the tree. Wired into `dispatch_window::Toggle`; other toggles
  (float/sticky/split/shadow) still error. Verified live: zoom-fullscreen grew the
  focused window to fill the space and toggling off restored it exactly.
  117 workspace tests, clippy clean.
- `window --warp <selector>`: new `Tree::warp_window` removes the focused window
  and re-inserts it at the target window's node, restructuring the BSP tree (vs
  `swap_windows`, which only exchanges leaf contents). Wired into `dispatch_window`
  next to `--swap`. The C `:NaturalWarp` child-distance heuristic and cross-space
  warp stay deferred (need live geometry / multi-view). `window --swap` already
  worked (pure swap + reflush). Both verified live on the remote: `--warp west`
  moved the bottom-right window into the left column and grew the right window to
  full height; `--swap east` traded two windows' slots. 115 workspace tests,
  clippy clean.
- Remote gotcha worth remembering: `active_displays()` returns empty when the
  MacBook display has slept, so the WM daemon aborts with "no active displays
  found" — keep `caffeinate -d -u` running before launching it (added to the
  runbook mentally; see `REMOTE_TESTING.local.md`).

### 2026-06-25

- Real `window --focus <selector>`: the pure core already resolves the target
  into `focused_window`; the daemon now enacts it on the live window via a new
  `AxSink::focus_window` that mirrors `window_manager_focus_window_with_raise` —
  `_SLPSSetFrontProcessWithOptions` + the synthesized make-key-window event
  records (`SLPSPostEventRecordTo`, byte layout copied from `g_event_bytes`) +
  `AXRaise`. PSN comes from Carbon `GetProcessForPID` (deprecated but resolves at
  runtime), the same source as the C process manager. The daemon calls it after a
  successful `window --focus` (guarded by `is_window_focus`, which ignores a bare
  `--focus` with no selector). `AppState::focused_window_id` getter added.
  113 workspace tests, clippy clean.
- Verified live on the remote MacBook Air (macOS 26.2): the full tiling daemon
  arranges three real Finder windows in a BSP layout; `space --rotate 90` over the
  socket re-tiles them; `window --focus west` moves the real key window (traffic
  lights follow, `has-focus` flips). Screenshots captured via the runbook in
  `REMOTE_TESTING.local.md` (git-excluded).

### 2026-06-24

- WM daemon now tracks the focused window: `AXFocusedWindowChanged` feeds
  `StateEvent::WindowFocused` into the pure core, guarded so only a focus on the
  active space is recorded (`AppState::window_space_id` added).
- First `--space` mutation: `space --focus <selector>` works without the
  scripting addition. The daemon intercepts a lone `--focus` in `WmWork::Message`
  (`try_space_focus`), resolves the selector against the global
  mission-control-ordered space list (`yabai_macos::mission_control_spaces`), and
  switches with `switch_space_by_gesture` — the high-velocity dock-swipe synthesis
  the C uses as its SA-free fallback (`space_manager_focus_space_using_gesture`).
  Single-display only: no cross-display cursor warp yet. Selector resolution
  (`resolve_space_target`) mirrors `parse_space_selector` for
  index/first/last/prev/next (no wrap); `recent`/`mouse`/labels are reported as
  unsupported. `--switch`/`--move`/`--create`/`--destroy`/`--swap`/`--display`
  still need the scripting addition (Phase 8). 113 workspace tests, clippy clean.
- Added `--experimental-space-probe [--focus <sel>]`: dumps the global
  mission-control space list (1-based, marks current) and optionally switches via
  the gesture. Exercises the SkyLight discovery + gesture path with no AX, sockets
  or daemon.
- Verified live on a remote macOS 26.2 (Tahoe, arm64) box over SSH: the release
  binary's SkyLight discovery works from a non-Aqua SSH session, and the gesture
  actually switches the active space (`--focus 2` -> sid 12, `--focus prev` -> sid
  1, confirmed by re-querying `current_space`). Note: posting the gesture needs
  Accessibility permission (the C daemon requires it too); the test box had it
  granted. `screencapture` can't grab the framebuffer from an SSH session (not in
  the Aqua login session) without root (`launchctl asuser`) or a LaunchAgent, so
  no screenshot was taken — the before/after `current_space` query is the proof.

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
`all` mode, immediate app-termination cleanup, debounced `window_moved` /
`window_resized` signals, `space --rotate`/`--balance` over the socket, and
`query --windows id,app,title` returning real values.

Other experimental flags in `main.rs`: `--experimental-ax-{focused-window,debug,
windows-for-pid,pid-debug,move-focused,move-pid,tile-pid,observe-pid}`,
`--experimental-cursor-location` (prints the live cursor point),
`--experimental-rust-{daemon,tile-daemon}` (the tile-daemon is the older
snapshot-only `Actor<AxSink>` version; the wm-daemon supersedes it).

End-to-end today: a real dynamic tiling WM across **all displays**, driven
entirely by the pure core. It seeds real space ids for every display (each in its
own usable frame), tiles each display's current space simultaneously, and routes
discovered windows to the display/space they're physically on. Active-space
changes are notified through NSWorkspace; app launch/termination are notified
too; space add/remove is refreshed by polling before daemon work. Window ops:
focus (raise), close, swap, warp, minimize/deminimize, toggle
float/zoom/native-fullscreen; space focus (gesture) and rotate/balance/mirror/layout;
`signal` add/list/remove with live firing on focus/app/space/move/resize/minimize/
deminimize/title-change events and app/title filters for metadata-carrying events;
`mouse_follows_focus` cursor centering on focus.

### Do these next, in order (Phase 5/6 breadth — the big remaining work)

1. Multi-space + Mission Control: space discovery, startup per-space trees, and
   window-to-space assignment/routing now exist for the first display; space
   add/remove is refreshed by polling and active-space changes are notified.
   `space --focus <sel>` now works (gesture-based, including cross-display cursor
   warp/display activation). Still to do: `--switch`/`--move`/`--create`/
   `--destroy`/`--swap`/`--display` (need the scripting addition, Phase 8), and,
   later, SLS create/destroy notifications.
2. Multi-display: done — the daemon tiles every display's current space at once,
   each in its own usable frame, routing windows to the display they're on.
   Display hot-plug is handled by polling/reconcile before daemon work and on the
   3s tick (physically verified unplug/replug). Still to do: cross-display
   window/space moves (`window --display` / `space --display`, need the scripting
   addition).
3. App launch/termination are now observed directly through NSWorkspace; the 3s
   tick remains a backstop for missed AX/window changes and CGWindowList pickup.
4. More window ops needing live state: done — `window --focus` with-raise
   (`AxSink::focus_window`), `--close`, `--warp`, `--toggle float`, `--toggle
   zoom-fullscreen`/`zoom-parent`, `--toggle native-fullscreen` (enter on the
   focused window; exit via id/`first`/`last`/single-window bare toggle),
   `--minimize`, `--deminimize` for numeric ids and `first`/`last`; `--swap`
   already worked. Still to do: remaining deminimize/native-fullscreen-exit
   selectors, focus without-raise, sticky/scratchpad, opacity/layer (opacity/layer/
   sticky/shadow all need the scripting addition — `scripting_addition_set_*`);
   mouse drag move/resize/swap. `mouse_follows_focus` is done (cursor warps to the
   focused window's center on focus, with the contained-skip); `focus_follows_mouse`
   still needs a CGEventTap.
   Signals: mostly done — `signal --add/--list/--remove`, app/title regex filters
   (including `!=` exclusion), and live firing of `window_created`,
   `window_destroyed`, `window_focused`, `application_launched/terminated`,
   `space_changed`, `window_moved`, `window_resized`, `window_minimized`,
   `window_deminimized`, `window_title_changed`, `application_activated`,
   `application_deactivated`, `application_hidden`, `application_visible`, and
   `application_front_switched` (with `YABAI_*` env vars, incl.
   `YABAI_RECENT_PROCESS_ID`), plus the context-free `space_changed`,
   `display_changed`, `system_woke`, `menu_bar_hidden_changed`, and
   `dock_did_change_pref`, and `display_added`/`display_removed` (from the display
   poll diff, not yet hot-plug-verified). `dock_did_restart` is wired but
   unverified (likely needs `[NSApp run]`; see session 20). Still to do:
   `space_created`/`space_destroyed`, `display_moved`/`display_resized` (need a
   CGDisplayReconfiguration callback), and `mission_control_enter`/`exit` (need
   SLS/private notifications).
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
  never bind `/tmp/yabai_$USER.socket` from Rust. The WM-daemon live tests run
  with the C service stopped so nothing fights the tiling.
- Workspace lints deny `clippy::undocumented_unsafe_blocks` — every `unsafe`
  block needs a `// SAFETY:` comment. `cargo fmt` reorders `use` lists
  (types/fns interleaved alphabetically); let it, then match its output.
- Verify each step with `cargo fmt --all && cargo clippy --workspace
  --all-targets && cargo test --workspace`. Currently 145 tests, clippy clean.
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
  done), and `recent`/`mouse`/`stack[.N]`/label selector resolution.
