#!/usr/bin/env bash
# Check a built PS5 Launcher OS image, without booting it: packages, libraries, services, the OS
# marker, the image signature policy and its key, and the labels. CI runs it on every image before it is
# pushed; it works on a local build too.
#
#   packaging/os/check-image.sh main   IMAGE IMAGE_REF
# ENGINE: podman (default) or docker. SIGNING_PUBKEY_FILE: the public key
# the image must ship (default packaging/os/signing/cosign.pub); its policy must require it for
# IMAGE_REF's repository, exactly as packaging/os/signing/*.in say.
set -euo pipefail
variant=${1:?main}
image=${2:?image to check}
image_ref=${3:?the channel the image follows}
engine=${ENGINE:-podman}
signing_pub=${SIGNING_PUBKEY_FILE:-packaging/os/signing/cosign.pub}
here=$(dirname "$0")

label() { "$engine" image inspect --format "{{ index .Config.Labels \"$1\" }}" "$image"; }
fail() { echo "FAIL $*" >&2; exit 1; }

[ -f "$signing_pub" ] || fail "no image signing public key at $signing_pub (packaging/os/signing/README.md)"
signing_sha256=$(sha256sum "$signing_pub" | cut -d' ' -f1)
repo=${image_ref%:*}
policy_json=$(sed "s|@REPOSITORY@|$repo|" "$here/signing/policy.json.in")
registries_yaml=$(sed "s|@REPOSITORY@|$repo|" "$here/signing/registries.yaml.in")
# The rules the policy must keep, whatever else the template says: reject by default, only our
# repository under docker, only with our key, and containers-storage allowed (bootc install run
# from the image reads it from there, under this policy).
# shellcheck disable=SC2016 # jq variables, not shell ones
policy_rules='.default == [{"type": "reject"}]
    and (.transports.docker | keys == [$r])
    and .transports.docker[$r] == [{"type": "sigstoreSigned", "keyPath": $k,
        "signedIdentity": {"type": "matchRepository"}}]
    and .transports["containers-storage"][""] == [{"type": "insecureAcceptAnything"}]'

case $variant in
main)
    base=$(label io.github.ps5-launcher.base-image)
    case $base in *@sha256:*) ;; *) fail "label io.github.ps5-launcher.base-image is not a digest: $base" ;; esac
    echo "ok   label io.github.ps5-launcher.base-image=$base"
    ;;
*) fail "unknown variant $variant" ;;
esac

# The checks inside the image. Values come in through the environment, never pasted into it.
# shellcheck disable=SC2016 # the script expands its variables inside the image, on purpose
"$engine" run --rm --platform linux/amd64 --entrypoint /usr/bin/bash \
    -e VARIANT="$variant" -e IMAGE_REF="$image_ref" \
    -e REPOSITORY="$repo" -e SIGNING_SHA256="$signing_sha256" \
    -e POLICY_JSON="$policy_json" -e POLICY_RULES="$policy_rules" -e REGISTRIES_YAML="$registries_yaml" \
    "$image" -euo pipefail -c '
ok() { echo "ok   $*"; }
fail() { echo "FAIL $*" >&2; exit 1; }
rpm -q sddm gamescope xorg-x11-server-Xwayland mesa-vulkan-drivers mesa-va-drivers-freeworld \
    ffmpeg-libs intel-media-driver mpv-libs yt-dlp alsa-lib plasma-workspace ps5-launcher \
    skopeo polkit NetworkManager bluez pipewire wireplumber udisks2 \
    openssl openssh-server firewalld jq python3 >/dev/null ||
    fail "a required package is missing"
ok "required packages"
# Loaded at run time by name (trailers, archive installs, sound).
for lib in libmpv.so.2 libarchive.so.13 libasound.so.2; do
    ldconfig -p | grep -q "$lib " || fail "missing $lib"
done
ok "run-time libraries"
for tool in skopeo pkexec bootc xdotool xrandr lspci; do
    command -v "$tool" >/dev/null || fail "missing $tool"
done
ok "tools"
ps5-launcher --version
for unit in sddm.service ps5-launcher-os-autologin.service ps5-boot-health.service \
    ps5-signature-policy.service firewalld.service; do
    [ "$(systemctl is-enabled "$unit")" = enabled ] || fail "$unit is not enabled"
done
[ "$(systemctl get-default)" = graphical.target ] || fail "the default target is not graphical"
[ "$(systemctl is-enabled bootc-fetch-apply-updates.timer || true)" = masked ] ||
    fail "bootc-fetch-apply-updates.timer is not masked"
ok "services, and the bootc update timer masked"
# SSH is off, not masked: an owner or a kickstart may turn it on. The preset keeps it off when
# systemd applies the presets at the first start (the image has an empty /etc/machine-id).
for unit in sshd.service sshd.socket; do
    [ "$(systemctl is-enabled "$unit" || true)" = disabled ] || fail "$unit is not disabled"
done
grep -qx "disable sshd.service" /usr/lib/systemd/system-preset/10-ps5-launcher-os.preset ||
    fail "no preset keeps sshd off at the first start"
ok "SSH server off (sshd.service, sshd.socket, and the preset)"
for unit in firewalld.service ps5-boot-health.service ps5-signature-policy.service; do
    grep -qx "enable $unit" /usr/lib/systemd/system-preset/10-ps5-launcher-os.preset ||
        fail "no preset enables $unit"
