#!/usr/bin/env bash
# CI only: the install test (the "install-test" job in .github/workflows/os.yml). Run it
# from the repository root on an x86_64 Linux host with KVM, QEMU, OVMF, socat, jq and docker.
#
# 1. Points FOLLOW_TAG at the main candidate, builds the installer ISO with an extra kickstart
#    (install-test.ks.in) and installs the candidate main image in a UEFI VM, with no one at the
#    keyboard. The installed system follows FOLLOW_TAG.
# 2. Boots it, and checks the system and the login session (check-system main --session
#    --enforced): ps5-signature-policy.service has made bootc store the signature enforcement,
#    by staging the same digest with --enforce-container-sigpolicy. Restart: the booted
#    deployment is enforced too.
# 3. The signature gates: bootc switch to the unsigned and to the wrong-key image is refused,
#    with and without --enforce-container-sigpolicy, and stages nothing. Then FOLLOW_TAG is moved
#    to the unsigned image, then to the wrong-key one: bootc upgrade refuses both. Then to the
#    signed upgrade image (the candidate plus a marker file): bootc upgrade stages it; restart,
#    check; bootc rollback, restart, check that the candidate runs again.
# 4. bootc switch --enforce-container-sigpolicy to the NVIDIA candidate, restart, check its kernel
#    arguments and modules (the VM has no NVIDIA card, so the driver cannot load); switch back to
#    main, restart, check that no NVIDIA kernel argument or file is left.
# Every step must pass; the first failure ends the test with a non-zero exit.
#
# The VM pulls the images from the registry, so they must be public. The host moves FOLLOW_TAG
# with `sudo skopeo copy`, so root's registry login must be in place (the workflow does it).
#
# IMAGE         image name without tag (ghcr.io/OWNER/ps5-launcher-fedora)
# MAIN_DIGEST, NVIDIA_DIGEST, UPGRADE_DIGEST   sha256:... of the three signed candidates
# MAIN_TAG, NVIDIA_TAG                         their testing tags
# UNSIGNED_DIGEST, UNSIGNED_TAG    CI only: the upgrade image, never signed
# WRONGKEY_DIGEST, WRONGKEY_TAG    CI only: the upgrade image, signed with a throwaway key
# FOLLOW_TAG    CI only: the tag the installed VM follows; this test moves it
# SECUREBOOT_CERT_FILE   the certificate for the ISO
# OUT           folder for the logs, screenshots and the test ISO (default target/os/install-test)
set -euo pipefail
: "${IMAGE:?}" "${MAIN_DIGEST:?}" "${NVIDIA_DIGEST:?}" "${UPGRADE_DIGEST:?}"
: "${MAIN_TAG:?}" "${NVIDIA_TAG:?}" "${FOLLOW_TAG:?}"
: "${UNSIGNED_DIGEST:?}" "${UNSIGNED_TAG:?}" "${WRONGKEY_DIGEST:?}" "${WRONGKEY_TAG:?}"
out=${OUT:-target/os/install-test}
here=$(dirname "$0")
ovmf_code=${OVMF_CODE:-/usr/share/OVMF/OVMF_CODE_4M.fd}
ovmf_vars=${OVMF_VARS:-/usr/share/OVMF/OVMF_VARS_4M.fd}
port=2222
password=tester

mkdir -p "$out"
out=$(cd "$out" && pwd)
step() { printf '\n=== %s\n' "$*"; }
die() {
    echo "FAIL: $*" >&2
    screenshot "failure"
    exit 1
}

if [ ! -w /dev/kvm ]; then
    echo "FAIL: no usable /dev/kvm: the install test needs KVM" >&2
    exit 1
fi

