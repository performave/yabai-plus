#!/bin/sh

set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
YABAI_BIN="${YABAI_BIN:-$ROOT/target/release/yabai}"
USER_NAME="${USER:-}"
TMP_ROOT="${TMPDIR:-/tmp}"
CONFIG_FILE=""
STDOUT_FILE=""
STDERR_FILE=""
DAEMON_PID=""

fail()
{
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

skip()
{
    printf 'SKIP: %s\n' "$*"
    exit 0
}

cleanup()
{
    status=$?

    if [ -n "$DAEMON_PID" ] && kill -0 "$DAEMON_PID" 2>/dev/null; then
        kill "$DAEMON_PID" 2>/dev/null || true
        for _ in 1 2 3 4 5; do
            if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
                break
            fi
            sleep 0.2
        done
        if kill -0 "$DAEMON_PID" 2>/dev/null; then
            kill -KILL "$DAEMON_PID" 2>/dev/null || true
        fi
        wait "$DAEMON_PID" 2>/dev/null || true
    fi

    [ -z "$CONFIG_FILE" ] || rm -f "$CONFIG_FILE"
    [ -z "$STDOUT_FILE" ] || rm -f "$STDOUT_FILE"
    [ -z "$STDERR_FILE" ] || rm -f "$STDERR_FILE"

    exit "$status"
}

trap cleanup EXIT HUP INT TERM

assert_eq()
{
    name="$1"
    actual="$2"
    expected="$3"

    if [ "$actual" != "$expected" ]; then
        fail "$name: expected '$expected', got '$actual'"
    fi
}

assert_json_array()
{
    name="$1"
    json="$2"

    JSON_INPUT="$json" python3 - "$name" <<'PY'
import json
import os
import sys

name = sys.argv[1]
try:
    value = json.loads(os.environ["JSON_INPUT"])
except Exception as error:
    print(f"FAIL: {name}: invalid JSON: {error}", file=sys.stderr)
    sys.exit(1)

if not isinstance(value, list):
    print(f"FAIL: {name}: expected JSON array", file=sys.stderr)
    sys.exit(1)
PY
}

assert_rule_present()
{
    json="$1"

    JSON_INPUT="$json" python3 - <<'PY'
import json
import os
import sys

rules = json.loads(os.environ["JSON_INPUT"])
for rule in rules:
    if rule.get("label") == "e2e-smoke" and rule.get("app") == "^YabaiE2E$":
        sys.exit(0)

print("FAIL: added rule was not present in rule list", file=sys.stderr)
sys.exit(1)
PY
}

assert_rule_absent()
{
    json="$1"

    JSON_INPUT="$json" python3 - <<'PY'
import json
import os
import sys

rules = json.loads(os.environ["JSON_INPUT"])
for rule in rules:
    if rule.get("label") == "e2e-smoke":
        print("FAIL: removed rule is still present in rule list", file=sys.stderr)
        sys.exit(1)
PY
}

[ -n "$USER_NAME" ] || skip "USER is not set"
[ -x "$YABAI_BIN" ] || fail "binary not executable: $YABAI_BIN"
command -v python3 >/dev/null 2>&1 || skip "python3 is required for JSON assertions"

if "$YABAI_BIN" -m query --displays >/dev/null 2>&1; then
    skip "a yabai instance is already running for user '$USER_NAME'"
fi

CONFIG_FILE="$(mktemp "$TMP_ROOT/yabai-e2e-config.XXXXXX")"
STDOUT_FILE="$(mktemp "$TMP_ROOT/yabai-e2e-out.XXXXXX")"
STDERR_FILE="$(mktemp "$TMP_ROOT/yabai-e2e-err.XXXXXX")"

"$YABAI_BIN" -c "$CONFIG_FILE" >"$STDOUT_FILE" 2>"$STDERR_FILE" &
DAEMON_PID=$!

ready=0
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
        if grep -q "could not access accessibility features" "$STDERR_FILE" 2>/dev/null; then
            skip "Accessibility permission is required for e2e smoke tests"
        fi
        if grep -q "could not acquire lock-file" "$STDERR_FILE" 2>/dev/null; then
            skip "another yabai process holds the per-user lock"
        fi
        if grep -q "display has separate spaces" "$STDERR_FILE" 2>/dev/null; then
            skip "Displays have separate Spaces must be enabled"
        fi

        printf '%s\n' "--- yabai stderr ---" >&2
        sed 's/^/  /' "$STDERR_FILE" >&2 || true
        fail "daemon exited before accepting messages"
    fi

    if displays_json="$("$YABAI_BIN" -m query --displays 2>/dev/null)"; then
        ready=1
        break
    fi

    sleep 0.25
done

[ "$ready" -eq 1 ] || fail "daemon did not become ready"

assert_json_array "query --displays" "$displays_json"
spaces_json="$("$YABAI_BIN" -m query --spaces)"
assert_json_array "query --spaces" "$spaces_json"
windows_json="$("$YABAI_BIN" -m query --windows)"
assert_json_array "query --windows" "$windows_json"

assert_eq "initial debug_output" "$("$YABAI_BIN" -m config debug_output)" "off"
"$YABAI_BIN" -m config debug_output on
assert_eq "enabled debug_output" "$("$YABAI_BIN" -m config debug_output)" "on"
"$YABAI_BIN" -m config debug_output off
assert_eq "disabled debug_output" "$("$YABAI_BIN" -m config debug_output)" "off"

"$YABAI_BIN" -m rule --add label=e2e-smoke 'app=^YabaiE2E$' manage=off
rules_json="$("$YABAI_BIN" -m rule --list)"
assert_json_array "rule --list" "$rules_json"
assert_rule_present "$rules_json"

"$YABAI_BIN" -m rule --remove e2e-smoke
rules_json="$("$YABAI_BIN" -m rule --list)"
assert_rule_absent "$rules_json"

if "$YABAI_BIN" -m query --not-a-real-command >/dev/null 2>&1; then
    fail "invalid query command unexpectedly succeeded"
fi

printf '%s\n' "PASS: yabai e2e smoke"
