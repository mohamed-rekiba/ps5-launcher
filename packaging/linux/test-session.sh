#!/usr/bin/env bash
# Tests the restart rules of ps5-launcher-session's inner loop, with a fake launcher that exits
# with the codes it is given, one per run:
#   packaging/linux/test-session.sh
set -euo pipefail
session=$(cd "$(dirname "$0")" && pwd)/ps5-launcher-session
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The fake launcher: each run takes the next exit code from $work/codes and logs it.
cat > "$work/launcher" <<'EOF'
#!/bin/sh
code=$(head -1 "$FAKE/codes")
sed -i.bak 1d "$FAKE/codes"
echo "$code" >> "$FAKE/runs"
exit "$code"
EOF
chmod +x "$work/launcher"

failures=0
# check NAME "CODES" WANT_RUNS WANT_EXIT
check() {
    local name=$1 codes=$2 want_runs=$3 want_exit=$4 got_exit=0
    tr ' ' '\n' <<< "$codes" > "$work/codes"
    : > "$work/runs"
    FAKE=$work PS5_LAUNCHER_BIN="$work/launcher" XDG_CACHE_HOME="$work/cache" \
        "$session" --inner || got_exit=$?
    local got_runs
    got_runs=$(wc -l < "$work/runs" | tr -d ' ')
    if [ "$got_runs" = "$want_runs" ] && [ "$got_exit" = "$want_exit" ]; then
        echo "ok   $name"
    else
        echo "FAIL $name: $got_runs runs (want $want_runs), exit $got_exit (want $want_exit)"
        failures=$((failures + 1))
    fi
}

check "exit 0 ends the session" "0" 1 0
check "exit 75 restarts the launcher" "75 75 0" 3 0
check "a crash restarts the launcher" "1 0" 2 0
check "a signal counts as a crash" "139 0" 2 0
check "the third crash in a minute stops the restarts" "1 2 3 0" 3 1
check "restarts in between do not count" "1 75 1 75 0" 5 0
check "restarts do not clear the crashes" "1 75 1 75 1 0" 5 1

# The whole session: a gamescope that fails at once, as on a PC whose GPU it cannot use. A
# display manager that logs in again (SDDM's Relogin) would start the session in a loop; the
# third start within 60 seconds must not start gamescope again.
mkdir -p "$work/bin"
printf '#!/bin/sh\necho started >> "%s/gamescope.log"\nexit 1\n' "$work" > "$work/bin/gamescope"
chmod +x "$work/bin/gamescope"
loops=0
for _ in 1 2 3 4; do
    PATH="$work/bin:$PATH" PS5_LAUNCHER_BIN="$work/launcher" XDG_CACHE_HOME="$work/cache" \
        XDG_STATE_HOME="$work/state" "$session" || loops=$((loops + 1))
done
starts=$(wc -l < "$work/gamescope.log" | tr -d ' ')
if [ "$starts" = 2 ] && [ "$loops" = 4 ]; then
    echo "ok   a session that keeps ending at once stops at the third start"
else
    echo "FAIL gamescope started $starts times (want 2); $loops of 4 sessions failed (want 4)"
    failures=$((failures + 1))
fi

# The screen output the launcher chose (Settings → Display) in session.conf: gamescope gets
# -W -H (and -r) only when the file sets valid numbers. A fake gamescope logs its arguments.
printf '#!/bin/sh\necho "$*" > "%s/gamescope.args"\nexit 0\n' "$work" > "$work/bin/gamescope"
mkdir -p "$work/config/ps5-launcher"
# output NAME WANT_ARGS_BEFORE_-f, then the lines of session.conf (none: no file)
output() {
    local name=$1 want=$2
    shift 2
    rm -rf "$work/out-state" "$work/config/ps5-launcher/session.conf" "$work/gamescope.args"
    [ $# -gt 0 ] && printf '%s\n' "$@" > "$work/config/ps5-launcher/session.conf"
    (cd "$work" && PATH="$work/bin:$PATH" PS5_LAUNCHER_BIN="$work/launcher" XDG_CACHE_HOME="$work/cache" \
        XDG_STATE_HOME="$work/out-state" XDG_CONFIG_HOME="$work/config" "$session") || true
    local got
    got=$(sed 's/ *-f -- .*//' "$work/gamescope.args" 2>/dev/null || echo "gamescope did not start")
    if [ "$got" = "$want" ]; then
        echo "ok   $name"
    else
        echo "FAIL $name: gamescope got \"$got\" (want \"$want\")"
        failures=$((failures + 1))
    fi
}
output "no session.conf: gamescope picks the output" ""
output "a size and a rate" "-W 2560 -H 1440 -r 144" "# Written by PS5 Launcher" WIDTH=2560 HEIGHT=1440 REFRESH=144
output "a size with an automatic rate" "-W 1920 -H 1080" WIDTH=1920 HEIGHT=1080 REFRESH=
output "a bad rate is left out" "-W 1920 -H 1080" WIDTH=1920 HEIGHT=1080 REFRESH=0
output "a width without a height is ignored" "" WIDTH=1920
output "a size that is not a number is ignored" "" WIDTH=abc HEIGHT=1080 REFRESH=60
output "a size out of range is ignored" "" WIDTH=99999 HEIGHT=1080
output "a size with a leading zero is ignored" "" WIDTH=01920 HEIGHT=1080
output "a size with extra words is ignored" "" "WIDTH=1920 -O DP-1" HEIGHT=1080
output "the file is read, never run" "" 'WIDTH=$(touch pwned)' 'HEIGHT=`touch pwned`'
if [ -e "$work/pwned" ]; then
    echo "FAIL session.conf ran a command"
    failures=$((failures + 1))
fi

[ "$failures" = 0 ]
