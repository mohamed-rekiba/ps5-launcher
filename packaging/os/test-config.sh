#!/usr/bin/env bash
# Release changes must not reuse another release's installer or checksum key.
# Expressions in bash -c are intentionally expanded by the child shell.
# shellcheck disable=SC2016
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
check() {
    env -u FEDORA_VERSION -u FEDORA_BASE -u FEDORA_CONTAINER_IMAGE -u FEDORA_ISO_VERSION \
        -u FEDORA_GPG_FINGERPRINT -u OS_IMAGE_NAME -u VM_MEMORY_MB -u VM_CPUS -u VM_DISK_GB \
        "$@" bash -euo pipefail -c 'source "$1/config.sh"; os_config_validate_iso' bash "$here"
}
check
if check FEDORA_VERSION=45 >/dev/null 2>&1; then
    echo 'FAIL: a new release reused installer metadata'; exit 1
fi
check FEDORA_VERSION=45 FEDORA_ISO_VERSION=45-1.2 \
    FEDORA_GPG_FINGERPRINT=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
if check FEDORA_VERSION=45 FEDORA_ISO_VERSION=44-1.7 \
    FEDORA_GPG_FINGERPRINT=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA >/dev/null 2>&1; then
    echo 'FAIL: mismatched installer accepted'; exit 1
fi
for setting in FEDORA_VERSION=bad OS_IMAGE_NAME=Bad/Name VM_MEMORY_MB=0 VM_CPUS=-1 \
    VM_DISK_GB=abc FEDORA_GPG_FINGERPRINT=invalid; do
    if check "$setting" >/dev/null 2>&1; then
        echo "FAIL: accepted $setting"; exit 1
    fi
done
env -u FEDORA_BASE -u FEDORA_CONTAINER_IMAGE FEDORA_VERSION=45 \
    bash -euo pipefail -c 'source "$1/config.sh"; [[ $FEDORA_BASE == quay.io/fedora/fedora-bootc:45 && $FEDORA_CONTAINER_IMAGE == quay.io/fedora/fedora:45 ]]' bash "$here"
echo 'OS build configuration tests passed'
