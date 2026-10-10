#!/usr/bin/env bash
# Tests packaging/os/files/usr/libexec/ps5-launcher-os/boot-health, the boot health check with
# one automatic rollback. It runs a copy whose fixed paths point at a test folder, with fake
# bootc, systemctl, journalctl and pgrep, a fake sysfs DRM tree, and a fake launcher that listens
# on the health socket. Its times are shortened: 6 s for the whole check, 2 s healthy in a row.
#   packaging/os/test-boot-health.sh
# It needs Linux (SO_PEERCRED, /proc, GNU stat), jq, python3 and flock. On macOS it runs itself
# in a Fedora container with docker.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=packaging/os/config.sh
source "$here/config.sh"
os_config_validate
if [ "$(uname -s)" != Linux ]; then
    repo=$(cd "$here/../.." && pwd)
    exec docker run --rm -v "$repo:/src:ro" -w /src -e FEDORA_VERSION "$FEDORA_CONTAINER_IMAGE" sh -c \
        'dnf -y -q install jq python3 util-linux procps-ng >/dev/null && packaging/os/test-boot-health.sh'
fi
work=$(mktemp -d)
cleanup() {
    [ -f "$work/server.pid" ] && kill "$(cat "$work/server.pid")" 2>/dev/null
    rm -rf "$work"
}
trap cleanup EXIT
uid=$(id -u)
python=$(python3 -c 'import os; print(os.path.realpath("/proc/self/exe"))')
mkdir -p "$work/bin" "$work/drm" "$work/drivers/amdgpu" "$work/drivers/nvidia" "$work/run/$uid"
sed -e "s|^health_dir=/var/lib/ps5-launcher-os/health$|health_dir=$work/health|" \
    -e "s|^lock_file=/run/ps5-launcher-os.lock$|lock_file=$work/lock|" \
    -e "s|^os_release=/usr/lib/ps5-launcher/os-release$|os_release=$work/os-release|" \
    -e "s|^sddm_user_conf=/etc/sddm.conf.d/10-ps5-launcher-os-user.conf$|sddm_user_conf=$work/sddm-user.conf|" \
    -e "s|^drm_dir=/sys/class/drm$|drm_dir=$work/drm|" \
    -e "s|^run_user=/run/user$|run_user=$work/run|" \
    -e "s|^boot_id_file=/proc/sys/kernel/random/boot_id$|boot_id_file=$work/boot_id|" \
    -e "s|^launcher_exe=/usr/bin/ps5-launcher$|launcher_exe=$python|" \
    -e "s|^budget=140$|budget=6|" -e "s|^stable=10$|stable=2|" -e "s|^lock_wait=20$|lock_wait=2|" \
    -e "s|^PATH=/usr/sbin:/usr/bin$|PATH=$work/bin:/usr/sbin:/usr/bin:/bin|" \
    "$here/files/usr/libexec/ps5-launcher-os/boot-health" > "$work/boot-health"
chmod +x "$work/boot-health"
for fixed in "^health_dir=$work/" "^lock_file=$work/" "^os_release=$work/" "^sddm_user_conf=$work/" \
    "^drm_dir=$work/" "^run_user=$work/" "^boot_id_file=$work/" "^launcher_exe=$python$" \
    "^budget=6$" "^stable=2$" "^lock_wait=2$" "^PATH=$work/bin:"; do
    grep -q "$fixed" "$work/boot-health" || { echo "FAIL the fixed line $fixed was not found"; exit 1; }
done

# --- Fakes ----------------------------------------------------------------------------------
# bootc: status prints $work/status.json; rollback logs, then acts as $work/rollback-mode says:
#   ok        queue the rollback (status: rollbackQueued true)
#   fail      exit 1
#   powercut  kill the health check (the PC lost power before bootc did anything)
cat > "$work/bin/bootc" <<FAKE
#!/bin/sh
case "\$1" in
status) cat "$work/status.json" ;;
rollback)
    echo rollback >> "$work/bootc.log"
    case \$(cat "$work/rollback-mode") in
    ok) jq '.status.rollbackQueued = true' "$work/status.json" > "$work/s.tmp" && mv "$work/s.tmp" "$work/status.json" ;;
    powercut) kill -KILL "\$(cat "$work/health.pid")"; exit 1 ;;
    *) exit 1 ;;
    esac ;;
