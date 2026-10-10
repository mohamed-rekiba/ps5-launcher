#!/usr/bin/env bash
# Tests packaging/os/files/usr/libexec/ps5-launcher/helper, the launcher's root helper. It runs
# a copy whose fixed paths point at a test SDDM folder and a fake bootc:
#   packaging/os/test-helper.sh
set -euo pipefail
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/sddm"
chmod 755 "$work/sddm"
sed -e "s|^sddm_dir=/etc/sddm.conf.d$|sddm_dir=$work/sddm|" \
    -e "s|^PATH=/usr/sbin:/usr/bin$|PATH=$work/bin:/usr/sbin:/usr/bin:/bin|" \
    "$(dirname "$0")/files/usr/libexec/ps5-launcher/helper" > "$work/helper"
chmod +x "$work/helper"
if ! grep -q "^sddm_dir=$work/sddm$" "$work/helper" || ! grep -q "^PATH=$work/bin:" "$work/helper"; then
    echo "FAIL the helper's fixed paths were not found"
    exit 1
fi
# The fake bootc logs its arguments.
printf '#!/bin/sh\necho "$*" >> "%s/bootc.log"\n' "$work" > "$work/bin/bootc"
chmod +x "$work/bin/bootc"
next="$work/sddm/90-ps5-launcher-os-next-session.conf"

failures=0
helper() {
    "$work/helper" "$@" 2>/dev/null
}
check() { # NAME, then the command that must succeed
    local name=$1
    shift
    if "$@"; then echo "ok   $name"; else echo "FAIL $name"; failures=$((failures + 1)); fi
}
fails() { ! "$@"; }

check "set-next-session plasma succeeds" helper set-next-session plasma
check "it writes a one-time Plasma login" grep -qx "Session=plasma.desktop" "$next"
check "clear-next-session succeeds" helper clear-next-session
check "it deletes the one-time login" test ! -e "$next"
check "clearing twice is fine" helper clear-next-session
check "only Plasma can be chosen" fails helper set-next-session ../../etc/shadow
check "nothing was written for it" test ! -e "$next"
check "update-check succeeds" helper update-check
check "update succeeds" helper update
check "rollback succeeds" helper rollback
check "they run the right bootc commands" \
    diff <(printf 'upgrade --check\nupgrade\nrollback\n') "$work/bootc.log"
ln -s "$work/elsewhere" "$next"
check "a planted symlink is replaced, not followed" helper set-next-session plasma
check "  so nothing is written where it pointed" test ! -e "$work/elsewhere"
check "  and the drop-in is a plain file" test ! -L "$next"
helper clear-next-session
mkdir "$work/dir" && ln -s "$work/dir" "$next"
check "a planted symlink to a folder is replaced too" helper set-next-session plasma
check "  so nothing is moved into that folder" test -z "$(ls -A "$work/dir")"
check "  and the drop-in is a plain file" test -f "$next" -a ! -L "$next"
helper clear-next-session
chmod 775 "$work/sddm"
check "a group-writable SDDM folder is refused" fails helper set-next-session plasma
chmod 755 "$work/sddm"
mv "$work/sddm" "$work/sddm.real" && ln -s "$work/sddm.real" "$work/sddm"
check "a symlinked SDDM folder is refused" fails helper set-next-session plasma
rm "$work/sddm" && mv "$work/sddm.real" "$work/sddm"
check "an unknown task is refused" fails helper rm -rf /
check "no task is refused" fails helper
check "extra arguments are refused" fails helper update --apply

[ "$failures" = 0 ]
