//! Discovery: the emulator addons in `<root>/emulators/*/emulator.yaml`.

use super::document::{self, Issue, Version};
use super::manifest::Emulator;
use super::yaml;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// One addon that passed every check.
#[derive(Clone, Debug, PartialEq)]
pub struct Addon {
    pub folder: PathBuf,
    pub emulator: Emulator,
}

/// One addon folder that was not used, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    /// The folder's name under emulators/.
    pub folder: String,
    pub issues: Vec<Issue>,
}

/// What a scan found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scan {
    /// By folder name, in byte order.
    pub addons: Vec<Addon>,
    /// By folder name, in byte order.
    pub rejected: Vec<Rejected>,
}

/// The document's name in an addon folder.
pub const DOCUMENT: &str = "emulator.yaml";

/// Scan `<root>/emulators`, one folder per addon, in byte order of the folder names. Entries
/// whose names start with "." (the launcher's staging folder, editors' files) and entries that
/// are not folders are skipped. A folder that is a symbolic link, or whose document or resources
/// are links or lie outside it, is rejected, and so is every addon whose id is not its folder's
/// name or is also another addon's id. One rejected addon never stops the others.
pub fn scan(root: &Path, launcher: Version) -> Scan {
    let dir = root.join("emulators");
    let mut entries: Vec<(std::ffi::OsString, fs::FileType)> = match fs::read_dir(&dir) {
        Ok(read) => read.flatten().filter_map(|e| Some((e.file_name(), e.file_type().ok()?))).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Scan::default(),
        Err(e) => return Scan { addons: Vec::new(), rejected: vec![Rejected { folder: String::new(), issues: vec![Issue::new("", format!("the folder cannot be read: {e}"))] }] },
    };
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    // Each folder: its name, and its document or why it has none.
    let mut read: Vec<(String, Result<Emulator, Vec<Issue>>)> = Vec::new();
    for (name, kind) in entries {
        let name = name.to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if kind.is_symlink() {
            read.push((name, Err(vec![Issue::new("", "the folder is a symbolic link; an addon must be a real folder")])));
        } else if kind.is_dir() {
            let folder = dir.join(&name);
            let result = read_document(&folder).and_then(|text| document::parse(&text, launcher));
            read.push((name, result));
        }
    }

    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, result) in &read {
        if let Ok(e) = result {
            owners.entry(e.id.to_string()).or_default().push(name.clone());
        }
    }
    let mut scan = Scan::default();
    for (name, result) in read {
        let folder = dir.join(&name);
        let emulator = result.and_then(|e| {
            let mut issues = Vec::new();
            if e.id.as_str() != name {
                issues.push(Issue::new("id", format!("\"{}\" is not the folder's name, \"{name}\"", e.id)));
            }
            let others: Vec<&str> = owners[e.id.as_str()].iter().filter(|n| **n != name).map(String::as_str).collect();
            if !others.is_empty() {
                issues.push(Issue::new("id", format!("\"{}\" is also the id in the folder {}", e.id, others.join(", "))));
            }
            for (field, path) in [("icon", &e.icon), ("controls", &e.controls)] {
                if let Some(path) = path {
                    if let Err(message) = resource(&folder, path.as_str()) {
                        issues.push(Issue::new(field, message));
                    }
                }
            }
            if issues.is_empty() { Ok(e) } else { Err(issues) }
        });
        match emulator {
            Ok(emulator) => scan.addons.push(Addon { folder, emulator }),
            Err(issues) => scan.rejected.push(Rejected { folder: name, issues }),
        }
    }
    scan
}

/// The text of a folder's document, read without following a link.
fn read_document(folder: &Path) -> Result<String, Vec<Issue>> {
    let fail = |message: String| vec![Issue::new("", message)];
    let path = folder.join(DOCUMENT);
    let meta = match fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(fail(format!("there is no {DOCUMENT}"))),
        Err(e) => return Err(fail(format!("{DOCUMENT} cannot be read: {e}"))),
    };
    if meta.file_type().is_symlink() {
        return Err(fail(format!("{DOCUMENT} is a symbolic link; an addon's files must be in its folder")));
    }
    if !meta.is_file() {
        return Err(fail(format!("{DOCUMENT} is not a file")));
    }
    let limit = yaml::MAX_BYTES as u64;
    let too_big = |len: u64| fail(format!("{DOCUMENT} is {} KiB; the limit is {} KiB", len.div_ceil(1024), limit / 1024));
    if meta.len() > limit {
        return Err(too_big(meta.len()));
    }
    let mut bytes = Vec::new();
    let opened = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(&path);
    opened.and_then(|f| f.take(limit + 1).read_to_end(&mut bytes)).map_err(|e| fail(format!("{DOCUMENT} cannot be read: {e}")))?;
    if bytes.len() as u64 > limit {
        return Err(too_big(bytes.len() as u64));
    }
    String::from_utf8(bytes).map_err(|_| fail(format!("{DOCUMENT} is not UTF-8 text")))
}

