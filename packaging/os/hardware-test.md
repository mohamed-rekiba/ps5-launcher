# PS5 Launcher OS: hardware test and results

CI has no GPU and no Secure Boot, so these tests run on real PCs before a promotion. Fill in
one copy of this page for each PC, and attach it with the log tarballs to the approval of the
`os-fedora-release` deployment. **The approval counts only for the run id, attempt and digests
written below.**

## Before you start

1. Download the `candidate-iso` artifact of the workflow run. Check the ISO:
   `sha256sum -c ps5-launcher-fedora-candidate-x86_64.iso.sha256`.
2. Write the ISO to a USB stick (for example with Fedora Media Writer, or `dd`).
3. Copy `manifest.txt` and `collect-hardware-logs.sh` to a second USB stick. The logs go there.
4. You need a USB keyboard for the blue MOK screen, and a controller.

In the installer, give the user a password and tick "Make this user administrator": the tests
need `sudo`. After each stage, open a terminal (Switch to desktop, then Konsole), load the
manifest and save the logs:

```bash
. /run/media/$USER/<stick>/manifest.txt
sudo bash /run/media/$USER/<stick>/collect-hardware-logs.sh <stage>
```

The script prints the booted digest. Write it in the stage's row. **Record the booted digest
after every restart.**

## The PC and the run

| | |
|---|---|
| Tester, date | |
| PC model (or motherboard) | |
| CPU | |
| GPU (model and PCI ID, `lspci -nn`) | |
| Firmware version | |
| Secure Boot (on or off, `mokutil --sb-state`) | |
| Run id and attempt (`RUN_ID`, `RUN_ATTEMPT`) | |
| Commit (`COMMIT`) | |
| `MAIN_DIGEST` | |
| `NVIDIA_DIGEST` | |

`collect-hardware-logs.sh` writes most of these lines into `hardware.txt`.

## Tests

Result: **Pass**, **Fail** or **Skipped** (say why). The stage name is the argument for
`collect-hardware-logs.sh`.

| # | Test and procedure | Pass when | Stage | Booted digest | Result |
|---|---|---|---|---|---|
| 1 | **Install from the ISO.** Boot the stick, plain entry "Install PS5 Launcher OS". Choose the disk, the user and the time zone. | The install ends and the PC restarts. | `install` | | |
| 2 | **First start.** Let the PC start by itself. | PS5 Launcher, full screen, no login screen. Booted digest = `MAIN_DIGEST`. | `first-start` | | |
| 3 | **Controller.** Use a controller (USB, then Bluetooth if you can) to move through the launcher. | Every button and stick works. | `controller` | | |
| 4 | **Sound over TV/HDMI.** Connect the PC to a TV or monitor with speakers by HDMI. Play a trailer. | Sound comes from the TV. | `sound` | | |
| 5a | **Upgrade.** `sudo bootc switch "$IMAGE@$UPGRADE_DIGEST"`, then restart. | The launcher starts. Booted digest = `UPGRADE_DIGEST`, and `/usr/lib/ps5-launcher/upgrade-test` exists. | `upgrade` | | |
| 5b | **Rollback.** `sudo bootc rollback`, then restart. | The launcher starts. Booted digest = `MAIN_DIGEST`; `sudo bootc status` shows the image as `$IMAGE:main` again. | `rollback` | | |
| 6 | **Switch to desktop and back.** In the launcher's Power menu, choose "Switch to desktop". Log out of Plasma. | Plasma starts; after the log out, the launcher starts again. | `desktop` | | |
| 6a | **Boot health check after a good start.** Three minutes after test 2 (or 5a, 5b), run `sudo cat /var/lib/ps5-launcher-os/health/last-good` and `journalctl -b -u ps5-boot-health`. | `last-good` names the booted digest; the journal says "healthy". No `transaction` file in that folder. | `health` | | |
| 6b | **The GRUB menu.** Restart and watch the screen after the firmware logo. | The GRUB menu shows for 5 seconds before it starts the system. | `grub` | | |
| 6c | **Recovery mode.** Right after test 2 (one system installed, so nothing to roll back to). With a USB keyboard: restart; in the GRUB menu press `e`; at the end of the line that starts with `linux` add ` systemd.unit=ps5-recovery.target`; press `Ctrl-X` or `F10`. Choose 3 (roll back). Then, with a network cable, choose 1 (switch to main; it stages the same image) and answer `n`. Then choose 4. | A text menu with four choices shows on the screen, without a login. Choice 3 shows bootc's error (no rollback deployment) and the menu comes back. After choice 4 the PC restarts into the launcher; the booted digest is still `MAIN_DIGEST`, and `sudo bootc status` shows no rollback queued. | `recovery` | | |
| 6d | **Power key.** In the launcher, press the PC's power key once, briefly. Then, in a terminal in Plasma, run `systemd-analyze cat-config systemd/logind.conf \| grep HandlePowerKey`. | In the launcher: its Power menu opens, and the PC stays on. The command prints `HandlePowerKey=suspend`. (In Plasma, Plasma's own power settings handle the key.) | `power-key` | | |

