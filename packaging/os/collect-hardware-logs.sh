#!/usr/bin/env bash
# shellcheck disable=SC2016 # save() runs its command text in bash -c, which expands it, on purpose
# PS5 Launcher OS hardware tests (hardware-test.md): save the logs of one test stage into one
# dated tarball. Run it on the tested PC after each stage, from a terminal:
#
#   sudo bash collect-hardware-logs.sh STAGE [OUTPUT_DIR]
#
# STAGE: a short name, for example install, first-start, upgrade, rollback, nvidia, main-again.
# OUTPUT_DIR: where the tarball goes (default: this script's folder, for example a USB stick).
# It prints the booted digest at the end: write it in hardware-test.md.
#
# What it saves: bootc status, the kernel command line, the GPU lines of lspci, the NVIDIA and
# nouveau modules, the GPU firmware diagnostics, the last 2000
# lines of this boot's journal, the session log, the list of the launcher's game logs with the
# end of the newest one, and the OS marker. The launcher has no log file of its own: the session
# wrapper writes its output into ~/.cache/ps5-launcher-session.log.
#
# What it never saves: keys, ~/.config/ps5-launcher (it holds tokens), network passwords. The
# journal can name Wi-Fi networks and the PC.
set -u

stage=${1:-}
if ! [[ $stage =~ ^[A-Za-z0-9_-]+$ ]]; then
    echo "usage: sudo bash $0 STAGE [OUTPUT_DIR]   (STAGE: letters, digits, - and _)" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
out_dir=${2:-$here}
if [ "$(id -u)" != 0 ]; then
    echo "warning: not root; bootc status and parts of the journal will be missing (use sudo)" >&2
fi

name=ps5-launcher-os-logs-$(date -u +%Y%m%dT%H%M%SZ)-$stage
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
dir=$work/$name
mkdir "$dir"

# save FILE COMMAND: one command line (pipes allowed) with 10 seconds to finish.
save() {
    local file=$1 cmd=$2
    {
        echo "\$ $cmd"
        timeout 10 bash -c "$cmd"
        echo "(exit $?)"
    } > "$dir/$file" 2>&1
}

# The user the PC logs in automatically, else the one who ran sudo.
user=$(sed -n 's/^User=//p' /etc/sddm.conf.d/10-ps5-launcher-os-user.conf 2>/dev/null | head -1)
user=${user:-${SUDO_USER:-$(id -un)}}
home=$(getent passwd "$user" | cut -d: -f6)
home=${home:-/nonexistent}
# The commands below read these from the environment, never pasted into the command text.
export LOG_HOME=$home LOG_STAGE=$stage

save hardware.txt 'echo "stage: $LOG_STAGE"; date -u; cat /sys/class/dmi/id/sys_vendor /sys/class/dmi/id/product_name /sys/class/dmi/id/bios_version 2>/dev/null; lscpu | grep -E "^Model name"; lspci -nn | grep -E "VGA|3D|Display"; uname -r'
save bootc-status.json 'bootc status --json'
save cmdline.txt 'cat /proc/cmdline'
save lspci-gpu.txt 'lspci -nnk | grep -A3 -E "VGA|3D|Display"'
save lsmod-gpu.txt 'lsmod | grep -E "^(nvidia|nouveau)"'
save journal.txt 'journalctl -b --no-pager | tail -n 2000'
save os-marker.txt 'cat /usr/lib/ps5-launcher/os-release'
save session.log 'tail -n 3000 "$LOG_HOME/.cache/ps5-launcher-session.log"'
save launcher-game-logs.txt 'd="$LOG_HOME/.cache/ps5-launcher/logs"; ls -la "$d"; f=$(ls -t "$d"/*.log 2>/dev/null | head -1); [ -n "$f" ] && { echo "== end of $f"; tail -n 200 "$f"; }'
save launcher-state.txt 'ls -la "$LOG_HOME/.local/state/ps5-launcher"'
# The run this PC was installed from, when manifest.txt sits next to this script.
[ -f "$here/manifest.txt" ] && cp "$here/manifest.txt" "$dir/"

if ! tar -C "$work" -czf "$out_dir/$name.tar.gz" "$name" 2>/dev/null; then
    echo "warning: cannot write to $out_dir; saving in $PWD" >&2
    out_dir=$PWD
    tar -C "$work" -czf "$out_dir/$name.tar.gz" "$name" || exit 1
fi

booted=$(timeout 10 bootc status --json 2>/dev/null | sed -n '/{/,$p' | sed '1s/^[^{]*//' |
    timeout 10 jq -r '.status.booted.image.imageDigest // empty' 2>/dev/null)
echo "Saved $out_dir/$name.tar.gz"
echo "Booted digest: ${booted:-unknown (run it with sudo, on the installed system)}"
