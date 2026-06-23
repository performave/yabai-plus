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
| 1 | Config: disable floats-topmost + hybrid (no-manage-on-startup) | feat | **in progress** |
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

**Status: in progress.**

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
