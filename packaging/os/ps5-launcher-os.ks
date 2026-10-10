# PS5 Launcher OS installer: Fedora's network installer with this file (packaging/os/build-iso.sh
# writes the image name in place of @IMAGE@).
#
# The installer asks only for the disk, the user and the time zone. The OS itself is downloaded
# from the container registry, so the PC needs an internet connection.
#
# Every PC gets the main image. NVIDIA PCs start on the open-source driver, and the launcher offers
# the NVIDIA driver later. Two more boot entries:
# - "Install with the NVIDIA driver" (kernel argument ps5los.nvidia) installs the NVIDIA image
#   directly, for cards that show nothing without it.
# - "Enroll the Secure Boot key again" (ps5los.enroll) queues the key and restarts, for when the
#   blue MOK screen was missed. Nothing is installed.
#
# The Secure Boot key password on both entries is 12345678. The boot menu shows it, because the
# blue MOK screen asks for it before Linux runs. (Enrolment that the launcher starts uses a new
# random password each time.)

# Enroll the Secure Boot key again: queue it, then restart into the blue MOK screen.
%pre --erroronfail --log=/tmp/ps5-launcher-os-enroll.log
if grep -qw ps5los.enroll /proc/cmdline; then
    set -eu
    key=$(find /run/install -maxdepth 4 -name ps5-launcher-os-secureboot.der 2>/dev/null | head -1)
    if [ -z "$key" ]; then
        echo "PS5 Launcher OS: the Secure Boot key is missing from this USB stick." >&2
        exit 1
    fi
    hash=$(mktemp)
    mokutil --generate-hash=12345678 > "$hash"
    mokutil --timeout -1
    mokutil --import "$key" --hash-file "$hash"
    echo "Key queued. At the restart, enroll it on the blue screen: password 12345678."
    reboot
    sleep 120
fi
%end

keyboard --vckeymap=us --xlayouts=us
rootpw --lock

%pre --erroronfail --log=/tmp/ps5-launcher-os-pre.log
image=@IMAGE@
tag=main
if grep -qw ps5los.nvidia /proc/cmdline; then
    tag=nvidia
fi
echo "Installing $image:$tag"
# The source needs a transport prefix; the update target must not have one.
echo "bootc --source-imgref=registry:$image:$tag --target-imgref=$image:$tag" > /tmp/ps5-launcher-os-source.ks
echo "$tag" > /tmp/ps5-launcher-os-tag
%end
%include /tmp/ps5-launcher-os-source.ks

# The NVIDIA image's modules are signed with PS5 Launcher OS's own key. With Secure Boot on,
# queue that key: after the restart a blue "MOK management" screen asks to enroll it (password
# 12345678; it needs a USB keyboard). The main image needs no key: Fedora's kernel is signed
# already. If the key cannot be queued, the installation stops: the NVIDIA driver would not load.
%post --nochroot --erroronfail --log=/tmp/ps5-launcher-os-secureboot.log
set -eu
fail() {
    echo "PS5 Launcher OS: $1" >&2
    echo "The NVIDIA driver cannot load with Secure Boot on without this key. Turn Secure Boot off" >&2
    echo "in the PC's firmware settings, or install with the plain entry and add the driver later." >&2
    exit 1
}
if [ "$(cat /tmp/ps5-launcher-os-tag)" != nvidia ]; then
    echo "Main image: no key to enroll"
    exit 0
fi
if [ ! -d /sys/firmware/efi ]; then
    echo "No UEFI: no Secure Boot, nothing to enroll"
    exit 0
fi
case $(mokutil --sb-state 2>&1) in
*"SecureBoot enabled"*) ;;
*)
    echo "Secure Boot is off: nothing to enroll"
    exit 0
    ;;
esac
key=$(find /run/install -maxdepth 4 -name ps5-launcher-os-secureboot.der 2>/dev/null | head -1)
[ -n "$key" ] || fail "the Secure Boot key is missing from this USB stick."
if mokutil --test-key "$key" | grep -q "is already enrolled"; then
    echo "The key is enrolled already"
    exit 0
fi
hash=$(mktemp)
mokutil --generate-hash=12345678 > "$hash" || fail "could not prepare the key password."
mokutil --timeout -1 || fail "could not set the MOK screen's timeout."
mokutil --import "$key" --hash-file "$hash" || fail "could not queue the Secure Boot key."
mokutil --list-new >/dev/null || fail "the Secure Boot key is not queued."
echo "Key queued. At the restart, enroll it on the blue screen: password 12345678."
%end
