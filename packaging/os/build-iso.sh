#!/usr/bin/bash
# Build the PS5 Launcher OS installer ISO: Fedora's network installer with
# packaging/os/ps5-launcher-os.ks. Runs in a Fedora container (it needs lorax's mkksiso, and
# Fedora's signing key from /etc/pki/rpm-gpg):
#   docker run --rm --privileged -v "$PWD:/src" -w /src -e IMAGE=ghcr.io/OWNER/ps5-launcher-fedora \
#       -e MAIN_DIGEST=sha256:... -e NVIDIA_DIGEST=sha256:... quay.io/fedora/fedora:44 packaging/os/build-iso.sh ps5-launcher-fedora-x86_64.iso [EXTRA.ks]
# IMAGE: the image name without tag. MAIN_DIGEST, NVIDIA_DIGEST: the sha256:... digests the ISO
# installs (the release ISO: the promoted digests, checked against the signing key first; the
# installer itself checks no signature, so the ISO never installs a tag). EXTRA.ks is appended to the kickstart (the unattended
# install test uses it; releases do not). SECUREBOOT_CERT_FILE: the certificate of the key the
# NVIDIA image's modules are signed with. ISO_CACHE: where Fedora's ISO is kept between runs.
# KERNEL_ARGS: more installer kernel arguments (the install test sends the installer to the serial
# console); releases add none.
set -euo pipefail
out=${1:?output ISO}
extra=${2:-}
image=${IMAGE:?image name without tag, for example ghcr.io/OWNER/ps5-launcher-fedora}
cert=${SECUREBOOT_CERT_FILE:-packaging/os/secureboot/public_key.der}
release=44
version=44-1.7
iso=Fedora-Everything-netinst-x86_64-$version.iso
base=https://dl.fedoraproject.org/pub/fedora/linux/releases/$release/Everything/x86_64/iso
cache=${ISO_CACHE:-/tmp}
# Fedora 44's primary key, which signs the CHECKSUM file. The key itself comes from this
# container's fedora-gpg-keys package; the fingerprint is checked too.
fedora_key=/etc/pki/rpm-gpg/RPM-GPG-KEY-fedora-$release-primary
fedora_fpr=36F612DCF27F7D1A48A835E4DBFCF71C6D9F90A6

if ! [[ $image =~ ^[a-z0-9][a-z0-9./_-]*$ ]]; then
    echo "Invalid image name: $image" >&2
    exit 1
fi
main_digest=${MAIN_DIGEST:-}
nvidia_digest=${NVIDIA_DIGEST:-}
for digest in "$main_digest" "$nvidia_digest"; do
    if ! [[ $digest =~ ^sha256:[0-9a-f]{64}$ ]]; then
        echo "MAIN_DIGEST and NVIDIA_DIGEST must be sha256:<64 hex>, got '$digest'" >&2
        exit 1
    fi
done
if [ ! -f "$cert" ]; then
    echo "No Secure Boot certificate at $cert: see packaging/os/secureboot/README.md" >&2
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
cp "$cert" "$files/ps5-launcher-os-secureboot.der"
ks=$(mktemp -d)/ps5-launcher-os.ks
sed -e "s|@IMAGE@|$image|" -e "s|@MAIN_DIGEST@|$main_digest|" -e "s|@NVIDIA_DIGEST@|$nvidia_digest|" \
    packaging/os/ps5-launcher-os.ks > "$ks"
if grep -q '@[A-Z_]*@' "$ks"; then
    echo "the kickstart still has a placeholder: $(grep -o '@[A-Z_]*@' "$ks" | head -1)" >&2
    exit 1
fi
if [ -n "$extra" ]; then
    cat "$extra" >> "$ks"
fi
rm -f "$out"
# The boot menu: "Install PS5 Launcher OS" (default, after 10 s); in place of Fedora's media
# check, "Install with the NVIDIA driver", for NVIDIA cards that show nothing on the open-source
# driver; and under Troubleshooting, in place of Fedora's rescue entry, "Enroll the Secure Boot
# key again". The two Secure Boot entries show the key password, 12345678: the blue MOK screen
# asks for it before Linux runs. mkksiso applies the replacements in this order, so the media
# check entry is renamed before the plain "install Fedora" replacement could touch it.
pw="Secure Boot key password: 12345678"
extra_args=()
if [ -n "${KERNEL_ARGS:-}" ]; then
    extra_args=(-c "$KERNEL_ARGS")
fi
mkksiso --ks "$ks" -V "PS5-Launcher-Fedora" "${extra_args[@]}" \
    -R "Test this media & install Fedora $release" "Install with the NVIDIA driver ($pw)" \
    -R "rd.live.check" "ps5los.nvidia" \
    -R "Install Fedora $release" "Install PS5 Launcher OS" \
    -R "install Fedora $release" "install PS5 Launcher OS" \
    -R "Rescue a Fedora system" "Enroll the Secure Boot key again ($pw)" \
    -R "inst.rescue" "ps5los.enroll" \
    -a "$files/ps5-launcher-os-secureboot.der" \
    -R 'set default="1"' 'set default="0"' \
    -R "set timeout=60" "set timeout=10" \
    "$cache/$iso" "$out"
# Every entry must be ours: Fedora's media check and rescue would install nothing useful.
xorriso -osirrox on -indev "$out" -extract /EFI/BOOT/grub.cfg "$files/grub.cfg" 2>/dev/null
cfg=$(cat "$files/grub.cfg")
for want in "Install PS5 Launcher OS" "Install with the NVIDIA driver ($pw)" "ps5los.nvidia" \
    "Enroll the Secure Boot key again ($pw)" "ps5los.enroll" "inst.ks=" 'set default="0"'; do
    grep -qF -- "$want" <<<"$cfg" || { echo "the ISO's boot menu lacks: $want" >&2; exit 1; }
done
for gone in "Fedora $release" rd.live.check inst.rescue; do
    if grep -qF -- "$gone" <<<"$cfg"; then
        echo "the ISO's boot menu still has: $gone" >&2
        exit 1
    fi
done
ls -la "$out"
