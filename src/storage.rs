//! Drives and partitions from `lsblk --json`, for the Storage page (Phase 6 of
//! docs/plans/ps5-launcher-os.md). Parsing lives here; `system::call` runs `lsblk_call`.
//!
//! Formatting a drive is not here: it needs its own root-helper task with its own polkit action
//! (and a second confirmation, and a refusal of the system disk), so it comes as a separate step.

use std::path::{Path, PathBuf};

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
    /// The OS runs from it: never offered for formatting.
    pub system: bool,
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

/// Where the OS mounts its own partitions (bootc mounts the root at /sysroot).
fn is_system_mount(mountpoint: &str) -> bool {
    matches!(mountpoint, "/" | "/sysroot" | "/boot" | "/boot/efi" | "/usr" | "/var" | "[SWAP]")
}

/// Whether the entry or anything below it (partition, LUKS, LVM) holds a system mount.
fn mounts_system(e: &Entry) -> bool {
    e.mountpoint.as_deref().is_some_and(is_system_mount) || e.children.iter().any(mounts_system)
}

/// The drives in lsblk's JSON. Empty and virtual devices (nbd, loop, zram, ram) are left out.
pub fn parse_drives(json: &str) -> Result<Vec<Drive>, String> {
    let lsblk: Lsblk = serde_json::from_str(json).map_err(|e| format!("unexpected lsblk output: {e}"))?;
    let virtual_name = |name: &str| ["nbd", "zram", "ram"].iter().any(|p| name.starts_with(p));
    Ok(lsblk
        .blockdevices
        .into_iter()
        .filter(|e| e.kind == "disk" && e.size > 0 && !virtual_name(&e.name))
        .map(|e| {
            let system = mounts_system(&e);
            Drive {
                name: e.model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).unwrap_or(e.name),
                path: e.path,
                bytes: e.size,
                removable: e.rm || e.hotplug,
                system,
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
            }
        })
        .collect())
}

pub fn lsblk_call() -> crate::system::Call {
    crate::system::Call::new("lsblk", &LSBLK_ARGS, 10)
}

/// The partitions the Storage page lists: those with a file system, without swap and the boot
/// partitions, which only the OS uses.
pub fn listed(p: &Partition) -> bool {
    p.fstype.as_deref().is_some_and(|f| f != "swap") && !matches!(p.mountpoint.as_deref(), Some("/boot" | "/boot/efi" | "[SWAP]"))
}

/// The folder "Use for games" adds to the game folders: `Games` on the partition.
pub fn games_dir(mountpoint: &str) -> PathBuf {
    Path::new(mountpoint).join("Games")
}

/// Whether "Use for games" is offered for a partition: mounted, not on the system disk (the game
/// folders there are in the home folder already), and not a game folder yet. `in_use` tells
/// whether a folder is in the game folders.
pub fn offers_games(drive: &Drive, p: &Partition, in_use: &dyn Fn(&Path) -> bool) -> bool {
    match p.mountpoint.as_deref() {
        Some(mp) if !drive.system && mp.starts_with('/') => !in_use(&games_dir(mp)),
        _ => false,
    }
}

/// "SANDISK ULTRA · 64.0 GB · USB": a drive's header on the Storage page.
pub fn drive_title(drive: &Drive) -> String {
    let mut parts = vec![drive.name.to_uppercase(), gigabytes(drive.bytes)];
    if drive.system {
        parts.push("SYSTEM".into());
    } else if drive.removable {
        parts.push("USB".into());
    }
    parts.join(" · ")
}

/// Sizes as drive makers print them: "64.0 GB", "1.0 TB".
pub fn gigabytes(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1000.0 { format!("{:.1} TB", gb / 1000.0) } else { format!("{gb:.1} GB") }
}

