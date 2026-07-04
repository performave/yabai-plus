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
  `opacity` effects and AX-backed `grid=` placement for new windows and
  `rule --apply`; remaining rule effects are parsed/stored but deferred. Per-space
  `space --gap`/`--padding` (abs/rel) are dispatched and
  survive reconciles; `window --grid` places a floating/unmanaged window on a grid.
  `window --raise`/`--lower` reorder a window's z-stacking (above/below an optional
  reference window) through the SA `order_window` opcode. `window --scratchpad`
  assigns/removes/recovers scratchpads and `window --toggle <label>` hides/shows
  them through the SA z-order opcodes.
  193 workspace tests pass. The shipped C `make` flow is unchanged.
- Last updated: 2026-07-04.
- User decisions captured:
  - The Rust rewrite may diverge permanently from upstream yabai. Rebaseability is no
    longer a primary constraint for this track.
  - Clean up edge cases and document breaking changes instead of preserving every
    bug-for-bug behavior.
  - For the scripting addition, use the most reliable engineering path rather than
    forcing literal Rust at the cost of fragile injection behavior.

## Progress log

### 2026-07-04 (session 73) — rule `grid=` effect via daemon boundary

- Extended the rule-effect daemon boundary beyond SA-only effects: matching
  `grid=r:c:x:y:w:h` now reuses the same `window --grid` frame computation and AX
  placement path for both newly discovered windows and `rule --apply`.
- Extracted a shared `window_grid_for_id` helper from `try_window_grid` so direct
  commands keep their existing errors while rule effects remain best-effort, like
  the C `window_manager_apply_rule_effects_to_window` caller.
- Renamed the pure collection test to
  `rule_apply_collects_daemon_boundary_effects` and included `grid=` in the
  collected effects handed from `AppState` to the daemon.
- **Verified live on the remote (macOS 26):** rebuilt/redeployed `/tmp/yabai-rust`,
  re-signed with `com.test.yabai`, refreshed the SSH-session Accessibility grant,
  and confirmed SA healthy. Isolated Finder daemon on `/tmp/yabai_rulegrid.socket`
  applied `rule --apply app=^Finder$ manage=off grid=1:2:0:0:1:1`; direct
  `--experimental-window-bounds` readback for Finder windows reported exact
  left-half frames (`55 33 707 923`). Restored with
  `rule --apply app=^Finder$ manage=on`, confirmed the daemon query returned 18
  tiled Finder windows, then stopped the isolated daemon.
- Verification: `cargo fmt --all`; targeted runtime/daemon tests;
  `cargo test --workspace` (193 tests); `cargo clippy --workspace --all-targets`
  (clean); `cargo build --release -p yabai`.

### 2026-07-04 (session 72) — SA-backed rule effects for sticky/layer/opacity

- Added a pure `AppliedRuleEffects` report path in `AppState` so `rule --apply`
  can keep regex matching and pure state changes in `yabai-runtime` while the
  daemon applies macOS effects at the boundary. Existing pure `rule --apply`
  behavior for `manage=` and `scratchpad=` remains unchanged.
- The daemon now applies rule `sticky=`, `sub-layer=`, and `opacity=` effects for
  both newly discovered windows and `rule --apply`. Rule SA effects are best-effort
  per C behavior: a failed sticky/layer/opacity SA call does not abort later rule
  effects or turn `rule --apply` into a command failure. Direct window commands
  still report SA failures.
- Added `rule_apply_collects_sa_backed_effects_for_daemon`, covering the effect
  collection handed to the daemon (`sticky`, `opacity`, `sub-layer`) without
  introducing macOS calls into `AppState` tests.
- **Verified live on the remote (macOS 26):** rebuilt/redeployed `/tmp/yabai-rust`,
  re-signed with `com.test.yabai`, SA healthy. Isolated daemon on
  `/tmp/yabai_rulefind.socket` applied `rule --apply op` with
  `opacity=0.61 sub-layer=above` to Finder; `--experimental-window-alpha 1395`
  reported `0.61`, then restore rule returned alpha to `1`. Isolated Calculator
  daemon on `/tmp/yabai_rulecalc.socket` applied combined
  `sticky=on opacity=0.61 sub-layer=above`; alpha read back `0.61`, daemon stayed
  responsive, then `sticky=off opacity=1.0 sub-layer=normal` restored alpha to `1`.
- Verification: `cargo fmt --all`; targeted runtime/daemon tests;
  `cargo test --workspace` (193 tests); `cargo clippy --workspace --all-targets`
  (clean); `cargo build --release -p yabai`.

### 2026-07-04 (session 71) — typed `window --sub-layer` daemon helper cleanup

- Finished the follow-up from session 67: the daemon SA helper for
  `window --sub-layer` now accepts the typed `Layer` enum directly instead of a
  string plus a second validation match. This keeps invalid values confined to the
  parser and leaves the macOS boundary consuming typed command data.
- Verification: `cargo fmt --all`; `cargo test -p yabai`; `cargo test --workspace`
  (192 tests); `cargo clippy --workspace --all-targets` (clean);
  `cargo build --release -p yabai`. No remote run: valid live sub-layer behavior
  was already verified in session 67, and this only removes redundant daemon-side
  string validation.

