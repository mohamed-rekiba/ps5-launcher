# PS5 Launcher OS (Fedora bootc)

A whole operating system that starts straight into PS5 Launcher. It is Fedora bootc: the OS is
one container image, updated as a whole, with the previous version kept for rollback. It
replaces the Bazzite image; its files (`os/`) and its workflow (`os.yml`) are removed.

**Not ready for a public release:** image signing is built but has not run end to end yet: it
needs the owner's signing key ([signing/README.md](signing/README.md)) and one green run of the
workflow. Until then, the workflow refuses to promote the images or to publish the ISO (see
[the public-release guard](#the-public-release-guard)).

## Images

`ghcr.io/<owner>/ps5-launcher-fedora`, where the owner is the repository's owner (or the
repository variable `OS_IMAGE_OWNER`):

| Tag | What it is |
|---|---|
| `main` | AMD and Intel graphics, and NVIDIA cards on the open-source driver. Every install starts here. |
| `nvidia` | `main` plus one layer: NVIDIA's driver, its kernel modules signed with our Secure Boot key, and the kernel arguments it needs. Built `FROM` the exact `main` digest, so a switch downloads only that layer. |
| `main-YYYYMMDD`, `nvidia-YYYYMMDD` | The same images, by date of the run that made them. |
| `testing-main-<run>`, `testing-nvidia-<run>` | Candidates of one workflow run. Not for installed PCs. |
| `testing-upgrade-<run>` | CI only: the install test's upgrade target (signed). |
| `testing-unsigned-<run>`, `testing-wrongkey-<run>` | CI only: the same image unsigned, and signed with a throwaway key. The install test's VM must refuse both. |
| `testing-follow-<run>` | CI only: the tag the install test's VM follows; the test moves it. |
| `sha256-<digest>.sig` | The cosign signature of that digest ([signing/README.md](signing/README.md)). |

**Never push to `ps5-launcher-os` or its tags:** installed Bazzite PCs update from those.

Each image tells the launcher what it is in `/usr/lib/ps5-launcher/os-release`:

```
IMAGE=main
IMAGE_REF=ghcr.io/<owner>/ps5-launcher-fedora:main
```

and ships the Secure Boot certificate as `/usr/share/ps5-launcher/secureboot/<sha256>.der`, with
the same SHA-256 in the label `io.github.ps5-launcher.secureboot-cert-sha256`. Other labels:
`io.github.ps5-launcher.base-image` (the Fedora base, by digest),
`io.github.ps5-launcher.launcher-version`, and on NVIDIA `io.github.ps5-launcher.main-image`
(the main image it was built on, by digest).

Both images also ship the image signing public key as `/usr/share/ps5-launcher/signing/cosign.pub`,
and a containers policy that requires it (see
[image signatures on the installed PC](#image-signatures-on-the-installed-pc)).

## Pieces

| File | What it does |
|---|---|
| [Containerfile](Containerfile) | The main image: `fedora-bootc:44` by digest, SDDM, gamescope, Mesa, RPM Fusion's video decoders, a minimal Plasma desktop, `skopeo` and `mokutil` for the helper, and the launcher rpm. |
| [Containerfile.nvidia](Containerfile.nvidia) | The NVIDIA image. Its first stage builds the kernel modules for the main image's kernel and signs them; the key is a build secret, never in a layer. |
| [files/](files) | Copied into both images: SDDM's automatic login into the PS5 Launcher session, the service that picks the user, the root helper, its signature check `verify-image` and its polkit policy, Plasma's autostart that ends a one-time "Switch to desktop", the preset (SSH server off; firewalld, the boot health check and the signature enforcement on), the [boot health check](#the-boot-health-check), the [signature enforcement](#image-signatures-on-the-installed-pc), [recovery mode](#recovery-mode), the [power key](#the-power-key) files, the GRUB menu drop-in, and the persistent journal. |
| [nvidia/](nvidia) | Copied into the NVIDIA image: kernel arguments (`kargs.d`) and the nouveau block. |
| [secureboot/](secureboot/README.md) | The certificate of the Secure Boot key, and how the owner makes it. |
| [signing/](signing/README.md) | The image signing public key, the templates of the containers policy and of its `registries.d` entry, how the owner makes the key, and how to rotate it. |
| [ps5-launcher-os.ks](ps5-launcher-os.ks) | The kickstart for Fedora's network installer. |
| [build-image.sh](build-image.sh) | Builds either image (podman, or docker with `ENGINE=docker`). |
| [check-image.sh](check-image.sh) | Checks a built image without booting it: packages, libraries, services, the OS marker, the certificate, the signature policy and its key, the labels, and the NVIDIA kernel. |
| [build-iso.sh](build-iso.sh) | Fedora 44's netinstall ISO with the kickstart. The ISO is checked against Fedora's CHECKSUM file, and that file against Fedora 44's GPG key. |
| [make-test-key.sh](make-test-key.sh) | **Tests only:** a throwaway Secure Boot key, and a throwaway signing public key. Never publish an image built with them. |
| [boottest/](boottest) | **CI only, never published:** the boot test image, the install test and its kickstart, the upgrade target, and `check-system`, the checks that run inside the VM. |
| [test-helper.sh](test-helper.sh) | Tests the root helper and `power-key-hold` (CI runs it with the Linux build). |
| [test-boot-health.sh](test-boot-health.sh) | Tests the boot health check with fake `bootc`, sysfs and a fake launcher socket. Linux only; on macOS it runs itself in a Fedora container. |
| [test-recovery-menu.sh](test-recovery-menu.sh) | Tests the recovery menu's choices with a fake helper. |
| [test-signature-policy.sh](test-signature-policy.sh) | Tests the first start's signature enforcement with a fake `bootc`. |
| [hardware-test.md](hardware-test.md) | The hardware tests before a promotion: the checklist and the results form. |
| [collect-hardware-logs.sh](collect-hardware-logs.sh) | Saves the logs of one hardware test stage into a dated tarball. |
| [../../.github/workflows/os-fedora.yml](../../.github/workflows/os-fedora.yml) | Builds, tests and publishes the images. |

## The installer

`build-iso.sh` makes `ps5-launcher-fedora-x86_64.iso` (a different name from Bazzite's
`ps5-launcher-os-x86_64.iso`, so neither can overwrite the other). The PC needs an internet
connection: the installer downloads the image. It asks only for the disk, the user and the time
zone. The ISO installs fixed digests, never a tag: the release ISO, the two digests that
`promote` checked and published (`MAIN_DIGEST`, `NVIDIA_DIGEST`); the installed PC then follows
the tag `main` or `nvidia`. The installer itself checks no signature, so the digest is what makes
the installed image the checked one. The boot menu:

| Entry | What it does |
|---|---|
| Install PS5 Launcher OS | The default, after 10 seconds. Installs `main`. |
| Install with the NVIDIA driver (Secure Boot key password: 12345678) | Installs `nvidia`, for NVIDIA cards that show nothing on the open-source driver. With Secure Boot on, it queues our key. If that fails, the installation stops with an error. |
| Troubleshooting → Enroll the Secure Boot key again (password: 12345678) | Queues the key again and restarts, for when the blue MOK screen was missed. Installs nothing. |

**The MOK password from the ISO is `12345678`.** After the restart, a blue "MOK management"
screen asks to enroll the key: choose *Enroll MOK*, *Continue*, *Yes*, type `12345678`, then
*Reboot*. This screen runs before Linux, so it needs a USB keyboard. When the launcher starts an
enrolment later, it uses a new random password each time.

**The installed system:**

- **No SSH server.** `sshd.service` and `sshd.socket` are disabled, and a preset
  (`files/usr/lib/systemd/system-preset/10-ps5-launcher-os.preset`) keeps them off when systemd
  applies its presets at the first start. They are not masked: an owner can turn SSH on with
  `sudo systemctl enable --now sshd`. Only CI's install test turns it on, for its key only.
- **The fallback user.** If the installer finished without a user, the PC makes `player` at the
  next start, so it still logs in. Its password is locked and it is not in `wheel`: SDDM's
  automatic login needs no password, and the launcher's helper is allowed for the user at the
  PC by polkit, not by group. An administrator needs a user made in the installer.

## Image signatures on the installed PC

Every image is signed with the project's cosign key ([signing/README.md](signing/README.md)).
On the PC:

- **The policy.** `/etc/containers/policy.json` refuses every image by default, and accepts our
  repository only with a signature by `/usr/share/ps5-launcher/signing/cosign.pub`
  (`/etc/containers/registries.d/ps5-launcher.yaml` turns on the signatures stored next to the
  image). Every pull checks it: bootc's (with or without `--enforce-container-sigpolicy`: bootc
  passes no `--insecure-policy` to skopeo), the helper's, and podman's. Pulls from other
  registries are refused; local images are accepted. [signing/README.md](signing/README.md)
  explains both choices.
- **bootc's stored choice.** `bootc switch --enforce-container-sigpolicy` stores the choice in the
  deployment's origin (`ostree-image-signed:docker://…`), and later `bootc upgrade` keeps it. With
  it, bootc also refuses to pull when the default policy accepts anything. `bootc status --json`
  shows it as `"signature": "containerPolicy"` in `.spec.image` (and in each deployment's
  `.image.image`); an unverified origin has no `signature` key (bootc 1.16.13,
  `crates/lib/src/spec.rs` lines 73-96).
- **The first start.** The installer's bootc command cannot store the choice, so a fresh install
  starts with an unverified origin. `ps5-signature-policy.service`
  ([`signature-policy`](files/usr/libexec/ps5-launcher-os/signature-policy)) runs at every start,
  beside the graphical start, and acts only on a fresh install (nothing staged, no rollback
  deployment) that follows our repository: it waits up to 2 minutes for a network and up to
  190 s for the boot health check to finish (which waits only 20 s for the shared lock), then runs
  `bootc switch --enforce-container-sigpolicy` to the tag it follows (main), or to the booted
  digest (NVIDIA, which follows digests). That stages the same image (or a newer signed one on
  main), enforced from the next restart on. Without a network it fails, and the next start tries
  again. After a rollback to the installed system it does nothing: staging again would bring back
  the image the boot health check just left; the helper's next update or switch stores the choice.
  It writes `/var/lib/ps5-launcher-os/signature-policy.json` (mode 0644, `"version": 1`) for the
  launcher: `state` is `enforced`, `not-our-image` (a local test build), `not-enforced` (not a
  fresh install), or `failed` (then the unit fails too), with `reason`, `image` and `time`.
  **The launcher does not read this file yet.**
- **The helper.** Before `update-check`, `update`, `switch`, `queue-key` and `key-state` trust an
  image, it finds the digest the tag points at (`skopeo inspect`, which checks no signature),
  then checks exactly that digest with
  [`verify-image`](files/usr/libexec/ps5-launcher/verify-image): skopeo's image proxy, the code
  bootc pulls through, opens the image under the system's policy, which downloads only the
  manifest, the signatures and the config, never a layer. The Secure Boot certificate label is
  read from that same checked digest. Every `bootc switch` passes
  `--enforce-container-sigpolicy`; `update` on main runs `bootc upgrade` when the choice is
  stored, and otherwise switches to its channel with the flag. An image that fails the check
  stops the task (exit 1) before bootc or mokutil runs.

## The boot health check

bootc does not notice a black screen. `ps5-boot-health.service` runs at every start, beside the
graphical start (it does not hold it back), and runs
[`/usr/libexec/ps5-launcher-os/boot-health`](files/usr/libexec/ps5-launcher-os/boot-health).
Within about 140 s (the unit's hard limit is 180 s), all of these must hold:

1. **The GPU that drives the screen** (a DRM connector with `status` = `connected`, then its
   card's `device/driver`) has the right driver: `nvidia` on the NVIDIA image, any bound GPU
   driver but `nvidia` (and not a firmware framebuffer) on main.
2. **gamescope runs** for the automatic-login user.
3. **The launcher answers** on its health socket for 10 seconds in a row. Plasma does not count.

The health socket is the launcher's side of the contract: a Unix stream socket at
`/run/user/<uid>/ps5-launcher-health.sock`, mode 0600, owned by the user. A client writes
`ping\n`; the launcher answers `ok\n` from its UI event loop within 2 seconds. The check accepts
the socket only when it is a real socket owned by that uid with mode 600, and when the process
listening on it (`SO_PEERCRED`, read with a few lines of Python) has that uid, the name
`ps5-launcher` and the executable `/usr/bin/ps5-launcher`.

**Healthy:** the booted digest becomes `last-good`. **Unhealthy:** it saves the journal and runs
`bootc rollback`, then restarts, only when all of these hold: it is the first start of this
digest (no attempt record; the record is written before the checks), no recovery is active,
nothing is staged, and bootc's rollback entry is `last-good`. Before `bootc rollback`, it records
the recovery (`failed_digest`, `destination_digest`, `rollback_attempts: 1`) durably, so a power
cut at any point never leads to a second try. It restarts only when bootc confirms the rollback
is queued to that digest. On the destination it never rolls back again, even when the check
fails there. A `bootc status` it cannot read, or state it does not know, changes nothing.
**No connected screen** (a TV in standby, say) decides nothing either: the attempt record is
removed, so the next start with the screen on is still the first attempt.

It shares the lock `/run/ps5-launcher-os.lock` with the helper's `update`, `switch` and
`rollback`, and reads `bootc status` again under that lock before it rolls back.

The state is in `/var/lib/ps5-launcher-os/health/` (root-owned, each file a JSON object with
`"version": 1`): `last-good`, `attempts/<digest>`, `transaction` (the active recovery),
`history/` (closed ones), `logs/` (the journal of the last 5 failed starts, root only), and
`notice.json` (mode 0644) for the launcher to show. Its `outcome` is one of `unhealthy`,
`rolling-back`, `rolled-back`, `rollback-unhealthy`, `rollback-interrupted` or
`rollback-failed`, with `reason`, `action` and the digests. The launcher deletes it with
`helper health-ack`. The journal is persistent.

## Recovery mode

A text menu for when the launcher and the desktop do not work, for example to leave the NVIDIA
image when its driver shows nothing. It needs a keyboard, no login and no password.

1. Restart the PC. The GRUB menu shows for 5 seconds (on PCs installed from this version on; on
   older installs it shows for 1 second: press an arrow key at once to stop the countdown).
2. Select the boot entry with the arrow keys and press `e`.
3. Go to the end of the line that starts with `linux` and add ` systemd.unit=ps5-recovery.target`.
4. Press `Ctrl-X` or `F10` to start.

The menu on the screen (tty1):

| Choice | What it does |
|---|---|
| 1 | Switch to the main image (`helper switch main`). Needs a network. |
| 2 | Switch to the NVIDIA image (`helper switch nvidia`). With Secure Boot on and the key not enrolled (`key-required` or `key-pending`), nothing is switched: the menu shows the enrolment steps, can queue the key, and shows its password digit by digit. |
| 3 | Roll back to the previous system (`helper rollback`). Needs no network. |
| 4 | Restart. |

The menu runs the root helper as root, so its checks stay. It shows every error and comes back.
The network starts as usual: a cable works; Wi-Fi works when the PC has joined that network
before. The change in step 3 is for one start only. Recovery mode does not start the login
screen or the boot health check.

The 5-second menu is a bootupd drop-in,
[`files/usr/lib/bootupd/grub2-static/configs.d/13_ps5-launcher-os.cfg`](files/usr/lib/bootupd/grub2-static/configs.d/13_ps5-launcher-os.cfg).
bootupd joins `configs.d` into `/boot/grub2/grub.cfg` when the system is installed; it does not
rewrite that file on updates, so PCs installed earlier keep the 1-second menu.
(`grub2-editenv - set menu_auto_hide=0` does nothing here: bootupd's static `grub.cfg` sets the
timeout after it loads `grubenv`, and has no auto-hide.)

## The power key

The launcher opens its own Power menu on the power key, in the OS only:

- `files/usr/lib/udev/rules.d/72-ps5-power-button.rules` gives the user at the PC (`uaccess`)
  the "Power Button" input device, so the launcher can read the key.
- `files/usr/lib/systemd/logind.conf.d/50-ps5-launcher-os.conf` sets `HandlePowerKey=suspend`
  (logind's own default is poweroff).
- The launcher runs `systemd-inhibit --what=handle-power-key --mode=block --no-ask-password
  --who=ps5-launcher --why="Launcher power menu" /usr/libexec/ps5-launcher-os/power-key-hold`.
  `power-key-hold` writes one byte, `r`, then waits for the end of its input and exits 0. While
  it runs, logind ignores the key. If the launcher stops, the lock ends and the key suspends again.

## The firewall

firewalld runs, with Fedora's default zone `public`: incoming connections are refused except
`ssh` (the SSH server itself is off), `mdns` and `dhcpv6-client`. No Samba service is open; file
sharing comes later.

## Build it yourself

From the repository root, on Linux with podman (or `ENGINE=docker`). On an Apple Silicon Mac the
x86_64 build runs emulated: slow, without `bootc container lint`, and the NVIDIA image does not
build there (akmods fails under emulation).

```bash
# 1. The launcher rpm, around a release binary (nfpm in a container).
gh release download v1.14.3 --pattern 'ps5-launcher-linux-x86_64.tar.gz*' --dir /tmp/release
(cd /tmp/release && sha256sum -c ps5-launcher-linux-x86_64.tar.gz.sha256)
tar -C /tmp/release -xzf /tmp/release/ps5-launcher-linux-x86_64.tar.gz
mkdir -p target/os/bin target/os/dist && cp /tmp/release/ps5-launcher-linux-x86_64/ps5-launcher target/os/bin/
docker run --rm --platform linux/amd64 -v "$PWD:/w" -w /w -e VERSION=1.14.3 \
    -e BINARY=target/os/bin/ps5-launcher goreleaser/nfpm:v2.47.0 \
    package -f packaging/linux/nfpm.yaml -p rpm -t target/os/dist

# 2. A throwaway Secure Boot key and signing public key (or use the real ones once committed).
packaging/os/make-test-key.sh

# 3. The images, and their checks.
export SECUREBOOT_CERT_FILE=target/os/test-key/public_key.der
export SIGNING_PUBKEY_FILE=target/os/test-key/cosign.pub
LAUNCHER_RPM=target/os/dist/ps5-launcher-1.14.3-1.x86_64.rpm \
BASE_IMAGE=quay.io/fedora/fedora-bootc@sha256:<digest> packaging/os/build-image.sh main
SECUREBOOT_KEY_FILE=target/os/test-key/private_key.priv packaging/os/build-image.sh nvidia
cert=$(sha256sum "$SECUREBOOT_CERT_FILE" | cut -d' ' -f1)
packaging/os/check-image.sh main localhost/ps5-launcher-fedora:main localhost/ps5-launcher-fedora:main "$cert"

# 4. The ISO (in a Fedora x86_64 container: mkksiso refuses an ISO of another architecture). It
#    installs the two digests you give it.
docker run --rm --privileged --platform linux/amd64 -v "$PWD:/src" -w /src \
    -e IMAGE=ghcr.io/<you>/ps5-launcher-fedora -e MAIN_DIGEST=sha256:<main> -e NVIDIA_DIGEST=sha256:<nvidia> \
    -e SECUREBOOT_CERT_FILE -e ISO_CACHE=/src/target/os \
    quay.io/fedora/fedora:44 packaging/os/build-iso.sh target/os/ps5-launcher-fedora-x86_64.iso
```

Find the current base digest with
`skopeo inspect --raw docker://quay.io/fedora/fedora-bootc:44 | sha256sum`.

## The workflow and its gates

`.github/workflows/os-fedora.yml` runs every day, after a launcher release (`workflow_call`), and
by hand. It resolves its inputs once: the launcher release (the input `launcher_tag`, or the
latest release), and the current digest of `fedora-bootc:44`, so each daily build picks up
Fedora's updates. Both go into the job summary and the image labels.

1. **main:** checks the launcher tarball against its `.sha256`, builds the rpm with
   `.github/actions/linux-packages`, builds and checks the main image, and pushes
   `testing-main-<run>`.
2. **nvidia:** checks that the `SECUREBOOT_KEY` secret belongs to the committed certificate,
   builds `FROM` the exact main digest with the key mounted only for signing, checks that every
   NVIDIA module matches the main kernel and is signed by our certificate, pushes
   `testing-nvidia-<run>`, and checks that it shares every main layer (the summary shows the
   download size of a switch).
3. **test-images** (CI only): the install test's upgrade target, the same image unsigned, and
   one signed with a throwaway key made in the job.
4. **sign**: signs the main, NVIDIA and upgrade digests with `OS_IMAGE_SIGNING_KEY`, with the run
   id, attempt, commit and variant as annotations, and verifies each against the committed
   `cosign.pub`. It runs in the environment `os-fedora-signing`, which only `main` may use
   ([signing/README.md](signing/README.md)).
5. **Automated gates**, in a VM with KVM. A runner without `/dev/kvm` fails them; the repository
   variable `OS_VM_RUNNER` can name a self-hosted runner with KVM.

   | Gate | Passes when |
   |---|---|
   | boot-test | A qcow2 of the main candidate (made with `image-builder`) boots, and `check-system main --session` passes: the OS marker, the masked update timer, the SSH server off, the tools, the certificate, no NVIDIA kernel arguments, the session, firewalld, the power key's files, the 5-second GRUB menu, and the boot health check (below). |
   | install-test | The ISO installs the candidate with no one at the keyboard (an extra kickstart via `build-iso.sh`'s second argument), and `check-system --enforced` passes after each step: first boot, where `ps5-signature-policy.service` makes bootc store the enforcement (`.spec.image.signature` is `containerPolicy`), then a restart into that deployment; `bootc switch` to the unsigned and to the wrong-key image is refused, with and without `--enforce-container-sigpolicy`, and stages nothing; with the followed tag moved to them, `bootc upgrade` refuses both; `bootc upgrade` to the signed image, then `bootc rollback`; `bootc switch --enforce-container-sigpolicy` to the NVIDIA candidate (kernel arguments and modules present) and back to main (no `nvidia` or `nouveau` kernel argument, no nouveau block, no NVIDIA module). Each step checks the booted digest, and that the booted deployment is enforced. |

   The session check passes when the launcher runs, or when gamescope failed only because the VM
   has no usable Vulkan device ("Failed to initialize Vulkan", "not a valid physical device"),
   the launcher did not crash, and the session wrapper fell back to Plasma through the root
   helper, and Plasma cleared the one-time login.
   **The boot health check in CI:** the VM has no GPU, so no start ever passes the check and no
   digest becomes `last-good`. `check-system` waits for the check to end, then asserts that it
   recorded this start's attempt with this boot's ID, found it unhealthy, did not roll back (no
   rollback queued, no recovery recorded), and wrote a readable `notice.json`; after the Plasma
   fallback, its reason must be "launcher not healthy (Plasma fallback)". CI cannot prove the
   rollback, the restart into the destination, or the GPU check on real drivers: only
   `test-boot-health.sh` covers those, with fakes, and the hardware tests cover a good start.
   The install test also makes the `candidate-iso` artifact for the hardware tests: an ISO that
   installs this run's tested **digests** (never a testing tag, which a rerun of the same run id
   would move), its `.sha256`, `manifest.txt` (run id and attempt, commit, the main, NVIDIA and
   upgrade digests, the Fedora base digest, the launcher version, the certificate and ISO
   checksums), [hardware-test.md](hardware-test.md) and
   [collect-hardware-logs.sh](collect-hardware-logs.sh). The job summary shows the manifest.
6. **release-guard**, in every run with `promote` or `attach_iso` on: fails while
   `OS_IMAGE_SIGNING` is not `enabled` (see below). Promotion and the ISO wait for it.
7. **promote**, only when the run was started with `promote` on, from `main`, after every gate
   passed, and after a reviewer of the environment `os-fedora-release` approved it. First it
   checks both digests: `cosign verify` against the committed key, with this run's id, the sign
   job's attempt, the commit and the variant; and Fedora 44's skopeo copying each digest under
   the images' own policy. Then it copies the tested digests to `main-YYYYMMDD`,
   `nvidia-YYYYMMDD`, `main` and `nvidia`, and checks each tag's digest and signature. **Scheduled
   runs and release runs never promote; they only push candidates.**
8. **iso**, only with `attach_iso` on and after promotion: checks again that `main` and `nvidia`
   point at the promoted digests and that they are signed, builds the ISO that installs exactly
   those digests, and attaches `ps5-launcher-fedora-x86_64.iso` and its `.sha256` to the
   launcher release, under 2 GiB.

**Promotion stays manual.** A reviewer approves only with hardware results
([hardware-test.md](hardware-test.md), filled in, with the log tarballs) that cite **this exact
run id and attempt, and this exact pair of main and NVIDIA digests**, as the install test's and
the promote job's summaries show them. Results for another run, another attempt or another
digest do not count: a rerun builds new digests.

### The public-release guard

Promotion and the release ISO publish images that installed PCs trust and update from. Image
signing is built (the signature in the workflow, its checks in `promote` and `iso`, and the
containers policy in the image) but has not run end to end, so the job `release-guard` fails
every run with `promote` or `attach_iso` on, with a message that says what is left. It has no
environment, so it fails before anyone is asked to approve.

The guard is the value `OS_IMAGE_SIGNING: disabled` at the top of
`.github/workflows/os-fedora.yml`. It is in the workflow file, not a repository variable, so a
settings click cannot lift it: only a reviewed change can. What is left before that change:

1. The owner's signing key and environments ([signing/README.md](signing/README.md)).
2. One green run of the whole workflow from `main` with those keys: signing, the install test's
   signature gates, and the rest.

Then set it to `enabled` in a separate, reviewed change that cites that run's id.

## The owner's setup

1. **The Secure Boot key:** follow [secureboot/README.md](secureboot/README.md). Until the
   certificate is committed and the secret is set, every run fails at the start with a pointer
   to it.
2. **The image signing key and its environment:** follow [signing/README.md](signing/README.md):
   the key pair, the environment `os-fedora-signing` (deployment branches: `main` only; no
   reviewers) with the secrets `OS_IMAGE_SIGNING_KEY` and `COSIGN_PASSWORD`, and the committed
   `cosign.pub`. Until `cosign.pub` is committed, every run fails at the start with a pointer to
   it; until the secrets are set, the `sign` job fails.
3. **Package visibility:** the first push creates `ghcr.io/<owner>/ps5-launcher-fedora` as a
   private package. Make it public (GitHub → your profile → Packages → ps5-launcher-fedora →
   Package settings → Change visibility). The install test and installed PCs pull without a
   login, so the install test fails until then.
4. **The environment `os-fedora-release`:** create it (Settings → Environments), add required
   reviewers, and allow only the `main` branch to deploy to it. Without reviewers, a run with
   `promote` on would promote right after the automated gates.
5. **Branch protection on `main`:** the two environments trust `main`, so protect it (Settings →
   Rules): changes only through reviewed pull requests.
6. **After a launcher release:** to build the image from release.yml, add a job there that calls
   this workflow with `secrets: inherit` (not done yet).
7. Optional repository variables: `OS_IMAGE_OWNER` (a fork's registry name), `OS_VM_RUNNER` (a
   runner with KVM).

## Hardware tests before promotion

CI has no GPU and no Secure Boot. Before approving a promotion, run
[hardware-test.md](hardware-test.md) with the run's `candidate-iso` artifact, on an AMD or Intel
PC and on an NVIDIA RTX 20 or newer PC (RTX 40 or 50 too, if you have one). It covers the
install, the first start, a controller, sound over HDMI, an upgrade and a rollback, the NVIDIA
driver with MOK enrolment, the ISO's NVIDIA entry, Switch to desktop, the boot health check's
`last-good` after a good start, recovery mode, and the power key.

The commands use the exact digests from `manifest.txt`, never a tag. Load it with
`. manifest.txt`, then:

| Step | Command, then restart | The booted digest must be |
|---|---|---|
| Upgrade | `sudo bootc switch --enforce-container-sigpolicy "$IMAGE@$UPGRADE_DIGEST"` | `UPGRADE_DIGEST` |
| Rollback | `sudo bootc rollback` | `MAIN_DIGEST` |
| To NVIDIA (key enrolled first, with Secure Boot on) | `sudo bootc switch --enforce-container-sigpolicy "$IMAGE@$NVIDIA_DIGEST"` | `NVIDIA_DIGEST` |
| Back to main | `sudo bootc switch --enforce-container-sigpolicy "$IMAGE@$MAIN_DIGEST"` | `MAIN_DIGEST` |

**After every restart, record the booted digest**: `sudo bash collect-hardware-logs.sh <stage>`
prints it and saves the logs (or read `.status.booted.image.imageDigest` from
`sudo bootc status --json`). `UPGRADE_DIGEST` is the install test's upgrade target: the
candidate plus one marker file, `/usr/lib/ps5-launcher/upgrade-test`. The launcher's NVIDIA
offer and `helper switch nvidia` use the release tag `nvidia`, not the candidate, so the test
switches to the candidate by digest.

## TODOs

- **Image signing (blocks a public release):** built, not run yet. Needs the owner's key
  ([signing/README.md](signing/README.md)) and one green run; then a separate change sets
  `OS_IMAGE_SIGNING: enabled` (see [the public-release guard](#the-public-release-guard)).
- The launcher does not show `/var/lib/ps5-launcher-os/signature-policy.json` yet (a failed
  enforcement at the first start shows only in `systemctl --failed`).
- Samba (file sharing) and the helper's `share on|off` are not in the image yet.
- Recovery mode is reached only by editing the GRUB entry; there is no menu entry for it, and
  PCs installed before the GRUB drop-in keep the 1-second menu.
- Old `testing-*` tags are not deleted from the registry.
- release.yml does not call this workflow yet.
