//! The GPU that drives the screen, and its driver (Phase 6 of docs/plans/ps5-launcher-os.md,
//! System → Display). The rule is the boot health check's
//! (packaging/os/files/usr/libexec/ps5-launcher-os/boot-health): the first DRM connector whose
//! `status` is `connected`, then its card's `device/driver`. Any other GPU in the PC does not
//! count. Every reader takes its sysfs folder, so tests use a fake tree.

use crate::system::Call;
use std::path::Path;

pub const NVIDIA: u16 = 0x10de;
pub const AMD: u16 = 0x1002;
pub const INTEL: u16 = 0x8086;

/// The kernel's folders the readers use on a real PC.
pub const DRM_DIR: &str = "/sys/class/drm";
pub const NVIDIA_MODULE: &str = "/sys/module/nvidia";

/// A GPU, from its PCI device in sysfs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Card {
    pub vendor: u16,
    pub device: u16,
    /// The PCI address, for example "0000:01:00.0" (for `lspci -s`).
    pub slot: Option<String>,
    /// The kernel driver bound to it ("amdgpu", "nvidia", …); None when none is bound.
    pub driver: Option<String>,
}

/// The connected screen and the GPU that drives it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Screen {
    /// The connector's name, for example "HDMI-A-1" (from "card1-HDMI-A-1").
    pub connector: String,
    pub card: Card,
    /// The connector's `modes` file, as the kernel wrote it.
    pub modes: String,
    /// The connector's EDID; empty when the kernel has none.
    pub edid: Vec<u8>,
}

/// A hexadecimal sysfs ID such as "0x10de\n".
fn hex_id(text: &str) -> Option<u16> {
    u16::from_str_radix(text.trim().strip_prefix("0x")?, 16).ok()
}

/// The last part of a symlink's target: a driver's name, a PCI slot.
fn link_name(path: &Path) -> Option<String> {
    let target = std::fs::read_link(path).ok()?;
    Some(target.file_name()?.to_string_lossy().into_owned())
}

/// The screen and its GPU, from `drm` (`/sys/class/drm`). The connectors are taken in name
/// order; the first connected one wins. None: no screen is connected, or its card has no PCI
/// IDs.
pub fn read_screen(drm: &Path) -> Option<Screen> {
    let mut names: Vec<String> = std::fs::read_dir(drm).ok()?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for name in names {
        let Some((card, connector)) = name.split_once('-') else { continue };
        if !card.starts_with("card") {
            continue;
        }
        let dir = drm.join(&name);
        if !std::fs::read_to_string(dir.join("status")).is_ok_and(|s| s.trim() == "connected") {
            continue;
        }
        let device = drm.join(card).join("device");
        let read = |file: &str| std::fs::read_to_string(device.join(file)).ok().and_then(|t| hex_id(&t));
        let (Some(vendor), Some(id)) = (read("vendor"), read("device")) else { continue };
        let card = Card { vendor, device: id, slot: link_name(&device), driver: link_name(&device.join("driver")) };
        return Some(Screen {
            connector: connector.to_string(),
            card,
            modes: std::fs::read_to_string(dir.join("modes")).unwrap_or_default(),
            edid: std::fs::read(dir.join("edid")).unwrap_or_default(),
        });
    }
    None
}