*) echo "\$*" >> "$work/bootc.log" ;;
esac
FAKE
printf '#!/bin/sh\necho "$*" >> "%s/systemctl.log"\n' "$work" > "$work/bin/systemctl"
printf '#!/bin/sh\necho "fake journal line"\n' > "$work/bin/journalctl"
# pgrep -u UID -x PATTERN: the running programs are the lines of $work/procs.
# shellcheck disable=SC2016 # $p is the fake's own variable, expanded when the fake runs
printf '#!/bin/sh\nfor p; do :; done\ngrep -Exq "$p" "%s/procs"\n' "$work" > "$work/bin/pgrep"
chmod +x "$work/bin/"*

# The fake launcher: listens on the health socket and answers "ok" (mode ok), never answers
# (silent), or answers but is not named ps5-launcher (impostor).
cat > "$work/server.py" <<'PY'
import ctypes, os, socket, sys
path, mode = sys.argv[1], sys.argv[2]
if mode != "impostor":
    ctypes.CDLL(None).prctl(15, b"ps5-launcher", 0, 0, 0)  # PR_SET_NAME
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(path)
os.chmod(path, 0o600)
s.listen(8)
while True:
    c, _ = s.accept()
    if c.recv(16) == b"ping\n" and mode != "silent":
        c.sendall(b"ok\n")
    if mode != "silent":
        c.close()
PY
sock="$work/run/$uid/ps5-launcher-health.sock"
launcher() { # ok|silent|impostor|none
    if [ -f "$work/server.pid" ]; then
        kill "$(cat "$work/server.pid")" 2>/dev/null || true
        rm -f "$work/server.pid" "$sock"
    fi
    [ "$1" = none ] && return
    python3 "$work/server.py" "$sock" "$1" &
    echo $! > "$work/server.pid"
    for _ in $(seq 50); do [ -S "$sock" ] && return; sleep 0.1; done
}