# --- VM helpers -----------------------------------------------------------------------------
qmp="$out/qmp.sock"
pidfile="$out/qemu.pid"
screenshot() { # NAME: a PPM of the VM screen, when a VM runs
    [ -S "$qmp" ] || return 0
    printf '{"execute":"qmp_capabilities"}\n{"execute":"screendump","arguments":{"filename":"%s/%s.ppm"}}\n' \
        "$out" "$1" | timeout 10 socat - "UNIX-CONNECT:$qmp" >/dev/null 2>&1 || true
}
start_vm() { # SERIAL_LOG, then more QEMU arguments
    local serial=$1
    shift
    rm -f "$qmp" "$pidfile"
    qemu-system-x86_64 -enable-kvm -machine q35 -cpu host -m 6G -smp 4 \
        -drive "if=pflash,format=raw,readonly=on,file=$ovmf_code" \
        -drive "if=pflash,format=raw,file=$out/vars.fd" \
        -drive "file=$out/disk.qcow2,if=virtio,format=qcow2" \
        -netdev "user,id=n0,hostfwd=tcp:127.0.0.1:$port-:22" -device virtio-net-pci,netdev=n0 \
        -device virtio-vga -display none \
        -qmp "unix:$qmp,server,nowait" -serial "file:$serial" \
        -daemonize -pidfile "$pidfile" "$@"
}
vm_running() { [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; }
stop_vm() {
    vm_running || return 0
    kill "$(cat "$pidfile")" 2>/dev/null || true
    sleep 3
}
trap stop_vm EXIT

ssh_opts=(-i "$out/id_ed25519" -p "$port" -o BatchMode=yes -o ConnectTimeout=5
    -o ServerAliveInterval=5 -o ServerAliveCountMax=3
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR)
# shellcheck disable=SC2029 # the commands are fixed strings of this script, meant for the VM
vm() { ssh "${ssh_opts[@]}" tester@127.0.0.1 "$@"; }
boot_id() { vm cat /proc/sys/kernel/random/boot_id 2>/dev/null; }
wait_ssh() { # [OLD_BOOT_ID]: wait up to 10 minutes for SSH, on a new boot when given
    local old=${1:-} id
    for _ in $(seq 120); do
        id=$(boot_id || true)
        if [ -n "$id" ] && [ "$id" != "$old" ]; then
            return 0
        fi
        vm_running || die "the VM stopped"
        sleep 5
    done
    die "no SSH into the VM after 10 minutes"
}
restart_vm() { # NAME: restart and wait for the new boot
    local old
    old=$(boot_id)
    vm sudo -n systemctl reboot || true
    wait_ssh "$old"
    screenshot "$1"
}
booted_digest() {
    # bootc may print a progress line before the JSON.
    vm sudo -n bootc status --json 2>/dev/null | tr -d '\r' | sed -n '/{/,$p' | sed '1s/^[^{]*//' | jq -r '.status.booted.image.imageDigest'
}
expect_digest() { # WANT, WHAT
    local got
    got=$(booted_digest)
    [ "$got" = "$1" ] || die "$2: booted $got, expected $1"
    echo "ok   $2 runs ($1)"
}
# The JSON of bootc status in the VM (bootc may print a progress line before it).
vm_status() { vm sudo -n bootc status --json --format-version=1 2>/dev/null | tr -d '\r' | sed -n '/{/,$p'; }
# Point FOLLOW_TAG at DIGEST in the registry (the VM's next bootc upgrade reads it).
follow() { # DIGEST
    local got
    sudo skopeo copy --quiet --retry-times 3 --preserve-digests "docker://$IMAGE@$1" "docker://$IMAGE:$FOLLOW_TAG" ||
        die "could not point $FOLLOW_TAG at $1"
    got=sha256:$(sudo skopeo inspect --raw "docker://$IMAGE:$FOLLOW_TAG" | sha256sum | cut -d' ' -f1)
    [ "$got" = "$1" ] || die "$FOLLOW_TAG is $got, not $1"
    echo "ok   $FOLLOW_TAG -> $1"
}
nothing_staged() { # WHAT
    [ "$(vm_status | jq -r '.status.staged')" = null ] || die "$1 staged a deployment"
}
# bootc status shows the signature enforcement as "signature": "containerPolicy" in an image
# reference (bootc 1.16.13 crates/lib/src/spec.rs 73-83; an unverified origin has no signature).
expect_booted_enforced() { # WHAT
    local got
    got=$(vm_status | jq -r '.status.booted.image.image.signature // "none"')
    [ "$got" = containerPolicy ] || die "$1: the booted deployment's signature is $got, not containerPolicy"
    echo "ok   $1: the booted deployment enforces the signature policy"
}
refused() { # WHAT, then the bootc command: it must fail on the signature and stage nothing
    local what=$1 out
    shift
    if out=$(vm sudo -n "$@" 2>&1); then
        echo "$out" | tail -5
        die "$what was accepted"
    fi
    echo "$out" | tail -3
    grep -qiE 'signature was required|signature verification failed|invalid signature' <<<"$out" ||
        die "$what failed, but not on the signature"
    nothing_staged "$what"
    echo "ok   $what was refused on its signature"
}
check_system() { # main|nvidia [--session] [--enforced], LOG
    local log=$1
    shift
    vm sudo -n /usr/bin/bash -s "$@" < "$here/check-system" | tee "$out/$log" ||
        die "check-system $* failed (see $log)"
}

# --- 1. Install -----------------------------------------------------------------------------
step "Pointing $FOLLOW_TAG at the main candidate"
follow "$MAIN_DIGEST"

step "Building the test ISO"
ssh-keygen -q -t ed25519 -N "" -f "$out/id_ed25519" <<<y >/dev/null
sed -e "s|@PASSWORD@|$password|" \
    -e "s|@SSHKEY@|$(cat "$out/id_ed25519.pub")|" \
    -e "s|@SOURCE@|$IMAGE@$MAIN_DIGEST|" \
    -e "s|@TARGET@|$IMAGE:$FOLLOW_TAG|" \
    "$here/install-test.ks.in" > "$out/install-test.ks"
docker run --rm --privileged -v "$PWD:/src" -v "$out:/out" -w /src \
    -e IMAGE="$IMAGE" -e MAIN_DIGEST="$MAIN_DIGEST" -e NVIDIA_DIGEST="$NVIDIA_DIGEST" \
    -e ISO_CACHE=/out -e KERNEL_ARGS="console=ttyS0,115200 inst.text" \
    -e SECUREBOOT_CERT_FILE="${SECUREBOOT_CERT_FILE:-packaging/os/secureboot/public_key.der}" \
    quay.io/fedora/fedora:44 packaging/os/build-iso.sh /out/install-test.iso /out/install-test.ks

step "Installing in a UEFI VM (up to 60 minutes)"
cp "$ovmf_vars" "$out/vars.fd"
qemu-img create -q -f qcow2 "$out/disk.qcow2" 30G
start_vm "$out/install-serial.log" -drive "file=$out/install-test.iso,media=cdrom,readonly=on"
for i in $(seq 720); do
    vm_running || break
    [ $((i % 60)) = 0 ] && screenshot "install-$((i / 12))min"
    sleep 5
done
if vm_running; then
    die "the installer did not finish within 60 minutes (see install-serial.log)"
fi
# The kickstart ends with poweroff. An error stops the installer with the VM still running, so
# a VM that stopped by itself finished the install; the first boot below proves it.
echo "ok   the installer finished and powered off"

# --- 2. First boot --------------------------------------------------------------------------
step "First boot"
start_vm "$out/boot1-serial.log"
wait_ssh
screenshot "boot1"
# The rest of the test uses sudo without a password; /etc keeps it across deployments.
# ssh joins its arguments into one remote command, so the command is one quoted string.
vm "sudo -S -p '' sh -c 'echo \"tester ALL=(ALL) NOPASSWD: ALL\" > /etc/sudoers.d/90-install-test && chmod 440 /etc/sudoers.d/90-install-test'" \
    <<<"$password"
vm "sudo -n sshd -T | grep -qx 'passwordauthentication no'" ||
    die "SSH in the test VM takes passwords: the kickstart's sshd drop-in is missing"
echo "ok   SSH in the test VM takes keys only"
expect_digest "$MAIN_DIGEST" "the installed candidate"
# check-system waits for ps5-signature-policy.service, so no bootc command below races it.
check_system boot1-check.log main --session --enforced
screenshot "boot1-session"
[ "$(vm_status | jq -r '.status.staged.image.imageDigest')" = "$MAIN_DIGEST" ] ||
    die "the enforcement did not stage the installed digest again"
echo "ok   the enforcement staged the same digest, $MAIN_DIGEST"

step "Restart into the enforced deployment"
restart_vm "boot2"
expect_digest "$MAIN_DIGEST" "the enforced deployment"
expect_booted_enforced "the enforced deployment"
check_system boot2-check.log main --enforced

# --- 3. Signature gates, upgrade and rollback -----------------------------------------------
step "bootc switch to unsigned and wrong-key images is refused"
refused "bootc switch --enforce-container-sigpolicy to the unsigned image" \
    bootc switch --enforce-container-sigpolicy "$IMAGE:$UNSIGNED_TAG"
refused "bootc switch --enforce-container-sigpolicy to the wrong-key image" \
    bootc switch --enforce-container-sigpolicy "$IMAGE:$WRONGKEY_TAG"
# The flag only stores the choice: the image's policy refuses an unsigned image of our repository
# on every pull (bootc passes no --insecure-policy to skopeo).
refused "bootc switch without the flag to the unsigned image" bootc switch "$IMAGE:$UNSIGNED_TAG"
refused "bootc switch without the flag to the wrong-key image" bootc switch "$IMAGE@$WRONGKEY_DIGEST"

step "bootc upgrade to unsigned and wrong-key images is refused"
follow "$UNSIGNED_DIGEST"
refused "bootc upgrade to the unsigned image" bootc upgrade
follow "$WRONGKEY_DIGEST"
refused "bootc upgrade to the wrong-key image" bootc upgrade

step "bootc upgrade to the signed image"
follow "$UPGRADE_DIGEST"
vm sudo -n bootc upgrade | tail -20
restart_vm "upgrade"
expect_digest "$UPGRADE_DIGEST" "the upgrade"
expect_booted_enforced "the upgrade"
vm test -f /usr/lib/ps5-launcher/upgrade-test || die "the upgrade's marker file is missing"
check_system upgrade-check.log main --enforced

step "bootc rollback"
vm sudo -n bootc rollback | tail -20
restart_vm "rollback"
expect_digest "$MAIN_DIGEST" "the rollback"
vm test ! -e /usr/lib/ps5-launcher/upgrade-test || die "the rollback still has the upgrade's marker"
check_system rollback-check.log main --enforced

# --- 4. Main to NVIDIA and back -------------------------------------------------------------
step "bootc switch to NVIDIA"
vm sudo -n bootc switch --enforce-container-sigpolicy "$IMAGE:$NVIDIA_TAG" | tail -20
restart_vm "nvidia"
expect_digest "$NVIDIA_DIGEST" "the NVIDIA image"
expect_booted_enforced "the NVIDIA image"
check_system nvidia-check.log nvidia --enforced

step "bootc switch back to main"
vm sudo -n bootc switch --enforce-container-sigpolicy "$IMAGE:$MAIN_TAG" | tail -20
restart_vm "main-again"
expect_digest "$MAIN_DIGEST" "main after NVIDIA"
expect_booted_enforced "main after NVIDIA"
check_system main-again-check.log main --session --enforced

vm sudo -n systemctl poweroff || true
echo
echo "RESULT: PASS (install, signature enforcement and gates, upgrade, rollback, switch to NVIDIA and back)"
