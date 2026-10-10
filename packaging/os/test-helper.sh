#!/usr/bin/env bash
# Tests packaging/os/files/usr/libexec/ps5-launcher/helper, the launcher's root helper. It runs
# a copy whose fixed paths point at a test folder, with fake bootc, skopeo, verify-image and
# mokutil:
#   packaging/os/test-helper.sh
set -euo pipefail
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/sddm" "$work/certs" "$work/registry"
chmod 755 "$work/sddm"
sed -e "s|^sddm_dir=/etc/sddm.conf.d$|sddm_dir=$work/sddm|" \
    -e "s|^os_release=/usr/lib/ps5-launcher/os-release$|os_release=$work/os-release|" \
    -e "s|^cert_dir=/usr/share/ps5-launcher/secureboot$|cert_dir=$work/certs|" \
    -e "s|^state_dir=/var/lib/ps5-launcher$|state_dir=$work/state|" \
    -e "s|^health_dir=/var/lib/ps5-launcher-os/health$|health_dir=$work/health|" \
    -e "s|^lock_file=/run/ps5-launcher-os.lock$|lock_file=$work/lock|" \
    -e "s|^verify=/usr/libexec/ps5-launcher/verify-image$|verify=$work/bin/verify-image|" \
    -e "s|^short=30$|short=1|" \
    -e "s|^long=7200$|long=2|" \
    -e "s|^PATH=/usr/sbin:/usr/bin$|PATH=$work/bin:/usr/sbin:/usr/bin:/bin|" \
    "$(dirname "$0")/files/usr/libexec/ps5-launcher/helper" > "$work/helper"
chmod +x "$work/helper"
for fixed in "^sddm_dir=$work/sddm$" "^os_release=$work/os-release$" "^cert_dir=$work/certs$" \
    "^state_dir=$work/state$" "^health_dir=$work/health$" "^lock_file=$work/lock$" \
    "^verify=$work/bin/verify-image$" "^short=1$" \
    "^long=2$" "^PATH=$work/bin:"; do
    if ! grep -q "$fixed" "$work/helper"; then
        echo "FAIL the helper's fixed path $fixed was not found"
        exit 1
    fi
done
printf 'IMAGE=main\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:main\n' > "$work/os-release"
da=sha256:$(printf 'a%.0s' $(seq 64))
dc=sha256:$(printf 'c%.0s' $(seq 64))
dm=sha256:$(printf 'e%.0s' $(seq 64))
# The fakes. bootc and timedatectl log their arguments; timedatectl lists the zones in
# $work/zones. skopeo answers from $work/registry/<tag> ("digest label"): the digest only, as
# skopeo inspect checks no signature. verify-image answers for a digest from the same files:
# the label, unless $work/sig/<digest> says "unsigned" or "wrong-key"; it logs its arguments.
# bootc status prints $work/bootc-status.json.
# mokutil answers from $work/sb-state and $work/mok ("enrolled", "pending" or "not"), and logs
# what --import gets on stdin.
# bootc hangs while $work/hang exists, to test the helper's own deadlines.
# shellcheck disable=SC2016 # the fake expands $* and $1 itself, when it runs
printf '#!/bin/sh\necho "$*" >> "%s/bootc.log"\n[ -e "%s/hang" ] && sleep 60\n[ "$1" = status ] && cat "%s/bootc-status.json"\nexit 0\n' \
    "$work" "$work" "$work" > "$work/bin/bootc"
