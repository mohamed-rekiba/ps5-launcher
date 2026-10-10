//! The default addons' lifecycle: copying the shipped defaults into the user's folder, keeping
//! a record of each copy, updating unchanged copies, and offering a newer default for a copy the
//! user changed (docs/plans/addons.md, "The advisor's fixes" 1 to 4).
//!
//! The files under the root (the launcher's data folder):
//!
//! - `emulators/<id>/`: the addons, the user's copies of the defaults among them.
//! - `emulators/.staging/`: copies being made. The scan skips it (its name starts with ".").
//! - `proposals/emulators/<id>/<revision>/`: a newer default for a copy the user changed. It is
//!   outside emulators/, so the scan never reads it as another addon.
//! - `addons-state.yaml`: the record of each shipped default (`Record`).
//! - `addons-journal.yaml`: the change in progress, so the next start can finish or undo it.
//! - `addons.lock`: one lock for every change.
//!
//! A default's revision is the digest of its shipped files, so it changes exactly when the
//! files change, with no number to maintain. The rules at each start:
//!
//! | Record | Folder | What happens |
//! |---|---|---|
//! | none (a new default) | missing | copied |
//! | none | present (the user's) | kept; the default is offered as a proposal |
//! | deleted | any | nothing: a deletion is never undone by itself |
//! | present | missing | recorded as deleted |
//! | same revision | present | nothing |
//! | other revision | unchanged (digest as recorded) | replaced by the new default |
//! | other revision | changed | kept; the new default is offered, once per revision |
//!
//! Every copy is a transaction: write the journal, stage the whole folder, check its digest,
//! publish it with a rename, write the state, remove the journal. The first failed step stops
//! the whole run, as a crash would; the next start reads the journal first.

use super::bundle::{DefaultSource, ShippedAddon};
use super::manifest::{EmulatorId, RelPath};
use super::safefs::{self, Dir, Kind, Refused};
use super::{yaml, Problem};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const STATE: &str = "addons-state.yaml";
const JOURNAL: &str = "addons-journal.yaml";
const LOCK: &str = "addons.lock";
const STATE_VERSION: u32 = 1;

// ------------------------------------------------------------------ digest


/// SHA-256, in lower-case hex, over every file sorted by its relative path ("/" between parts,
/// compared as bytes). For each file: the path's length as 8 bytes big-endian, the path, the
/// content's length as 8 bytes big-endian, the content. Folders count only through their files.
pub fn digest_files(files: &[(String, Vec<u8>)]) -> String {
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut h = Sha256::new();
    for (path, content) in sorted {
        add(&mut h, path.as_bytes(), content);
    }
    format!("{:x}", h.finalize())
}

fn add(h: &mut Sha256, path: &[u8], content: &[u8]) {
    h.update((path.len() as u64).to_be_bytes());
    h.update(path);
    h.update((content.len() as u64).to_be_bytes());
    h.update(content);
}

/// The digest of a folder: see `digest_files`. Links and special files are refused; nothing
/// below `path` is reached through a link.
pub fn digest_folder(path: &Path) -> io::Result<String> {
    digest_dir(&Dir::open(path).map_err(|e| safefs::named(path.as_os_str(), e))?)
}

/// The digest of the folder `parts` below the root, reached through no link.
fn digest_below(root: &Path, parts: &[&str]) -> io::Result<String> {
    digest_dir(&Dir::open_below(root, parts)?)
}

/// `digest_below` for a path under the root.
fn digest_at(root: &Path, path: &Path) -> io::Result<String> {
    let rel = path.strip_prefix(root).map_err(io::Error::other)?;
    let parts: Vec<&str> = rel.iter().map(|p| p.to_str().unwrap_or("")).collect();
    digest_below(root, &parts)
}

/// The most bytes in one addon folder the launcher digests.
pub const MAX_FOLDER_BYTES: u64 = 64 * 1024 * 1024;
/// The most files and folders in one addon folder.
pub const MAX_ENTRIES: usize = 4096;
/// The deepest folder in an addon folder.
pub const MAX_DEPTH: usize = 16;

