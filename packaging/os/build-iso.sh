#!/usr/bin/bash
# Build the PS5 Launcher OS installer ISO: Fedora's network installer with
# packaging/os/ps5-launcher-os.ks. Runs in a Fedora container (it needs lorax's mkksiso, and
# Fedora's signing key from /etc/pki/rpm-gpg):
#   docker run --rm --privileged -v "$PWD:/src" -w /src -e IMAGE=ghcr.io/OWNER/ps5-launcher-fedora \
#       -e FEDORA_VERSION -e FEDORA_ISO_VERSION -e FEDORA_GPG_FINGERPRINT -e MAIN_DIGEST=sha256:... "quay.io/fedora/fedora:$FEDORA_VERSION" packaging/os/build-iso.sh ps5-launcher-fedora-x86_64.iso [EXTRA.ks]
# IMAGE: the image name without tag. MAIN_DIGEST: the sha256:... digests the ISO
# installs (the release ISO: the promoted digests, checked against the signing key first; the
# installer itself checks no signature, so the ISO never installs a tag). EXTRA.ks is appended to the kickstart (the unattended
# install test uses it; releases do not). ISO_CACHE keeps Fedora's ISO between runs.
# KERNEL_ARGS: more installer kernel arguments (the install test sends the installer to the serial
# console); releases add none.
set -euo pipefail
out=${1:?output ISO}
extra=${2:-}
image=${IMAGE:?image name without tag, for example ghcr.io/OWNER/ps5-launcher-fedora}
# shellcheck source=packaging/os/config.sh
source "$(dirname "$0")/config.sh"
os_config_validate_iso
release=$FEDORA_VERSION
version=$FEDORA_ISO_VERSION
iso=Fedora-Everything-netinst-x86_64-$version.iso
base=https://dl.fedoraproject.org/pub/fedora/linux/releases/$release/Everything/x86_64/iso
cache=${ISO_CACHE:-/tmp}
# The selected Fedora release's primary key, which signs the CHECKSUM file. The key itself comes from this
# container's fedora-gpg-keys package; the fingerprint is checked too.
fedora_key=/etc/pki/rpm-gpg/RPM-GPG-KEY-fedora-$release-primary
fedora_fpr=$FEDORA_GPG_FINGERPRINT

if ! [[ $image =~ ^[a-z0-9][a-z0-9./_-]*$ ]]; then
    echo "Invalid image name: $image" >&2
    exit 1
fi
main_digest=${MAIN_DIGEST:-}
if ! [[ $main_digest =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "MAIN_DIGEST must be sha256:<64 hex>, got '$main_digest'" >&2
    exit 1
fi

dnf -y -q install lorax xorriso curl gnupg2 >/dev/null
if [ ! -f "$cache/$iso" ]; then
    curl -fsSL --retry 3 -o "$cache/$iso.part" "$base/$iso"
    mv "$cache/$iso.part" "$cache/$iso"
fi

# Only a CHECKSUM file with a good signature from Fedora's key is used to check the ISO.
gpgdir=$(mktemp -d)
curl -fsSL --retry 3 -o "$gpgdir/CHECKSUM.signed" "$base/Fedora-Everything-$version-x86_64-CHECKSUM"
gpg --batch --quiet --homedir "$gpgdir" --import "$fedora_key"
gpg --batch --homedir "$gpgdir" --with-colons --fingerprint | grep -qx "fpr:::::::::$fedora_fpr:"
gpg --batch --homedir "$gpgdir" --export > "$gpgdir/fedora.gpg"
gpgv --keyring "$gpgdir/fedora.gpg" --output "$gpgdir/CHECKSUM" "$gpgdir/CHECKSUM.signed"
want=$(sed -n "s/^SHA256 ($iso) = \([0-9a-f]\{64\}\)$/\1/p" "$gpgdir/CHECKSUM")
[ -n "$want" ] || { echo "$iso is not in Fedora's CHECKSUM file" >&2; exit 1; }
echo "$want  $cache/$iso" | sha256sum -c -

files=$(mktemp -d)
ks=$(mktemp -d)/ps5-launcher-os.ks
sed -e "s|@IMAGE@|$image|" -e "s|@MAIN_DIGEST@|$main_digest|" \
    packaging/os/ps5-launcher-os.ks > "$ks"
if grep -q '@[A-Z_]*@' "$ks"; then
    echo "the kickstart still has a placeholder: $(grep -o '@[A-Z_]*@' "$ks" | head -1)" >&2
    exit 1
fi
if [ -n "$extra" ]; then
    cat "$extra" >> "$ks"
fi
rm -f "$out"
extra_args=()
if [ -n "${KERNEL_ARGS:-}" ]; then
    extra_args=(-c "$KERNEL_ARGS")
fi
mkksiso --ks "$ks" -V "PS5-Launcher-Fedora" "${extra_args[@]}" \
    -R "Install Fedora $release" "Install PS5 Launcher OS" \
    -R "install Fedora $release" "install PS5 Launcher OS" \
    -R 'set default="1"' 'set default="0"' \
    -R "set timeout=60" "set timeout=10" \
    "$cache/$iso" "$out"
# Keep Fedora's media check and rescue entries.
xorriso -osirrox on -indev "$out" -extract /EFI/BOOT/grub.cfg "$files/grub.cfg" 2>/dev/null
cfg=$(cat "$files/grub.cfg")
for want in "Install PS5 Launcher OS" "inst.ks=" 'set default="0"'; do
    grep -qF -- "$want" <<<"$cfg" || { echo "the ISO's boot menu lacks: $want" >&2; exit 1; }
done
for gone in "Install Fedora $release" "install Fedora $release"; do
    if grep -qF -- "$gone" <<<"$cfg"; then
        echo "the ISO's boot menu still has: $gone" >&2
        exit 1
    fi
done
ls -la "$out"
