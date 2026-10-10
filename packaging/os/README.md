# PS5 Launcher OS (Fedora bootc)

Fedora bootc starts straight into PS5 Launcher. The OS is one container image, updated as a
whole, with the previous deployment kept for rollback. AMD, Intel and NVIDIA use Fedora's
kernel drivers and Mesa. NVIDIA uses nouveau and Mesa NVK; the launcher reports the actual
bound driver and does not infer Vulkan support from a PCI ID.

Fedora owns the boot chain and Secure Boot trust. There is no project-owned module signing
key, certificate, MOK enrollment, or separate NVIDIA image. RPM Fusion remains enabled for
video codecs.

**Not ready for public release:** container-image signing still needs the owner's key and one
successful end-to-end workflow run. `OS_IMAGE_SIGNING: disabled` in
[os.yml](../../.github/workflows/os.yml) blocks promotion and release ISO publication. Container
image signatures authenticate OS downloads and are separate from Fedora's Secure Boot.

## Image and tags

The repository is `ghcr.io/<owner>/<image-name>`, where `<owner>` defaults to the GitHub
repository owner or `OS_IMAGE_OWNER`. The image name defaults to `ps5-launcher-fedora`
and can be changed with `OS_IMAGE_NAME`. Never push to the old `ps5-launcher-os` repository or its
tags: existing Bazzite PCs still follow them.

| Tag | Purpose |
|---|---|
| `main` | Installed PCs' update channel for all GPU vendors. |
| `main-YYYYMMDD` | Dated release image. |
| `testing-main-<run>` | Candidate for one workflow run. |
| `testing-upgrade-<run>` | Signed upgrade target used by the installation test. |
| `testing-unsigned-<run>`, `testing-wrongkey-<run>` | Negative signature tests; the VM must refuse both. |
| `testing-follow-<run>` | Moving channel used by the installation test. |
| `sha256-<digest>.sig` | Cosign signature attachment for that digest. |

The image ships `/usr/lib/ps5-launcher/os-release` with `IMAGE=main` and its `IMAGE_REF` channel.
OCI labels record the Fedora base digest and launcher version. The public image-signing key
is `/usr/share/ps5-launcher/signing/cosign.pub`.

## Build and install

[Containerfile](Containerfile) installs SDDM, gamescope, Mesa, GPU firmware, video codecs, a
Plasma fallback desktop, the launcher RPM, and the OS services. [build-image.sh](build-image.sh)
stages a small build context under `target/os/context`. It accepts only `main`.

```sh
# Test-only public key; never publish an image made with it.
packaging/os/make-test-key.sh
export SIGNING_PUBKEY_FILE=target/os/test-key/cosign.pub
LAUNCHER_RPM=dist/ps5-launcher-1.16.0-1.x86_64.rpm packaging/os/build-image.sh main
packaging/os/check-image.sh main localhost/ps5-launcher-fedora:main localhost/ps5-launcher-fedora:main
```

`ENGINE=docker` selects Docker instead of Podman. `IMAGE`, `IMAGE_REF`, `LAUNCHER_VERSION` and
`BASE_IMAGE` override the defaults; pin the base by digest. Apple Silicon builds emulate
x86_64 and skip bootc container lint. Test builds still require an image-signing public key;
this change does not add an unsigned build mode.

[build-iso.sh](build-iso.sh) runs inside the selected Fedora release with `IMAGE` and `MAIN_DIGEST`. It verifies
Fedora's network installer using Fedora's signed checksum, then embeds
[ps5-launcher-os.ks](ps5-launcher-os.ks). The installer asks for the disk, user and time zone.
It downloads the exact tested image digest and sets `main` as the update channel. Fedora's
media-check and rescue entries remain available. There are no driver or key-enrollment entries.

```sh
source packaging/os/config.sh
docker run --rm --privileged -v "$PWD:/src" -w /src \
    -e IMAGE=ghcr.io/OWNER/ps5-launcher-fedora -e MAIN_DIGEST=sha256:REPLACE_WITH_64_HEX_DIGEST \
    -e FEDORA_VERSION -e FEDORA_ISO_VERSION -e FEDORA_GPG_FINGERPRINT \
    -e ISO_CACHE=/src/target/os "$FEDORA_CONTAINER_IMAGE" \
    packaging/os/build-iso.sh ps5-launcher-fedora-x86_64.iso
```

An optional second argument appends an extra kickstart for unattended CI installs.
`KERNEL_ARGS` adds installer arguments; release builds leave it unset.

## Image signatures on the installed PC

[signing/README.md](signing/README.md) documents the cosign key and GitHub environments.
`policy.json.in` rejects registry images by default and permits this repository only with a
signature by the shipped key. Local transports remain allowed for image-builder installs.
The registries configuration enables legacy sigstore attachments.

Anaconda installs a pinned digest without authenticating its container signature. On a fresh
install, `ps5-signature-policy.service` stores bootc's signature enforcement with
`bootc switch --enforce-container-sigpolicy`. It waits for networking and shares a lock with
the helper and boot health service. It avoids restaging after a rollback.