/// The NVIDIA driver's version (`/sys/module/nvidia/version`), when it is loaded.
pub fn nvidia_version(module: &Path) -> Option<String> {
    let v = std::fs::read_to_string(module.join("version")).ok()?;
    let v = v.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// `lspci -nn -s <slot>`: the card's name (lspci may be missing; the name is then a fallback).
pub fn lspci_call(slot: &str) -> Call {
    Call::new("lspci", &["-nn", "-s", slot], 5)
}

/// The card's name from `lspci -nn` output, for example
/// `01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA104 [GeForce RTX 3070] [10de:2484] (rev a1)`
/// gives "NVIDIA GeForce RTX 3070". The brand name in brackets wins over the chip's code name.
pub fn parse_lspci(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    let (_, rest) = line.split_once("]: ")?;
    // Drop " (rev a1)", then the " [10de:2484]" IDs at the end.
    let rest = rest.split(" (rev ").next()?.trim_end();
    let rest = match rest.rfind(" [") {
        Some(at) if rest[at + 2..].trim_end_matches(']').contains(':') => &rest[..at],
        _ => rest,
    };
    let (vendor, model) = [("NVIDIA Corporation ", "NVIDIA"), ("Advanced Micro Devices, Inc. [AMD/ATI] ", "AMD"), ("Intel Corporation ", "Intel")]
        .iter()
        .find_map(|(long, short)| rest.strip_prefix(long).map(|m| (*short, m)))
        .unwrap_or(("", rest));
    // "GA104 [GeForce RTX 3070]" → "GeForce RTX 3070".
    let model = match (model.find('['), model.ends_with(']')) {
        (Some(open), true) => &model[open + 1..model.len() - 1],
        _ => model,
    };
    let name = if vendor.is_empty() { model.to_string() } else { format!("{vendor} {model}") };
    (!name.trim().is_empty()).then(|| name.trim().to_string())
}

/// A name for the card when lspci gives none: "NVIDIA graphics card (10de:2484)".
pub fn fallback_name(card: &Card) -> String {
    let vendor = match card.vendor {
        NVIDIA => "NVIDIA",
        AMD => "AMD",
        INTEL => "Intel",
        _ => "Unknown",
    };
    format!(
        "{vendor} graphics card ({:04x}:{:04x})",
        card.vendor, card.device
    )
}

/// The Display page's driver line.
pub fn driver_text(card: &Card, nvidia_version: Option<&str>) -> String {
    match card.driver.as_deref() {
        Some("nvidia") => match nvidia_version {
            Some(v) => format!("Driver: NVIDIA {v}"),
            None => "Driver: NVIDIA".into(),
        },
        Some(driver) => format!("Driver: open-source ({driver})"),
        None => "Driver: none loaded".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A fake /sys: a PCI device with `vendor`, `device` and a `driver` link, and a DRM card with
    /// its connectors (name, status).
    fn card(sys: &Path, n: u32, slot: &str, vendor: &str, device: &str, driver: Option<&str>, connectors: &[(&str, &str)]) {
        let pci = sys.join("devices/pci0000:00").join(slot);
        std::fs::create_dir_all(&pci).unwrap();
        std::fs::write(pci.join("vendor"), format!("{vendor}\n")).unwrap();
        std::fs::write(pci.join("device"), format!("{device}\n")).unwrap();
        if let Some(d) = driver {
            let drv = sys.join("bus/pci/drivers").join(d);
            std::fs::create_dir_all(&drv).unwrap();
            symlink(&drv, pci.join("driver")).unwrap();
        }
        let drm = sys.join("class/drm");
        let dir = drm.join(format!("card{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        symlink(&pci, dir.join("device")).unwrap();
        for (name, status) in connectors {
            let c = drm.join(format!("card{n}-{name}"));
            std::fs::create_dir_all(&c).unwrap();
            std::fs::write(c.join("status"), format!("{status}\n")).unwrap();
            std::fs::write(c.join("modes"), "1920x1080\n1280x720\n").unwrap();
        }
    }

    #[test]
    fn the_screen_is_driven_by_the_card_with_the_connected_connector() {
        let sys = tempfile::tempdir().unwrap();
        // An NVIDIA card with nothing connected, and the AMD card that drives the screen.
        card(sys.path(), 0, "0000:01:00.0", "0x10de", "0x2484", Some("nouveau"), &[("DP-1", "disconnected")]);
        card(sys.path(), 1, "0000:03:00.0", "0x1002", "0x73df", Some("amdgpu"), &[("DP-2", "disconnected"), ("HDMI-A-1", "connected")]);
        let screen = read_screen(&sys.path().join("class/drm")).unwrap();
        assert_eq!(screen.connector, "HDMI-A-1");
        assert_eq!(screen.card, Card { vendor: AMD, device: 0x73df, slot: Some("0000:03:00.0".into()), driver: Some("amdgpu".into()) });
        assert_eq!(screen.modes, "1920x1080\n1280x720\n");
        assert!(screen.edid.is_empty());
    }

    #[test]
    fn a_card_without_a_driver_and_no_screen_at_all() {
        let sys = tempfile::tempdir().unwrap();
        card(sys.path(), 0, "0000:01:00.0", "0x10de", "0x1c03", None, &[("DP-1", "connected")]);
        let screen = read_screen(&sys.path().join("class/drm")).unwrap();
        assert_eq!(screen.card.driver, None);
        assert_eq!(screen.card.device, 0x1c03);

        let none = tempfile::tempdir().unwrap();
        card(none.path(), 0, "0000:01:00.0", "0x1002", "0x73df", Some("amdgpu"), &[("DP-1", "disconnected")]);
        assert_eq!(read_screen(&none.path().join("class/drm")), None, "nothing connected");
        assert_eq!(read_screen(&none.path().join("missing")), None, "no DRM folder");
    }

    #[test]
    fn the_nvidia_driver_version() {
        let module = tempfile::tempdir().unwrap();
        assert_eq!(nvidia_version(module.path()), None, "not loaded");
        std::fs::write(module.path().join("version"), "580.95.05\n").unwrap();
        assert_eq!(nvidia_version(module.path()).as_deref(), Some("580.95.05"));
    }

    #[test]
    fn the_card_name_from_lspci() {
        let nvidia = "01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA104 [GeForce RTX 3070] [10de:2484] (rev a1)\n";
        assert_eq!(parse_lspci(nvidia).as_deref(), Some("NVIDIA GeForce RTX 3070"));
        let amd = "03:00.0 VGA compatible controller [0300]: Advanced Micro Devices, Inc. [AMD/ATI] Navi 22 [Radeon RX 6700/6700 XT/6750 XT / 6800M/6850M XT] [1002:73df] (rev c1)";
        assert_eq!(parse_lspci(amd).as_deref(), Some("AMD Radeon RX 6700/6700 XT/6750 XT / 6800M/6850M XT"));
        let intel = "00:02.0 VGA compatible controller [0300]: Intel Corporation Alder Lake-P GT2 [Iris Xe Graphics] [8086:46a6] (rev 0c)";
        assert_eq!(parse_lspci(intel).as_deref(), Some("Intel Iris Xe Graphics"));
        let plain = "00:02.0 VGA compatible controller [0300]: Some Vendor Chip 9 [abcd:0001]";
        assert_eq!(parse_lspci(plain).as_deref(), Some("Some Vendor Chip 9"));
        assert_eq!(parse_lspci(""), None);
        assert_eq!(parse_lspci("garbage"), None);
        assert_eq!(fallback_name(&Card { vendor: NVIDIA, device: 0x2484, slot: None, driver: None }), "NVIDIA graphics card (10de:2484)");
    }

    #[test]
    fn lspci_runs_with_a_time_limit() {
        let call = lspci_call("0000:01:00.0");
        assert_eq!(call.program, "lspci");
        assert_eq!(call.args, ["-nn", "-s", "0000:01:00.0"]);
        assert!(call.secs <= 10);
    }

    #[test]
    fn the_driver_line() {
        let mut card = Card { vendor: AMD, device: 0x73df, slot: None, driver: Some("amdgpu".into()) };
        assert_eq!(driver_text(&card, None), "Driver: open-source (amdgpu)");
        card.driver = Some("nouveau".into());
        assert_eq!(driver_text(&card, None), "Driver: open-source (nouveau)");
        card.driver = Some("nvidia".into());
        assert_eq!(driver_text(&card, Some("580.95.05")), "Driver: NVIDIA 580.95.05");
        assert_eq!(driver_text(&card, None), "Driver: NVIDIA");
        card.driver = None;
        assert_eq!(driver_text(&card, None), "Driver: none loaded");
    }
}
