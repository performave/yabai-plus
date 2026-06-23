# Rust rewrite compatibility contract

This document records the user-visible behavior that the Rust rewrite should treat
as the starting contract. The C implementation is still the reference while this
file is incomplete, but this document should grow into the test oracle for the
rewrite.

The rewrite may intentionally break edge cases when the break is documented here,
in `CHANGELOG.md`, and in release notes.

## Binary and startup contract

Primary binary name: `yabai`.

Version output:

```text
yabai-<version>
```

Current daemon preconditions:

- Must not run as root for normal daemon mode.
- Must have Accessibility permission.
- Requires macOS "Displays have separate Spaces" enabled.
- Acquires a per-user lock file at `/tmp/yabai_$USER.lock`.
- Uses the daemon IPC socket path `/tmp/yabai_$USER.socket`.
- Uses the scripting-addition socket path `/tmp/yabai-sa_$USER.socket`.

CLI options currently exposed by `src/yabai.c`:

| Option | Behavior |
|---|---|
| `--load-sa` | Install if needed, then inject the scripting addition into Dock. Requires root and compatible SIP/boot-arg state. |
| `--uninstall-sa` | Remove the installed scripting addition. Requires root and compatible SIP state. |
| `--check-sa` | Talk to the running payload socket and report loaded/outdated/missing-support state. Does not require root. |
| `--install-sudoers` | Install a sha256-pinned passwordless sudoers rule for `--load-sa`. Requires root. |
| `--uninstall-sudoers` | Remove the sudoers rule. Requires root. |
| `--install-service` | Write the launchd service plist. |
| `--uninstall-service` | Remove the launchd service plist. |
| `--start-service` | Enable, load, and start the service. |
| `--restart-service` | Restart the service when possible. |
| `--stop-service` | Stop the running service instance. |
| `--message`, `-m` | Send the remaining args as one daemon message. |
| `--config`, `-c` | Use the specified config file for daemon startup. |
| `--verbose`, `-V` | Enable debug output. |
| `--version`, `-v` | Print version and exit. |
| `--help`, `-h` | Print usage and exit. |

## Client IPC contract

The `yabai -m` client sends one message to `/tmp/yabai_$USER.socket`.

Wire format:

- Native-endian 32-bit signed integer payload length.
- Payload is each argv token after `-m` as a NUL-terminated string.
- Payload ends with one extra NUL byte.
- The client then shuts down the write side and streams the daemon response.

Response convention:

- Normal responses are written to stdout by the client.
- Failure responses start with the single failure marker from `FAILURE_MESSAGE`; the
  client strips that marker, writes the rest to stderr, and exits non-zero.

Rust compatibility target:

- Keep this wire format so the Rust client can talk to the C daemon and the C
  client can talk to the Rust daemon during migration.
- Add explicit bounds and malformed-message handling; document any changed errors.

## Message domains

Top-level daemon domains from `src/message.c`:

- `config`
- `display`
- `space`
- `window`
- `query`
- `rule`
- `signal`

Unknown domains fail with:

```text
unknown domain '<domain>'
```

Rust compatibility target:

- Preserve domain names.
- Prefer a typed parser that rejects trailing garbage and malformed values
  consistently, even where the C parser currently accepts incidental forms.

## Common selectors

Common selector tokens used across displays, spaces, and windows:

- `prev`
- `next`
- `first`
- `last`
- `recent`
- `north`
- `east`
- `south`
- `west`
- `mouse`
- `stack`
- `stack.<n>`

Display selectors:

- Numeric arrangement index.
- Directional/common selectors where meaningful.
- Display label.

Space selectors:

- Numeric Mission Control index.
- Common selectors where meaningful.
- Space label.

Window selectors:

- Numeric window id.
- Directional/common selectors where meaningful.
- Stack selectors.
- Scratchpad label where command-specific.

Reserved display labels:

- `north`, `east`, `south`, `west`, `prev`, `next`, `first`, `last`, `recent`,
  `mouse`

Reserved space labels:

- `prev`, `next`, `first`, `last`, `recent`, `mouse`

Reserved scratchpad labels:

