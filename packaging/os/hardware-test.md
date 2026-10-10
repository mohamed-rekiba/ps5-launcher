# Hardware checks before promotion

Use the candidate ISO artifact from the exact workflow run being promoted. Run this sheet on
an AMD or Intel PC and an NVIDIA PC. CI does not test real GPU rendering or firmware Secure
Boot. Fedora handles boot trust; there is no launcher key to enroll.

Load the artifact's `manifest.txt`, verify the ISO checksum, and record:

| Item | Result |
|---|---|
| Run ID, attempt, commit | |
| MAIN_DIGEST, UPGRADE_DIGEST | |
| ISO SHA-256 | |
| PC, GPU, monitor and firmware | |
| Secure Boot enabled or disabled in firmware | |

For each stage, collect logs with `sudo bash collect-hardware-logs.sh STAGE`. Record the
booted digest (`sudo bootc status --json`), result, and log archive. Do not approve results
from a different run or digest.

| Stage | Check | Result and logs |
|---|---|---|
| Install | Boot the ISO and install the main image. First boot matches MAIN_DIGEST; setup and launcher work. No project key or enrollment prompt appears. | |
| Graphics | AMD/Intel use their Fedora kernel drivers. NVIDIA binds to nouveau, loads required firmware/GSP on supported hardware, and renders Vulkan using NVK. Check actual rendering; PCI detection alone is insufficient. | |
| Secure Boot | Repeat boot and graphics checks with firmware Secure Boot enabled. Fedora's boot chain and kernel work without a project certificate. | |
| Play | Run representative games, trailers, sound over HDMI, resolution/refresh changes, and controller input. Record GPU and any performance limits. | |
| Desktop | Switch to desktop; Plasma works. On the following reboot the launcher returns. | |
| Upgrade | Switch to the signed UPGRADE_DIGEST with `sudo bootc switch --enforce-container-sigpolicy "$IMAGE@$UPGRADE_DIGEST"`; restart and verify digest and session. | |
| Rollback | `sudo bootc rollback`, restart, and verify MAIN_DIGEST and session. | |
| Boot health | Confirm a healthy session records success. Exercise a failed first boot and confirm only one automatic rollback to the recorded healthy digest. | |
| Recovery | Start with `systemd.unit=ps5-recovery.target`; test main-channel switch, rollback and restart. No driver/key menu is present. | |
| Suspend | Test power-button suspend/resume and controller reconnect. | |

Attach this completed sheet and archives to the promotion approval. If a stage is skipped,
explain why; record graphics failures instead of treating an untested card as supported.