### 2026-07-04 (session 70) — pure rule `scratchpad=` effect application

- Extended rule application beyond `manage=` for one pure runtime-owned effect:
  `scratchpad=<label>`. Matching known windows are now assigned the scratchpad
  label through `AppState::set_window_scratchpad`, which also floats/untile them,
  matching the pure state side of C `window_manager_apply_rule_effects`.
- Renamed the internal rule-effect applicator from `apply_manage_effects_to_window`
  to `apply_rule_effects_to_window` and routed new-window rules, `rule --apply`,
  label-based applies, and ad-hoc applies through it. Duplicate scratchpad labels
  remain non-fatal, matching C's ignore-on-failure behavior.
- Added `rule_apply_enacts_scratchpad_for_known_windows` covering a matching Finder
  window becoming scratchpad/floating and leaving the tiled tree while a non-match
  remains tiled.
- Verification: `cargo fmt --all`; targeted rule tests; `cargo test --workspace`
  (192 tests); `cargo clippy --workspace --all-targets` (clean);
  `cargo build --release -p yabai`. No remote run: this is pure runtime state; the
  SA show/hide/move behavior for scratchpads was already verified in session 62.

### 2026-07-04 (session 69) — typed `window --scratchpad` command model

- Promoted `window --scratchpad [label|recover]` from the last raw window action
  into the typed Rust command model (`WindowAction::Scratchpad(ScratchpadAction)`).
  The parser now distinguishes bare removal, `recover`, and label assignment while
  leaving C-faithful label validation (reserved keywords, numeric labels, duplicate
  labels) in the daemon/runtime path where scratchpad state is available.
- Updated the daemon SA interceptor to consume the typed scratchpad action directly.
  Valid behavior is unchanged: assign floats/untiles, bare remove moves/orders/focuses
  and retiles, and `recover` orders all active AX-registered windows back in.
- Removed the last `WindowAction::Raw` variant from the window command model.
- Verification: `cargo fmt --all`; targeted scratchpad parser/runtime/daemon tests;
  `cargo test --workspace` (191 tests); `cargo clippy --workspace --all-targets`
  (clean); `cargo build --release -p yabai`. No new remote run: this is a typed
  parser/interceptor refactor of the scratchpad behavior verified live in session 62.

### 2026-07-04 (session 68) — typed `window --insert <dir>` command model

- Promoted `window --insert north|east|south|west|stack` from a raw string action
  into the typed Rust command model (`WindowAction::Insert(InsertDirection)`),
  reusing the existing layout enum. The parser now validates the closed direction
  set up front and reports the standard unknown-value parse error for bad values.
- Simplified `AppState` dispatch to consume the typed insert direction directly;
  valid behavior is unchanged (pure BSP insertion marker on the focused window's
  own space). Invalid-direction errors now fail at parse time instead of inside
  the runtime action arm.
- Verification: `cargo fmt --all`; targeted parser/runtime tests;
  `cargo test --workspace` (191 tests); `cargo clippy --workspace --all-targets`
  (clean); `cargo build --release -p yabai`. No new remote run: this is a pure
  parser/runtime refactor of the already live-verified insert behavior from
  session 54.

### 2026-07-04 (session 67) — typed `window --sub-layer` command model

- Promoted `window --sub-layer below|normal|above|auto` from a raw string action
  into the typed Rust command model (`WindowAction::SubLayer(Layer)`), reusing the
  existing `Layer` enum from rule effects. The parser now validates the closed
  value set up front and reports the standard unknown-value parse error for bad
  values.
- Simplified the daemon SA interceptor to consume the typed layer value via
  `Layer::as_str()`; valid live behavior is unchanged (`set_layer`, no reflow).
- **Verified live on the remote (macOS 26):** rebuilt/redeployed `/tmp/yabai-rust`,
  re-signed with the stable test identifier, SA healthy. Rust WM daemon on
  `/tmp/yabai_layer.socket`; focused Finder window `1404`, ran
  `window --sub-layer below` and `window --sub-layer normal`, then confirmed the
  daemon remained responsive with `query --windows` (`17` windows). Isolated daemon
  was stopped afterward.
- Verification: `cargo fmt --all`; targeted parser/daemon tests;
  `cargo test --workspace` (191 tests); `cargo clippy --workspace --all-targets`
  (clean); `cargo build --release -p yabai`.

### 2026-07-04 (session 66) — typed `window --opacity <float>` command model

- Promoted `window --opacity <float>` from a raw string action into the typed Rust
  command model (`WindowAction::Opacity(f32)`). The parser now validates the C
  range (`0.0..=1.0`) up front and reports the standard unknown-value parse error
  for malformed or out-of-range opacity values.
- Simplified the daemon SA interceptor to consume the typed opacity value directly;
  the live behavior is unchanged for valid commands (`set_opacity` with
  `config.window_opacity_duration`, no reflow).