On a PC with an **NVIDIA** card (RTX 20 or newer), also:

| # | Test and procedure | Pass when | Stage | Booted digest | Result |
|---|---|---|---|---|---|
| 7 | **Open-source driver at first start.** Install with the plain entry (test 1 and 2). | The launcher shows; `lsmod` lists `nouveau`. Note whether it is smooth. | `nouveau` | | |
| 8 | **The offer path.** If the launcher offers the NVIDIA driver, follow it. Otherwise run `sudo /usr/libexec/ps5-launcher/helper queue-key` and write down the password it prints. Both use the release tag `nvidia`: if that tag does not exist yet, use `sudo mokutil --import /usr/share/ps5-launcher/secureboot/*.der` (password 12345678) and write "release tag missing". | The key is queued (or the helper prints `key-enrolled`). | `offer` | | |
| 9 | **MOK enrolment with a USB keyboard** (Secure Boot on; else Skipped). Restart. On the blue screen: *Enroll MOK*, *Continue*, *Yes*, the password, *Reboot*. | `mokutil --list-enrolled` shows `CN=PS5 Launcher OS`. | `mok` | | |
| 10 | **Switch to the NVIDIA candidate.** `sudo bootc switch "$IMAGE@$NVIDIA_DIGEST"`, then restart. (`helper switch nvidia` stages the release tag, not this candidate.) | Booted digest = `NVIDIA_DIGEST`. `nvidia-smi` shows the card; `lsmod` lists `nvidia`, not `nouveau`; the launcher shows. Note the download size `bootc switch` printed. | `nvidia` | | |
| 11 | **Switch back.** `sudo bootc switch "$IMAGE@$MAIN_DIGEST"`, then restart. | Booted digest = `MAIN_DIGEST`. `cat /proc/cmdline` has no `nvidia` or `nouveau` argument; `lsmod` lists `nouveau`; the launcher shows. | `main-again` | | |
| 12 | **"Install with the NVIDIA driver" from the ISO** (Secure Boot on if you can). Boot the stick, choose that entry, install. On the blue screen, enroll with password **12345678**. | Booted digest = `NVIDIA_DIGEST`. `nvidia-smi` works; the launcher shows. | `iso-nvidia` | | |

On the NVIDIA PC, also run test 6c with choice 2 after test 9 (key enrolled): it stages the
release tag `nvidia`, as `helper switch nvidia` does; with the key not enrolled it must show
`key-required` and the steps, and switch nothing.

Tests 10 and 11 pin the PC to a digest. Afterwards, run `sudo bootc switch "$IMAGE:main"` (or
reinstall) so the PC follows the release tag again.

## Notes

Anything odd: black screens, slow menus, error messages, the time each restart took.