- `float`, `sticky`, `shadow`, `split`, `zoom-parent`, `zoom-fullscreen`,
  `windowed-fullscreen`, `native-fullscreen`, `expose`, `pip`, `recover`

## Config domain

Current config keys:

- `debug_output`
- `mouse_follows_focus`
- `window_sublayer_auto`
- `manage`
- `focus_follows_mouse`
- `display_arrangement_order`
- `window_origin_display`
- `window_placement`
- `window_insertion_point`
- `window_zoom_persist`
- `window_opacity`
- `window_opacity_duration`
- `window_animation_duration`
- `window_animation_easing`
- `window_shadow`
- `menubar_opacity`
- `active_window_opacity`
- `normal_window_opacity`
- `insert_feedback_color`
- `top_padding`
- `bottom_padding`
- `left_padding`
- `right_padding`
- `layout`
- `window_gap`
- `split_ratio`
- `split_type`
- `auto_balance`
- `mouse_modifier`
- `mouse_action1`
- `mouse_action2`
- `mouse_drop_action`
- `external_bar`
- `skip_window_focus_animation`

Config values and enums to preserve unless intentionally changed:

- Boolean values: `on`, `off`.
- `focus_follows_mouse`: `autofocus`, `autoraise`, plus off/disabled behavior.
- `display_arrangement_order`: `default`, `horizontal`, `vertical`.
- `window_origin_display`: `default`, `focused`, `cursor`.
- `window_placement`: `first_child`, `second_child`.
- `window_insertion_point`: `focused`, `first`, `last`.
- `layout`: `bsp`, `stack`, `float`.
- `split_type`: `vertical`, `horizontal`, `auto`.
- `mouse_modifier`: `alt`, `shift`, `cmd`, `ctrl`, `fn`.
- Mouse actions: `move`, `resize`, `swap`, `stack`.
- `external_bar`: `main:<top>:<bottom>` or `all:<top>:<bottom>` shape from C
  parser.

## Display domain

Commands:

- `display --focus <DISPLAY_SEL>`
- `display --space <SPACE_SEL>`
- `display --label <label>`

Query fields for displays, in current output order when all fields are selected:

- `id`
- `uuid`
- `index`
- `label`
- `frame` with `x`, `y`, `w`, `h` formatted to four decimals
- `spaces`
- `has-focus`

## Space domain

Commands:

- `space --focus <SPACE_SEL>`
- `space --switch <SPACE_SEL>`
- `space --create`
- `space --destroy [SPACE_SEL]`
- `space --move <SPACE_SEL>`
- `space --swap <SPACE_SEL>`
- `space --display <DISPLAY_SEL>`
- `space --equalize [x-axis|y-axis]`
- `space --balance [x-axis|y-axis]`
- `space --mirror <x-axis|y-axis>`
- `space --rotate <90|180|270>`
- `space --padding <abs|rel>:<top>:<bottom>:<left>:<right>`
- `space --gap <abs|rel>:<gap>`
- `space --toggle <padding|gap|mission-control|show-desktop>`
- `space --layout <bsp|stack|float>`
- `space --label <label>`

Query fields for spaces, in current output order when all fields are selected:

- `id`
- `uuid`
- `index`
- `label`
- `type`
- `display`
- `windows`
- `first-window`
- `last-window`
- `has-focus`
- `is-visible`
- `is-native-fullscreen`

## Window domain

Commands:

- `window --focus <WINDOW_SEL>`
- `window --close [WINDOW_SEL]`
- `window --minimize [WINDOW_SEL]`
- `window --deminimize <WINDOW_SEL>`
- `window --display <DISPLAY_SEL>`
- `window --space <SPACE_SEL>`
- `window --swap <WINDOW_SEL>`
- `window --warp <WINDOW_SEL>`
- `window --stack <WINDOW_SEL>`
- `window --insert <north|east|south|west|stack>`
- `window --grid <rows>:<cols>:<x>:<y>:<w>:<h>`
- `window --move <abs|rel>:<dx>:<dy>`
- `window --resize <top|bottom|left|right|top_left|top_right|bottom_left|bottom_right|abs>:<dx>:<dy>`
- `window --ratio <abs|rel>:<ratio>`
- `window --sub-layer <below|normal|above|auto>`
- `window --opacity <opacity>`
- `window --raise`
- `window --lower`
- `window --toggle <float|sticky|shadow|split|zoom-parent|zoom-fullscreen|windowed-fullscreen|native-fullscreen|expose|pip>`
- `window --scratchpad <label|recover>`