/// Check that a resource is a regular file inside the folder, reached through no link.
/// `relative` is a checked `RelPath`: no leading "/", no "." or ".." parts.
fn resource(folder: &Path, relative: &str) -> Result<(), String> {
    let mut at = folder.to_path_buf();
    let parts: Vec<&str> = relative.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        at.push(part);
        let so_far = parts[..=i].join("/");
        let meta = fs::symlink_metadata(&at).map_err(|_| format!("{relative} is not in the addon's folder"))?;
        if meta.file_type().is_symlink() {
            return Err(format!("{so_far} is a symbolic link; an addon's files must be in its folder"));
        }
        if i + 1 == parts.len() && !meta.is_file() {
            return Err(format!("{relative} is not a file"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const LAB: &str = include_str!("../../testdata/addons/emulators/ps4-lab/emulator.yaml");

    fn root() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("addons-").tempdir().unwrap()
    }

    /// An addon folder `name` whose document is the test emulator with the id `id`.
    fn add(root: &Path, name: &str, id: &str) -> PathBuf {
        let folder = root.join("emulators").join(name);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("emulator.yaml"), LAB.replace("id: ps4-lab", &format!("id: {id}"))).unwrap();
        folder
    }

    fn found(s: &Scan) -> Vec<&str> {
        s.addons.iter().map(|a| a.emulator.id.as_str()).collect()
    }

    fn rejected(s: &Scan) -> Vec<(String, String)> {
        s.rejected.iter().map(|r| (r.folder.clone(), r.issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))).collect()
    }

    fn now() -> Version {
        Version::parse("1.14.3").unwrap()
    }

    #[test]
    fn no_folder_is_no_addons_and_no_problems() {
        let r = root();
        assert_eq!(scan(r.path(), now()), Scan::default());
        fs::create_dir(r.path().join("emulators")).unwrap();
        assert_eq!(scan(r.path(), now()), Scan::default());
    }

    #[test]
    fn addons_come_in_folder_name_order() {
        let r = root();
        for id in ["zeta", "alpha", "mid-1", "mid-0"] {
            add(r.path(), id, id);
        }
        let s = scan(r.path(), now());
        assert_eq!(found(&s), ["alpha", "mid-0", "mid-1", "zeta"]);
        assert_eq!(s.addons[0].folder, r.path().join("emulators/alpha"));
        assert!(s.rejected.is_empty());
    }

    #[test]
    fn a_broken_addon_is_rejected_alone_with_its_name_and_the_reason() {
        let r = root();
        add(r.path(), "good", "good");
        let bad = add(r.path(), "bad", "bad");
        fs::write(bad.join("emulator.yaml"), LAB.replace("id: ps4-lab", "id: bad").replace("enabled: true", "enabled: yes")).unwrap();
        let s = scan(r.path(), now());
        assert_eq!(found(&s), ["good"]);
        let rejected = rejected(&s);
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0, "bad");
        assert!(rejected[0].1.contains("enabled"), "{}", rejected[0].1);
    }

    #[test]
    fn the_id_must_be_the_folder_name() {
        let r = root();
        add(r.path(), "lab", "ps4-lab");
        assert_eq!(rejected(&scan(r.path(), now())), [("lab".to_string(), "id: \"ps4-lab\" is not the folder's name, \"lab\"".to_string())]);
    }

    #[test]
    fn two_addons_with_one_id_are_both_rejected() {
        let r = root();
        add(r.path(), "ps4-lab", "ps4-lab");
        add(r.path(), "ps4-lab-copy", "ps4-lab");
        add(r.path(), "other", "other");
        let s = scan(r.path(), now());
        assert_eq!(found(&s), ["other"], "scan order does not pick a winner");
        assert_eq!(
            rejected(&s),
            [
                ("ps4-lab".to_string(), "id: \"ps4-lab\" is also the id in the folder ps4-lab-copy".to_string()),
                ("ps4-lab-copy".to_string(), "id: \"ps4-lab\" is not the folder's name, \"ps4-lab-copy\"\nid: \"ps4-lab\" is also the id in the folder ps4-lab".to_string()),
            ]
        );
    }

    #[test]
    fn hidden_folders_files_and_special_files_are_skipped() {
        let r = root();
        add(r.path(), "ps4-lab", "ps4-lab");
        add(r.path(), ".staging", "ps4-lab");
        add(r.path(), ".proposals", "anything");
        fs::write(r.path().join("emulators/README.txt"), "notes").unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(r.path().join("emulators/socket")).unwrap();
        let s = scan(r.path(), now());
        assert_eq!(found(&s), ["ps4-lab"]);
        assert!(s.rejected.is_empty(), "{:?}", s.rejected);
    }

    #[test]
    fn a_linked_folder_or_document_is_rejected() {
        let r = root();
        let elsewhere = root();
        let target = add(elsewhere.path(), "linked", "linked");
        fs::create_dir_all(r.path().join("emulators")).unwrap();
        std::os::unix::fs::symlink(&target, r.path().join("emulators/linked")).unwrap();
        let doc = r.path().join("emulators/doc-link");
        fs::create_dir(&doc).unwrap();
        std::os::unix::fs::symlink(target.join("emulator.yaml"), doc.join("emulator.yaml")).unwrap();
        let s = scan(r.path(), now());
        assert!(s.addons.is_empty());
        assert_eq!(
            rejected(&s),
            [
                ("doc-link".to_string(), "emulator.yaml is a symbolic link; an addon's files must be in its folder".to_string()),
                ("linked".to_string(), "the folder is a symbolic link; an addon must be a real folder".to_string()),
            ]
        );
    }

    #[test]
    fn a_folder_without_a_readable_document_is_rejected() {
        let r = root();
        fs::create_dir_all(r.path().join("emulators/empty")).unwrap();
        let big = add(r.path(), "big", "big");
        fs::write(big.join("emulator.yaml"), vec![b'#'; 256 * 1024 + 1]).unwrap();
        let binary = add(r.path(), "binary", "binary");
        fs::write(binary.join("emulator.yaml"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(
            rejected(&scan(r.path(), now())),
            [
                ("big".to_string(), "emulator.yaml is 257 KiB; the limit is 256 KiB".to_string()),
                ("binary".to_string(), "emulator.yaml is not UTF-8 text".to_string()),
                ("empty".to_string(), "there is no emulator.yaml".to_string()),
            ]
        );
    }

    #[test]
    fn an_addon_for_a_newer_launcher_is_rejected() {
        let r = root();
        let folder = add(r.path(), "ps4-lab", "ps4-lab");
        fs::write(folder.join("emulator.yaml"), LAB.replace("schema_version: 1\n", "schema_version: 1\nmin_launcher_version: 1.15.0\n")).unwrap();
        assert_eq!(rejected(&scan(r.path(), now())), [("ps4-lab".to_string(), "min_launcher_version: the addon needs launcher 1.15.0 or newer; this is 1.14.3".to_string())]);
        assert_eq!(found(&scan(r.path(), Version::parse("1.15.0").unwrap())), ["ps4-lab"]);
    }

    fn with_resources(folder: &Path, icon: &str, controls: &str) {
        let id = folder.file_name().unwrap().to_str().unwrap();
        let text = LAB.replace("id: ps4-lab", &format!("id: {id}")).replace("enabled: true\n", &format!("enabled: true\nicon: {icon}\ncontrols: {controls}\n"));
        fs::write(folder.join("emulator.yaml"), text).unwrap();
    }

    #[test]
    fn resources_inside_the_folder_are_accepted() {
        let r = root();
        let folder = add(r.path(), "ps4-lab", "ps4-lab");
        fs::create_dir(folder.join("media")).unwrap();
        fs::write(folder.join("media/icon.svg"), "<svg/>").unwrap();
        fs::write(folder.join("controls.yaml"), "{}").unwrap();
        with_resources(&folder, "media/icon.svg", "controls.yaml");
        let s = scan(r.path(), now());
        assert_eq!(found(&s), ["ps4-lab"], "{:?}", s.rejected);
    }

    #[test]
    fn resources_must_be_files_inside_the_folder() {
        let r = root();
        let outside = root();
        fs::write(outside.path().join("icon.svg"), "<svg/>").unwrap();
        fs::create_dir(outside.path().join("media")).unwrap();
        fs::write(outside.path().join("media/icon.svg"), "<svg/>").unwrap();

        let missing = add(r.path(), "a-missing", "a-missing");
        with_resources(&missing, "icon.svg", "controls.yaml");
        let linked_file = add(r.path(), "b-linked-file", "b-linked-file");
        std::os::unix::fs::symlink(outside.path().join("icon.svg"), linked_file.join("icon.svg")).unwrap();
        fs::write(linked_file.join("controls.yaml"), "{}").unwrap();
        with_resources(&linked_file, "icon.svg", "controls.yaml");
        let linked_folder = add(r.path(), "c-linked-folder", "c-linked-folder");
        std::os::unix::fs::symlink(outside.path().join("media"), linked_folder.join("media")).unwrap();
        fs::write(linked_folder.join("controls.yaml"), "{}").unwrap();
        with_resources(&linked_folder, "media/icon.svg", "controls.yaml");
        let folder_not_file = add(r.path(), "d-folder", "d-folder");
        fs::create_dir(folder_not_file.join("icon.svg")).unwrap();
        fs::write(folder_not_file.join("controls.yaml"), "{}").unwrap();
        with_resources(&folder_not_file, "icon.svg", "controls.yaml");

        let s = scan(r.path(), now());
        assert!(s.addons.is_empty());
        assert_eq!(
            rejected(&s),
            [
                ("a-missing".to_string(), "icon: icon.svg is not in the addon's folder\ncontrols: controls.yaml is not in the addon's folder".to_string()),
                ("b-linked-file".to_string(), "icon: icon.svg is a symbolic link; an addon's files must be in its folder".to_string()),
                ("c-linked-folder".to_string(), "icon: media is a symbolic link; an addon's files must be in its folder".to_string()),
                ("d-folder".to_string(), "icon: icon.svg is not a file".to_string()),
            ]
        );
    }
}