fn digest_dir(dir: &Dir) -> io::Result<String> {
    let mut found = Vec::new();
    let mut budget = Budget { entries: 0, bytes: 0 };
    collect(dir, &[], 0, &mut budget, &mut found)?;
    found.sort();
    let mut h = Sha256::new();
    for (rel, content) in found {
        add(&mut h, &rel, &content);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// What a digest has used so far of its limits.
struct Budget {
    entries: usize,
    bytes: u64,
}

fn collect(dir: &Dir, prefix: &[u8], depth: usize, budget: &mut Budget, found: &mut Vec<(Vec<u8>, Vec<u8>)>) -> io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(io::Error::other(format!("the folder is more than {MAX_DEPTH} levels deep")));
    }
    for (name, kind) in dir.entries()? {
        budget.entries += 1;
        if budget.entries > MAX_ENTRIES {
            return Err(io::Error::other(format!("the folder holds more than {MAX_ENTRIES} files and folders")));
        }
        let rel = if prefix.is_empty() { name.as_bytes().to_vec() } else { [prefix, b"/", name.as_bytes()].concat() };
        let shown = String::from_utf8_lossy(&rel).into_owned();
        match kind {
            Kind::Dir => collect(&dir.dir(&name).map_err(|e| safefs::named(&shown, e))?, &rel, depth + 1, budget, found)?,
            Kind::File => {
                // The size is known before a byte is read: fstat in `file`.
                let file = dir.file(&name, safefs::MAX_FILE).map_err(|e| safefs::named(&shown, e))?;
                let len = file.metadata()?.len();
                budget.bytes += len;
                if budget.bytes > MAX_FOLDER_BYTES {
                    return Err(io::Error::other(format!("the folder holds more than {} KiB", MAX_FOLDER_BYTES / 1024)));
                }
                let mut content = Vec::new();
                file.take(len.saturating_add(1)).read_to_end(&mut content)?;
                if content.len() as u64 > len {
                    return Err(io::Error::other(format!("{shown} grew while it was read")));
                }
                found.push((rel, content));
            }
            Kind::Link => return Err(safefs::named(&shown, Refused::Link)),
            Kind::Other => return Err(io::Error::other(format!("{shown} is not a file or a folder"))),
        }
    }
    Ok(())
}


// ------------------------------------------------------------------ file operations

/// The file changes the lifecycle makes, one primitive each, so a test can stop it at any step
/// or change the disk between two steps. Nothing here flushes by itself: the lifecycle calls
/// `sync_dir` where the order of changes must survive a power loss.
pub trait FileOps {
    /// Make one folder; its parent exists.
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    /// Write a file that must not exist yet, without following a link, and flush it.
    fn write_new(&self, path: &Path, data: &[u8]) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Flush a folder's entries (new, renamed and removed names) to the disk.
    fn sync_dir(&self, path: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// Remove a folder and everything in it, without following links.
    fn remove_dir_all(&self, path: &Path) -> io::Result<()>;
    /// Take the lock file's exclusive lock; None when another process holds it.
    fn lock(&self, path: &Path) -> io::Result<Option<Lock>>;
}

/// An exclusive lock (flock), held until it is dropped.
pub struct Lock {
    _file: File,
}

/// The real file system.
pub struct RealFiles {
    /// How long `lock` waits for another process.
    pub lock_wait: Duration,
    /// Flush files and folders to the disk. Only tests turn it off, for speed.
    pub durable: bool,
}

impl Default for RealFiles {
    fn default() -> Self {
        RealFiles { lock_wait: Duration::from_secs(5), durable: true }
    }
}

impl FileOps for RealFiles {
    fn create_dir(&self, path: &Path) -> io::Result<()> {
        fs::create_dir(path)
    }

    fn write_new(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o644).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?;
        f.write_all(data)?;
        if self.durable { f.sync_all() } else { Ok(()) }
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        if !self.durable {
            return Ok(());
        }
        fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?.sync_all()
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        // std removes a link itself, never what it points to.
        fs::remove_dir_all(path)
    }

