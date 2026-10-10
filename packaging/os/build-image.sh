#!/usr/bin/env bash
# Build a PS5 Launcher OS image (see packaging/os/README.md).
#
#   LAUNCHER_RPM=dist/ps5-launcher-1.16.0-1.x86_64.rpm packaging/os/build-image.sh main
#
# Run it from the repository root. It stages only the files the image needs in target/os/context
# (the repository's target/ folder is far too big to send as a build context), then builds with
# podman, or with docker when ENGINE=docker.
#
# IMAGE: the image name without tag (default: localhost/ps5-launcher-fedora). The image is
#   tagged IMAGE:main.
# IMAGE_REF: the channel the image follows (default: IMAGE:main).
#   CI sets ghcr.io/OWNER/ps5-launcher-fedora:main.
# SIGNING_PUBKEY_FILE: the public key the image is signed with (default:
#   packaging/os/signing/cosign.pub). The image ships it, with a containers policy that requires
#   it for the image's own repository (the name part of IMAGE_REF).
#
# main only:
# LAUNCHER_RPM: the launcher rpm (from packaging/linux/nfpm.yaml). Required.
# BASE_IMAGE: fedora-bootc, by digest (default: quay.io/fedora/fedora-bootc:$FEDORA_VERSION, unpinned, with a
#   warning). CI resolves the digest once for each run.
#
set -euo pipefail
# shellcheck source=packaging/os/config.sh
source "$(dirname "$0")/config.sh"
os_config_validate
variant=${1:?main}
engine=${ENGINE:-podman}
image=${IMAGE:-localhost/$OS_IMAGE_NAME}
image_ref=${IMAGE_REF:-$image:$variant}
signing_pub=${SIGNING_PUBKEY_FILE:-packaging/os/signing/cosign.pub}
ctx=target/os/context

case $variant in
main) ;;
*)
    echo "unknown variant: $variant (main)" >&2
    exit 2
    ;;
esac
if [ ! -f "$signing_pub" ]; then
    echo "No image signing public key at $signing_pub." >&2
    echo "The project owner makes it once: see packaging/os/signing/README.md." >&2
    echo "For a local test build, make a throwaway one with packaging/os/make-test-key.sh." >&2
    exit 1
fi

rm -rf "$ctx"
mkdir -p "$ctx/launcher" "$ctx/signing"
cp "$signing_pub" "$ctx/signing/cosign.pub"
cp packaging/os/signing/policy.json.in packaging/os/signing/registries.yaml.in "$ctx/signing/"
cp -R packaging/os/files "$ctx/"

# x86_64 image, also on an Apple Silicon Mac (emulated there, so slow, and without the lint,
# which the emulator cannot run).
args=(build --platform linux/amd64
    --build-arg "IMAGE_REF=$image_ref")
case $(uname -m) in
x86_64) ;;
*) args+=(--build-arg LINT=0); echo "Emulated build: skipping bootc container lint" >&2 ;;
esac

case $variant in
main)
    rpm=${LAUNCHER_RPM:?the launcher rpm; see packaging/os/README.md}
    base=${BASE_IMAGE:-$FEDORA_BASE}
    case $base in
    *@sha256:*) ;;
    *) echo "warning: BASE_IMAGE $base is not pinned by digest" >&2 ;;
    esac
    # The version is in the rpm's name: ps5-launcher-<version>-<release>.<arch>.rpm.
    name=$(basename "$rpm")
    version=${LAUNCHER_VERSION:-$(echo "$name" | sed -n 's/^ps5-launcher-\(.*\)-[^-]*\.x86_64\.rpm$/\1/p')}
    if [ -z "$version" ]; then
        echo "Cannot read the launcher version from $name; set LAUNCHER_VERSION" >&2
        exit 1
    fi
    cp "$rpm" "$ctx/launcher/"
    "$engine" "${args[@]}" -f packaging/os/Containerfile \
        --build-arg "BASE_IMAGE=$base" --build-arg "LAUNCHER_VERSION=$version" \
        -t "$image:main" "$ctx"
    ;;
esac
