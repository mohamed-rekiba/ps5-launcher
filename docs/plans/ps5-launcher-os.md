# Plan: PS5 Launcher OS on Fedora bootc, and the system options in the launcher

Status: in progress on branch `feat/os-system-layer` (draft PR #2 in the owner's fork). The
spike and the prototype are on `spike/os-fedora-bootc`. Decisions were made with the project's
advisor (Codex) and are recorded here and in the commit messages.

## Where it stands

| Phase | State |
|---|---|
| 0a. Image and installer spike | Done in CI: Fedora 44 bootc builds and boots under KVM, the signed NVIDIA modules build, the helper and SDDM fallback work. Real-hardware questions are open. |
| 0b. UI prototype | Done; picks below. |
| 1. OS image | Built: `packaging/os/`, `.github/workflows/os-fedora.yml` with VM gates and manual promotion. Not run yet: needs the owner's Secure Boot key, a public GHCR package and the release environment. **Image signing is built, not run yet: it needs the owner's signing key and one green end-to-end run, then a reviewed change lifts the release guard.** |
| 2. System layer and session | Done. |
| 3. Power menu, Quick Menu, PS button, power key | Done. |
| 4. Settings with the side rail | Done; then the side rail was replaced by the old right-side sheet at the owner's request. The System areas open as sub-sheets in the same panel. |
| 5. On-screen keyboard | Done. |
| 6. System pages | Network, Sound, Storage, Updates done. Display (with the NVIDIA offer) and Time in progress. Controllers (battery, Bluetooth pairing, Forget) built, not yet tried with a real adapter. File sharing and formatting not built. |
| 7. First-start setup | Not started. |
| 8. Docs, migration, release | Not started. |

Built for the image beyond the original plan: a boot health check that rolls back once after a
failed first start, a recovery mode reached from GRUB, firewalld, SSH off, and a hardware test kit
(`packaging/os/hardware-test.md`, `collect-hardware-logs.sh`).

## Goal

PS5 Launcher runs in three modes:

| Mode | What it is | Exists today |
|---|---|---|
| Desktop | A window on KDE, GNOME or macOS | Yes |
| Session | The "PS5 Launcher" login session: gamescope with nothing behind it | Yes (`packaging/linux/ps5-launcher-session`) |
| OS | A whole operating system that starts straight into the launcher | Yes, on Bazzite (`os/`). This plan replaces it. |

The OS moves from Bazzite to **Fedora bootc** (image mode: the whole OS is one container image,
updated atomically, with rollback from the boot menu). The launcher gets the system options that
an OS needs: power, network, Bluetooth, sound, storage, updates, an on-screen keyboard and a
first-start setup.

## Decisions

- Base: Fedora bootc, on a pinned Fedora release and a pinned base image digest.
- Build tool: osbuild's **`image-builder` CLI** (`--bootc-ref`), for VM and disk images.
  bootc-image-builder is being deprecated in its favour by the same project
  ([notice](https://osbuild.org/docs/bootc/deprecation-notice/)). The Anaconda ISO type is also
  planned for removal, so the release ISO is a network installer with a kickstart (Phase 0).
- One Settings screen with a side rail in two groups, **Launcher** and **System**.
- A **Quick Menu** (like the PS5 Control Center) for volume, Wi-Fi, controllers, downloads and power.
- An on-screen keyboard built in Slint, inside the launcher, made for a controller.
- Power actions leave Settings and go to one **Power menu** (table below).
- NVIDIA kernel modules are built at image build time from RPM Fusion's akmod and signed with our
  own Secure Boot key.
- **NVIDIA driver on offer, not forced.** One ISO installs the main image; NVIDIA PCs start on the
  open-source drivers (nouveau and NVK). The launcher finds the card and offers "Install the NVIDIA
  driver", which switches the PC to the NVIDIA image. Two ways out that need no launcher: an ISO
  boot entry "Install with the NVIDIA driver", and a recovery boot entry (Phase 1).
- **Fallback desktop: a minimal but complete KDE Plasma.** Not Plasma Mobile (touch first). Plasma
  Bigscreen (TV first, packaged in Fedora) gets a short controller test in Phase 0b as an option.
- The UI asks the system what it can do (capabilities). It never asks "which mode am I in?".
- **Never push the new image to the tags that Bazzite installs follow** (`ps5-launcher-os:stable`
  and `:nvidia`). The new image gets a new name (open question 3).

### Power menu for each mode

| Action | Desktop (app) | Session (gamescope) | OS |
|---|---|---|---|
| Close game | While a game runs | While a game runs | While a game runs |
| Close launcher | Yes | No | No |
| Sleep | No (the desktop handles it) | If logind allows it | If logind allows it |
| Restart | No | Yes | Yes; "Update and restart" when an update is ready |
| Power off | No | Yes | Yes |
| Log out | No (the desktop handles it) | Yes | No in the first version; comes with the multi-user login screen |
| Switch to desktop | No | If a desktop session is installed | If a desktop session is installed |
| Restart launcher | No | Settings → About → Troubleshooting | Settings → About → Troubleshooting |

Close game also works on macOS. "Is a game running" is launcher state, not a system capability.

Rules for every power action, in the Power menu and in the Quick Menu alike:

- **Something would be lost** (a game runs, an install or a file copy runs): a dialog says what,
  and offers the safe choice first.
- **Nothing would be lost:** a 3-second countdown, and Ⓑ cancels it.
- **"Power off when done"** waits for installs and file copies that run when it is chosen. It does
  not wait for games or downloads, or for work started later. The Quick Menu shows "Powering off
  after Astro Bot installs · Cancel". If the install fails, the PC stays on and shows the error.
- **Downloads:** today they come back paused after a restart (`src/downloads.rs:6`). New: the
  downloads that were running at a planned Restart, Power off, Sleep or Log out resume by
  themselves at the next start, with a toast "3 downloads continued". After a crash they stay paused, as
  today.
- **Errors are shown.** If logind refuses (an inhibitor, a "challenge" answer that needs a
  password), the menu says so. It never fails silently.

## Check command

```bash
cargo build --profile ci --locked && cargo test --profile ci --locked && scripts/smoke-test.sh target/ci/ps5-launcher
```

The OS image has its own checks (Phase 1). There is no clippy or rustfmt step in CI today; this
plan does not add one.

## Order and parallel work

```
Phase 0a  Installer and image spike ──► Phase 1  OS image + CI ──────────────┐
Phase 0b  UI prototype ──► Phase 2  System layer + session ──► Phase 3 ...    ├─► Phase 8  Docs, migration, release
                                                    Phases 3 to 7 (launcher UI) ┘
```

Phase 0a goes first because it can change the plan: if the installer or the signed NVIDIA module
does not work, the UI work does not help. Phases 0b and 2 to 7 are launcher code and run in
parallel with Phase 1.

## Phase 0a: Installer and image spike (prove it works)

A throwaway branch that answers these questions, on a pinned Fedora release:

1. A minimal `fedora-bootc` image with gamescope and our rpm starts into the launcher.
2. Fedora's netinstall ISO with a kickstart installs it. The kickstart `bootc` command exists
   since Fedora 43; its source needs a transport prefix (for example `registry:`), its update
   target does not. Verify on the real netinstaller, not only in the kickstart parser.
3. After install: `bootc upgrade` to a second image, then `bootc rollback`, both work.
4. NVIDIA: the kmod built at image build time loads on real hardware **with Secure Boot on**,
   after MOK enrolment.
5. `image-builder --bootc-ref` makes a qcow2 that boots in QEMU.
6. **The first start on the open-source NVIDIA drivers:** on the pinned kernel, Mesa and
   gamescope, with gamescope driving the screen directly (not nested in a desktop). Test at least
   one Turing or Ampere card and one Ada or Blackwell card. Record which cards show the launcher
   and which do not. If many fail, the installer choice comes back as the main path.
7. **The switch from main to NVIDIA:** queue the key, restart, enroll, `bootc switch`, restart.
   Measure the real download size when the NVIDIA image is built `FROM` the exact main digest.
8. **The switch back:** `bootc switch` from NVIDIA to main leaves no nouveau block, kernel
   argument or NVIDIA configuration behind.

Check-in: a short report of what worked, with versions pinned. The plan is updated before Phase 1.

**Results so far** (CI runs of `.github/workflows/os-fedora-spike.yml` on the owner's fork, GitHub
runner with KVM; nothing on real hardware yet):

| Question | Result |
|---|---|
| 1. Image with gamescope and the launcher | Builds natively on `fedora-bootc:44` (kernel 7.2.9, Mesa 26.2.4, gamescope 3.16.29, Plasma 6.7.5), 1.94 GB; `bootc container lint` passes. RPM Fusion's ffmpeg and freeworld VA drivers replace Fedora's cleanly. |
| 2. Netinstall ISO with the `bootc` kickstart | ISO builds (1.2 GB, under the 2 GiB limit). **Not installed yet** (needs the images in a registry). |
| 3. Upgrade and rollback | Not tested yet. |
| 4. Signed NVIDIA kmod | `akmod-nvidia` 615.71.09 builds the open modules (Dual MIT/GPL) for the image's kernel and signs all five with our key. akmods builds as its own user, so the key mount must be readable (mode 0444). Loading under Secure Boot: **needs real hardware**. |
| 5. qcow2 from `image-builder` | Works; the qcow2 boots in QEMU/KVM. SDDM logs the fallback user in to the PS5 Launcher session. |
| 6. Open-source NVIDIA driver at first start | **Needs real hardware.** In the VM, gamescope refuses the software renderer ("Failed to initialize Vulkan: not a valid physical device"), so the VM cannot show the launcher. |
| 7, 8. Switch to NVIDIA and back | NVIDIA image = main + 3 layers, about 0.5 GB uncompressed. Real download size and the switch itself: not tested yet. |

Found and fixed along the way:
- A session that ends at once made SDDM's Relogin loop. The wrapper now gives up at the third
  start within 60 s; in the VM it then asked the helper (through pkexec, no password, as the
  polkit policy says) for Plasma, and SDDM started Plasma. This proves the helper, the polkit
  policy and the SDDM next-session contract on Fedora 44.
- fedora-bootc enables `bootc-fetch-apply-updates.timer` (automatic apply and reboot); the image
  masks it.
- logind's answer through busctl is exactly `s "yes"`, as `system::parse_logind_can` expects.

## Phase 0b: UI prototype (throwaway)

A Slint prototype with fake data, run with `--windowed`, to try the controller flow:

1. Settings, full screen: the rail (icons + labels, folds to icons when focus is in the page),
   L1/R1 between categories, search.
2. The Quick Menu over a dimmed screen, with cards that open in place.
3. The Power menu, the countdown and the loss dialog.
4. The on-screen keyboard: docked at the bottom, the edited field lifted above it, layouts for
   password, address, number and search, controller shortcuts, hold Ⓐ for accents, game-title
   suggestions in search.
5. The "Install the NVIDIA driver" flow: the offer, the download, the Secure Boot screen that
   explains the blue MOK screen and its keyboard need, and the restarts.
6. Plasma Bigscreen in a VM with a controller, to decide between it and minimal Plasma.

Output: screenshots and a list of design changes. Nothing from the prototype is merged.

**Result** (prototype: `cargo run --example prototype_os_ui` on branch `spike/os-fedora-bootc`;
picks decided with the project's advisor):

| Surface | Pick | Why |
|---|---|---|
| Settings | A, the side rail, with labels shown while the rail has focus | Both groups stay visible; the page gets the width when the rail folds. |
| Quick Menu | B, the right drawer | Seven cards do not fit the bottom strip without cutting their text. |
| Power menu | A, the centred list | Compact, and it fits every mode's set of actions. |
| Keyboard | A, docked QWERTY | Larger keys, easier to aim with a D-pad and to read from a sofa. |
| NVIDIA flow | The stepper, with numbered steps | With Secure Boot on, key enrolment comes before the driver download. |

Fixes before Phases 3 to 5: one focus style everywhere (the red focused "Close game" is too
loud) with the button prompts of the connected controller; rail labels and status readable from a
sofa; every flow complete for the controller (Back, focus return, loss dialog, cancel, errors).

**MOK password:** a random 8-digit number for each enrolment request, shown on screen with each
digit numbered, "no spaces", and "write this down or take a photo before the restart". MokManager
can ask for single characters by position, and the screen explains that. This replaces the fixed
`ps5launcher` password in the spike kickstart (to change in Phase 1).

## Phase 1: The OS image (`packaging/os/`)

Replaces `os/` and `.github/workflows/os.yml` (the old workflow keeps running for Bazzite users
until the migration in Phase 8 ends).

### Files

| File | What it does |
|---|---|
| `packaging/os/Containerfile` | The main image, `FROM` the pinned `fedora-bootc` digest. |
| `packaging/os/Containerfile.nvidia` | The NVIDIA image, `FROM` the **exact tested digest of the main image**, plus one driver layer. A first stage builds and signs the kmods against that image's kernel. |
| `packaging/os/files/` | Configuration copied into `/usr` where possible, so updates replace it. |
| `packaging/os/ps5-launcher-os.ks` | The kickstart for the network installer. |
| `packaging/os/build-iso.sh` | Fedora's netinstall ISO with the kickstart. |
| `packaging/os/README.md` | How it is built and tested. |
| `.github/workflows/os-fedora.yml` | Build, check, sign, push to a testing tag, then promote the tested digest. |

### NVIDIA kmod build (first stage of `Containerfile.nvidia`)

- Read the kernel version from the main image's `/usr/lib/modules/`. Never use `uname -r`: in a
  container build it returns the build host's kernel.
- The NVIDIA package install must not upgrade the kernel; the kernel packages are locked during
  the transaction. CI checks that the kernel in the NVIDIA image is the main image's kernel.
- The two images are published as a pair. The NVIDIA image carries a label with its parent main
  digest. Main and NVIDIA share every layer except the driver layer, so a switch downloads only
  that layer, plus any main layers the PC does not have yet. CI reports the compressed size of the
  driver layer.
- Install the matching `kernel-devel`, then `akmod-nvidia`, then
  `akmods --force --kernels <that version>`. Install the resulting kmod RPM in the final stage, run
  `depmod` for that kernel, and regenerate the initramfs if the module or the nouveau block must
  be in it.
- The signing key comes in only as a build secret mount for the signing step. It is never in a
  layer or a build cache. Secret name: `SECUREBOOT_KEY` (the user sets the value).
- Checks: the vermagic of every NVIDIA module matches the image kernel. A signer string is not
  proof; Phase 0a and the release test prove that the module loads under Secure Boot.
- If any of this fails (a new kernel that the driver does not support yet), CI does not publish,
  and users keep the last good image.

### NVIDIA: which cards, and how the driver gets installed

**Cards.**

- Turing and newer (RTX 20, GTX 16 and later): the NVIDIA image with the open kernel modules.
  The NVIDIA GSP firmware these cards need comes with the GPU firmware packages.
- Maxwell, Pascal and Volta: no NVIDIA image in the first version (open question 5). They stay on
  the open-source drivers. NVK supports them, but nouveau mostly cannot raise their clock speed,
  so the launcher says plainly: "Games will be very slow on this card."
- Kepler and older: the same message.
- Detection uses the **GPU that drives the screen**, by PCI device ID, not any NVIDIA chip in the
  PC.

**The offer in the launcher** (first-start setup, and System → Display afterwards):

1. "NVIDIA GeForce RTX 3070 found. Install the NVIDIA driver to play at full speed. Download:
   about N MB, needs a restart." **Install driver** / **Later**.
2. Before downloading, the helper checks for a network connection, free disk space and an update
   already waiting. Each case gets its own message.
3. **Secure Boot on:** first the key, then the switch. The helper queues the key. The launcher
   explains the blue MOK screen, shows the password in large text, and says plainly that **this
   screen needs a USB keyboard** (it runs before Linux, so the on-screen keyboard cannot work
   there). Restart. The launcher checks that the key is enrolled (`mokutil`). Only then does it
   stage the NVIDIA image. Restart again. If the key was not enrolled, nothing is switched, and
   the launcher offers to try again.
4. **Secure Boot off:** stage the NVIDIA image, then one restart.
5. The page shows each state separately: downloading, ready, waiting for the key, restart needed.

**Going back.** "Use the open-source driver" is a `bootc switch` to the main image. Rollback is
only the emergency path: after one NVIDIA update, the previous deployment is NVIDIA too.

**A changed graphics card.** At each start the launcher compares the GPU with the image: NVIDIA
card on main offers the driver; no NVIDIA card on the NVIDIA image offers the switch back.

**Health check and automatic recovery.** bootc does not notice a black screen. A boot health
unit checks that the expected driver is bound to the GPU, gamescope runs, and the launcher
answers. If the check fails on the first start of a new deployment, the PC rolls back once and
starts the previous deployment. The launcher then shows what happened, and the logs are kept.

**Ways out that need no launcher.**

- ISO boot menu: **"Install with the NVIDIA driver"**, for cards where the open-source driver shows
  nothing. The kickstart installs the NVIDIA image and queues the key.
- Recovery boot entry in the installed system: switches to the other image in text mode, then
  restarts.

### Contents of the image

- **Session:** SDDM, set to log in automatically into the "PS5 Launcher" session.
- **Fallback desktop:** a minimal but complete KDE Plasma session: Plasma shell, KWin, System
  Settings, the polkit authentication agent, network and Bluetooth applets, Dolphin (file
  manager), Konsole, a browser. The exact package list is written and tested in Phase 0a. Plasma
  Bigscreen replaces it only if the Phase 0b test says so. Before "Switch to desktop", the
  launcher says "The desktop works best with a mouse and keyboard".
- **Graphics:** Mesa (OpenGL and Vulkan), the GPU firmware packages, gamescope, Vulkan loader.
- **Video decoding (trailers):** RPM Fusion's `mesa-va-drivers-freeworld` and `ffmpeg` **replace**
  Fedora's packages (a swap, not an add; check for conflicts and that the Mesa versions match).
  `intel-media-driver` for Intel. The test plays real H.264 and H.265 video, not only checks that
  the packages are installed.
- **Launcher helpers installed explicitly:** `mpv-libs` and `yt-dlp` are only "recommends" in
  `packaging/linux/nfpm.yaml`, and the image must not depend on weak dependencies.
- **NVIDIA image only:** the kmods, the driver libraries, kernel arguments in
  `/usr/lib/bootc/kargs.d/*.toml` (nouveau off, `nvidia-drm.modeset=1`).
- **Services the launcher drives:** NetworkManager, BlueZ, PipeWire with WirePlumber, udisks2,
  Samba and firewalld (Samba off by default), time sync.
- **No automatic updates by bootc:** `bootc-fetch-apply-updates.timer` is on in fedora-bootc
  (seen in the spike's VM boot) and would download, apply and restart by itself. The image masks
  it; updates go only through the helper.
- **Power key:** logind keeps `HandlePowerKey=suspend` as the default. The inhibitor lock only
  stops logind; it does not give the key to the launcher. So, in the OS only:
  - A udev rule in the image gives the active user read access to the power button input device
    (`KEY_POWER`). The launcher's controller code ignores it today, because it opens only
    joystick devices.
  - The launcher opens that device first. Only when that works does it take the
    `handle-power-key` inhibitor lock. Then a press opens the Power menu flow with the loss
    dialog.
  - The lock is a file descriptor. If the launcher crashes, the lock is released and logind
    handles the key again. The button never stops working.
  - On a normal Linux PC (session mode), the launcher leaves the power key to logind.
- **Privileged helper:** polkit does not raise the rights of an arbitrary command. A small
  root-owned helper (`/usr/libexec/ps5-launcher/helper`) accepts a fixed list of verbs:
  `update-check`, `update`, `rollback`, `share on|off` (only the Samba unit and its firewall
  service), `set-next-session`, `clear-next-session`, `format <drive>`, `queue-key`,
  `switch main|nvidia` (only our two image names, and only signed images, by the image signature
  policy). A polkit action allows it with no password for
  the active local user. The launcher calls it through `pkexec`. Every verb is tested with no
  desktop authentication agent running.
- **OS marker:** `/usr/lib/ps5-launcher/os-release` tells the launcher that it runs in the OS and
  which image (`main` or `nvidia`).

### Secure Boot key lifecycle

- The public certificate is in the repository and in the image. The kickstart queues it with
  `mokutil --import`; the user enrolls it once on the blue MOK screen.
- If enrolment is missed: the ISO keeps the "Enroll the Secure Boot key again" boot entry, and the
  launcher's About page shows the state and the steps.
- **Key rotation must never strand an NVIDIA PC.** Queuing a key is only a request; the user can
  skip it, and machines can skip releases. So:
  - Each NVIDIA image carries a label with the fingerprint of the key that signed its modules.
  - The automatic bootc update timer is off. Updates go only through the helper, which reads the
    label of the new image before it stages it. If Secure Boot is on and that key is not enrolled
    (`mokutil --list-enrolled`), the helper does not stage the image. It queues the key, and the
    launcher shows "Enroll the new key at the next restart" with the steps.
  - The current deployment stays as the rollback entry until the new one has started with its
    modules loaded.
- The Fedora kernel's own Secure Boot trust is separate. Only our modules need our key.

### Checks in CI, and publishing

- `bootc container lint` on both images.
- Required packages and libraries present (`rpm -q`, `ldconfig -p` for libmpv and libarchive).
- The NVIDIA checks above.
- Boot test: `image-builder` makes a qcow2, QEMU boots it, and a screenshot of the VM screen is
  compared against the launcher's first screen. CI checks for `/dev/kvm` first. GitHub does not
  officially support nested virtualization; without KVM the test runs on a self-hosted runner.
- CI pushes to a testing tag. Only a digest that passed every check is promoted to the release
  tags.
- Before each OS release, by hand: the network install from the ISO, MOK enrolment, an update and
  a rollback. The qcow2 test does not cover these.
- The ISO must stay under GitHub's 2 GiB limit per release file; the existing size check stays.

## Phase 2: System layer and session (`src/system.rs`)

### The system layer

One module with a small interface per area. The UI reads `Capabilities` and calls these.

| Area | Backend (first version) | Capability |
|---|---|---|
| Mode | `PS5_LAUNCHER_SESSION`, the OS marker file | `mode` |
| Power | `systemctl`, `loginctl`, logind's `CanSuspend`/`CanReboot`/`CanPowerOff` (answers `yes`, `no`, `challenge`, `na`) and its inhibitor list | `can_sleep`, `can_power`, `can_log_out`, `has_desktop` |
| Network | `nmcli` | `manages_network` |
| Bluetooth | `bluetoothctl` | `manages_bluetooth` |
| Sound | `wpctl` | `manages_sound` |
| Storage | `lsblk --json`, `udisksctl`, the helper for format | `manages_storage` |
| Updates | `bootc status --json`, the helper for check, update and rollback | `os_updates` |
| Time | `timedatectl` | `manages_time` |
| File sharing | The helper | `can_share` |

- Command-line tools first, as the launcher already does with xdotool and xrandr. No new crate.
  If live events need D-Bus later, adding `zbus` is a new dependency and needs approval first.
- Every call runs off the UI thread, and handles: the tool missing, the service not running, a
  non-zero exit, and output with escaped characters (nmcli escapes `:` in Wi-Fi names).
- `util::power` changes: today it only reports a failure to start `systemctl`. It must check the
  exit status and return the error to the UI.
- Test first (TDD): parsers get fixture files with real tool output. The Power menu rule is one
  pure function with a test for every row and every rule above.

### The session wrapper

`packaging/linux/ps5-launcher-session` today runs gamescope once, with no restart, and prefers
`~/.local/bin/ps5-launcher` over the system copy. Changes:

- **Supervision** (done on branch `feat/os-system-layer`, tested by
  `packaging/linux/test-session.sh`): the wrapper keeps gamescope running across launcher
  restarts. Exit 0 ends the session; exit 75 (`system::EXIT_RESTART_LAUNCHER`) restarts the
  launcher at once and does not count as a crash. Other non-zero exits and signals count as
  crashes; the third crash within 60 seconds stops the restarts.
- **Log out** exits 0 (no `loginctl terminate-session` needed). **Switch to desktop** sets the
  next session through the helper and exits 0 only when that worked; otherwise the launcher shows
  the error and stays open.
- **After too many crashes in the OS** the wrapper asks the helper for Plasma and ends the
  session. If there is no desktop or the helper refuses, it stays up without the launcher, so
  SDDM's Relogin does not loop. A blank screen is the known limit; a safe-mode screen can follow.
- **In the OS, the image's `/usr/bin/ps5-launcher` is always used.** An old copy in
  `~/.local/bin` must not win there. The session on a normal Linux PC keeps today's rule.
- **Switch to desktop** (a real logout and login: Plasma is not running behind gamescope). The
  SDDM contract in the OS:
  - The base config (in `/usr`) sets automatic login into the PS5 Launcher session with
    `Relogin=true`, so the end of any session logs in again.
  - The helper's `set-next-session plasma` writes a one-time drop-in in `/etc/sddm.conf.d/` that
    sets the automatic login session to Plasma. Then the launcher ends its session.
  - At Plasma login, an autostart entry calls the helper's `clear-next-session`, which deletes the
    drop-in. Logging out of Plasma then returns to PS5 Launcher. A boot-time unit also deletes a
    leftover drop-in, so a crash cannot leave the PC stuck in Plasma.
  - Because of `Relogin=true`, Log out in the OS would only log in again. So in the first version,
    the OS hides Log out (see the table) until the multi-user login screen exists.

## Phase 3: Input routing, Power menu and Quick Menu

### Input routing first

- `src/gamepad.rs` reports button releases too (today it drops them), and tracks which device a
  press came from and disconnects.
- **PS button:** the short action fires on release. Holding for 2 seconds opens the Power menu and
  cancels the short action. Today `on_pad` toggles focus on press (`src/app.rs:1375`), so a hold
  would first jump to the game.
- PS short press during a game: bring the launcher forward with the Quick Menu open. In the
  launcher while a game runs: back to the game (today's behaviour).
- **Known risk:** evdev does not stop the game from receiving the same presses. While the Quick
  Menu is open over a game, Ⓐ may also reach the game. Test with KytyPS5 and shadPS4. If it
  happens, try `EVIOCGRAB` on the controller while the menu is open. Not verified: games that use
  hidraw (SDL's DualSense driver) are not stopped by that grab.

### Power menu and Quick Menu

- The Power menu follows the table and the rules. The Restart and Quit rows leave Settings
  (`src/settings.rs:306`, `src/settings.rs:541`).
- Quick Menu cards: Sound (volume, output), Wi-Fi, Controllers (with battery), Downloads, Sleep,
  Restart, Power off. During a game, Close game is the first card. Its power cards use the same
  dialogs and countdown.
- The top bar gets Wi-Fi and volume icons in the existing pill style; selecting them opens the
  Quick Menu.

## Phase 4: Settings redesign

- Full-screen page with the side rail.
  - **Launcher:** Games, Playing, Downloads, Emulators, Appearance.
  - **System:** Network, Controllers, Sound, Display, Storage, Updates, Time, File sharing, About.
- Existing rows move into the Launcher group and **stay available in every mode**:
  - "Resolution" is the resolution that KytyPS5 tells the game. It stays under Emulators.
  - "Display" (which monitor the launcher opens on) stays under Launcher → Appearance.
  - System → Display is new: the screen output of the OS (gamescope output resolution and refresh
    rate). It appears only in session and OS mode.
- System categories show only when their capability is on. In desktop mode one row says "Your
  desktop manages these" with **Open system settings**: `systemsettings` on KDE,
  `gnome-control-center` on GNOME, System Settings on macOS; the row is hidden when none is found.
- Search over every row in both groups. A dot on a category when something waits.

## Phase 5: On-screen keyboard

Can be built in parallel with Phase 4; it does not depend on the Settings redesign.

- Input routing: while the keyboard is open it gets controller input first. Search and tab
  shortcuts are off. Today `on_pad` ends text editing on any controller press; that changes.
- Hold Ⓐ for accents needs the release events from Phase 3.
- L2 and R2: on Linux the launcher ignores the analog trigger axes today. Either read the axes, or
  move shift and symbols to other buttons. Test on DualSense, DualShock 4 and an Xbox controller.
- It opens when the last input came from a controller, and closes when a physical key is pressed.
- English layout first. Not verified: Slint's support for right-to-left text, needed for Arabic.

## Phase 6: System pages

Each page's backend contract (commands, errors, what happens without the service) is written and
tested before its UI. One check-in per page.

1. **Network:** Wi-Fi list with strength, connect (keyboard for the password), forget, wired status.
2. **Controllers:** pair, with pictures of the buttons to hold (DualSense, DualShock 4, Xbox),
   battery, forget.
3. **Sound:** output device, volume.
4. **Display:** gamescope output resolution and refresh rate. The graphics driver: which one runs,
   and the NVIDIA offer or "Use the open-source driver" (Phase 1).
5. **Storage:** drives and free space. A new drive shows a toast "Use it for games?": add it as a
   game folder, or format it for games. Format (through the helper, ext4 or exFAT, to decide)
   needs a second confirmation and refuses the system disk.
6. **Updates:** system, launcher, KytyPS5 and shadPS4 on one page. System updates download in the
   background and apply at the next restart. "Undo the last system update" is under Advanced. In
   the OS, the launcher comes with the image and its own updater is off; the emulators keep
   updating in the home folder.
7. **Time:** time zone from a list. Detecting it from the network calls an outside service, so it
   asks first (open question 6).
8. **File sharing:** one switch. It turns on Samba and opens its firewall service. The page shows the
   address in large text, a user name and a password, and steps for Windows and macOS.
9. **About:** versions, GPU and driver, Secure Boot state, Save logs to USB, Restart launcher.
   **Factory reset** (two confirmations) deletes the launcher's settings, cache and the user's
   game folders it manages, and does not touch the OS image. The exact list is written before the
   code.
10. **Sleep timer:** Never / 15 min / 30 min / 1 hour; never while a game, download or install
    runs (the launcher holds an idle inhibitor lock then).

The launcher's own text stays English. Translating the launcher is a separate project;
`localectl` only sets the system language.

## Phase 7: First-start setup

At the first start in the OS: network → time zone → pair controllers → graphics driver (only when
an NVIDIA card is found) → game drive. Every step can be skipped. The driver downloads in the
background while the player goes on with the setup. The installer asks only for the disk and the
user.

## Phase 8: Docs, migration and release

- README "As a console" rewritten. `packaging/README.md` and `packaging/os/README.md` updated.
  `docs/DEVELOPMENT.md` gets the system layer and the helper.
- Bazzite users: the launcher shows a notice with the steps to move to the new image. The old
  workflow keeps building the Bazzite image for the agreed time, then stops.
- `os/` and the old workflow are deleted when that time ends.
- Release notes through the existing Release Please flow.

## Decisions after the spike

- **Image names:** `ghcr.io/mohamedalirashad/ps5-launcher-fedora:main` and `:nvidia` (the owner is
  a workflow variable). Candidates are `testing-main-<run id>` and `testing-nvidia-<run id>`;
  promoted digests also get dated tags. The Bazzite repository and its tags are never used.
- **Publishing:** candidates only on schedule and releases; promotion only by hand, from `main`,
  after the VM gates, behind the `os-fedora-release` environment, citing hardware results for that
  exact run and digest pair. A guard in the workflow file blocks promotion and ISO publication
  until image signing exists.
- **The launcher goes in as the rpm** built from the release binary; the image installs the
  rpm's recommendations explicitly.
- **MOK passwords:** random 8 digits for launcher-driven enrolment; `12345678` only for the ISO's
  "Install with the NVIDIA driver" entry.
- **NVIDIA key gate:** the image label `io.github.ps5-launcher.secureboot-cert-sha256` names the
  certificate; the helper checks it with `mokutil --test-key` (pending does not count) and stages
  the NVIDIA image by digest. `bootc status` and every helper task run as root through pkexec,
  each with its own deadline.
- **SSH off** in the release image (a preset keeps it off); the fallback user has a locked
  password and is not an administrator.
- **Boot health:** our own unit, not greenboot; one automatic rollback, only on the first attempt
  of a new image, never retried; no connected screen decides nothing.
- **Recovery:** `systemd.unit=ps5-recovery.target`, typed once in the GRUB menu; a dedicated menu
  entry is deferred.
- **Power key:** a udev rule, logind's `HandlePowerKey=suspend` as the fallback, and the launcher
  holding the `handle-power-key` inhibitor through `systemd-inhibit` and a pipe.
- **Image signing (built, not run yet):** a dedicated cosign key pair (cosign v3.1.3, legacy
  sigstore attachments in GHCR, simple signing payload, no transparency log); a reject-by-default
  `/etc/containers/policy.json` and a `registries.d` entry in both images;
  `--enforce-container-sigpolicy` on every helper switch; the helper checks the exact digest
  through skopeo's image proxy before it trusts it (skopeo inspect checks nothing); the public
  ISO pins the promoted digests, and the first start turns enforcement on. Secrets
  `OS_IMAGE_SIGNING_KEY` and `COSIGN_PASSWORD` in the environment `os-fedora-signing` (main only).
  See `packaging/os/signing/README.md`.

## Open questions

1. ~~Desktop in the image~~: decided, minimal KDE Plasma; Bigscreen tested in Phase 0b.
2. **Multiple users:** not in the first version. A controller-friendly "Who's playing?" login
   screen (greetd running the launcher in a greeter mode) comes later, with multi-user.
3. ~~New image name and tags~~: decided (see above). Still open: how long the Bazzite image keeps
   being built for current users.
4. **NVIDIA license:** confirm what redistributing the driver inside our image requires before the
   first public release.
5. ~~Older NVIDIA cards~~: decided, not supported in the first version; they run on the
   open-source driver, and the launcher warns that games will be very slow.
6. **Time zone detection:** use an outside geolocation service (asks first), or a list only?
7. **Hybrid laptops (Intel + NVIDIA):** out of scope for the first version unless someone can test.

## Not verified

- The kickstart `bootc` command on the real Fedora netinstaller (Phase 0a).
- A signed NVIDIA kmod loading under Secure Boot on a bootc image (Phase 0a).
- ~~KVM on GitHub's standard runners~~: they have it (the spike's VM boot ran there).
- RPM Fusion packaging of the NVIDIA 580 legacy branch for current Fedora.
- gamescope on the current NVIDIA driver, and gamescope on NVK (there are open gamescope issues
  about NVK).
- The real download size of the switch to the NVIDIA image.
- Plasma Bigscreen with a controller.
- Whether controller presses reach a game while the Quick Menu is open, and whether a grab stops them.
- The polkit defaults for NetworkManager, BlueZ, udisks2 and timedated without an authentication agent.