The root helper verifies the channel by its exact digest before checking updates or switching.
Every switch stores signature enforcement; later bootc upgrades obey that choice. There are
no driver-key checks. The helper exposes status, update-check, update, rollback, switch main,
one-time desktop login, time zone/NTP settings, and health-notice acknowledgment.
`queue-key`, `key-state`, and `switch nvidia` are refused.

## Boot health, recovery and desktop

`ps5-boot-health.service` checks the screen's bound GPU driver, gamescope and the launcher's
health socket. A failed first boot can roll back once to the recorded healthy digest, then
restart. The shared lock prevents concurrent deployment changes. Its notice appears after the
first-start setup. Plasma does not count as a healthy launcher session.

To enter recovery, edit the GRUB kernel command line and append
`systemd.unit=ps5-recovery.target`. The root text menu can follow the main channel, roll back,
or restart. It does not need the launcher or a login. A network cable is the easiest way to
make an image switch work.

“Switch to desktop” selects Plasma for the next automatic login. The launcher session returns
on the following boot. The first-start setup covers network, time zone, controllers and game
drives. Older saved graphics-driver steps advance to Game drive, and old NVIDIA setup data is
ignored without losing other settings.

SSH stays disabled, with a preset keeping it off at first boot; an owner or CI kickstart may
enable it. Firewalld is enabled. The journal persists. The power button retains the session's
existing suspend behavior.

## Workflow and gates

[os.yml](../../.github/workflows/os.yml) runs manually or for `v*` release tags. Tag runs wait
for the launcher release artifacts; prereleases are skipped. It resolves the Fedora base,
launcher version and image-signing public key once, then builds, checks and pushes one main
candidate. It also prepares the upgrade and negative signature test images.

The signing job uses `OS_IMAGE_SIGNING_KEY` and `COSIGN_PASSWORD` in `os-fedora-signing`, checks
the private/public key match, signs main and upgrade digests, and verifies their provenance.
There is no kernel-module signing secret.

| Gate | Required behavior |
|---|---|
| Image check | Packages, services, signature policy/key, Fedora nouveau module, NVK ICD/library, and GPU firmware are present. |
| Boot test | A qcow2 boots under KVM, the launcher session and OS checks pass. |
| Install test | The ISO installs, signature enforcement persists, unsigned/wrong-key pulls are refused, signed upgrade and rollback work. |
| Hardware tests | [hardware-test.md](hardware-test.md) passes on real AMD/Intel and NVIDIA PCs for the exact candidate digest. |

The candidate ISO artifact contains the ISO, checksum, `manifest.txt`, hardware test sheet and
log collector. Approval must cite its run ID, attempt and main digest. Promotion requires the
`os-fedora-release` environment and the release guard; it verifies signatures with cosign and
Fedora's containers policy, then copies the tested digest to `main-YYYYMMDD` and `main`.
It refuses to move installed PCs back to an older launcher version. The release ISO embeds
that exact promoted digest and is attached only after promotion.

## Owner setup and validation

Set up the image key and environments using [signing/README.md](signing/README.md), make the
GHCR package public, and provide a runner with `/dev/kvm` (or configure `OS_VM_RUNNER`). The
workflow fails its VM gates without KVM. Run manually with promotion and ISO attachment off
until one complete run passes. Keep `OS_IMAGE_SIGNING: disabled` until a separate reviewed
change cites that successful run.

Local checks include `test-helper.sh`, `test-recovery-menu.sh`, `test-signature-policy.sh`,
`test-boot-health.sh`, and `../linux/test-session.sh`, plus ShellCheck and actionlint. Linux-only tests
run in a Fedora container on macOS. Real GPU rendering and firmware Secure Boot behavior
must be checked on hardware before a release.

## GitHub repository variables

Set these under Settings → Secrets and variables → Actions → Variables. Unset values use
the defaults below; local builds accept the corresponding environment variables in
[config.sh](config.sh).

| Repository variable | Default |
|---|---|
| `OS_FEDORA_VERSION` | `44` |
| `OS_FEDORA_BASE_IMAGE` | `quay.io/fedora/fedora-bootc:<release>` |
| `OS_FEDORA_ISO_VERSION` | `44-1.7` for release 44 |
| `OS_FEDORA_GPG_FINGERPRINT` | Fedora 44 checksum-signing key |
| `OS_IMAGE_OWNER` | Repository owner |
| `OS_IMAGE_NAME` | `ps5-launcher-fedora` |
| `OS_RUNNER` | `ubuntu-24.04` |
| `OS_VM_RUNNER` | `OS_RUNNER`, then `ubuntu-24.04` |
| `OS_VM_MEMORY_MB` | `6144` |
| `OS_VM_CPUS` | `4` |
| `OS_VM_DISK_GB` | `30` |
| `OS_ARTIFACT_RETENTION_DAYS` | `14` |

When changing Fedora releases, also set the matching installer revision and Fedora's official
primary checksum-signing key fingerprint. Preparation rejects missing or mismatched installer
metadata before building. The base image digest is resolved once per run. Action integrity pins
and the reviewed image-signing release guard remain in source control.