    fn lock(&self, path: &Path) -> io::Result<Option<Lock>> {
        let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o644).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?;
        let start = Instant::now();
        loop {
            // SAFETY: flock only reads the descriptor, which `file` keeps open.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Some(Lock { _file: file }));
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::WouldBlock {
                return Err(e);
            }
            if start.elapsed() >= self.lock_wait {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

// ------------------------------------------------------------------ the state file

/// What addons-state.yaml records for one shipped addon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    /// The default's revision the copy came from: the digest of the shipped files.
    pub revision: String,
    /// The copy's digest as the launcher left it. Another digest means the user changed it.
    pub digest: String,
    /// The user deleted the copy; the launcher never brings it back by itself.
    pub deleted: bool,
    /// The revision last offered as a proposal, so the same offer is not repeated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offered: Option<String>,
}

impl Record {
    fn clean(revision: &str) -> Record {
        Record { revision: revision.into(), digest: revision.into(), deleted: false, offered: None }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    state_version: u32,
    #[serde(default)]
    emulators: BTreeMap<String, Record>,
}

/// The records in `<root>/addons-state.yaml`: None when the file is missing, Err when it cannot
/// be read.
pub fn records(root: &Path) -> Result<Option<BTreeMap<String, Record>>, String> {
    let Some(text) = read_own(root, STATE)? else { return Ok(None) };
    yaml::load(&text).map_err(|e| e.to_string())?;
    let state: StateFile = yaml::typed(&text).map_err(|e| e.to_string())?;
    if state.state_version != STATE_VERSION {
        return Err(format!("state_version {} is not {STATE_VERSION}", state.state_version));
    }
    Ok(Some(state.emulators))
}

/// One of the launcher's own text files in the root, read without following a link and within
/// the YAML size limit: None when it is missing.
fn read_own(root: &Path, name: &str) -> Result<Option<String>, String> {
    let read = Dir::open(root).and_then(|dir| dir.read(name.as_ref(), yaml::MAX_BYTES as u64));
    match read {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| format!("{name} is not UTF-8 text")),
        Err(Refused::Missing) => Ok(None),
        Err(e) => Err(format!("{name} {e}")),
    }
}

// ------------------------------------------------------------------ the journal

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    /// A default copied into a missing folder.
    Copy,
    /// A folder replaced by its default.
    Replace,
    /// A default written as a proposal.
    Offer,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    step: Step,
    /// Checked as an `EmulatorId` when read: it names a folder.
    id: String,
    revision: String,
}

// ------------------------------------------------------------------ the public actions

/// Where a newer default for an edited copy is put: outside emulators/, so the scan never reads it.
pub fn proposal_folder(root: &Path, id: &str, revision: &str) -> PathBuf {
    root.join("proposals").join("emulators").join(id).join(&revision[..16.min(revision.len())])
}

/// Bring the user's copies of the default emulator addons up to date. Called at every start.
pub fn reconcile(root: &Path, source: &dyn DefaultSource, files: &dyn FileOps) -> Vec<Problem> {
    locked(root, files, |run| {
        run.recover()?;
        match records(root) {
            Ok(Some(state)) => run.state = state,
            Ok(None) if !present(&root.join("emulators")) => run.save()?,
            Ok(None) => return run.stop(Problem::new(STATE, "the file is missing, so the launcher left the addon folders as they are. Restore missing defaults to record them again")),
            Err(e) => return run.stop(Problem::new(STATE, format!("the file cannot be read, so the launcher left the addon folders as they are. Restore missing defaults to record them again ({e})"))),
        }
        for addon in source.emulators() {
            run.update(&addon)?;
        }
        Ok(())
    })
}

/// Copy every default whose folder is missing, the deleted ones too ("Restore missing
/// defaults"). Rebuilds a missing or broken state file. Existing folders stay as they are; one
/// without a record is recorded as a copy of the current default, so a change in it is kept.
pub fn restore_missing_defaults(root: &Path, source: &dyn DefaultSource, files: &dyn FileOps) -> Vec<Problem> {
    locked(root, files, |run| {
        run.recover()?;
        run.state = records(root).ok().flatten().unwrap_or_default();
        for addon in source.emulators() {
            let revision = digest_files(&addon.files);
            if !present(&run.target(&addon.id)) {
                run.copy(&addon, &revision)?;
            } else if !run.state.contains_key(&addon.id) {
                run.state.insert(addon.id.clone(), Record::clean(&revision));
            }
        }
        run.save()
    })
}

/// Replace one addon's folder with its shipped default, edits included ("Reset to default").
pub fn reset_to_default(root: &Path, source: &dyn DefaultSource, files: &dyn FileOps, id: &str) -> Vec<Problem> {
    locked(root, files, |run| {
        let subject = format!("emulators/{id}");
        let Some(addon) = source.emulators().into_iter().find(|a| a.id == id) else {
            return run.stop(Problem::new(subject, "the launcher has no default for this addon"));
        };
        run.recover()?;
        match records(root) {
            Ok(Some(state)) => run.state = state,
            _ => return run.stop(Problem::new(STATE, "the file is missing or cannot be read. Restore missing defaults first")),
        }
        let revision = digest_files(&addon.files);
        if present(&run.target(id)) { run.replace(&addon, &revision, None).map(|_| ()) } else { run.copy(&addon, &revision) }
    })
}

// ------------------------------------------------------------------ one run

/// Why a run stopped early; the problem is already in the run's list.
struct Stopped;

/// One action under the lock: the state as read, and the problems so far.
struct Run<'a> {
    root: &'a Path,
    files: &'a dyn FileOps,
    state: BTreeMap<String, Record>,
    problems: Vec<Problem>,
}

