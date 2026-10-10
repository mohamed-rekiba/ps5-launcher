#!/usr/bin/env bash
# Tests packaging/os/files/usr/libexec/ps5-launcher-os/signature-policy, which makes bootc store
# the signature enforcement on a fresh install. It runs a copy whose fixed paths point at a test
# folder, with a fake bootc (its status comes from a file; a switch with the enforcement flag
# makes that status enforced) and a fake nm-online:
#   packaging/os/test-signature-policy.sh
# It needs Linux (flock), jq and GNU coreutils. On macOS it runs itself in a Fedora container.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=packaging/os/config.sh
source "$here/config.sh"
os_config_validate
if [ "$(uname -s)" != Linux ]; then
    repo=$(cd "$here/../.." && pwd)
    exec docker run --rm -v "$repo:/src:ro" -w /src -e FEDORA_VERSION "$FEDORA_CONTAINER_IMAGE" sh -c \
        'dnf -y -q install jq util-linux diffutils >/dev/null && packaging/os/test-signature-policy.sh'
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
sed -e "s|^os_release=/usr/lib/ps5-launcher/os-release$|os_release=$work/os-release|" \
    -e "s|^state=/var/lib/ps5-launcher-os/signature-policy.json$|state=$work/state/signature-policy.json|" \
    -e "s|^lock_file=/run/ps5-launcher-os.lock$|lock_file=$work/lock|" \
    -e "s|^lock_wait=60$|lock_wait=1|" \
    -e "s|^health_wait=190$|health_wait=4|" \
    -e "s|^PATH=/usr/sbin:/usr/bin$|PATH=$work/bin:/usr/sbin:/usr/bin:/bin|" \
    "$here/files/usr/libexec/ps5-launcher-os/signature-policy" > "$work/signature-policy"
chmod +x "$work/signature-policy"
for fixed in "^os_release=$work/" "^state=$work/" "^lock_file=$work/" "^lock_wait=1$" "^health_wait=4$" \
    "^PATH=$work/bin:"; do
    grep -q "$fixed" "$work/signature-policy" || { echo "FAIL the fixed line $fixed was not found"; exit 1; }
done
repo=ghcr.io/owner/ps5-launcher-fedora
booted=sha256:$(printf 'b%.0s' $(seq 64))
# bootc: "status" prints $work/status.json; "switch" logs its arguments, fails when
# $work/switch-fails exists, and otherwise marks the spec enforced (as a staged deployment would).
cat > "$work/bin/bootc" <<FAKE
#!/bin/sh
case "\$1" in
status) cat "$work/status.json" ;;
switch)
    echo "\$*" >> "$work/bootc.log"
    [ -e "$work/switch-fails" ] && { echo "error: Source image rejected: A signature was required" >&2; exit 1; }
    [ "\$2" = --enforce-container-sigpolicy ] || exit 0
    jq '.spec.image.signature = "containerPolicy" | .status.staged = {}' "$work/status.json" > "$work/s.tmp"
    mv "$work/s.tmp" "$work/status.json" ;;
esac
FAKE
printf '#!/bin/sh\nexit 0\n' > "$work/bin/nm-online"
# systemctl: the boot health check is active while $work/health-active exists.
printf '#!/bin/sh\n[ -e "%s/health-active" ]\n' "$work" > "$work/bin/systemctl"
chmod +x "$work/bin/bootc" "$work/bin/nm-online" "$work/bin/systemctl"

status() { # IMAGE TRANSPORT SIGNATURE STAGED ROLLBACK (signature "" = none; staged/rollback true|false)
    jq -n --arg i "$1" --arg t "$2" --arg s "$3" --argjson st "$4" --argjson rb "$5" --arg d "$booted" '{
        spec: {image: ({image: $i, transport: $t} + (if $s == "" then {} else {signature: $s} end))},
        status: {booted: {image: {imageDigest: $d}},
            staged: (if $st then {} else null end), rollback: (if $rb then {} else null end)}}' \
        > "$work/status.json"
}
failures=0
check() { # NAME, then the command that must succeed
    local name=$1
    shift
    if "$@"; then echo "ok   $name"; else echo "FAIL $name"; failures=$((failures + 1)); fi
}
run() { # EXPECTED_EXIT
    local got=0
    : > "$work/bootc.log"
    "$work/signature-policy" > "$work/out" 2>&1 || got=$?
    [ "$got" = "$1" ]
}
state() { jq -r .state "$work/state/signature-policy.json"; }