A=sha256:$(printf 'a%.0s' $(seq 64))
B=sha256:$(printf 'b%.0s' $(seq 64))
C=sha256:$(printf 'c%.0s' $(seq 64))
D=sha256:$(printf 'd%.0s' $(seq 64))
status() { # BOOTED ROLLBACK [STAGED]: a bootc status like bootc 1.16's
    jq -n --arg b "$1" --arg r "$2" --arg s "${3:-}" '{apiVersion: "org.containers.bootc/v1", kind: "BootcHost",
        status: {booted: {image: {imageDigest: $b}},
                 rollback: (if $r == "" then null else {image: {imageDigest: $r}} end),
                 staged: (if $s == "" then null else {image: {imageDigest: $s}} end),
                 rollbackQueued: false}}' > "$work/status.json"
}
gpu() { # DRIVER, on the card with the connected screen
    rm -rf "$work/drm"/*
    mkdir -p "$work/drm/card1-HDMI-A-1" "$work/drm/card1/device" "$work/drm/card0-DP-1"
    echo connected > "$work/drm/card1-HDMI-A-1/status"
    echo disconnected > "$work/drm/card0-DP-1/status"
    ln -s "$work/drivers/$1" "$work/drm/card1/device/driver"
}
new_boot() { python3 -c 'import uuid; print(uuid.uuid4())' > "$work/boot_id"; : > "$work/bootc.log"; : > "$work/systemctl.log"; }
fresh() { # a new PC: no health state
    rm -rf "$work/health" "$work/lock"
    printf 'IMAGE=main\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:main\n' > "$work/os-release"
    printf '[Autologin]\nUser=%s\n' "$(id -un)" > "$work/sddm-user.conf"
    gpu amdgpu
    echo ok > "$work/rollback-mode"
    new_boot
}
health() { # runs the check; its output goes to $work/out
    "$work/boot-health" > "$work/out" 2>&1 &
    echo $! > "$work/health.pid"
    wait "$(cat "$work/health.pid")" || true
}
j() { jq -r "$2" "$work/health/$1"; } # FILE FILTER
att() { echo "attempts/${1#sha256:}"; }

failures=0
check() { # NAME, then the command that must succeed
    local name=$1
    shift
    if "$@"; then echo "ok   $name"; else echo "FAIL $name"; failures=$((failures + 1)); sed 's/^/     | /' "$work/out"; fi
}
fails() { ! "$@"; }
is() { [ "$1" = "$2" ]; }
has() { case $1 in *"$2"*) return 0 ;; *) return 1 ;; esac; }
no_rollback() { ! grep -q rollback "$work/bootc.log" && ! grep -q reboot "$work/systemctl.log"; }

# --- Unknown status ------------------------------------------------------------------------------
fresh
echo '{"apiVersion":"org.containers.bootc/v1","kind":"BootcHost","status":{"booted":null}}' > "$work/status.json"
health
check "a status with no booted image changes nothing" no_rollback
check "  and records nothing" test -z "$(ls -A "$work/health/attempts")"
check "  and says why" grep -q "bootc status is unknown" "$work/out"
echo 'Error: not JSON' > "$work/status.json"
health
check "a status that is not JSON changes nothing" no_rollback
check "  and records nothing" test -z "$(ls -A "$work/health/attempts")"

# --- A good first start --------------------------------------------------------------------------
fresh
status "$A" ""
echo gamescope > "$work/procs"
launcher ok
health
check "a good first start is healthy" is "$(j "$(att "$A")" .result)" healthy
check "  with this boot's ID" is "$(j "$(att "$A")" .boot_id)" "$(cat "$work/boot_id")"
check "  and becomes last-good" is "$(j last-good .digest)" "$A"
check "  with no rollback" no_rollback
check "  and no notice" test ! -e "$work/health/notice.json"
check "  and state the launcher can read" test "$(stat -c %a "$work/health/last-good")" = 644

# --- A failed first start: exactly one rollback, then the destination ------------------------------
new_boot
status "$B" "$A"
launcher silent
health
check "a failed first start rolls back" is "$(grep -c rollback "$work/bootc.log")" 1
check "  and restarts" grep -qx -- "--no-block reboot" "$work/systemctl.log"
check "  after recording the recovery" is "$(j transaction '[.failed_digest, .destination_digest, .rollback_attempts] | join(" ")')" "$B $A 1"
check "  the attempt says why" has "$(j "$(att "$B")" .reason)" "launcher not healthy (no answer"
check "  and the notice says it rolls back" is "$(j notice.json .outcome)" rolling-back
check "  readable by the launcher" test "$(stat -c %a "$work/health/notice.json")" = 644
check "  the logs are kept" test -n "$(ls "$work/health/logs")"
check "  but last-good stays" is "$(j last-good .digest)" "$A"
# The PC restarts into A, the destination. Its check fails too: no second rollback.
new_boot
status "$A" "$B"
health
check "on the destination, a failed check does not roll back again" no_rollback
check "  the notice says the previous system is unhealthy too" is "$(j notice.json .outcome)" rollback-unhealthy
check "  and the recovery stays open" test -e "$work/health/transaction"
check "  with one rollback attempt" is "$(j transaction .rollback_attempts)" 1
# Restarted again, the destination works: the recovery is complete.
new_boot
launcher ok
health
check "a healthy destination completes the recovery" test ! -e "$work/health/transaction"
check "  into the history" grep -q '"completed"' "$work/health/history/"*
check "  the notice says it rolled back" is "$(j notice.json .outcome)" rolled-back
check "  naming the failed system" is "$(j notice.json .failed_digest)" "$B"
check "  last-good is the destination" is "$(j last-good .digest)" "$A"
# B again (say, after a power cut the PC started B by hand): not its first attempt.
new_boot
status "$B" "$A"
launcher silent
health
check "a failed start that is not the first attempt does not roll back" no_rollback
check "  and says so" has "$(j "$(att "$B")" .action)" "not the first start"

# --- The rollback entry is not last-good -----------------------------------------------------------
fresh
status "$A" ""
echo gamescope > "$work/procs"
launcher ok
health
new_boot
status "$C" "$D"
launcher silent
health
check "no rollback when the rollback entry is not last-good" no_rollback
check "  and the notice says why" has "$(j notice.json .action)" "is not the last good system"
check "  and no recovery is recorded" test ! -e "$work/health/transaction"

# --- A staged deployment, a busy lock, a failed bootc rollback -------------------------------------
new_boot
status "$D" "$A" "$C"
health
check "no rollback when something is staged" no_rollback
if command -v flock >/dev/null; then
    new_boot
    status "$B" "$A"
    rm -f "$work/health/$(att "$B")"
    # The helper takes the lock after the attempt is recorded, and holds it past the decision.
    (sleep 1 && exec flock "$work/lock" sleep 16) &
    holder=$!
    health
    wait "$holder"
    check "no rollback while the helper holds the lock" no_rollback
    check "  and the notice says so" has "$(j notice.json .action)" "lock"
fi
new_boot
status "$B" "$A"
rm -f "$work/health/$(att "$B")"
echo fail > "$work/rollback-mode"
health
check "when bootc rollback fails" is "$(j notice.json .outcome)" rollback-failed
check "  there is no restart" fails grep -q reboot "$work/systemctl.log"

# --- A power cut between the record and the rollback ------------------------------------------------
fresh
status "$A" ""
echo gamescope > "$work/procs"
launcher ok
health
new_boot
status "$B" "$A"
launcher silent
echo powercut > "$work/rollback-mode"
health
check "power cut: the recovery was recorded before the rollback" is "$(j transaction .failed_digest)" "$B"
check "  and nothing restarted" fails grep -q reboot "$work/systemctl.log"
# The PC starts B again. Even with bootc working now, the rollback is never tried again.
new_boot
echo ok > "$work/rollback-mode"
health
check "after the power cut, no second try" no_rollback
check "  the notice says it was interrupted" is "$(j notice.json .outcome)" rollback-interrupted
check "  the recovery still counts one attempt" is "$(j transaction .rollback_attempts)" 1

# --- Plasma runs and the launcher is silent ----------------------------------------------------------
fresh
status "$A" ""
echo plasmashell > "$work/procs"
launcher none
start=$(date +%s)
health
check "Plasma with no launcher is a failure" is "$(j "$(att "$A")" .result)" unhealthy
check "  launcher not healthy (Plasma fallback)" has "$(j "$(att "$A")" .reason)" "launcher not healthy (Plasma fallback)"
check "  decided without waiting out the whole check" test $(($(date +%s) - start)) -lt 5
check "  no last-good on a new PC, so no rollback" no_rollback
check "  and the notice says why" has "$(j notice.json .action)" "no last-good"
check "  and nothing became last-good" test ! -e "$work/health/last-good"
# What counts is the launcher's answer from gamescope: Plasma beside them changes nothing.
printf 'plasmashell\ngamescope\n' > "$work/procs"
launcher ok
new_boot
status "$B" "$A"
health
check "the launcher's answer counts, not Plasma's presence" is "$(j "$(att "$B")" .result)" healthy

# --- The launcher's socket and the GPU ---------------------------------------------------------------
fresh
status "$A" ""
echo gamescope > "$work/procs"
launcher impostor
health
check "another program on the health socket does not count" is "$(j "$(att "$A")" .result)" unhealthy
fresh
status "$A" ""
launcher ok
chmod 666 "$sock"
health
check "a socket others can write to does not count" is "$(j "$(att "$A")" .result)" unhealthy
fresh
status "$A" ""
launcher ok
gpu nvidia
health
check "the main image with the NVIDIA driver is unhealthy" has "$(j "$(att "$A")" .reason)" "no usable driver"
# A TV in standby: no connected screen says nothing about the new system. The start is neither
# good nor bad, and the next start is still its first attempt.
fresh
status "$A" ""
echo gamescope > "$work/procs"
launcher ok
health
new_boot
status "$B" "$A"
echo disconnected > "$work/drm/card1-HDMI-A-1/status"
launcher silent
health
check "no connected screen decides nothing" no_rollback
check "  records no attempt, so the next start is still the first" test ! -e "$work/health/$(att "$B")"
check "  shows no notice" test ! -e "$work/health/notice.json"
check "  keeps last-good" is "$(j last-good .digest)" "$A"
check "  and says why" grep -q "no connected screen" "$work/out"
new_boot
gpu amdgpu
health
check "with the TV on, a failed first start still rolls back" is "$(grep -c rollback "$work/bootc.log")" 1

# --- State it does not know --------------------------------------------------------------------------
fresh
status "$B" "$A"
launcher silent
mkdir -p "$work/health"
echo '{"version": 2, "digest": "'"$A"'"}' > "$work/health/last-good"
health
check "a last-good of another version changes nothing" no_rollback
check "  and records no attempt" test ! -e "$work/health/$(att "$B")"

[ "$failures" = 0 ]