- **Verified live on the remote (macOS 26):** rebuilt/redeployed `/tmp/yabai-rust`,
  re-signed with the stable test identifier, SA healthy. Rust WM daemon on
  `/tmp/yabai_opacity.socket`; Finder window `1395` ran `window --opacity 0.62`,
  and `--experimental-window-alpha 1395` reported `0.62`. Restored with
  `window --opacity 1.0` and read back alpha `1`. Isolated daemon was stopped
  afterward.
- Verification: `cargo fmt --all`; targeted parser/daemon tests;
  `cargo test --workspace` (191 tests); `cargo clippy --workspace --all-targets`
  (clean); `cargo build --release -p yabai`.

### 2026-07-04 (session 65) — trailing selectors for `window --close` / `--minimize`

- Added command-specific trailing selector support for `window --close <sel>` and
  `window --minimize <sel>` in the typed command model. `WindowAction::Close` and
  `WindowAction::Minimize` now carry optional selectors, matching the C
  `parse_window_selector(..., optional=true)` behavior for those commands while
  preserving the existing leading-target form (`window <sel> --close/--minimize`).
- Runtime dispatch now resolves a trailing close/minimize selector and makes it the
  acting/focused window before the daemon's macOS post-action runs. This lets the
  existing AX close-button and AX minimize paths operate on the selected window
  without adding a second macOS implementation path.
- **Verified live on the remote (macOS 26):** rebuilt/redeployed `/tmp/yabai-rust`,
  re-signed with the stable test identifier, SA healthy. Rust WM daemon on
  `/tmp/yabai_minimize.socket`; `window --minimize first` minimized Finder window
  `1386` (it left `query --windows`), then `window --deminimize first` restored it.
  Isolated daemon was stopped afterward. `window --close <sel>` was not live-tested
  to avoid destroying a Finder window; parser/runtime retargeting is covered by
  tests and uses the already-verified close post-action.
- Verification: `cargo fmt --all`; targeted parser/runtime tests;
  `cargo test --workspace` (191 tests); `cargo clippy --workspace --all-targets`
  (clean); `cargo build --release -p yabai`.

### 2026-07-04 (session 64) — typed parser support for `window --deminimize <sel>`

- Promoted the C-style trailing `window --deminimize <WINDOW_SEL>` shape into the
  typed Rust command model. `WindowAction::Deminimize` now carries an optional
  selector, and the parser accepts bare, leading-target (`window <sel>
  --deminimize`), and trailing-target (`window --deminimize <sel>`) forms. This
  closes the gap where the live daemon interceptor accepted the syntax, but the
  pure parser/golden layer still treated the trailing selector as an unknown
  command.
- Simplified the daemon restore interceptor to consume the typed `Message::Window`
  output: a command-specific trailing selector wins over the leading acting-window
  target, mirroring C's per-command selector override model.
- **Verified live on the remote (macOS 26):** rebuilt/redeployed `/tmp/yabai-rust`,
  re-signed with the stable test identifier, SA healthy. Rust WM daemon on
  `/tmp/yabai_parser.socket`; minimized Finder window `1377`, confirmed it left
  `query --windows`, then `window --deminimize first` restored it to the query.
  Isolated remote daemon was stopped afterward.
- Verification: `cargo fmt --all`; targeted parser/daemon tests;
  `cargo test --workspace` (191 tests); `cargo clippy --workspace --all-targets`
  (clean); `cargo build --release -p yabai`.

### 2026-07-04 (session 63) — C-style trailing selectors for deminimize + native-fullscreen exit

- Fixed a selector-grammar compatibility gap in the Rust WM daemon interceptors.
  `window --deminimize <sel>` now resolves the same numeric/`first`/`last`
  minimized-window registry selectors as the earlier leading-target form
  (`window <sel> --deminimize`). This matches the documented C command shape in
  `docs/rust-rewrite-compat.md`; unsupported selectors still fail explicitly
  instead of being guessed from unrelated live state.
- `window --toggle native-fullscreen <id|first|last>` now resolves registered
  fullscreen windows for the exit half, in addition to the existing leading-target
  form and bare single-fullscreen-window exit. Non-registered numeric ids still
  fall through as enter/normal command validation, preserving the prior behavior.
- **Verified live on the remote (macOS 26):** Rust WM daemon on
  `/tmp/yabai_selector.socket`, SA healthy. Minimized Finder window `1321`; it left
  `query --windows`, then `window --deminimize first` restored it. Entered native
  fullscreen for the same Finder window (`window 1321 --toggle native-fullscreen`);
  it left `query --windows`, then `window --toggle native-fullscreen 1321` restored
  it to the query. Isolated remote daemon was stopped afterward.
- Verification: `cargo fmt --all`; `cargo test -p yabai`; `cargo test --workspace`
  (191 tests); `cargo clippy --workspace --all-targets` (clean);
  `cargo build --release -p yabai`.

### 2026-07-04 (session 62) — `window --scratchpad` + `--toggle <label>` + verified live

- Implemented scratchpads, previously only parsed as a raw `WindowAction` and
  unhandled. The runtime now tracks unique `window_id -> scratchpad label` state,
  exposes `query --windows scratchpad`, and treats scratchpad windows as floating so
  reconcile keeps them out of BSP trees until the label is removed. A bare
  `window --scratchpad` is now parsed as removal (empty raw arg), matching C.