Query fields for windows, in current output order when all fields are selected:

- `id`
- `pid`
- `app`
- `title`
- `scratchpad`
- `frame` with `x`, `y`, `w`, `h` formatted to four decimals
- `role`
- `subrole`
- `root-window`
- `display`
- `space`
- `level`
- `sub-level`
- `layer`
- `sub-layer`
- `opacity`
- `split-type`
- `split-child`
- `stack-index`
- `can-move`
- `can-resize`
- `has-focus`
- `has-shadow`
- `has-parent-zoom`
- `has-fullscreen-zoom`
- `has-ax-reference`
- `is-native-fullscreen`
- `is-visible`
- `is-minimized`
- `is-hidden`
- `is-floating`
- `is-sticky`
- `is-grabbed`

## Query domain

Commands:

- `query --displays [properties] [--display <DISPLAY_SEL>]`
- `query --spaces [properties] [--space <SPACE_SEL>|--display <DISPLAY_SEL>|--window <WINDOW_SEL>]`
- `query --windows [properties] [--window <WINDOW_SEL>|--space <SPACE_SEL>|--display <DISPLAY_SEL>]`

Property selection:

- Optional property token is a comma-separated list.
- If no property token is provided, C behavior emits all fields.
- Unknown property names fail.

Rust cleanup candidate:

- Treat empty property segments like `id,,uuid` as invalid. The C parser mutates the
  token in-place and may produce incidental behavior that should not be preserved.

## Rule domain

Commands:

- `rule --add [--one-shot] key=value ...`
- `rule --remove <index|label>`
- `rule --apply <index|label>`
- `rule --list`

Rule keys:

- `app`
- `title`
- `role`
- `subrole`
- `display`
- `space`
- `opacity`
- `manage`
- `sticky`
- `mouse_follows_focus`
- `sub-layer`
- `native-fullscreen`
- `grid`
- `label`
- `scratchpad`

Rule output fields from `rule --list`:

- `index`
- `label`
- `app`
- `title`
- `role`
- `subrole`
- `display`
- `space`
- `follow_space`
- `opacity`
- `manage`
- `sticky`
- `mouse_follows_focus`
- `sub-layer`
- `native-fullscreen`
- `grid`
- `scratchpad`
- `one-shot`
- `flags`

Rust cleanup candidates:

- Make optional booleans a typed representation internally instead of preserving
  `RULE_PROP_UD` integer details.
- Keep JSON spelling stable for now, including `follow_space` and `sub-layer`.

## Signal domain

Commands:

- `signal --add key=value ...`
- `signal --remove <index|label>`
- `signal --list`

Signal keys:

- `app`
- `title`
- `active`
- `event`
- `action`
- `label`

Signal `active` values:

- `yes`
- `no`

Signal events:

- `application_launched`
- `application_terminated`
- `application_front_switched`
- `application_activated`
- `application_deactivated`
- `application_visible`
- `application_hidden`
- `window_created`
- `window_destroyed`
- `window_focused`
- `window_moved`
- `window_resized`
- `window_minimized`
- `window_deminimized`
- `window_title_changed`
- `space_created`
- `space_destroyed`
- `space_changed`
- `display_added`
- `display_removed`
- `display_moved`
- `display_resized`
- `display_changed`
- `mission_control_enter`
- `mission_control_exit`
- `dock_did_change_pref`
- `dock_did_restart`
- `menu_bar_hidden_changed`
- `system_woke`

Signal actions execute through:

```text
/usr/bin/env sh -c <action>
```

Environment variables are event-specific and currently include examples such as
`YABAI_PROCESS_ID`, `YABAI_RECENT_PROCESS_ID`, `YABAI_WINDOW_ID`, `YABAI_SPACE_ID`,
`YABAI_SPACE_INDEX`, and display equivalents. The Rust rewrite should pin this list
with dedicated golden tests before replacing signal execution.

