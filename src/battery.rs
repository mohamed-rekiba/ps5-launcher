//! Controller batteries from /sys/class/power_supply, for the Controllers page and the Quick
//! Menu's Controllers card (docs/plans/ps5-launcher-os.md, Phase 6). Every reader takes its sysfs
//! folder, so tests use a fake tree.
//!
//! Each controller driver registers its battery with the HID device as the parent, so the
//! battery's `device` link points at the folder that holds the controller's input device:
//! - hid-playstation (DualSense, DualShock 4): `ps-controller-battery-<mac>`
//!   (drivers/hid/hid-playstation.c, ps_device_register_battery: "ps-controller-battery-%pMR",
//!   registered on &hdev->dev).
//! - hid-sony (older kernels' DualShock 4, Sixaxis): `sony_controller_battery_<mac>`, or
//!   `sony_controller_battery_<mac>_<id>` (hid-sony.c, sony_battery_probe).
//! - hid-nintendo (Switch Pro): `nintendo_switch_controller_battery_<hid device>`
//!   (hid-nintendo.c, joycon_power_supply_create). It has `capacity_level`, not `capacity`.
//! - Any HID device that reports a battery (an Xbox controller over Bluetooth, which
//!   hid-microsoft hands to hid-input): `hid-<uniq or hid device>-battery` (hid-input.c,
//!   hidinput_setup_battery, v6.17), `hid-<…>-battery-<report id>` on newer kernels.
//! - xpad (Xbox controllers by USB or the Xbox wireless adapter) registers no battery.
//!
//! %pMR prints the MAC in lower case. Every one of them reports `scope` Device; the PC's own
//! battery reports System.

use crate::gamepad::InputPad;
use std::path::{Path, PathBuf};

/// sysfs, where an input device's `S: Sysfs=` path starts.
pub const SYS: &str = "/sys";

/// How full a battery is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Level {
    /// `capacity`, 0 to 100.
    Percent(u8),
    /// `capacity_level` (hid-nintendo): "Critical", "Low", "Normal", "High" or "Full".
    Coarse(String),
    Unknown,
}

/// A device battery in /sys/class/power_supply.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Battery {
    /// The folder's name, for example ps-controller-battery-a0:ab:51:5f:23:1a.
    pub name: String,
    pub level: Level,
    /// `status` says Charging.
    pub charging: bool,
    /// Where the `device` link points: the controller's HID device.
    device: Option<PathBuf>,
}

/// A connected controller and its battery, if the PC knows it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Controller {
    pub pad: InputPad,
    pub battery: Option<Battery>,
}

/// A sysfs attribute without its line end; None when it cannot be read.
fn attr(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok().map(|s| s.trim().to_string())
}

/// The device batteries under `power_supply` (/sys/class/power_supply), in name order. Reading
/// `capacity` can ask the device itself (hid-input), so call this off the UI thread.
pub fn read(power_supply: &Path) -> Vec<Battery> {
    let Ok(entries) = std::fs::read_dir(power_supply) else { return Vec::new() };
    let mut batteries: Vec<Battery> = entries
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            // Every supply has `type`; a battery without `scope` is not a device's.
            if attr(&dir, "type").as_deref() != Some("Battery") || attr(&dir, "scope").as_deref() != Some("Device") {
                return None;
            }
            let level = match (attr(&dir, "capacity").and_then(|c| c.parse::<u8>().ok()), attr(&dir, "capacity_level")) {
                (Some(percent), _) if percent <= 100 => Level::Percent(percent),
                (_, Some(level)) if !level.is_empty() && level != "Unknown" => Level::Coarse(level),
                _ => Level::Unknown,
            };
            Some(Battery {
                name: entry.file_name().to_string_lossy().into_owned(),
                level,
                charging: attr(&dir, "status").as_deref() == Some("Charging"),
                device: std::fs::canonicalize(dir.join("device")).ok(),
            })
        })
        .collect();
    batteries.sort_by(|a, b| a.name.cmp(&b.name));
    batteries
}

/// The MAC address in a battery's name, in upper case: "ps-controller-battery-a0:ab:51:5f:23:1a"
/// → "A0:AB:51:5F:23:1A".
pub fn mac_in(name: &str) -> Option<String> {
    name.as_bytes()
        .windows(17)
        .filter_map(|w| std::str::from_utf8(w).ok())
        .map(|s| s.to_ascii_uppercase())
        .find(|s| crate::bluetooth::valid_mac(s))
}

/// The battery of `pad`: the one whose device holds the pad's input device (under `sys`), or else
/// the one whose name has the pad's MAC address.
pub fn of<'a>(pad: &InputPad, sys: &Path, batteries: &'a [Battery]) -> Option<&'a Battery> {
    let input = (!pad.sysfs.is_empty()).then(|| std::fs::canonicalize(sys.join(pad.sysfs.trim_start_matches('/'))).ok()).flatten();
    let by_folder = input.and_then(|input| batteries.iter().find(|b| b.device.as_ref().is_some_and(|d| input.starts_with(d))));
    by_folder.or_else(|| {
        let mac = pad.uniq.to_ascii_uppercase();
        crate::bluetooth::valid_mac(&mac).then(|| batteries.iter().find(|b| mac_in(&b.name).as_deref() == Some(mac.as_str()))).flatten()
    })
}