printf 'IMAGE=main\nIMAGE_REF=%s:main\n' "$repo" > "$work/os-release"
status "$repo:main" registry "" false false
check "a fresh main install is switched with the enforcement" run 0
check "  to the tag it follows" grep -qx "switch --enforce-container-sigpolicy $repo:main" "$work/bootc.log"
check "  and the state says enforced" test "$(state)" = enforced
check "  readable by the launcher" test "$(stat -c %a "$work/state/signature-policy.json")" = 644
check "  in version 1" test "$(jq -r .version "$work/state/signature-policy.json")" = 1
check "the next start changes nothing" run 0
check "  no switch" test ! -s "$work/bootc.log"
check "  still enforced" test "$(state)" = enforced

printf 'IMAGE=main\nIMAGE_REF=%s:main\n' "$repo" > "$work/os-release"
status "$repo:main" registry "" false true
check "after a rollback to the installed system nothing is staged again" run 0
check "  no switch" test ! -s "$work/bootc.log"
check "  and the state says not-enforced" test "$(state)" = not-enforced
status "$repo:main" registry "" true false
check "with a deployment staged nothing is switched either" run 0
check "  no switch" test ! -s "$work/bootc.log"

status "localhost/ps5-launcher-fedora:boottest" registry "" false false
check "a local test image is left alone" run 0
check "  no switch" test ! -s "$work/bootc.log"
check "  and the state says not-our-image" test "$(state)" = not-our-image
status "$repo:main" containers-storage "" false false
check "another transport is left alone too" run 0
check "  no switch" test ! -s "$work/bootc.log"
status "ghcr.io/owner/ps5-launcher-fedora-evil:main" registry "" false false
check "a repository that only starts with ours is not ours" run 0
check "  no switch" test ! -s "$work/bootc.log"

status "$repo:main" registry "" false false
touch "$work/switch-fails"
check "a failed switch fails the unit" run 1
check "  and the state says failed" test "$(state)" = failed
check "  with bootc's reason" grep -q "A signature was required" "$work/state/signature-policy.json"
rm "$work/switch-fails"
check "the next start tries again, and enforces" run 0
check "  enforced" test "$(state)" = enforced

status "$repo:main" registry "" false false
touch "$work/health-active"
(sleep 2 && rm "$work/health-active") &
start=$(date +%s)
check "the switch waits for the boot health check to end" run 0
check "  then switches" grep -qx "switch --enforce-container-sigpolicy $repo:main" "$work/bootc.log"
check "  after it ended" test $(($(date +%s) - start)) -ge 2
wait
touch "$work/health-active"
start=$(date +%s)
status "$repo:main" registry "" false false
check "a boot health check that does not end is waited for only a while" run 0
check "  then the switch runs anyway" test -s "$work/bootc.log"
check "  after the wait" test $(($(date +%s) - start)) -ge 4
rm "$work/health-active"

status "$repo:main" registry "" false false
flock "$work/lock" sleep 3 &
holder=$!
sleep 0.5
check "a held lock fails the unit, without a switch" run 1
check "  no switch" test ! -s "$work/bootc.log"
wait "$holder"
printf 'IMAGE=main\n' > "$work/os-release"
check "an OS marker without IMAGE_REF fails" run 1
echo '{}' > "$work/status.json"
printf 'IMAGE=main\nIMAGE_REF=%s:main\n' "$repo" > "$work/os-release"
check "a bootc status without an image fails" run 1
# A disk made straight from the image (image-builder, the CI boot test) follows no registry:
# bootc shows a booted system with no image spec. That is not our image, not a failure.
jq -n --arg d "$booted" '{spec: {image: null}, status: {booted: {image: {imageDigest: $d}}, staged: null, rollback: null}}' \
    > "$work/status.json"
check "a system that follows no image is left alone" run 0
check "  no switch" test ! -s "$work/bootc.log"
check "  and the state says not-our-image" test "$(state)" = not-our-image

[ "$failures" = 0 ]