done
ok "the firewall, the boot health check and the signature enforcement on (units and preset)"
# The firewall: Samba waits (file sharing comes later); SSH stays allowed, for an owner who turns
# the server on, and for the install test.
zone=$(firewall-offline-cmd --get-default-zone)
services=$(firewall-offline-cmd --zone="$zone" --list-services)
case " $services " in *" samba "* | *" samba-client "*) fail "the firewall zone $zone opens Samba: $services" ;; esac
case " $services " in *" ssh "*) ;; *) fail "the firewall zone $zone does not allow ssh: $services" ;; esac
ok "firewall zone $zone: $services"
# The boot health check, recovery mode and the power key.
for exe in /usr/libexec/ps5-launcher-os/boot-health /usr/libexec/ps5-launcher-os/recovery-menu \
    /usr/libexec/ps5-launcher-os/power-key-hold; do
    [ "$(stat -c %a "$exe")" = 755 ] || fail "$exe is not mode 0755"
done
systemd-analyze verify --man=no ps5-boot-health.service ps5-recovery.target ps5-recovery-menu.service \
    ps5-signature-policy.service ||
    fail "systemd-analyze verify found errors in the new units"
[ "$(systemctl is-enabled ps5-recovery-menu.service || true)" = static ] ||
    fail "ps5-recovery-menu.service must only start with ps5-recovery.target"
grep -q "ConditionKernelCommandLine=!systemd.unit=ps5-recovery.target" /usr/lib/systemd/system/ps5-boot-health.service ||
    fail "the boot health check would run in recovery mode"
grep -qx "set timeout=5" /usr/lib/bootupd/grub2-static/configs.d/13_ps5-launcher-os.cfg ||
    fail "no GRUB drop-in with a 5 s menu"
[ -f /usr/lib/tmpfiles.d/ps5-launcher-os.conf ] && [ ! -e /var/lib/ps5-launcher-os ] ||
    fail "the health folder must come from tmpfiles.d, not the image"
ok "boot health check, recovery target and menu, GRUB menu drop-in"
printf "%s\n" "SUBSYSTEM==\"input\", KERNEL==\"event*\", ATTRS{name}==\"Power Button\", TAG+=\"uaccess\"" |
    cmp -s - /usr/lib/udev/rules.d/72-ps5-power-button.rules || fail "the power button udev rule is not the exact rule"
udevadm verify /usr/lib/udev/rules.d/72-ps5-power-button.rules >/dev/null || fail "udevadm verify refuses the power button rule"
systemd-analyze cat-config systemd/logind.conf | grep -qx "HandlePowerKey=suspend" ||
    fail "logind does not read HandlePowerKey=suspend"
systemd-analyze cat-config systemd/journald.conf | grep -qx "Storage=persistent" || fail "the journal is not persistent"
ok "power key: udev rule, logind HandlePowerKey=suspend; persistent journal"
printf "IMAGE=%s\nIMAGE_REF=%s\n" "$VARIANT" "$IMAGE_REF" | cmp -s - /usr/lib/ps5-launcher/os-release ||
    fail "the OS marker is not IMAGE=$VARIANT, IMAGE_REF=$IMAGE_REF"
ok "OS marker: IMAGE=$VARIANT IMAGE_REF=$IMAGE_REF"
# The image signature policy (packaging/os/signing/README.md): the shipped key, a policy that
# refuses everything by default and requires that key for this repository, and sigstore
# attachments for it.
key=/usr/share/ps5-launcher/signing/cosign.pub
[ -f "$key" ] && [ "$(sha256sum "$key" | cut -d" " -f1)" = "$SIGNING_SHA256" ] &&
    [ "$(stat -c %a "$key")" = 644 ] || fail "the signing key $key is missing, not 0644, or not the expected key"
policy=/etc/containers/policy.json
[ "$(jq -S . "$policy")" = "$(jq -S . <<<"$POLICY_JSON")" ] ||
    fail "$policy is not packaging/os/signing/policy.json.in for $REPOSITORY"
jq -e --arg r "$REPOSITORY" --arg k "$key" "$POLICY_RULES" "$policy" >/dev/null ||
    fail "$policy does not require the signing key for $REPOSITORY only"
printf "%s\n" "$REGISTRIES_YAML" | cmp -s - /etc/containers/registries.d/ps5-launcher.yaml ||
    fail "registries.d/ps5-launcher.yaml is not packaging/os/signing/registries.yaml.in for $REPOSITORY"
for exe in /usr/libexec/ps5-launcher/verify-image /usr/libexec/ps5-launcher-os/signature-policy; do
    [ "$(stat -c %a "$exe")" = 755 ] || fail "$exe is not mode 0755"
done
python3 -I -c "import ast, sys; ast.parse(open(sys.argv[1]).read())" /usr/libexec/ps5-launcher/verify-image ||
    fail "verify-image does not parse"
ok "signature policy: reject by default; $REPOSITORY only signed by $key (sha256 $SIGNING_SHA256)"
kver=$(ls /usr/lib/modules)
[ "$(echo "$kver" | wc -l)" = 1 ] || fail "more than one kernel: $kver"
case $VARIANT in
main)
    ! modinfo -k "$kver" nvidia >/dev/null 2>&1 || fail "main has the NVIDIA module"
    [ ! -e /usr/lib/bootc/kargs.d/00-nvidia.toml ] || fail "main has the NVIDIA kernel arguments"
    modinfo -k "$kver" nouveau >/dev/null || fail "no Fedora nouveau module"
    rpm -q nvidia-gpu-firmware >/dev/null || fail "no NVIDIA firmware package"
    find /usr/share/vulkan/icd.d -name "*nouveau*.json" | grep -q . || fail "no Mesa NVK ICD"
    find /usr/lib64 -name "libvulkan_nouveau.so*" | grep -q . || fail "no Mesa NVK library"
    ok "Fedora nouveau, NVIDIA firmware and Mesa NVK (kernel $kver)"
    ;;
esac
'
echo "ok   $variant image $image"
