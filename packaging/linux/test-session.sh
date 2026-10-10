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

[ "$failures" = 0 ]