enforced_status() { # SIGNATURE: what bootc status says about the image it follows
    jq -n --arg s "$1" '{spec: {image: ({image: "ghcr.io/owner/ps5-launcher-fedora:main", transport: "registry"}
        + (if $s == "" then {} else {signature: $s} end))}}' > "$work/bootc-status.json"
}
enforced_status containerPolicy
cat > "$work/bin/skopeo" <<FAKE
#!/bin/sh
for a in "\$@"; do case "\$a" in docker://*) ref=\$a ;; esac; done
tag=\${ref##*:}
[ -f "$work/registry/\$tag" ] || { echo "manifest unknown" >&2; exit 1; }
cut -d' ' -f1 "$work/registry/\$tag"
FAKE
cat > "$work/bin/verify-image" <<FAKE
#!/bin/sh
echo "\$*" >> "$work/verify.log"
digest=\${1##*@}
case "\$(cat "$work/sig/\$digest" 2>/dev/null)" in
unsigned) echo "verify-image: OpenImage: A signature was required, but no signature exists" >&2; exit 1 ;;
wrong-key) echo "verify-image: OpenImage: cryptographic signature verification failed" >&2; exit 1 ;;
esac
for f in "$work"/registry/*; do
    read -r d label < "\$f"
    [ "\$d" = "\$digest" ] && { echo "\$label"; exit 0; }
done
echo "verify-image: OpenImage: manifest unknown" >&2
exit 1
FAKE
mkdir -p "$work/sig"
echo "$dm main-label" > "$work/registry/main"
cat > "$work/bin/mokutil" <<FAKE
#!/bin/sh
case "\$1" in
--sb-state) cat "$work/sb-state" ;;
--test-key)
    case \$(cat "$work/mok") in
    enrolled) echo "\$2 is already enrolled"; exit 0 ;;
    pending) echo "\$2 is already in the enrollment request"; exit 0 ;;
    *) echo "\$2 is not enrolled"; exit 1 ;;
    esac ;;
--import)
    [ "\$(cat "$work/mok")" = pending ] && { echo "SKIP: \$2 is already in the enrollment request"; exit 0; }
    echo "\$2" > "$work/import.file"; cat > "$work/import.stdin"; echo pending > "$work/mok" ;;
--revoke-import) echo not > "$work/mok"; echo revoked >> "$work/revoke.log" ;;
esac
FAKE
cat > "$work/bin/timedatectl" <<FAKE
#!/bin/sh
echo "\$*" >> "$work/timedatectl.log"
[ "\$1" = list-timezones ] && cat "$work/zones"
exit 0
FAKE
printf 'Africa/Abidjan\nAmerica/Argentina/Buenos_Aires\nEurope/Berlin\nUTC\n' > "$work/zones"
chmod +x "$work/bin/bootc" "$work/bin/skopeo" "$work/bin/verify-image" "$work/bin/mokutil" "$work/bin/timedatectl"
# macOS has no timeout on the helper's PATH (Homebrew's coreutils has one); Fedora does.
if ! PATH=/usr/sbin:/usr/bin:/bin command -v timeout >/dev/null; then
    ln -s "$(command -v timeout || command -v gtimeout)" "$work/bin/timeout"
fi
# macOS has no sha256sum on the helper's PATH; Fedora does.
if ! PATH=/usr/sbin:/usr/bin:/bin command -v sha256sum >/dev/null; then
    printf '#!/bin/sh\nexec shasum -a 256 "$@"\n' > "$work/bin/sha256sum"
    chmod +x "$work/bin/sha256sum"
fi
# macOS has no flock; this one takes no lock (the lock test below needs a real one).
real_flock=1
if ! PATH=/usr/sbin:/usr/bin:/bin command -v flock >/dev/null; then
    real_flock=0
    printf '#!/bin/sh\nexit 0\n' > "$work/bin/flock"
    chmod +x "$work/bin/flock"
fi
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
    diff <(printf 'upgrade --check\nstatus --json --format-version=1\nupgrade\nrollback\n') "$work/bootc.log"
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
# The NVIDIA image's update gate. The new image's label names its signing certificate by SHA-256.
cert=$(printf 'fake DER certificate' | { sha256sum 2>/dev/null || shasum -a 256; })
cert=${cert%% *}
printf 'fake DER certificate' > "$work/certs/$cert.der"
echo "$da $cert" > "$work/registry/nvidia"
printf 'IMAGE=nvidia\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:nvidia\n' > "$work/os-release"
gate() { # EXPECTED_EXIT, then the helper's arguments; the output goes to $work/out
    local want=$1 got=0
    shift
    : > "$work/bootc.log"
    helper "$@" > "$work/out" || got=$?
    [ "$got" = "$want" ]
}
echo "Cannot determine secure boot state." > "$work/sb-state"
echo not > "$work/mok"
check "an unknown Secure Boot state stops the update" gate 1 update
check "  without touching bootc" test ! -s "$work/bootc.log"
printf 'SecureBoot enabled\nSecureBoot validation is disabled in shim\n' > "$work/sb-state"
check "Secure Boot with validation off in shim needs no key" gate 0 update
echo "SecureBoot disabled" > "$work/sb-state"
check "NVIDIA without Secure Boot needs no key" gate 0 update
check "  but still follows its channel by digest" \
    grep -qx "switch --enforce-container-sigpolicy ghcr.io/owner/ps5-launcher-fedora@$da" "$work/bootc.log"
echo "SecureBoot enabled" > "$work/sb-state"
echo enrolled > "$work/mok"
check "NVIDIA with its key enrolled stages the checked digest" gate 0 update
check "  by that digest, never the moving tag" \
    grep -qx "switch --enforce-container-sigpolicy ghcr.io/owner/ps5-launcher-fedora@$da" "$work/bootc.log"
echo not > "$work/mok"
check "a key that is not enrolled stops the update" gate 3 update
check "  and says key-required" grep -qx "key-required" "$work/out"
check "  without touching bootc" test ! -s "$work/bootc.log"
echo pending > "$work/mok"
check "a key that only waits for the blue screen does not count" gate 4 update
check "  and says key-pending" grep -qx "key-pending" "$work/out"
echo enrolled > "$work/mok"
echo "$da 0000" > "$work/registry/nvidia"
check "an image signed with an unknown key is refused" gate 1 update
check "  without touching bootc" test ! -s "$work/bootc.log"
echo "$da" > "$work/registry/nvidia"
check "an image without the label is refused" gate 1 update
other=$(printf 'another certificate' | { sha256sum 2>/dev/null || shasum -a 256; })
other=${other%% *}
printf 'not what the label says' > "$work/certs/$other.der"
echo "$da $other" > "$work/registry/nvidia"
check "a certificate whose contents do not match its name is refused" gate 1 update
rm "$work/certs/$other.der"
printf 'another certificate' > "$work/elsewhere.der"
ln -s "$work/elsewhere.der" "$work/certs/$other.der"
check "a symlinked certificate is refused" gate 1 update
check "  without touching bootc" test ! -s "$work/bootc.log"
rm "$work/certs/$other.der"
echo "$da $cert" > "$work/registry/nvidia"
echo not > "$work/mok"
check "queue-key queues the new image's key" gate 0 queue-key
check "  with its certificate" grep -qx "$work/certs/$cert.der" "$work/import.file"
check "  and prints an 8-digit password" grep -qxE "[0-9]{8}" "$work/out"
check "  which it gives mokutil twice" diff <(cat "$work/out" "$work/out") "$work/import.stdin"
first=$(cat "$work/out")
check "queue-key again replaces the waiting request" gate 0 queue-key
check "  by revoking it first" grep -qx revoked "$work/revoke.log"
check "  so the new password is the one mokutil got" diff <(cat "$work/out" "$work/out") "$work/import.stdin"
check "  and it is a new one" test "$(cat "$work/out")" != "$first"
# key-state only reads: it never queues or revokes a key.
: > "$work/revoke.log"
echo not > "$work/mok"
rm -f "$work/import.file"
check "key-state with the key missing says missing" gate 0 key-state
check "  in one word" grep -qx missing "$work/out"
check "  and queues nothing" test ! -e "$work/import.file"
echo pending > "$work/mok"
check "key-state with the key queued says pending" gate 0 key-state
check "  in one word" grep -qx pending "$work/out"
check "  and revokes nothing" test ! -s "$work/revoke.log"
echo enrolled > "$work/mok"
check "key-state with the key enrolled says enrolled" gate 0 key-state
check "  in one word" grep -qx enrolled "$work/out"
echo "SecureBoot disabled" > "$work/sb-state"
check "key-state without Secure Boot says secure-boot-off" gate 0 key-state
check "  in one word" grep -qx secure-boot-off "$work/out"
echo "Cannot determine secure boot state." > "$work/sb-state"
check "key-state with an unknown Secure Boot state fails" gate 1 key-state
echo "SecureBoot enabled" > "$work/sb-state"
echo "$da 0000" > "$work/registry/nvidia"
check "key-state for an image signed with an unknown key fails" gate 1 key-state
echo "$da $cert" > "$work/registry/nvidia"
printf 'IMAGE=main\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:main\n' > "$work/os-release"
check "key-state on main asks about the NVIDIA image's key" gate 0 key-state
check "  which is enrolled" grep -qx enrolled "$work/out"
printf 'IMAGE=nvidia\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:nvidia\n' > "$work/os-release"
echo enrolled > "$work/mok"
check "queue-key with the key already enrolled queues nothing" gate 0 queue-key
check "  and says key-enrolled" grep -qx key-enrolled "$work/out"
echo not > "$work/mok"
echo enrolled > "$work/mok"
echo "SecureBoot enabled" > "$work/sb-state"
gate 0 update
check "staging remembers the digest" grep -qx "$da" "$work/state/nvidia-digest"
check "update-check on NVIDIA with nothing new says up-to-date" gate 0 update-check
check "  from the channel, not bootc" grep -qx up-to-date "$work/out"
echo "$dc $cert" > "$work/registry/nvidia"
check "update-check on NVIDIA with a newer image says update-available" gate 0 update-check
check "  and its digest" grep -qx "update-available $dc" "$work/out"
check "  without touching bootc" test ! -s "$work/bootc.log"
echo "$da $cert" > "$work/registry/nvidia"
printf 'IMAGE=main\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:main\n' > "$work/os-release"
check "update-check on main asks bootc" gate 0 update-check
check "  with upgrade --check" grep -qx "upgrade --check" "$work/bootc.log"
check "switch nvidia from main stages the checked NVIDIA digest" gate 0 switch nvidia
check "  on the same repository" \
    grep -qx "switch --enforce-container-sigpolicy ghcr.io/owner/ps5-launcher-fedora@$da" "$work/bootc.log"
echo not > "$work/mok"
check "switch nvidia without the key enrolled says key-required" gate 3 switch nvidia
printf 'IMAGE=nvidia\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:nvidia\n' > "$work/os-release"
check "switch main needs no key" gate 0 switch main
check "  and follows the main tag" grep -qx "switch --enforce-container-sigpolicy ghcr.io/owner/ps5-launcher-fedora:main" "$work/bootc.log"
check "switch to anything else is refused" gate 2 switch ../../evil
check "status reads bootc's state" gate 0 status
check "  as JSON" grep -qx "status --json" "$work/bootc.log"
# Image signatures: every image the helper trusts is checked by its exact digest first. An
# unsigned image, or one signed with another key, stops the task before bootc or mokutil runs.
sig() { # unsigned|wrong-key|signed, then the digests
    local state=$1
    shift
    for d in "$@"; do
        if [ "$state" = signed ]; then rm -f "$work/sig/$d"; else echo "$state" > "$work/sig/$d"; fi
    done
}
refused() { # the helper's arguments: exit 1, and neither bootc nor mokutil's import ran
    rm -f "$work/import.file"
    gate 1 "$@" && test ! -s "$work/bootc.log" && test ! -e "$work/import.file"
}
echo "SecureBoot enabled" > "$work/sb-state"
echo enrolled > "$work/mok"
printf 'IMAGE=nvidia\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:nvidia\n' > "$work/os-release"
for bad in unsigned wrong-key; do
    sig "$bad" "$da"
    : > "$work/verify.log"
    check "NVIDIA update to a $bad image is refused before staging" refused update
    check "  after checking exactly that digest and its label" \
        grep -qx "ghcr.io/owner/ps5-launcher-fedora@$da io.github.ps5-launcher.secureboot-cert-sha256" "$work/verify.log"
    check "update-check on NVIDIA with a $bad image fails" refused update-check
    check "  and offers nothing" fails grep -q update-available "$work/out"
    echo not > "$work/mok"
    check "queue-key for a $bad image queues nothing" refused queue-key
    check "key-state for a $bad image fails" refused key-state
    echo enrolled > "$work/mok"
done
sig signed "$da"
: > "$work/verify.log"
check "a signed NVIDIA image is staged" gate 0 update
check "  by the checked digest, with the policy enforced" \
    grep -qx "switch --enforce-container-sigpolicy ghcr.io/owner/ps5-launcher-fedora@$da" "$work/bootc.log"
check "  and its digest was checked first" grep -q "@$da " "$work/verify.log"
sig unsigned "$dm"
check "switch main from NVIDIA to an unsigned main is refused" refused switch main
sig signed "$dm"
printf 'IMAGE=main\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:main\n' > "$work/os-release"
for bad in unsigned wrong-key; do
    sig "$bad" "$da"
    check "switch nvidia from main to a $bad image is refused" refused switch nvidia
    sig "$bad" "$dm"
    check "update-check on main with a $bad channel fails before bootc" refused update-check
    check "switch main to a $bad image is refused" refused switch main
done
sig signed "$da" "$dm"
enforced_status ""
check "update on main without the stored enforcement" gate 0 update
check "  switches to its channel with the enforcement" \
    grep -qx "switch --enforce-container-sigpolicy ghcr.io/owner/ps5-launcher-fedora:main" "$work/bootc.log"
check "  and does not run a plain bootc upgrade" fails grep -qx upgrade "$work/bootc.log"
sig unsigned "$dm"
: > "$work/bootc.log"
check "  an unsigned channel is refused there too" gate 1 update
check "    without a switch" fails grep -q "^switch" "$work/bootc.log"
sig signed "$dm"
enforced_status containerPolicy
check "update on main with the stored enforcement runs bootc upgrade" gate 0 update
check "  which follows the stored choice" grep -qx upgrade "$work/bootc.log"
# The helper stops a task that runs too long by itself (as root; the launcher's own deadline
# cannot stop a pkexec'd process). The test copy's long deadline is 2 s.
printf 'IMAGE=main\nIMAGE_REF=ghcr.io/owner/ps5-launcher-fedora:main\n' > "$work/os-release"
touch "$work/hang"
start=$(date +%s)
check "a download that hangs is stopped by the helper's deadline" gate 124 update
check "  within seconds" test $(($(date +%s) - start)) -lt 15
rm "$work/hang"
# update, switch and rollback wait for the lock the boot health check shares with them.
if [ "$real_flock" = 1 ]; then
    : > "$work/bootc.log"
    flock "$work/lock" sleep 4 &
    holder=$!
    sleep 0.5
    check "rollback waits for the shared lock, then gives up" gate 1 rollback
    check "  without touching bootc" test ! -s "$work/bootc.log"
    check "switch gives up too" gate 1 switch main
    wait "$holder"
    check "rollback runs once the lock is free" gate 0 rollback
else
    echo "skip the lock tests: no flock here"
fi
# The time zone and automatic time: timedated wants an admin password for both
# (auth_admin_keep), so the helper sets them, and only to a zone timedatectl lists.
tz() { # EXPECTED_EXIT, then the helper's arguments
    local want=$1 got=0
    shift
    : > "$work/timedatectl.log"
    helper "$@" > "$work/out" || got=$?
    [ "$got" = "$want" ]
}
check "set-timezone sets a listed zone" tz 0 set-timezone Europe/Berlin
check "  with timedatectl" grep -qx "set-timezone Europe/Berlin" "$work/timedatectl.log"
check "set-timezone takes three parts" tz 0 set-timezone America/Argentina/Buenos_Aires
check "set-timezone takes a zone without a region" tz 0 set-timezone UTC
for bad in ../../etc/shadow /etc/passwd Europe/../../etc/shadow Europe/Nowhere Europe "" -h \
    "Europe/Berlin
UTC" "Europe/Berlin "; do
    check "set-timezone refuses ${bad:-an empty zone}" tz 1 set-timezone "$bad"
    check "  without setting it" fails grep -q "^set-timezone" "$work/timedatectl.log"
done
check "set-timezone with no zone is refused" tz 2 set-timezone
check "set-timezone with two zones is refused" tz 2 set-timezone UTC Europe/Berlin
check "set-ntp on turns automatic time on" tz 0 set-ntp on
check "  with timedatectl" grep -qx "set-ntp true" "$work/timedatectl.log"
check "set-ntp off turns it off" tz 0 set-ntp off
check "  with timedatectl" grep -qx "set-ntp false" "$work/timedatectl.log"
check "set-ntp takes only on or off" tz 2 set-ntp yes
check "  without touching timedatectl" test ! -s "$work/timedatectl.log"

mkdir -p "$work/health"
echo '{}' > "$work/health/notice.json"
check "health-ack deletes the boot check's notice" helper health-ack
check "  so it is gone" test ! -e "$work/health/notice.json"
check "health-ack with no notice is fine" helper health-ack
check "an unknown task is refused" fails helper rm -rf /
check "no task is refused" fails helper
check "extra arguments are refused" fails helper update --apply

# The power-key holder the launcher runs under systemd-inhibit: one byte "r", then it waits for
# the end of its input and exits 0.
hold="$(dirname "$0")/files/usr/libexec/ps5-launcher-os/power-key-hold"
check "power-key-hold writes exactly r" test "$(: | "$hold" | od -An -c | tr -d ' ')" = r
check "  and exits 0 at the end of input" sh -c ": | '$hold' >/dev/null"
{ sleep 2; } | "$hold" > "$work/hold.out" &
holder=$!
sleep 1
check "  it waits while its input stays open" kill -0 "$holder"
check "  after saying r" test "$(cat "$work/hold.out")" = r
wait "$holder"
check "  then ends with exit 0" test $? = 0

[ "$failures" = 0 ]
