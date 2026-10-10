//! Drives and partitions from `lsblk --json`, for the Storage page (Phase 6 of
//! docs/plans/ps5-launcher-os.md). Only parsing lives here; `system::run_output` runs lsblk with
//! `LSBLK_ARGS`.
#![allow(dead_code)] // nothing calls it until the Storage page (Phase 6)

/// The lsblk arguments `parse_drives` expects.
pub const LSBLK_ARGS: [&str; 4] = ["--json", "-b", "-o", "NAME,PATH,SIZE,TYPE,FSTYPE,MOUNTPOINT,RM,HOTPLUG,LABEL,MODEL"];

/// A whole disk: an internal drive, or a USB stick or disk.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Drive {
    pub path: String,
    /// The model, or the device name when there is none.
    pub name: String,
    pub bytes: u64,
    /// Removable or hot-plugged (USB): offered as "Use it for games?".
    pub removable: bool,
    pub partitions: Vec<Partition>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Partition {
    pub path: String,
    pub bytes: u64,
    pub fstype: Option<String>,
    pub label: Option<String>,
    pub mountpoint: Option<String>,
}

/// One entry of lsblk's JSON, with only the columns in `LSBLK_ARGS`.
#[derive(serde::Deserialize)]
struct Entry {
    name: String,
    path: String,
    size: u64,
    #[serde(rename = "type")]
    kind: String,
    fstype: Option<String>,
    mountpoint: Option<String>,
    rm: bool,
    hotplug: bool,
    label: Option<String>,
    model: Option<String>,
    #[serde(default)]
    children: Vec<Entry>,
}

#[derive(serde::Deserialize)]
struct Lsblk {
    blockdevices: Vec<Entry>,
}

/// The drives in lsblk's JSON. Empty and virtual devices (nbd, loop, zram, ram) are left out.
pub fn parse_drives(json: &str) -> Result<Vec<Drive>, String> {
    let lsblk: Lsblk = serde_json::from_str(json).map_err(|e| format!("unexpected lsblk output: {e}"))?;
    let virtual_name = |name: &str| ["nbd", "zram", "ram"].iter().any(|p| name.starts_with(p));
    Ok(lsblk
        .blockdevices
        .into_iter()
        .filter(|e| e.kind == "disk" && e.size > 0 && !virtual_name(&e.name))
        .map(|e| Drive {
            name: e.model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).unwrap_or(e.name),
            path: e.path,
            bytes: e.size,
            removable: e.rm || e.hotplug,
            partitions: e
                .children
                .into_iter()
                .map(|c| Partition {
                    path: c.path,
                    bytes: c.size,
                    fstype: c.fstype,
                    label: c.label,
                    mountpoint: c.mountpoint,
                })
                .collect(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured on Fedora 44 (util-linux 2.41.5) in a container, cut to one empty nbd device and
    /// the real disks.
    const CAPTURED: &str = r#"{
   "blockdevices": [
      {"name": "nbd0", "path": "/dev/nbd0", "size": 0, "type": "disk", "fstype": null,
       "mountpoint": null, "rm": false, "hotplug": false, "label": null, "model": null},
      {"name": "vda", "path": "/dev/vda", "size": 994662416384, "type": "disk", "fstype": null,
       "mountpoint": null, "rm": false, "hotplug": false, "label": null, "model": null,
       "children": [
          {"name": "vda1", "path": "/dev/vda1", "size": 994661367808, "type": "part", "fstype": null,
           "mountpoint": "/etc/resolv.conf", "rm": false, "hotplug": false, "label": null, "model": null}
       ]},
      {"name": "vdb", "path": "/dev/vdb", "size": 677490688, "type": "disk", "fstype": null,
       "mountpoint": null, "rm": false, "hotplug": false, "label": null, "model": null}
   ]
}"#;

    #[test]
    fn real_disks_with_their_partitions() {
        let drives = parse_drives(CAPTURED).unwrap();
        assert_eq!(drives.len(), 2, "the empty nbd device is left out");
        assert_eq!(
            drives[0],
            Drive {
                path: "/dev/vda".into(),
                name: "vda".into(),
                bytes: 994662416384,
                removable: false,
                partitions: vec![Partition {
                    path: "/dev/vda1".into(),
                    bytes: 994661367808,
                    fstype: None,
                    label: None,
                    mountpoint: Some("/etc/resolv.conf".into()),
                }],
            }
        );
        assert!(drives[1].partitions.is_empty());
    }

    #[test]
    fn a_usb_stick_is_removable_and_named_by_its_model() {
        let json = r#"{"blockdevices": [
            {"name": "sda", "path": "/dev/sda", "size": 64023257088, "type": "disk", "fstype": null,
             "mountpoint": null, "rm": true, "hotplug": true, "label": null, "model": "SanDisk Ultra",
             "children": [
                {"name": "sda1", "path": "/dev/sda1", "size": 64022208512, "type": "part",
                 "fstype": "exfat", "mountpoint": "/run/media/player/GAMES", "rm": true,
                 "hotplug": true, "label": "GAMES", "model": null}
             ]},
            {"name": "zram0", "path": "/dev/zram0", "size": 8589934592, "type": "disk", "fstype": "swap",
             "mountpoint": "[SWAP]", "rm": false, "hotplug": false, "label": "zram0", "model": null},
            {"name": "loop0", "path": "/dev/loop0", "size": 1048576, "type": "loop", "fstype": null,
             "mountpoint": null, "rm": false, "hotplug": false, "label": null, "model": null}
        ]}"#;
        let drives = parse_drives(json).unwrap();
        assert_eq!(drives.len(), 1, "zram and loop devices are left out");
        assert_eq!(drives[0].name, "SanDisk Ultra");
        assert!(drives[0].removable);
        assert_eq!(drives[0].partitions[0].fstype.as_deref(), Some("exfat"));
        assert_eq!(drives[0].partitions[0].label.as_deref(), Some("GAMES"));
    }

    #[test]
    fn output_that_is_not_lsblk_json_is_an_error() {
        assert!(parse_drives("").is_err());
        assert!(parse_drives("lsblk: unknown column").is_err());
        assert!(parse_drives(r#"{"devices": []}"#).is_err());
    }
}