- **`window --scratchpad <label>`** validates C's reserved labels (`float`, `sticky`,
  `shadow`, `split`, `zoom-parent`, `zoom-fullscreen`, `windowed-fullscreen`,
  `native-fullscreen`, `expose`, `pip`, `recover`), rejects numeric labels, rejects
  duplicate labels with the C string, records the label, and floats/untiles the
  window. **Bare `window --scratchpad`** moves the window to the active space,
  orders it in, focuses it, clears the label, and retiles it. **`recover`** orders
  all active AX-registered windows in via the SA `order_window_in` opcode.
- **`window --toggle <label>`** now checks scratchpad labels before falling through to
  normal toggles: if the scratchpad is on the visible space and ordered in, it is
  hidden with SA `order_window(wid, 0, 0)`; if hidden, it is ordered back in and
  focused; if on another space, it is moved to the active space via
  `move_window_to_space`, ordered in, and focused. Added the `SLSWindowIsOrderedIn`
  read-only wrapper (`yabai_macos::space::window_is_ordered_in`) for the hide/show
  decision.
- **Verified live on the remote (macOS 26):** WM daemon on `/tmp/yabai_scratch.socket`,
  SA healthy. Assigned Finder window `1270` to scratchpad label `scratch`; it left
  the tiled query. `window --toggle scratch` hid it: `--experimental-windows-on-space
  1` changed from containing `1270` to omitting it. A second toggle restored `1270`
  to the space list. Bare `window 1270 --scratchpad` returned it to the tiled query
  with empty `scratchpad` and frame `65 500 692 446`. Duplicate assignment
  (`window 1225 --scratchpad scratch`) failed with
  `the given scratchpad is already assigned to a different window!` and exit 1.
