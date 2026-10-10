#!/usr/bin/env bash
# Shared defaults for local builds and CI. os.yml maps OS_* repository variables to these
# environment variables; callers may override them directly for local builds.
FEDORA_VERSION=${FEDORA_VERSION:-44}
FEDORA_CONTAINER_IMAGE=${FEDORA_CONTAINER_IMAGE:-quay.io/fedora/fedora:$FEDORA_VERSION}
FEDORA_BASE=${FEDORA_BASE:-quay.io/fedora/fedora-bootc:$FEDORA_VERSION}
OS_IMAGE_NAME=${OS_IMAGE_NAME:-ps5-launcher-fedora}
VM_MEMORY_MB=${VM_MEMORY_MB:-6144}
VM_CPUS=${VM_CPUS:-4}
VM_DISK_GB=${VM_DISK_GB:-30}

# Installer revisions and Fedora's checksum-signing key are release-specific. Never reuse
# another release's defaults: changing releases requires explicit installer metadata.
case $FEDORA_VERSION in
44)
    FEDORA_ISO_VERSION=${FEDORA_ISO_VERSION:-44-1.7}
    FEDORA_GPG_FINGERPRINT=${FEDORA_GPG_FINGERPRINT:-36F612DCF27F7D1A48A835E4DBFCF71C6D9F90A6}
    ;;
*)
    FEDORA_ISO_VERSION=${FEDORA_ISO_VERSION:-}
    FEDORA_GPG_FINGERPRINT=${FEDORA_GPG_FINGERPRINT:-}
    ;;
esac

os_config_validate() {
    [[ $FEDORA_VERSION =~ ^[1-9][0-9]*$ ]] || { echo "Invalid FEDORA_VERSION: $FEDORA_VERSION" >&2; return 1; }
    [[ $OS_IMAGE_NAME =~ ^[a-z0-9][a-z0-9._-]*$ ]] || { echo "Invalid OS_IMAGE_NAME: $OS_IMAGE_NAME" >&2; return 1; }
    local value
    for value in "$VM_MEMORY_MB" "$VM_CPUS" "$VM_DISK_GB"; do
        [[ $value =~ ^[1-9][0-9]*$ ]] || { echo "VM resources must be positive integers: $value" >&2; return 1; }
    done
}

os_config_validate_iso() {
    os_config_validate || return
    [[ $FEDORA_ISO_VERSION =~ ^${FEDORA_VERSION}-[0-9]+\.[0-9]+$ ]] || {
        echo "FEDORA_ISO_VERSION must match release $FEDORA_VERSION (set OS_FEDORA_ISO_VERSION in GitHub)." >&2
        return 1
    }
    [[ $FEDORA_GPG_FINGERPRINT =~ ^[A-F0-9]{40}$ ]] || {
        echo "Set FEDORA_GPG_FINGERPRINT to Fedora's primary key fingerprint (OS_FEDORA_GPG_FINGERPRINT in GitHub)." >&2
        return 1
    }
}
export FEDORA_VERSION FEDORA_CONTAINER_IMAGE FEDORA_BASE FEDORA_ISO_VERSION FEDORA_GPG_FINGERPRINT
export OS_IMAGE_NAME VM_MEMORY_MB VM_CPUS VM_DISK_GB
