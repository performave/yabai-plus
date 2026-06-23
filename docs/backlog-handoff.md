# yabai-plus backlog — handoff tracker

This doc is the living, committed record of the yabai-plus backlog. It survives
across sessions and rebases. The plan file is just a working draft; **this file is
the source of truth for status**.

## How this doc works

- One row per backlog item in the table below; status is updated as work lands.
- One section per item further down, expanded with the detailed design/notes when
  that item is picked up, and with a "Landed" note (commit/behavior) when it ships.
- Update the table row and the item section in the same change that lands the work.

## Backlog tracker

| # | Item | Type | Status |
|---|------|------|--------|
| 1 | Config: disable floats-topmost + hybrid (no-manage-on-startup) | feat | **landed** |
| 2 | Add command to create/update the `--load-sa` sudoers file | feat | queued |
| 3 | Cross-display focus lands on sticky Arc instead of leaving focus | bug | queued (needs repro) |
| 4 | Switching to an empty space triggers show-desktop (always on Tahoe) | bug | queued (needs repro) |
| 5 | Are these caused by the Arc PiP fix? | diag | **answered (below)** |
| 6 | Zoom window refuses to resize despite space being allocated | bug | queued (needs repro) |

---

## Item 1 — `window_sublayer_auto` + `manage` config

Two independent boolean `yabai -m config` keys that replace the wildcard
`rule --add app=".*" manage=off sub-layer=normal` plus the python post-startup loop
in `yabairc`. Both default to **on** (current upstream behavior preserved).

- **`window_sublayer_auto on|off`** — when `off`, `LAYER_AUTO` resolves to
  `LAYER_NORMAL` for everything, so floats no longer sit above tiles.
  (`window_manager_set_window_layer`, `src/window_manager.c`.)
- **`manage on|off`** — when `off`, hybrid mode: a window is only tiled if a rule
  sets `manage=on` (`WINDOW_RULE_MANAGED`); everything else floats, including
  pre-existing windows at startup.

Both keys re-apply retroactively to existing windows when toggled at runtime, so a
restart is not required and the python loop is no longer needed.

After this lands, the yabairc simplifies to:

```sh
yabai -m config manage off                 # hybrid: only rule-managed windows tile
yabai -m config window_sublayer_auto off   # floats no longer forced above tiles
# delete the app=".*" wildcard rule and the python post-startup loop
# keep the targeted rules (CleanShot X, Adobe ... manage=on)
```

**Landed.** Implemented as two `struct window_manager` flags (`enable_window_sublayer_auto`,
`manage`), both defaulting to `true`:

- `window_manager_set_window_layer` resolves `LAYER_AUTO` to `LAYER_NORMAL` for
  everything when `enable_window_sublayer_auto` is off.
- **Plan correction:** the plan said `window_manager_adjust_layer` needed no change.
  That was wrong. `adjust_layer` is the dominant *real-time* path (focus/tile/mouse
  events in `event_loop.c`, `mouse_handler.c`, `space_manager.c`) and it passes its
  `layer` argument straight to the SA without consulting the flag, so the config had
  no runtime effect until `adjust_layer` was also gated (an `LAYER_BELOW` request is
  demoted to `LAYER_NORMAL` when the flag is off).
- `window_manager_should_manage_window` and the auto-float path in
  `window_manager_create_and_add_window` gate on `manage`; rule-managed
  (`WINDOW_RULE_MANAGED`) windows are always exempt.
- Runtime toggles re-apply to existing windows via
  `window_manager_set_window_sublayer_auto_enabled` and
  `window_manager_set_manage_enabled` (replaces the python loop).
- Wired in `src/message.c` (`window_sublayer_auto`, `manage`) and documented in
  `doc/yabai.asciidoc`. NOTE: `doc/yabai.1` was not regenerated here — `asciidoctor`
  was not installed; run `make man` on a machine that has it.

### Verification (canary, Tahoe)

- **1b `manage` — fully verified.** `manage off` retroactively floated 9/10 existing
  windows (the 10th, Spotify, reports an empty AX role → `window_is_real` false →
  ineligible, correctly left untouched); `manage on` retiled all 10. A new Finder
  window opened floating under `manage off`, and with a `manage=on` rule added it
  tiled instead — confirming rule-managed windows still tile in hybrid mode. Config
  set/read round-trips and bad values are rejected.
- **1a `window_sublayer_auto` — code correct, not visually confirmable on this setup.**
  On Tahoe the `query` `level`/`sub-level`/`sub-layer` fields read `0/normal` for all
  windows regardless of the SA's actual sub-layer ordering, so the effect can't be
  observed programmatically. The visual A/B (float over a focused tile) showed the
  focused tiled window on top in *both* ON and OFF states — i.e., focus-raise
  dominates and the BELOW-sublayer ordering produced no observable float-above-tile
  effect here, so toggling the flag made no visible difference. The code change is
  correct for the documented mechanism; **recommend re-checking against the specific
  windows that originally motivated the `sub-layer=normal` hack** (those may be
  sticky/PiP windows, which overlaps with Item 3). Possible that the Tahoe SA
  sub-level path is itself ineffective for ordinary windows — worth a follow-up.

**Status: landed (1b verified; 1a verified by construction, visual confirmation
pending on motivating windows).**

---

## Item 2 — SA sudoers command

Add `yabai --install-sudoers` / `--uninstall-sudoers` near the existing `--load-sa`
handling (`src/yabai.c`, impl in `src/sa.m`) that writes a sha256-pinned passwordless
rule to `/private/etc/sudoers.d/yabai` (root + `visudo -c` validation). Matches the
dev-loop rule referenced in `docs/debugging.md`. Self-contained.

**Status: queued.**

---

## Item 3 — cross-display focus lands on sticky Arc

`display_manager_focus_display` (`src/display_manager.c:465`) picks rank-1 (top
z-order) on the target space; a sticky Arc window qualifies and wins. Likely fix:
skip ineligible/sticky windows when selecting the focus target across displays.
Needs a live repro first.

**Status: queued (needs repro).**

---

## Item 4 — empty-space show-desktop (Tahoe)

Trace `space_manager_focus_space` (`src/space_manager.c:993`) / SA `do_space_focus`
(`src/osax/payload.m:560`) on an empty space; confirm whether
`com.apple.showdesktop` is being implicitly triggered. Needs a Tahoe repro.

**Status: queued (needs repro).**

---

## Item 5 — Are these caused by the Arc PiP fix? (diagnostic, answered)

**No. None of the other problems are caused by the Arc PiP fix.** That fix is only
the mouse-warp patch in `src/event_loop.c` (`window_did_receive_focus`) — it
suppresses cursor centering to/from ineligible windows and never changes focus
targeting or space switching. Specifically:

- Zoom-resize is the `AXEnhancedUserInterface` path (`misc/helpers.h:524`).
- Cross-display focus landing on Arc is pre-existing rank-1 selection in
  `display_manager_focus_display` (`src/display_manager.c:465`).
- Empty-space show-desktop is macOS/SLS behavior.

**Status: answered.**

---

## Item 6 — Zoom won't resize

`AX_ENHANCED_UI_WORKAROUND` (`misc/helpers.h:524`) wraps resize; Zoom may need the
toggle held across the move+resize, or a longer settle. Needs a live repro to
characterize.

**Status: queued (needs repro).**
