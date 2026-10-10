# Plan: Fedora-only graphics, and test builds without keys

Status: **approved**. The owner chose ("fix all"): drop the NVIDIA proprietary image (only
Fedora-signed kernel modules; NVIDIA cards use nouveau + Mesa NVK; no Secure Boot key of ours),
and let test builds run without the image signing key. The advisor (Codex) wrote the plan below.


1. **Launcher behavior**

Settings → Display keeps GPU name, bound driver, resolution and refresh rate. On our Fedora OS, NVIDIA gets an info line: “Uses Fedora’s nouveau driver and Mesa NVK. Updates arrive with system updates.” Do not claim NVK is active based only on PCI detection.

Remove setup’s Graphics driver step. Add the same information to Finish when relevant. No install button, download estimate, restart flow or key instructions.

Keep `gpu.rs` detection and generic driver reporting. Keep proprietary-driver version reporting for Bazzite/session users if useful. Delete `nvidia.rs`, its persisted flow, callbacks, `NvidiaCard`, `KeySteps`, and NVIDIA key/update states across Rust and Slint.

Delete `nvidia_ids.rs` as a support gate. It lists NVIDIA open-module compatibility, not NVK capability. Unknown IDs must not mean “old GPU.” Current NVK supports Kepler and later; Turing+ also defaults to NVK+Zink for OpenGL. A pre-Turing warning should say performance may be limited, never “NVK unsupported.” Use verified generation data or a general NVIDIA performance note. [Mesa NVK documentation](https://docs.mesa3d.org/drivers/nvk.html)

Keep a small deserialization compatibility path: ignore old `nvidia` config data and map saved `setup_step: graphics` forward. Otherwise deleting that enum value can invalidate an existing config.

2. **Existing installations**

Given the owner’s confirmed release history, no Fedora `:nvidia` installations exist. No channel migration, MOK removal or paired-image compatibility is needed.

Bazzite remains responsible for its own drivers and updates. Preserve session-mode behavior and actual driver reporting. Restrict Fedora-specific explanatory text to our OS.

3. **Fedora graphics**

Do not add `nouveau.config=NvGspRm=1`. Current upstream nouveau enables GSP by default on supported Turing+ hardware; the override is unnecessary. Verify the exact kernel in the pinned Fedora base during implementation rather than assuming all “7.x” builds match. [Kernel source](https://raw.githubusercontent.com/torvalds/linux/master/drivers/gpu/drm/nouveau/nvkm/subdev/gsp/tu102.c)

The main Containerfile already installs `mesa-dri-drivers`, `mesa-vulkan-drivers`, `vulkan-loader` and `nvidia-gpu-firmware`. Fedora’s Vulkan package supplies NVK; NVIDIA firmware is a `linux-firmware` subpackage. No extra driver package is needed. Add image checks for the NVK library/ICD, nouveau module and required firmware. [Fedora Mesa package](https://packages.fedoraproject.org/pkgs/mesa/mesa-vulkan-drivers/fedora-44.html), [Fedora firmware package](https://packages.fedoraproject.org/pkgs/linux-firmware/nvidia-gpu-firmware/)

Delete `Containerfile.nvidia`, `nvidia/`, `secureboot/`, certificate labels, `SECUREBOOT_KEY`, MOK commands and passwords. Remove `mokutil` unless retained solely for hardware diagnostics. Keep RPM Fusion dependencies needed for codecs.

4. **Unsigned test path**

Choose signing mode **before building**. Missing secrets with a present public key otherwise still produce an unusable signature-required image.

Add an early credential preflight in `os-fedora-signing`. It exports only mode and public-key fingerprint. Missing public key or required secrets selects `unsigned-test`. Malformed keys, mismatched keys and actual signing failures fail the run. Never silently downgrade those failures. Account for environment branch restrictions so test-only branches can bypass credential access.

Unsigned candidates go to a separate repository, such as `ps5-launcher-fedora-test`, with run-specific tags and `IMAGE_REF`. Bake an immutable `unsigned-test` marker and OCI label. Show “Unsigned test image — not for real PCs” in launcher status, workflow summaries and test ISO naming.

Its policy rejects by default. Only that test repository gets `insecureAcceptAnything`; preserve required local transports. Ship no dummy public key. Production policy remains `sigstoreSigned`.

Keep `--enforce-container-sigpolicy`: it enforces the selected policy, although the test policy does not authenticate signatures. `signature-policy` and `check-system` must report this explicitly as unsigned test mode.

`install-test` still installs, boots, upgrades and rolls back unsigned candidates. Skip only cryptographic rejection assertions, recording them as skipped. Do not merely skip assertions while leaving the restrictive policy installed. Signed mode retains unsigned/wrong-key rejection tests and current signing flags.

Keep `OS_IMAGE_SIGNING: disabled`. Additionally require signed mode, successful signing for this attempt, production-policy validation and existing approvals in release guard, promotion and ISO attachment. Reject the unsigned marker even if somebody later signs that digest.

5. **Ordered implementation slices**

One agent should proceed in this order:

- Test config compatibility, setup progression and display text; remove launcher flows.
- Test removed helper commands fail without side effects; simplify updates, recovery and signature enforcement. Remove NVIDIA digest state and variant branches.
- Test signed/unsigned mode selection and policy generation; update Containerfile, build/check scripts and main-only ISO/kickstart.
- Rewire workflow dependencies, preflight, signing, install tests and release guards. Test that unsigned mode cannot promote with signing enabled.
- Update hardware tests, logs and README. Require real NVIDIA tests for nouveau binding, GSP loading, NVK rendering and Secure Boot.

Run `cargo test --profile ci --locked`, Rust formatting, Slint compilation, shell syntax checks, ShellCheck and local actionlint on macOS. The session-wrapper tests can run there too.