- Verification: `cargo fmt --all`; `cargo test --workspace` (191 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-04 (session 61) — `window --toggle expose` (CoreDock) + `--toggle pip` (SA scale) + verified live

- Implemented the last two window `--toggle` variants, both previously parsed as
  `WindowAction::Toggle("expose"|"pip")` but unhandled (fell to `AppState`'s
  "window toggle '…' not yet handled" arm).
- **`--toggle pip`** — faithful to C `window_manager_toggle_window_pip`: scale the
  acting window into (or out of) a picture-in-picture miniature via the SA
  `scale_window` opcode (`SA_OPCODE_WINDOW_SCALE`, already in the client),
  targeting the usable bounds of the window's display inset by that display's
  active-space padding (the C `view_check_flag(dview, VIEW_ENABLE_PADDING)`
  branch, reusing `AppState::grid_insets`). The SA opcode itself self-toggles
  between the scaled and identity transforms, so **no daemon-side state is
  kept** — each call just sends `scale_window`. New `window_toggle_pip_via_sa`
  helper, dispatched as a `WindowAction::Toggle("pip")` arm inside
  `try_scripting_addition` next to sticky/shadow. Applies to any window, managed
  or not, like C.
- **`--toggle expose`** — faithful to C `window_manager_toggle_window_expose`:
  focus the acting window with a raise (`AxSink::focus_window`), then trigger App
  Exposé for its app via `CoreDockSendNotification(CFSTR("com.apple.expose.front.awake"), 0)`.
  New `yabai_macos::coredock` module (`CoreDockSendNotification` FFI from the
  already-linked `ApplicationServices`, plus a local CFString helper) and a
  standalone `try_window_expose` daemon interceptor slotted after
  `try_window_windowed_fullscreen`.
- New read-only probe `--experimental-window-transform <wid>`
  (`SLSGetWindowTransform`, `yabai_macos::space::window_transform`) — pip sets a
  scale transform invisible to the AX/CG frame, so bounds/alpha probes can't see
  it; this dumps `a b c d tx ty` to verify the opcode took effect.
- No parser/`AppState` change (the toggle argument is already a free string).
- Verified live on the remote (macOS 26): a tiled Finder window 1270 at
  `67 45 690 899`. `--toggle pip` moved its transform from the identity
  translation (`a=1 d=1 tx=-67 ty=-45`) to a scaled PIP (`a≈1.988 d≈1.989
  tx=-2209 ty=-89`, matching C's `do_window_scale` bottom-right ¼-size formula);
  a second toggle restored the identity transform exactly. `SLSGetWindowBounds`
  stayed `67 45 690 899` throughout (pure transform, invisible to AX). `--toggle
  expose` dispatched cleanly (rc=0, daemon stayed alive) through the known-good
  `focus_window` primitive + the CoreDock notification; its transient Mission
  Control animation isn't verifiable over SSH (no Screen Recording / GUI session).
  188 tests, clippy clean.

### 2026-07-04 (session 60) — `window --toggle windowed-fullscreen` (save/fill/restore via AX) + verified live

- Implemented `window --toggle windowed-fullscreen`, previously parsed as
  `WindowAction::Toggle("windowed-fullscreen")` but unhandled (it fell to `AppState`'s
  "window toggle '…' not yet handled" arm). Faithful to C
  `window_manager_toggle_window_windowed_fullscreen` (`window_manager.c`): entering
  saves the window's current frame and resizes it to fill the display's usable bounds
  (`display_bounds_constrained(did, true)` — menu bar / dock excluded, **no** yabai
  padding/gap); exiting restores the saved frame. Applied to **any** window, managed or
  not, exactly as C does (no managed rejection, unlike `--grid`/`--move`).
- **Daemon glue only** (no parser or `AppState` change): a new
  `try_window_windowed_fullscreen` interceptor in `crates/yabai/src/main.rs` sits in the
  window dispatch chain after `try_window_resize`. State is a new
  `windowed_frames: HashMap<u32, Area>` in the daemon loop — presence = the C
  `WINDOW_WINDOWED` flag, value = the saved `windowed_frame`. Toggle-on locates the
  window's display from its live frame center (same lookup as `try_window_grid`, but
  filling the raw visible bounds with no padding inset) and `AxSink::set_frame`s it,
  saving the prior frame; toggle-off `set_frame`s the saved frame and drops the entry
  (a failed restore keeps the entry so a retry can re-attempt).
- **Verified live on the remote (macOS 26):** WM daemon (gap/padding 10, 2 displays).
  Floated Finder window `1225` at `65 43 692 903` (display 1, visible frame `55 33
  1415 923`). `window 1225 --toggle windowed-fullscreen` → live bounds (read via
  `--experimental-window-bounds`, SLS) became `55 33 1415 923` (full display fill, no
  padding); a second toggle restored `65 43 692 903` exactly. Both exit 0.
- Remaining window `--toggle` gaps: `expose` (fires `com.apple.expose.front.awake` via
  CoreDock — low verifiability) and `pip` (SA `scale_window`); plus `--scratchpad`.
- Verification: `cargo fmt --all`; `cargo test --workspace` (188 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-04 (session 59) — `window --raise [sel]` / `--lower [sel]` via SA `order_window` + verified live

- Implemented `window --raise`/`--lower`, previously parsed but unhandled (they fell
  to the daemon's `_ => continue` and then `AppState`'s "not yet handled" arm). Faithful
  to C `window --raise`/`--lower` (`message.c`), which take an **optional** window
  selector and call `scripting_addition_order_window(acting_wid, ±1, reference_wid)` —
  a bare command orders the acting window above/below **everything** (reference id 0),
  a given selector orders it above/below that specific window.
- **Parser:** `WindowAction::Raise`/`Lower` now carry `Option<Selector>` (was a bare
  unit variant). The `--raise`/`--lower` arms peek for a trailing non-`--` token as the
  optional reference selector, exactly like `--focus` (a following `--command` is not
  consumed). New `window_raise_lower_optional_selector` parser test.
- **Daemon glue:** the SA client already exposed `order_window(a, order, b)`
  (`SA_OPCODE_WINDOW_ORDER = 0x10`), so the work was wiring — two new arms in
  `try_scripting_addition` route `Raise(sel)`/`Lower(sel)` to a new `window_order_via_sa`
  helper (`order` +1/-1), resolving the acting window and the optional reference (id 0
  when bare), with the C `daemon_fail` strings ("could not raise/lower window with id
  '…' due to an error with the scripting-addition."). Purely a z-order change, so no
  re-tile. Since the interceptor runs unconditionally, a missing SA surfaces as the same
  faithful error (the client fails to connect).
- **Note:** a numeric window selector passes through unvalidated (`resolve_window`
  `Selector::Index` returns the id as-is), so `--raise 999999` is a silent SA no-op —
  consistent with the rest of the port's explicit-id handling, close to C's
  `parse_window_selector` returning silently on an unfound window.
- **Verified live on the remote (macOS 26):** WM daemon (`all`, gap/padding 10), two
  floated overlapping Finder windows (1224/1225), z-order read via
  `--experimental-windows-on-space 1` (`SLSCopyWindowsWithOptionsAndTags`, front→back).
  - `window 1225 --lower 1224` → order `…1224, 1225…` (1225 sank behind 1224), exit 0;
    `window 1225 --raise 1224` → order `…1225, 1224…` (1225 rose above 1224), exit 0.
  - Bare `window 1224 --raise` moved 1224 to the front of the normal window level
    (higher-level system windows correctly stayed ahead); bare `--lower` sank it to the
    back — both exit 0.
- Verification: `cargo fmt --all`; `cargo test --workspace` (188 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-04 (session 58) — `window --resize handle:dw:dh` (managed fence + unmanaged AX) + verified live

- Implemented `window --resize`, previously incomplete: the pure `WindowAction::Resize`
  arm blindly fence-resized the **active** tree (wrong space for a window on another
  display), never rejected absolute resizing, and did nothing for unmanaged windows.
  Now faithful to C `window_manager_resize_window_relative`, which splits on whether
  the acting window is managed:
  - **Managed (tiled):** absolute (`abs:`) resizing is rejected with `cannot use
    absolute resizing on a managed window.` (C `WINDOW_OP_ERROR_INVALID_OPERATION`);
    a directional handle nudges the enclosing fence(s) on the window's **own** space
    (like `--ratio`), reporting `cannot locate a bsp node fence.` when the window is a
    lone root node.
  - **Unmanaged (float/untracked):** `abs:w:h` sets the AX size leaving the origin
    fixed; a directional handle grows/shrinks the frame from the dragged edge, with
    `top`/`left` also moving the origin so the opposite edge stays put — the exact
    arithmetic of `window_manager_resize_window_relative_internal`.
- **Pure core:** fixed the `WindowAction::Resize` arm in `app_state.rs` to resolve the
  focused window's own space (`window_space`), reject managed-`abs`, and surface the
  fence error. Unmanaged windows are a validated no-op in the pure model (the daemon
  owns the AX effect). Three new runtime tests: managed abs-reject + fence adjust,
  lone-root fence error, unmanaged pure no-op.
- **Daemon glue:** new `try_window_resize` interceptor (after `try_window_move`,
  before `try_space_focus`) mirrors `try_window_move`/`try_window_grid`. It owns the
  **unmanaged** AX path and the managed-`abs` rejection; a managed **directional**
  resize returns `None` so the pure fence math runs via the normal dispatch. The
  relative-frame arithmetic is inlined (trivial, exactly as C, no pure helper — same
  rationale as `--move`).
- **Verified live on the remote (macOS 26.5.x):** daemon gap 10 / padding 10.
  - Unmanaged (floated window 1183): `--resize abs:500:400` → origin fixed, size
    500×400 (`SLSGetWindowBounds` readback). From a reset 800,100 600×400:
    `left:60:0` → **860 100 540 400**, `right:120:0` → grows width origin-fixed,
    `top:0:80` → **800 180 600 320**, `bottom:0:50` → **800 100 600 450** — each
    matching the C formula exactly (positive `dw`/`dh` on a `left`/`top` handle
    shrinks from that edge; a settle delay is needed because AX applies size
    asynchronously, so an immediate readback lags one step).
  - Managed (tiled window 1182): `--resize abs:400:400` → exit 1,
    `cannot use absolute resizing on a managed window.`; `--resize right:100:0` →
    the fence moved, width 692→791, exit 0.
- Verification: `cargo fmt --all`; `cargo test --workspace` (187 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-03 (session 57) — `window --move abs|rel:dx:dy` (float/unmanaged) + verified live

- Implemented `window --move`, previously an unhandled `WindowAction::Move` (fell to
  the "window action not yet handled" arm). Mirrors C
  `window_manager_move_window_relative`: move targets an **unmanaged**
  (floating/untracked) window and repositions it, leaving its size unchanged; a
  **managed** (tiled) window is rejected with `cannot move a managed window.`
  (C `WINDOW_OP_ERROR_INVALID_SRC_VIEW`). `abs` sets the origin to `(dx, dy)`; `rel`
  offsets the current origin by `(dx, dy)`.
- **Daemon glue only** (no pure helper — the arithmetic is trivial abs/rel, exactly
  as the C command has no pure computation): new `try_window_move` interceptor
  (before the generic dispatch, right after `try_window_grid`) resolves the acting
  window, rejects it if it's in a layout tree (`window_space_id(wid).is_some()`),
  reads its live AX frame (`AxSink::window_frame`), computes the new origin, and
  applies it via `AxSink::set_frame` keeping `w`/`h`. The parser has produced
  `WindowAction::Move { kind, dx, dy }` since the Phase-2 grammar port, so no
  grammar change was needed.
- **Verified live on the remote (macOS 26.5.1):** daemon gap 10 / padding 10, a
  Finder grid on space 1. Focus + float window 1183 (tiled at 65,500 692×446), then:
  - `--move abs:200:150` → bounds **200 150 692 446** (`SLSGetWindowBounds` readback),
    size unchanged.
  - `--move rel:80:-40` → **280 110 692 446**.
  - `--move rel:-30:60` → **250 170 692 446**.
  - `window 1182 --move abs:0:0` on a still-**managed** window → exit 1,
    `cannot move a managed window.`
  - Verified `--toggle float` targeting: with focus on a *different* window (1182),
    `window 1183 --toggle float` floats **1183** (the selected target), then
    `--move` succeeds on it. This matches C: `dispatch_window` resolves a target
    selector and makes it the focused window before running the actions
    (`app_state.rs:1091`), so `window <sel> --toggle float` acts on `<sel>` — the
    same as C's `acting_window` (C `message.c:2076-2084`, which starts at the
    focused window and overrides with the parsed selector). Note `--toggle float`
    is idempotent: run it once per window (toggling twice re-tiles it), which is the
    only reason an earlier same-session double-toggle made `--move` transiently see
    a "managed" window.
- Verification: `cargo fmt --all`; `cargo test --workspace` (184 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-03 (session 56) — `window --grid r:c:x:y:w:h` (float/unmanaged) + verified live

- Implemented `window --grid`, previously an unhandled `WindowAction::Grid` (fell to
  the "window action not yet handled" arm). Mirrors C `window_manager_apply_grid`:
  grid targets an **unmanaged** (floating/untracked) window and places it in a
  `w`×`h` block of a `c`×`r` cell grid over its display's usable area; a **managed**
  (tiled) window is rejected with `cannot apply grid layout to a managed window.`
  (C `WINDOW_OP_ERROR_INVALID_SRC_VIEW`).
- **Pure core:** new `yabai_core::geometry::grid_frame(bounds, padding, gap, spec)`
  (`spec = [r,c,x,y,w,h]`). Clamps the spec into range, insets `bounds` (the
  display's usable frame, C `display_bounds_constrained`) by the space's padding and
  per-edge window gap, then measures the requested block back from the far edge so
  rounding accumulates away from the origin exactly as C does. One documented
  divergence: `r`/`c` are clamped to ≥1 (C uses `unsigned`, so `0` underflows).
  Three unit tests (no-inset cells, padding+gap insets, out-of-range/degenerate
  clamping).
- **Daemon glue:** `try_window_grid` interceptor (before the generic dispatch)
  resolves the acting window, rejects it if it's in a layout tree
  (`window_space_id(wid).is_some()`), finds its display from its live AX frame
  (`AxSink::window_frame` → center in `display_frames`), pulls that display's
  active-space padding/gap via the new `AppState::grid_insets(sid)` (per-space
  `space --gap`/`--padding` overrides else global config), computes the frame, and
  applies it with `AxSink::set_frame`.
- **Verified live on the remote (macOS 26.5.1):** daemon gap 0 / padding 0, display
  1 usable frame ≈ (55, 33, 1415, 923).
  - `window --focus 1151; --toggle float; --grid 1:2:0:0:1:1` (left half) →
    **(55, 33, 707, 923)**, exact (`SLSGetWindowBounds` readback).
  - `--grid 2:2:1:1:1:1` (bottom-right quarter) → **(762, 495, 707, 462)**, matching
    the computed (762.5, 494.5, 707.5, 461.5) modulo AX half-pixel rounding.
  - `window 1150 --grid 1:2:0:0:1:1` on a still-**managed** window → exit 1,
    `cannot apply grid layout to a managed window.`
- Verification: `cargo fmt --all`; `cargo test --workspace` (184 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-03 (session 55) — per-space `space --gap` / `space --padding` (abs/rel) + verified live

- Wired `space --gap type:gap` and `space --padding type:t:b:l:r` through
  `AppState::dispatch_space`, previously parsed but unhandled (the parser has
  understood both since the Phase-2 grammar port). Both act on the **selected (or
  active) space** itself — like `--label` — not the active-space tree the other
  space actions mutate.
- `set_space_gap` mirrors C `space_manager_set_gap_for_space`: `abs` sets, `rel`
  adjusts and clamps to zero (`add_and_clamp_to_zero`), stored on the tree's
  `LayoutConfig::gap`, then re-tiles. `set_space_padding` mirrors
  `space_manager_set_padding_for_space`: `abs` sets all four, `rel` adjusts each
  clamped-to-zero, then re-insets the root area and re-tiles.
- **Persistence across reconciles.** C keeps per-space gap/padding on the `view`,
  so it survives re-layout. Rust keeps the gap on the surviving `Tree` (the daemon
  reconciles via the guarded `add_space_to_display`, which never recreates an
  existing tree) and the padding in a new `space_paddings: HashMap<sid,[i32;4]>`
  consulted by `set_space_frame` on every reconcile — plus `space_usable:
  HashMap<sid,Area>` caching the last un-padded frame so a `--padding` change
  re-insets immediately without waiting for the next reconcile. `set_space_frame`
  now reads the per-space override via `space_padding(sid)` (override else global
  config), so global-config padding still applies to spaces without an override.
- Errors: both report the C strings `cannot set gap for a non-managed space.` /
  `cannot set padding for a non-managed space.` on a float space.
- Added runtime tests `space_gap_dispatch_sets_and_adjusts`,
  `space_padding_dispatch_reinsets_from_usable_frame`, and
  `space_gap_and_padding_error_on_float_space` (abs + rel + float error).
- **Verified live on the remote (macOS 26.5.1):** daemon up with baseline gap 10 /
  padding 10, a 2×2 Finder grid on space 1.
  - `space --gap abs:40` grew the inter-window gaps from ~11 to ~41px (columns
    65..742 | 783..1460), outer padding untouched (top-left still (65,43)).
  - `space --padding abs:80:80:80:80` moved the outer inset 10→80 on all edges:
    top-left window origin (65,43)→(135,113) (exactly +70,+70) and the right edge
    at 1390 = 1470−80.
  - `space --padding rel:-40:…` (→40): left inset 135→95 (=55+40), top 113→73.
    `space --gap rel:-30` (→10): rc 0.
  - On a `--layout float` space both commands returned the exact C error strings
    with exit 1.
- Verification: `cargo fmt --all`; `cargo test --workspace` (181 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-03 (session 54) — pure `window --insert` (north/east/south/west/stack) + verified live

- Implemented `window --insert <dir>`, previously an unhandled `Raw` action. Pure
  BSP op mirroring the C `window_manager_set_window_insertion`: marks the focused
  window's node as the pending insertion point so the *next* added window splits in
  the chosen direction (or stacks). New `yabai-core` `InsertDirection`
  (north/east/south/west/stack) + `Tree::set_window_insertion`, which sets the node's
  `split`/`child`/`insert_dir` and the view `insertion_point`, clears any prior
  marker on a different window, and toggles off when the same direction is
  re-selected. Direction→(split,child): N=(horizontal,first), E=(vertical,second),
  S=(horizontal,second), W=(vertical,first); stack marks `insert_dir=STACK`.
- The consumption side was already present (`pick_insertion_leaf` honors
  `insertion_point`, `resolve_split`/`split_node` honor the node's `split`/`child`);
  added the one missing piece — `add_window` now stacks onto the target leaf when its
  `insert_dir == STACK` instead of splitting (C `view_add_window_node` `do_stack`).
- `dispatch_window` handles `Raw { command: "--insert", arg }` on the focused
  window's own space, with the C error strings: `the acting window is not within a
  bsp space.` (non-BSP), `the acting window is not managed.` (untiled), and
  `value '<x>' is not a valid option for DIR_SEL` (bad direction).
- Added layout tests (directional placement E/W/N, stack join, toggle-off/unknown)
  and a runtime test (dispatch + east placement + bad-arg error).
- **Verified live on the remote (macOS 26.5.1):** focused window 902 (x769, full
  689-wide right column), `window --insert east`, then opened a new Finder window —
  it landed **east**: 902 shrank to x769 w338 and the new window took x1120 w338 (the
  east half), the rest of the layout untouched. Confirms the marker is set and
  honored by the daemon's new-window reconcile path (`assign_window_to_space` →
  `add_window`).
- Verification: `cargo fmt --all`; `cargo test --workspace` (178 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-03 (session 53) — `window --swap`/`--warp`/`--stack` own-space fix + verified live

- Fixed the latent active-vs-own-space bug (flagged in session 52) in
  `window --swap`, `--warp`, and `--stack`: all three used `active_tree_mut()`, so a
  focused window on a **non-active** display was operated on in the wrong (active)
  space's tree — a silent no-op. Added `AppState::window_tree_mut(window_id)`, which
  returns the tree of the space that actually contains the window (falling back to
  the active tree only when the window is untiled), mirroring the C
  `window_manager_find_managed_window`, and pointed the three ops at it. Matches the
  session-52 `--ratio` fix and the existing `resize_tiled_window` precedent.
- Added `window_swap_uses_focused_windows_own_space` (two windows on space 2 while
  space 1 is active; the swap must land on space 2).
- **Verified live on the remote (macOS 26.5.1):** with the active space on display 2
  (sid 92), `window --focus 901; window --swap 902` — both on display 1's non-active
  space — swapped their frames ((67,45)↔(769,45)). Before the fix this was a no-op
  (the swap hit the empty active space).
- Verification: `cargo fmt --all`; `cargo test --workspace` (174 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

### 2026-07-03 (session 52) — pure `window --ratio` (abs/rel) + own-space fix + verified live

- Implemented `window --ratio abs|rel:<f>`, previously an unhandled window action.
  Pure BSP op in `yabai-core`: `Tree::adjust_window_ratio(window_id, relative, ratio)`
  mirrors the C `window_manager_adjust_window_ratio` — sets the window's **parent**
  node ratio (`rel` adds, `abs` replaces), clamped to `[0.1, 0.9]`, then recomputes
  the parent subtree. `dispatch_window`'s `Ratio` arm maps `ValueType::Rel`→relative
  and reports the faithful errors `cannot adjust ratio of a non-managed window.`
  (window not in any tree) and `cannot adjust ratio of a root node.` (no parent).
- **Bug found + fixed during live testing:** the first cut used `active_tree_mut()`,
  but the C acts on the window's **own** view (`window_manager_find_managed_window`).
  On the remote the focused window was on display 1 while the active space was on
  display 2, so the active tree lacked the window and `--ratio` wrongly returned
  `cannot adjust ratio of a non-managed window.` (exit 1). Fixed to resolve the
  focused window's own space via `window_space` before adjusting; added
  `window_ratio_uses_focused_windows_own_space` (window on a non-active space) to
  lock it in. (The same latent active-vs-own-space issue in Swap/Warp/Stack was
  fixed in session 53 via the shared `window_tree_mut` helper.)
- Added `adjust_window_ratio_abs_and_rel` and `adjust_window_ratio_root_or_unknown_window_fails`
  (layout) plus `window_ratio_dispatches_and_errors_on_root` (runtime).
- **Verified live on the remote (macOS 26.5.1):** window 901 (top of a horizontal
  split with 1016) — `window --ratio abs:0.7` grew 901's height 266→620 (~0.7 of the
  ~886 shared height) and pushed 1016 down to 266; `abs:0.3` restored 266. The daemon
  re-tiled to screen, and the command applied even though 901's space was **not** the
  active space (display 2 was active) — proving the own-space fix.
- Verification: `cargo fmt --all`; `cargo test --workspace` (173 tests);
  `cargo clippy --workspace --all-targets` (clean); `cargo build --release -p yabai`.

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
    `manage=`, `scratchpad=`, `sticky=`, `sub-layer=`, `opacity=`, and `grid=`
    on new windows and `rule --apply`.

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