/// Each controller with its battery. `sys` is /sys.
pub fn controllers(pads: Vec<InputPad>, sys: &Path) -> Vec<Controller> {
    if pads.is_empty() {
        return Vec::new();
    }
    let batteries = read(&sys.join("class/power_supply"));
    pads.into_iter().map(|pad| Controller { battery: of(&pad, sys, &batteries).cloned(), pad }).collect()
}

/// "80%", "80%, charging", "Charging", "Low", or empty when nothing is known.
pub fn text(battery: &Battery) -> String {
    let level = match &battery.level {
        Level::Percent(p) => format!("{p}%"),
        Level::Coarse(level) => level.clone(),
        Level::Unknown => String::new(),
    };
    match (level.is_empty(), battery.charging) {
        (true, true) => "Charging".into(),
        (false, true) => format!("{level}, charging"),
        _ => level,
    }
}

#[cfg(test)]
impl Battery {
    /// A battery of no device, for other modules' tests.
    pub fn fake(level: Level, charging: bool) -> Battery {
        Battery { name: "fake".into(), level, charging, device: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::Bus;
    use std::os::unix::fs::symlink;

    /// A HID device folder under a fake /sys, with an input device in it. Returns the input
    /// device's `S: Sysfs=` path.
    fn hid(sys: &Path, dir: &str, input: &str) -> String {
        std::fs::create_dir_all(sys.join(dir).join("input").join(input)).unwrap();
        format!("/{dir}/input/{input}")
    }

    /// A power supply linked to `device` (under the fake /sys), with these files.
    fn supply(sys: &Path, name: &str, device: &str, files: &[(&str, &str)]) {
        let dir = sys.join("class/power_supply").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        symlink(sys.join(device), dir.join("device")).unwrap();
        for (file, value) in files {
            std::fs::write(dir.join(file), format!("{value}\n")).unwrap();
        }
    }

    fn pad(name: &str, uniq: &str, sysfs: &str) -> InputPad {
        InputPad { name: name.into(), bus: Some(Bus::Bluetooth), uniq: uniq.into(), sysfs: sysfs.into() }
    }

    const DUALSENSE: &str = "devices/virtual/misc/uhid/0005:054C:0CE6.0001";
    const PRO: &str = "devices/virtual/misc/uhid/0005:057E:2009.0004";
    const XBOX: &str = "devices/virtual/misc/uhid/0005:045E:0B13.0005";

    #[test]
    fn a_battery_belongs_to_the_controller_whose_hid_device_it_hangs_off() {
        let sys = tempfile::tempdir().unwrap();
        let s = sys.path();
        let ds = hid(s, DUALSENSE, "input20");
        let xbox = hid(s, XBOX, "input31");
        let device = [("type", "Battery"), ("scope", "Device")];
        supply(s, "ps-controller-battery-a0:ab:51:5f:23:1a", DUALSENSE, &[&device[..], &[("capacity", "80"), ("status", "Discharging")]].concat());
        // A newer kernel's generic HID name, with the report id at the end.
        supply(s, "hid-98:7a:14:01:02:03-battery-4", XBOX, &[&device[..], &[("capacity", "45"), ("status", "Charging")]].concat());
        let pads = vec![pad("DualSense Wireless Controller", "a0:ab:51:5f:23:1a", &ds), pad("Xbox Wireless Controller", "", &xbox)];
        let found = controllers(pads, s);
        let level = |i: usize| found[i].battery.as_ref().map(|b| (b.level.clone(), b.charging));
        assert_eq!(level(0), Some((Level::Percent(80), false)));
        assert_eq!(level(1), Some((Level::Percent(45), true)), "matched by its folder, not its name");
    }

    #[test]
    fn the_pcs_own_battery_and_other_supplies_are_not_controllers() {
        let sys = tempfile::tempdir().unwrap();
        let s = sys.path();
        std::fs::create_dir_all(s.join("devices/LNXSYSTM:00/PNP0C0A:00")).unwrap();
        supply(s, "BAT0", "devices/LNXSYSTM:00/PNP0C0A:00", &[("type", "Battery"), ("scope", "System"), ("capacity", "60"), ("status", "Discharging")]);
        supply(s, "AC", "devices/LNXSYSTM:00/PNP0C0A:00", &[("type", "Mains"), ("online", "1")]);
        // A mouse's battery: a device battery, but of no controller.
        let mouse = hid(s, "devices/virtual/misc/uhid/0005:046D:B023.0006", "input40");
        supply(s, "hid-c4:0a:11:22:33:44-battery", "devices/virtual/misc/uhid/0005:046D:B023.0006", &[("type", "Battery"), ("scope", "Device"), ("capacity", "90")]);
        let batteries = read(&s.join("class/power_supply"));
        assert_eq!(batteries.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["hid-c4:0a:11:22:33:44-battery"], "only device batteries");
        let ds = hid(s, DUALSENSE, "input20");
        let found = controllers(vec![pad("DualSense Wireless Controller", "a0:ab:51:5f:23:1a", &ds)], s);
        assert_eq!(found[0].battery, None, "the mouse's battery is not the controller's");
        assert!(mouse.ends_with("input40"));
    }

    #[test]
    fn without_a_matching_folder_the_mac_in_the_name_decides() {
        let sys = tempfile::tempdir().unwrap();
        let s = sys.path();
        // The battery hangs off a folder the pad's path does not show (the pad's path is gone).
        std::fs::create_dir_all(s.join(DUALSENSE)).unwrap();
        supply(s, "sony_controller_battery_1c:a0:b8:4e:91:02", DUALSENSE, &[("type", "Battery"), ("scope", "Device"), ("capacity", "30"), ("status", "Discharging")]);
        let found = controllers(vec![pad("Wireless Controller", "1C:A0:B8:4E:91:02", "/devices/gone/input/input9")], s);
        assert_eq!(found[0].battery.as_ref().map(|b| b.level.clone()), Some(Level::Percent(30)));
        let other = controllers(vec![pad("Wireless Controller", "1c:a0:b8:4e:91:03", "/devices/gone/input/input9")], s);
        assert_eq!(other[0].battery, None, "another MAC");
        let none = controllers(vec![pad("Wireless Controller", "", "")], s);
        assert_eq!(none[0].battery, None, "no MAC and no path");
    }

    #[test]
    fn the_switch_pro_controller_only_has_a_coarse_level() {
        let sys = tempfile::tempdir().unwrap();
        let s = sys.path();
        let pro = hid(s, PRO, "input50");
        supply(s, "nintendo_switch_controller_battery_0005:057E:2009.0004", PRO, &[("type", "Battery"), ("scope", "Device"), ("capacity_level", "High"), ("status", "Discharging")]);
        let found = controllers(vec![pad("Nintendo Switch Pro Controller", "98:b6:e9:00:11:22", &pro)], s);
        assert_eq!(found[0].battery.as_ref().map(|b| b.level.clone()), Some(Level::Coarse("High".into())));
    }

    #[test]
    fn a_missing_or_broken_folder_has_no_batteries() {
        assert_eq!(read(Path::new("/nonexistent/power_supply")), []);
        let sys = tempfile::tempdir().unwrap();
        let s = sys.path();
        let ds = hid(s, DUALSENSE, "input20");
        // No capacity yet (no report arrived): the level is unknown.
        supply(s, "ps-controller-battery-a0:ab:51:5f:23:1a", DUALSENSE, &[("type", "Battery"), ("scope", "Device"), ("capacity", "garbage"), ("status", "Unknown")]);
        let found = controllers(vec![pad("DualSense Wireless Controller", "", &ds)], s);
        assert_eq!(found[0].battery.as_ref().map(|b| b.level.clone()), Some(Level::Unknown));
        assert_eq!(controllers(Vec::new(), s), []);
    }

    #[test]
    fn the_mac_comes_out_of_each_drivers_name() {
        assert_eq!(mac_in("ps-controller-battery-a0:ab:51:5f:23:1a").as_deref(), Some("A0:AB:51:5F:23:1A"));
        assert_eq!(mac_in("sony_controller_battery_1c:a0:b8:4e:91:02_3").as_deref(), Some("1C:A0:B8:4E:91:02"));
        assert_eq!(mac_in("hid-98:7a:14:01:02:03-battery").as_deref(), Some("98:7A:14:01:02:03"));
        assert_eq!(mac_in("nintendo_switch_controller_battery_0005:057E:2009.0004"), None);
        assert_eq!(mac_in("hid-0005:045E:0B13.0005-battery"), None);
        assert_eq!(mac_in("BAT0"), None);
    }

    fn battery(level: Level, charging: bool) -> Battery {
        Battery { name: "b".into(), level, charging, device: None }
    }

    #[test]
    fn the_level_reads_short() {
        assert_eq!(text(&battery(Level::Percent(80), false)), "80%");
        assert_eq!(text(&battery(Level::Percent(80), true)), "80%, charging");
        assert_eq!(text(&battery(Level::Unknown, true)), "Charging");
        assert_eq!(text(&battery(Level::Coarse("Low".into()), false)), "Low");
        assert_eq!(text(&battery(Level::Coarse("Full".into()), true)), "Full, charging");
        assert_eq!(text(&battery(Level::Unknown, false)), "");
    }
}
