#!/usr/bin/env bash
# Tests packaging/os/files/usr/libexec/ps5-launcher-os/recovery-menu, the text menu of recovery
# mode. It runs a copy whose fixed paths point at a test folder, with a fake helper (it logs its
# arguments; $work/exit-<task> sets its exit code, $work/out-<task> its output) and fake systemctl
# and nmcli. The choices come on stdin; the end of input ends the menu.
#   packaging/os/test-recovery-menu.sh
set -euo pipefail
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
# macOS has bash only in /bin.
sed -e "1s|^#!/usr/bin/bash$|#!/usr/bin/env bash|" \
    -e "s|^helper=/usr/libexec/ps5-launcher/helper$|helper=$work/helper|" \
    -e "s|^os_release=/usr/lib/ps5-launcher/os-release$|os_release=$work/os-release|" \
    -e "s|^notice=/var/lib/ps5-launcher-os/health/notice.json$|notice=$work/notice.json|" \
    -e "s|^PATH=/usr/sbin:/usr/bin$|PATH=$work/bin:/usr/sbin:/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin|" \
    "$(dirname "$0")/files/usr/libexec/ps5-launcher-os/recovery-menu" > "$work/menu"
chmod +x "$work/menu"
for fixed in "^helper=$work/helper$" "^os_release=$work/os-release$" "^notice=$work/notice.json$" "^PATH=$work/bin:"; do
    grep -q "$fixed" "$work/menu" || { echo "FAIL the menu's fixed path $fixed was not found"; exit 1; }
done
printf 'IMAGE=main\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:main\n' > "$work/os-release"
cat > "$work/helper" <<FAKE
#!/bin/sh
echo "\$*" >> "$work/helper.log"
task=\$(echo "\$*" | tr ' ' '-')
[ -f "$work/out-\$task" ] && cat "$work/out-\$task"
exit \$(cat "$work/exit-\$task" 2>/dev/null || echo 0)
FAKE
printf '#!/bin/sh\necho "$*" >> "%s/systemctl.log"\n' "$work" > "$work/bin/systemctl"
printf '#!/bin/sh\necho connected\n' > "$work/bin/nmcli"
chmod +x "$work/helper" "$work/bin/"*
echo '{"status": {"booted": {"image": {"imageDigest": "sha256:1234"}}}}' > "$work/out-status"

failures=0
check() { # NAME, then the command that must succeed
    local name=$1
    shift
    if "$@"; then echo "ok   $name"; else echo "FAIL $name"; failures=$((failures + 1)); sed 's/^/     | /' "$work/screen"; fi
}
menu() { # the choices, one per line; the screen goes to $work/screen
    rm -f "$work/helper.log" "$work/systemctl.log" "$work"/exit-*
    touch "$work/helper.log" "$work/systemctl.log"
    for setting in ${settings[@]+"${settings[@]}"}; do echo "${setting#*=}" > "$work/${setting%%=*}"; done
    printf '%s\n' "$@" | "$work/menu" > "$work/screen" 2>&1 || echo "(the menu exited with $?)" >> "$work/screen"
}
screen_has() { grep -qF -- "$1" "$work/screen"; }
helper_ran() { # the tasks, in order, besides status
    if [ $# = 0 ]; then if grep -qv '^status$' "$work/helper.log"; then return 1; fi; return 0; fi
    diff <(printf '%s\n' "$@") <(grep -v '^status$' "$work/helper.log") >/dev/null
}
no_restart() { [ ! -s "$work/systemctl.log" ]; }

settings=()
menu
check "the menu shows the four choices" screen_has "4  Restart"
check "  the image and its digest" screen_has "This system: main image, sha256:1234"
check "  and ends at the end of input" true

menu 1 n
check "1 switches to the main image" helper_ran "switch main"
check "  says when it starts" screen_has "The main image starts at the next restart"
check "  and restarts only when asked" no_restart

settings=(exit-switch-main=1)
menu 1 4
check "a failed switch says so" screen_has "The switch failed (exit 1)"
check "  and the menu comes back" test "$(grep -c "Choose 1-4" "$work/screen")" = 2
check "  where 4 restarts" grep -qx reboot "$work/systemctl.log"

settings=(exit-switch-nvidia=3 out-queue-key=12345678)
menu 2 y
check "2 with the key not enrolled says key-required" screen_has "key-required"
check "  shows the enrolment steps" screen_has "Enroll MOK"
check "  queues the key when asked" helper_ran "switch nvidia" "queue-key"
check "  shows the password" screen_has "password for the blue screen is: 12345678"
check "  digit by digit" screen_has "digit 8: 8"
check "  and does not restart by itself" no_restart

settings=(exit-switch-nvidia=4)
menu 2 n
check "2 with the key pending says key-pending" screen_has "key-pending"
check "  and queues nothing unless asked" helper_ran "switch nvidia"

settings=(exit-switch-nvidia=3 out-queue-key=key-enrolled)
menu 2 y
check "a key already enrolled is said plainly" screen_has "already enrolled"
check "  with no password shown" sh -c "! grep -q 'password for the blue screen' '$work/screen'"

settings=(exit-switch-nvidia=3 exit-queue-key=1)
menu 2 y
check "a key that cannot be queued is an error" screen_has "Could not queue the key (exit 1)"

settings=()
menu 2 n
check "2 with the key enrolled switches" helper_ran "switch nvidia"
check "  and says the NVIDIA image starts" screen_has "The NVIDIA image starts at the next restart"

menu 3 y
check "3 rolls back" helper_ran "rollback"
check "  and restarts when asked" grep -qx reboot "$work/systemctl.log"

settings=(exit-rollback=1)
menu 3
check "a failed rollback says so" screen_has "The rollback failed (exit 1)"
check "  without a restart" no_restart

settings=()
menu 4
check "4 restarts" grep -qx reboot "$work/systemctl.log"
check "  and runs no helper task" helper_ran

menu 9 "rm -rf /"
check "anything else is refused" test "$(grep -c "Type 1, 2, 3 or 4" "$work/screen")" = 2
check "  and runs no helper task" helper_ran

echo '{"version": 1, "outcome": "rolled-back", "reason": "launcher not healthy (Plasma fallback)"}' > "$work/notice.json"
menu
check "the boot check's last notice is shown" screen_has "Last boot check: rolled-back: launcher not healthy (Plasma fallback)"

[ "$failures" = 0 ]