## Scripting-addition protocol

Socket path:

```text
/tmp/yabai-sa_$USER.socket
```

Packet format from `src/sa.m` and `src/osax/payload.m`:

- Native-endian signed 16-bit payload length.
- One opcode byte.
- Packed native-endian arguments in C layout order.
- Sender waits for a one-byte response/dummy read.
- `SA_SOCKET_BUFF_LEN` is `0x1000`.

OSAX version currently expected by this tree:

```text
2.1.30
```

Attribute bitmask:

- `OSAX_ATTRIB_DOCK_SPACES` = `0x01`
- `OSAX_ATTRIB_DPPM` = `0x02`
- `OSAX_ATTRIB_ADD_SPACE` = `0x04`
- `OSAX_ATTRIB_REM_SPACE` = `0x08`
- `OSAX_ATTRIB_MOV_SPACE` = `0x10`
- `OSAX_ATTRIB_SET_WINDOW` = `0x20`
- `OSAX_ATTRIB_ANIM_TIME` = `0x40`

Opcodes:

| Opcode | Name | Arguments |
|---:|---|---|
| `0x01` | `SA_OPCODE_HANDSHAKE` | none; payload responds with version and attributes |
| `0x02` | `SA_OPCODE_SPACE_FOCUS` | `uint64_t sid` |
| `0x03` | `SA_OPCODE_SPACE_CREATE` | `uint64_t sid` |
| `0x04` | `SA_OPCODE_SPACE_DESTROY` | `uint64_t sid` |
| `0x05` | `SA_OPCODE_SPACE_MOVE` | `uint64_t src_sid`, `uint64_t dst_sid`, `uint64_t src_prev_sid`, `bool focus` |
| `0x06` | `SA_OPCODE_WINDOW_MOVE` | `uint32_t wid`, `int x`, `int y` |
| `0x07` | `SA_OPCODE_WINDOW_OPACITY` | `uint32_t wid`, `float opacity` |
| `0x08` | `SA_OPCODE_WINDOW_OPACITY_FADE` | `uint32_t wid`, `float opacity`, `float duration` |
| `0x09` | `SA_OPCODE_WINDOW_LAYER` | `uint32_t wid`, `int layer` |
| `0x0A` | `SA_OPCODE_WINDOW_STICKY` | `uint32_t wid`, `bool sticky` |
| `0x0B` | `SA_OPCODE_WINDOW_SHADOW` | `uint32_t wid`, `bool shadow` |
| `0x0C` | `SA_OPCODE_WINDOW_FOCUS` | `uint32_t wid` |
| `0x0D` | `SA_OPCODE_WINDOW_SCALE` | `uint32_t wid`, `float x`, `float y`, `float w`, `float h` |
| `0x0E` | `SA_OPCODE_WINDOW_SWAP_PROXY_IN` | count plus animation/proxy window pairs; see `src/sa.m` |
| `0x0F` | `SA_OPCODE_WINDOW_SWAP_PROXY_OUT` | count plus animation/proxy window pairs; see `src/sa.m` |
| `0x10` | `SA_OPCODE_WINDOW_ORDER` | `uint32_t a_wid`, `int order`, `uint32_t b_wid` |
| `0x11` | `SA_OPCODE_WINDOW_ORDER_IN` | `int count`, then `uint32_t wid` repeated |
| `0x12` | `SA_OPCODE_WINDOW_LIST_TO_SPACE` | `uint64_t sid`, `int count`, then `uint32_t wid` repeated |
| `0x13` | `SA_OPCODE_WINDOW_TO_SPACE` | `uint64_t sid`, `uint32_t wid` |

Rust compatibility target:

- Keep the existing wire protocol while the legacy payload is in use.
- Keep `--check-sa` semantics: exit zero only when payload responds, version matches,
  and all expected attribute bits are present.
- Preserve the Apple Silicon loader PAC ABI patch before injection.
- Do not hardened-runtime sign injected artifacts.

## Documented breaking changes ledger

Add entries here when the Rust rewrite intentionally changes behavior.

| Area | Change | Migration note |
|---|---|---|
| _none yet_ | _none yet_ | _none yet_ |