/// Take the lock, run `action`, and return its problems.
fn locked(root: &Path, files: &dyn FileOps, action: impl FnOnce(&mut Run) -> Result<(), Stopped>) -> Vec<Problem> {
    if let Err(e) = fs::create_dir_all(root) {
        return vec![Problem::new("addons", format!("the folder cannot be made: {e}"))];
    }
    let _lock = match files.lock(&root.join(LOCK)) {
        Ok(Some(lock)) => lock,
        Ok(None) => return vec![Problem::new("addons", "another launcher is changing the addons; the defaults were not checked this time")],
        Err(e) => return vec![Problem::new("addons", format!("the lock file cannot be used: {e}"))],
    };
    let mut run = Run { root, files, state: BTreeMap::new(), problems: Vec::new() };
    let _ = (|| {
        run.tidy()?;
        run.no_links()?;
        action(&mut run)
    })();
    run.problems
}

fn present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// A path under the root as the user sees it, for messages.
fn shown(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
}

fn parent(path: &Path) -> &Path {
    path.parent().expect("every path here is under the root")
}

/// The temporary file `write_atomic` writes first.
fn temporary(root: &Path, name: &str) -> PathBuf {
    root.join(format!(".{name}.tmp"))
}

impl Run<'_> {
    fn stop(&mut self, problem: Problem) -> Result<(), Stopped> {
        self.problems.push(problem);
        Err(Stopped)
    }

    /// A failed file change stops the run, as a crash would.
    fn check<T>(&mut self, subject: &str, what: &str, result: io::Result<T>) -> Result<T, Stopped> {
        result.map_err(|e| {
            self.problems.push(Problem::new(subject, format!("{what}: {e}")));
            Stopped
        })
    }

    fn target(&self, id: &str) -> PathBuf {
        self.root.join("emulators").join(id)
    }

    fn staging(&self, name: &str) -> PathBuf {
        self.root.join("emulators").join(".staging").join(name)
    }

    /// Remove the temporary files an interrupted write left.
    fn tidy(&mut self) -> Result<(), Stopped> {
        for name in [STATE, JOURNAL] {
            self.remove(&temporary(self.root, name))?;
        }
        Ok(())
    }

    /// Refuse to start when one of the launcher's folders is a link: every change goes below
    /// them, and a link would send it out of the data folder.
    fn no_links(&mut self) -> Result<(), Stopped> {
        for name in ["emulators", "emulators/.staging", "proposals", "proposals/emulators", "kept", "kept/emulators"] {
            if fs::symlink_metadata(self.root.join(name)).is_ok_and(|m| m.file_type().is_symlink()) {
                return self.stop(Problem::new(name, "is a symbolic link; the launcher does not follow links in its folders"));
            }
        }
        Ok(())
    }

    /// Check that no folder between the root and `path` is a link, before a change there.
    /// What is missing is fine: the change itself then fails.
    fn real_parents(&mut self, path: &Path) -> Result<(), Stopped> {
        let rel = path.strip_prefix(self.root).unwrap_or(path);
        let mut at = self.root.to_path_buf();
        let parts: Vec<_> = rel.iter().collect();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            at.push(part);
            match fs::symlink_metadata(&at) {
                Ok(m) if m.file_type().is_symlink() => return self.stop(Problem::new(shown(self.root, &at), "is a symbolic link; the launcher does not follow links in its folders")),
                Ok(m) if !m.is_dir() => return self.stop(Problem::new(shown(self.root, &at), "is not a folder")),
                _ => {}
            }
        }
        Ok(())
    }

    /// The folder under the root, made where it is missing; never through a link.
    fn ensure(&mut self, parts: &[&str]) -> Result<PathBuf, Stopped> {
        let mut at = self.root.to_path_buf();
        for part in parts {
            at.push(part);
            match fs::symlink_metadata(&at) {
                Ok(m) if m.file_type().is_symlink() => return self.stop(Problem::new(shown(self.root, &at), "is a symbolic link; the launcher does not follow links in its folders")).map(|()| at),
                Ok(m) if !m.is_dir() => return self.stop(Problem::new(shown(self.root, &at), "is not a folder")).map(|()| at),
                Ok(_) => {}
                Err(_) => self.mkdir(&at)?,
            }
        }
        Ok(at)
    }

    /// Make one folder and flush its name in the parent.
    fn mkdir(&mut self, path: &Path) -> Result<(), Stopped> {
        self.real_parents(path)?;
        let subject = shown(self.root, path);
        let result = self.files.create_dir(path).and_then(|()| self.files.sync_dir(parent(path)));
        self.check(&subject, "the folder cannot be made", result)
    }

    /// Rename, then flush both folders' entries.
    fn rename(&mut self, from: &Path, to: &Path, subject: &str, what: &str) -> Result<(), Stopped> {
        self.real_parents(from)?;
        self.real_parents(to)?;
        let mut result = self.files.rename(from, to).and_then(|()| self.files.sync_dir(parent(to)));
        if parent(from) != parent(to) {
            result = result.and_then(|()| self.files.sync_dir(parent(from)));
        }
        self.check(subject, what, result)
    }

    /// Remove a file or a folder (a link itself, not what it points to), and flush the parent.
    fn remove(&mut self, path: &Path) -> Result<(), Stopped> {
        self.real_parents(path)?;
        let result = match fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() => self.files.remove_dir_all(path).and_then(|()| self.files.sync_dir(parent(path))),
            Ok(_) => self.files.remove_file(path).and_then(|()| self.files.sync_dir(parent(path))),
            Err(_) => Ok(()),
        };
        let subject = shown(self.root, path);
        self.check(&subject, "cannot be removed", result)
    }

    /// Replace a file under the root in one step: a temporary file, a rename, a flush.
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> Result<(), Stopped> {
        let tmp = temporary(self.root, name);
        self.remove(&tmp)?;
        let result = self.files.write_new(&tmp, data);
        self.check(name, "the file cannot be written", result)?;
        self.rename(&tmp, &self.root.join(name), name, "the file cannot be written")
    }

    fn save(&mut self) -> Result<(), Stopped> {
        let file = StateFile { state_version: STATE_VERSION, emulators: self.state.clone() };
        let text = format!(
            "# Written by the launcher: the default each addon folder came from. Do not edit.\n{}",
            serde_norway::to_string(&file).expect("the state is plain data")
        );
        self.write_atomic(STATE, text.as_bytes())
    }

    fn begin(&mut self, step: Step, id: &str, revision: &str) -> Result<(), Stopped> {
        let id = match EmulatorId::try_from(id.to_string()) {
            Ok(id) => id,
            Err(e) => return self.stop(Problem::new(format!("emulators/{id}"), e)),
        };
        let journal = Journal { step, id: id.to_string(), revision: revision.into() };
        let text = serde_norway::to_string(&journal).expect("the journal is plain data");
        self.write_atomic(JOURNAL, text.as_bytes())
    }

    fn end(&mut self) -> Result<(), Stopped> {
        self.remove(&self.root.join(JOURNAL))
    }

    /// Write the addon's files into a fresh staging folder, flush every file and folder, and
    /// check the copy's digest.
    fn stage(&mut self, name: &str, addon: &ShippedAddon, revision: &str) -> Result<PathBuf, Stopped> {
        let staging = self.ensure(&["emulators", ".staging"])?;
        let dir = staging.join(name);
        self.remove(&dir)?;
        let subject = format!("emulators/{}", addon.id);
        let result = (|| {
            self.files.create_dir(&dir)?;
            let mut made = vec![dir.clone()];
            for (path, content) in &addon.files {
                RelPath::try_from(path.clone()).map_err(io::Error::other)?;
                let mut at = dir.clone();
                let parts: Vec<&str> = path.split('/').collect();
                for part in &parts[..parts.len() - 1] {
                    at.push(part);
                    if !made.contains(&at) {
                        self.files.create_dir(&at)?;
                        made.push(at.clone());
                    }
                }
                self.files.write_new(&dir.join(path), content)?;
            }
            // Deepest first, then the staging folder that holds the new name.
            made.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
            for folder in made.iter().chain([&staging]) {
                self.files.sync_dir(folder)?;
            }
            if digest_below(self.root, &["emulators", ".staging", name])? != revision {
                return Err(io::Error::other("the copy does not match the default"));
            }
            Ok(())
        })();
        self.check(&subject, "the default cannot be copied", result)?;
        Ok(dir)
    }

    /// Copy a default into its missing folder.
    fn copy(&mut self, addon: &ShippedAddon, revision: &str) -> Result<(), Stopped> {
        let subject = format!("emulators/{}", addon.id);
        self.begin(Step::Copy, &addon.id, revision)?;
        let staged = self.stage(&addon.id, addon, revision)?;
        self.rename(&staged, &self.target(&addon.id), &subject, "the default cannot be put in place")?;
        self.state.insert(addon.id.clone(), Record::clean(revision));
        self.save()?;
        self.end()
    }

    /// Replace an addon's folder with its default. With `unchanged`, the copy moved aside must
    /// still have that digest: if the user changed it meanwhile, it goes back in place, nothing
    /// is replaced, and the result is false.
    fn replace(&mut self, addon: &ShippedAddon, revision: &str, unchanged: Option<&str>) -> Result<bool, Stopped> {
        let subject = format!("emulators/{}", addon.id);
        self.begin(Step::Replace, &addon.id, revision)?;
        let staged = self.stage(&addon.id, addon, revision)?;
        let old = self.staging(&format!("{}.old", addon.id));
        self.remove(&old)?;
        let target = self.target(&addon.id);
        self.rename(&target, &old, &subject, "the old copy cannot be moved aside")?;
        if let Some(expected) = unchanged {
            if digest_below(self.root, &["emulators", ".staging", &format!("{}.old", addon.id)]).ok().as_deref() != Some(expected) {
                self.rename(&old, &target, &subject, "the changed copy cannot be put back")?;
                self.remove(&staged)?;
                self.end()?;
                return Ok(false);
            }
        }
        self.rename(&staged, &target, &subject, "the default cannot be put in place")?;
        self.state.insert(addon.id.clone(), Record::clean(revision));
        self.save()?;
        self.remove(&old)?;
        self.end()?;
        Ok(true)
    }

    /// Write a default as a proposal next to the user's copy, and record the offer.
    fn offer(&mut self, addon: &ShippedAddon, revision: &str, record: Record) -> Result<PathBuf, Stopped> {
        let subject = format!("emulators/{}", addon.id);
        self.begin(Step::Offer, &addon.id, revision)?;
        let staged = self.stage(&format!("{}.offer", addon.id), addon, revision)?;
        let dest = proposal_folder(self.root, &addon.id, revision);
        self.ensure(&["proposals", "emulators", &addon.id])?;
        self.remove(&dest)?;
        self.rename(&staged, &dest, &subject, "the new default cannot be offered")?;
        self.state.insert(addon.id.clone(), Record { offered: Some(revision.into()), ..record });
        self.save()?;
        self.end()?;
        Ok(dest)
    }

    /// Apply the rules in the module's table to one shipped default.
    fn update(&mut self, addon: &ShippedAddon) -> Result<(), Stopped> {
        let id = addon.id.as_str();
        let subject = format!("emulators/{id}");
        let revision = digest_files(&addon.files);
        let target = self.target(id);
        let Some(record) = self.state.get(id).cloned() else {
            if !present(&target) {
                return self.copy(addon, &revision);
            }
            if digest_below(self.root, &["emulators", id]).ok().as_deref() == Some(revision.as_str()) {
                self.state.insert(id.to_string(), Record::clean(&revision));
                return self.save();
            }
            let dest = self.offer(addon, &revision, Record::clean(&revision))?;
            let message = format!("this folder was already there, so the launcher kept it; the default is in {}", shown(self.root, &dest));
            self.problems.push(Problem::new(subject, message));
            return Ok(());
        };
        if record.deleted {
            return Ok(());
        }
        if !present(&target) {
            self.state.insert(id.to_string(), Record { deleted: true, ..record });
            return self.save();
        }
        if record.revision == revision {
            return Ok(());
        }
        match digest_below(self.root, &["emulators", id]) {
            Ok(d) if d == revision => {
                self.state.insert(id.to_string(), Record::clean(&revision));
                self.save()
            }
            Ok(d) if d == record.digest => {
                if self.replace(addon, &revision, Some(&record.digest))? {
                    return Ok(());
                }
                // The user changed it while the launcher was replacing it.
                self.offer_changed(addon, &revision, record)
            }
            _ => self.offer_changed(addon, &revision, record),
        }
    }

    /// Keep a copy the user changed and offer the new default, once per revision.
    fn offer_changed(&mut self, addon: &ShippedAddon, revision: &str, record: Record) -> Result<(), Stopped> {
        if record.offered.as_deref() == Some(revision) {
            return Ok(());
        }
        let dest = self.offer(addon, revision, record)?;
        let message = format!("you changed this addon, so the launcher kept it; its new default is in {}", shown(self.root, &dest));
        self.problems.push(Problem::new(format!("emulators/{}", addon.id), message));
        Ok(())
    }

    /// Finish or undo the change an earlier run left in the journal. A change is finished only
    /// when its copy is complete and in place; otherwise it is undone. With no state file, it
    /// is only undone: a record is never made up from nothing.
    fn recover(&mut self) -> Result<(), Stopped> {
        let text = match read_own(self.root, JOURNAL) {
            Ok(Some(t)) => t,
            Ok(None) => return Ok(()),
            Err(e) => return self.stop(Problem::new(JOURNAL, format!("the file cannot be read, so the launcher left the addon folders as they are ({e})"))),
        };
        let journal: Journal = match yaml::load(&text).and_then(|_| yaml::typed(&text)) {
            Ok(j) => j,
            Err(e) => return self.stop(Problem::new(JOURNAL, format!("the file cannot be read, so the launcher left the addon folders as they are ({e})"))),
        };
        let revision_ok = journal.revision.len() == 64 && journal.revision.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let id = match EmulatorId::try_from(journal.id.clone()) {
            Ok(id) if revision_ok => id.to_string(),
            _ => return self.stop(Problem::new(JOURNAL, "the file names no valid change, so the launcher left the addon folders as they are")),
        };
        let mut state = match records(self.root) {
            Ok(state) => state,
            Err(e) => return self.stop(Problem::new(STATE, format!("the file cannot be read, so the launcher left the addon folders as they are ({e})"))),
        };
        let root = self.root;
        let complete = |path: &Path| digest_at(root, path).ok().as_deref() == Some(journal.revision.as_str());
        let target = self.target(&id);
        let subject = format!("emulators/{id}");
        match journal.step {
            Step::Copy | Step::Replace => {
                let old = self.staging(&format!("{id}.old"));
                if complete(&target) {
                    if let Some(s) = state.as_mut() {
                        s.insert(id.clone(), Record::clean(&journal.revision));
                    }
                } else if !present(&target) && present(&old) {
                    self.rename(&old, &target, &subject, "the old copy cannot be put back")?;
                } else if present(&old) {
                    // Another folder took the name meanwhile: keep both, and say where.
                    let kept = self.ensure(&["kept", "emulators", &id])?;
                    let free = (1..).map(|n| kept.join(n.to_string())).find(|p| !present(p)).expect("a free number");
                    self.rename(&old, &free, &subject, "the earlier copy cannot be kept")?;
                    let message = format!("an interrupted update found another folder here; the launcher kept both, and the earlier copy is in {}", shown(self.root, &free));
                    self.problems.push(Problem::new(subject.clone(), message));
                }
                self.remove(&old)?;
                self.remove(&self.staging(&id))?;
            }
            Step::Offer => {
                if complete(&proposal_folder(self.root, &id, &journal.revision)) {
                    if let Some(record) = state.as_mut().and_then(|s| s.get_mut(&id)) {
                        record.offered = Some(journal.revision.clone());
                    } else if let Some(s) = state.as_mut() {
                        s.insert(id.clone(), Record { offered: Some(journal.revision.clone()), ..Record::clean(&journal.revision) });
                    }
                }
                self.remove(&self.staging(&format!("{id}.offer")))?;
            }
        }
        if let Some(s) = state {
            self.state = s;
            self.save()?;
        }
        self.end()
    }
}