/// A file system's usual name: lsblk prints "exfat", people say "exFAT".
pub fn fs_name(fstype: &str) -> String {
    match fstype {
        "exfat" => "exFAT".into(),
        "vfat" => "FAT32".into(),
        "ntfs" | "ntfs3" => "NTFS".into(),
        "btrfs" => "Btrfs".into(),
        "xfs" => "XFS".into(),
        "crypto_LUKS" => "Encrypted".into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_system_names() {
        assert_eq!(fs_name("exfat"), "exFAT");
        assert_eq!(fs_name("ntfs3"), "NTFS");
        assert_eq!(fs_name("ext4"), "ext4");
    }

    fn part(fstype: Option<&str>, mountpoint: Option<&str>) -> Partition {
        Partition { path: "/dev/sda1".into(), bytes: 1, fstype: fstype.map(String::from), label: None, mountpoint: mountpoint.map(String::from) }
    }

    #[test]
    fn the_page_lists_partitions_with_a_file_system() {
        let drives = parse_drives(include_str!("testdata/fedora44-vm-lsblk.json")).unwrap();
        let listed: Vec<_> = drives[0].partitions.iter().filter(|p| listed(p)).map(|p| p.path.as_str()).collect();
        assert_eq!(listed, ["/dev/vda4"], "the BIOS boot, EFI and boot partitions are left out");
        assert!(!listed_part(Some("swap"), None));
        assert!(listed_part(Some("exfat"), None), "an unmounted stick still shows");
    }

    fn listed_part(fstype: Option<&str>, mountpoint: Option<&str>) -> bool {
        listed(&part(fstype, mountpoint))
    }

    #[test]
    fn use_for_games_adds_a_games_folder() {
        assert_eq!(games_dir("/run/media/player/GAMES"), PathBuf::from("/run/media/player/GAMES/Games"));
    }

    #[test]
    fn use_for_games_needs_a_mounted_partition_off_the_system_disk() {
        let mut drive = parse_drives(include_str!("testdata/fedora44-vm-lsblk.json")).unwrap().remove(0);
        let none = |_: &Path| false;
        let root = drive.partitions[3].clone();
        assert!(!offers_games(&drive, &root, &none), "the system disk");
        drive.system = false;
        assert!(offers_games(&drive, &root, &none));
        assert!(!offers_games(&drive, &part(Some("exfat"), None), &none), "not mounted");
        let used = |p: &Path| p == Path::new("/sysroot/Games");
        assert!(!offers_games(&drive, &root, &used), "a game folder already");
    }

    #[test]
    fn drive_titles() {
        let drives = parse_drives(include_str!("testdata/fedora44-vm-lsblk.json")).unwrap();
        assert_eq!(drive_title(&drives[0]), "VDA · 10.7 GB · SYSTEM");
        let usb = Drive { path: "/dev/sda".into(), name: "SanDisk Ultra".into(), bytes: 64_023_257_088, removable: true, system: false, partitions: vec![] };
        assert_eq!(drive_title(&usb), "SANDISK ULTRA · 64.0 GB · USB");
        assert_eq!(gigabytes(2_000_398_934_016), "2.0 TB");
    }

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
                system: false,
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
    fn the_disk_the_os_runs_from_is_the_system_disk() {
        let drives = parse_drives(include_str!("testdata/fedora44-vm-lsblk.json")).unwrap();
        assert_eq!(drives.len(), 1, "the DVD drive (type rom) is left out");
        assert!(drives[0].system, "/sysroot, /boot and /boot/efi are on it");
        assert_eq!(drives[0].partitions.len(), 4);
        assert!(!parse_drives(CAPTURED).unwrap()[1].system);
    }

    #[test]
    fn a_root_inside_luks_and_lvm_still_marks_the_system_disk() {
        // lsblk nests: disk → partition → crypt → lvm. Only the deepest one is mounted.
        let json = r#"{"blockdevices": [
            {"name": "nvme0n1", "path": "/dev/nvme0n1", "size": 512110190592, "type": "disk",
             "fstype": null, "mountpoint": null, "rm": false, "hotplug": false, "label": null,
             "model": "Samsung SSD 980",
             "children": [
                {"name": "nvme0n1p1", "path": "/dev/nvme0n1p1", "size": 629145600, "type": "part",
                 "fstype": "vfat", "mountpoint": null, "rm": false, "hotplug": false, "label": null, "model": null},
                {"name": "nvme0n1p2", "path": "/dev/nvme0n1p2", "size": 511479848960, "type": "part",
                 "fstype": "crypto_LUKS", "mountpoint": null, "rm": false, "hotplug": false, "label": null, "model": null,
                 "children": [
                    {"name": "luks-1", "path": "/dev/mapper/luks-1", "size": 511462023168, "type": "crypt",
                     "fstype": "LVM2_member", "mountpoint": null, "rm": false, "hotplug": false, "label": null, "model": null,
                     "children": [
                        {"name": "vg-root", "path": "/dev/mapper/vg-root", "size": 511462023168, "type": "lvm",
                         "fstype": "ext4", "mountpoint": "/sysroot", "rm": false, "hotplug": false, "label": null, "model": null}
                     ]}
                 ]}
             ]}
        ]}"#;
        let drives = parse_drives(json).unwrap();
        assert!(drives[0].system);
        assert_eq!(drives[0].partitions.len(), 2, "partitions are the disk's direct children");
    }

    #[test]
    fn output_that_is_not_lsblk_json_is_an_error() {
        assert!(parse_drives("").is_err());
        assert!(parse_drives("lsblk: unknown column").is_err());
        assert!(parse_drives(r#"{"devices": []}"#).is_err());
    }
}
