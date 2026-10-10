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
use super::{yaml, Problem};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const STATE: &str = "addons-state.yaml";
const JOURNAL: &str = "addons-journal.yaml";
const LOCK: &str = "addons.lock";
const STATE_VERSION: u32 = 1;

// ------------------------------------------------------------------ digest

/// The digest of a folder: see `digest_files`. Links and special files are refused.
pub fn digest_folder(path: &Path) -> io::Result<String> {
    let mut found = Vec::new();
    collect(path, &[], &mut found)?;
    found.sort();
    let mut h = Sha256::new();
    for (rel, file) in found {
        add(&mut h, &rel, &fs::read(file)?);
    }
    Ok(format!("{:x}", h.finalize()))
}

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

fn collect(dir: &Path, prefix: &[u8], found: &mut Vec<(Vec<u8>, PathBuf)>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let rel = if prefix.is_empty() { name.as_bytes().to_vec() } else { [prefix, b"/", name.as_bytes()].concat() };
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(io::Error::other(format!("{} is a symbolic link", String::from_utf8_lossy(&rel))));
        } else if kind.is_dir() {
            collect(&entry.path(), &rel, found)?;
        } else if kind.is_file() {
            found.push((rel, entry.path()));
        } else {
            return Err(io::Error::other(format!("{} is not a file or a folder", String::from_utf8_lossy(&rel))));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ file operations

/// The file changes the lifecycle makes, so a test can stop it at any step.
pub trait FileOps {
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    /// Write a new file (in a staging folder).
    fn write(&self, path: &Path, data: &[u8]) -> io::Result<()>;
    /// Replace a file in one step: readers see the old content or the new, never a part.
    fn write_atomic(&self, path: &Path, data: &[u8]) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    fn remove_dir_all(&self, path: &Path) -> io::Result<()>;
    /// Take the lock file's exclusive lock; None when another process holds it.
    fn lock(&self, path: &Path) -> io::Result<Option<Lock>>;
}

/// An exclusive lock (flock), held until it is dropped.
pub struct Lock {
    _file: File,
}

/// The real file system. Writes and renames are flushed to the disk before they count.
pub struct RealFiles {
    /// How long `lock` waits for another process.
    pub lock_wait: Duration,
}

impl Default for RealFiles {
    fn default() -> Self {
        RealFiles { lock_wait: Duration::from_secs(5) }
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) => File::open(dir)?.sync_all(),
        None => Ok(()),
    }
}

impl FileOps for RealFiles {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    fn write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        let mut f = File::create(path)?;
        f.write_all(data)?;
        f.sync_all()
    }

    fn write_atomic(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = path.with_file_name(format!(".{name}.tmp"));
        self.write(&tmp, data)?;
        fs::rename(&tmp, path)?;
        sync_parent(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)?;
        sync_parent(to)?;
        sync_parent(from)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::remove_dir_all(path)
    }

    fn lock(&self, path: &Path) -> io::Result<Option<Lock>> {
        let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
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
    let text = match fs::read_to_string(root.join(STATE)) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    yaml::load(&text).map_err(|e| e.to_string())?;
    let state: StateFile = yaml::typed(&text).map_err(|e| e.to_string())?;
    if state.state_version != STATE_VERSION {
        return Err(format!("state_version {} is not {STATE_VERSION}", state.state_version));
    }
    Ok(Some(state.emulators))
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
            Ok(None) if !root.join("emulators").exists() => run.save()?,
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
        if present(&run.target(id)) { run.replace(&addon, &revision) } else { run.copy(&addon, &revision) }
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
    if let Err(e) = files.create_dir_all(root) {
        return vec![Problem::new("addons", format!("the folder cannot be made: {e}"))];
    }
    let _lock = match files.lock(&root.join(LOCK)) {
        Ok(Some(lock)) => lock,
        Ok(None) => return vec![Problem::new("addons", "another launcher is changing the addons; the defaults were not checked this time")],
        Err(e) => return vec![Problem::new("addons", format!("the lock file cannot be used: {e}"))],
    };
    let mut run = Run { root, files, state: BTreeMap::new(), problems: Vec::new() };
    let _ = action(&mut run);
    run.problems
}

fn present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// A path under the root as the user sees it, for messages.
fn shown(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
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

    fn save(&mut self) -> Result<(), Stopped> {
        let file = StateFile { state_version: STATE_VERSION, emulators: self.state.clone() };
        let text = format!(
            "# Written by the launcher: the default each addon folder came from. Do not edit.\n{}",
            serde_norway::to_string(&file).expect("the state is plain data")
        );
        let result = self.files.write_atomic(&self.root.join(STATE), text.as_bytes());
        self.check(STATE, "the file cannot be written", result)
    }

    fn remove(&mut self, path: &Path) -> Result<(), Stopped> {
        let result = match fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() => self.files.remove_dir_all(path),
            Ok(_) => self.files.remove_file(path),
            Err(_) => Ok(()),
        };
        let subject = shown(self.root, path);
        self.check(&subject, "cannot be removed", result)
    }

    fn begin(&mut self, step: Step, id: &str, revision: &str) -> Result<(), Stopped> {
        let id = match EmulatorId::try_from(id.to_string()) {
            Ok(id) => id,
            Err(e) => return self.stop(Problem::new(format!("emulators/{id}"), e)),
        };
        let journal = Journal { step, id: id.to_string(), revision: revision.into() };
        let text = serde_norway::to_string(&journal).expect("the journal is plain data");
        let result = self.files.write_atomic(&self.root.join(JOURNAL), text.as_bytes());
        self.check(JOURNAL, "the file cannot be written", result)
    }

    fn end(&mut self) -> Result<(), Stopped> {
        let result = self.files.remove_file(&self.root.join(JOURNAL));
        self.check(JOURNAL, "the file cannot be removed", result)
    }

    /// Write the addon's files into a fresh staging folder and check the copy's digest.
    fn stage(&mut self, name: &str, addon: &ShippedAddon, revision: &str) -> Result<PathBuf, Stopped> {
        let dir = self.staging(name);
        self.remove(&dir)?;
        let subject = format!("emulators/{}", addon.id);
        let result = (|| {
            self.files.create_dir_all(&dir)?;
            for (path, content) in &addon.files {
                RelPath::try_from(path.clone()).map_err(io::Error::other)?;
                let file = dir.join(path);
                if let Some(parent) = file.parent() {
                    self.files.create_dir_all(parent)?;
                }
                self.files.write(&file, content)?;
            }
            if digest_folder(&dir)? != revision {
                return Err(io::Error::other("the copy does not match the default"));
            }
            Ok(())
        })();
        self.check(&subject, "the default cannot be copied", result)?;
        Ok(dir)
    }

    /// Copy a default into its missing folder.
    fn copy(&mut self, addon: &ShippedAddon, revision: &str) -> Result<(), Stopped> {
        self.begin(Step::Copy, &addon.id, revision)?;
        let staged = self.stage(&addon.id, addon, revision)?;
        let result = self.files.rename(&staged, &self.target(&addon.id));
        self.check(&format!("emulators/{}", addon.id), "the default cannot be put in place", result)?;
        self.state.insert(addon.id.clone(), Record::clean(revision));
        self.save()?;
        self.end()
    }

    /// Replace an addon's folder with its default.
    fn replace(&mut self, addon: &ShippedAddon, revision: &str) -> Result<(), Stopped> {
        let subject = format!("emulators/{}", addon.id);
        self.begin(Step::Replace, &addon.id, revision)?;
        let staged = self.stage(&addon.id, addon, revision)?;
        let old = self.staging(&format!("{}.old", addon.id));
        self.remove(&old)?;
        let target = self.target(&addon.id);
        let result = self.files.rename(&target, &old);
        self.check(&subject, "the old copy cannot be moved aside", result)?;
        let result = self.files.rename(&staged, &target);
        self.check(&subject, "the default cannot be put in place", result)?;
        self.state.insert(addon.id.clone(), Record::clean(revision));
        self.save()?;
        self.remove(&old)?;
        self.end()
    }

    /// Write a default as a proposal next to the user's copy, and record the offer.
    fn offer(&mut self, addon: &ShippedAddon, revision: &str, record: Record) -> Result<PathBuf, Stopped> {
        let subject = format!("emulators/{}", addon.id);
        self.begin(Step::Offer, &addon.id, revision)?;
        let staged = self.stage(&format!("{}.offer", addon.id), addon, revision)?;
        let dest = proposal_folder(self.root, &addon.id, revision);
        self.remove(&dest)?;
        let parent = dest.parent().expect("a proposal folder has a parent").to_path_buf();
        let result = self.files.create_dir_all(&parent).and_then(|()| self.files.rename(&staged, &dest));
        self.check(&subject, "the new default cannot be offered", result)?;
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
            if digest_folder(&target).ok().as_deref() == Some(revision.as_str()) {
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
        match digest_folder(&target) {
            Ok(d) if d == revision => {
                self.state.insert(id.to_string(), Record::clean(&revision));
                self.save()
            }
            Ok(d) if d == record.digest => self.replace(addon, &revision),
            _ if record.offered.as_deref() == Some(revision.as_str()) => Ok(()),
            _ => {
                let dest = self.offer(addon, &revision, record)?;
                let message = format!("you changed this addon, so the launcher kept it; its new default is in {}", shown(self.root, &dest));
                self.problems.push(Problem::new(subject, message));
                Ok(())
            }
        }
    }

    /// Finish or undo the change an earlier run left in the journal. A change is finished only
    /// when its copy is complete and in place; otherwise it is undone. With no state file, it
    /// is only undone: a record is never made up from nothing.
    fn recover(&mut self) -> Result<(), Stopped> {
        let path = self.root.join(JOURNAL);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return self.stop(Problem::new(JOURNAL, format!("the file cannot be read, so the launcher left the addon folders as they are ({e})"))),
        };
        let journal: Journal = match yaml::load(&text).and_then(|_| yaml::typed(&text)) {
            Ok(j) => j,
            Err(e) => return self.stop(Problem::new(JOURNAL, format!("the file cannot be read, so the launcher left the addon folders as they are ({e})"))),
        };
        let Ok(id) = EmulatorId::try_from(journal.id.clone()).map(|id| id.to_string()) else {
            return self.stop(Problem::new(JOURNAL, "the file names no valid addon, so the launcher left the addon folders as they are"));
        };
        let mut state = match records(self.root) {
            Ok(state) => state,
            Err(e) => return self.stop(Problem::new(STATE, format!("the file cannot be read, so the launcher left the addon folders as they are ({e})"))),
        };
        let complete = |path: &Path| digest_folder(path).ok().as_deref() == Some(journal.revision.as_str());
        let target = self.target(&id);
        match journal.step {
            Step::Copy | Step::Replace => {
                let old = self.staging(&format!("{id}.old"));
                if complete(&target) {
                    if let Some(s) = state.as_mut() {
                        s.insert(id.clone(), Record::clean(&journal.revision));
                    }
                } else if !present(&target) && present(&old) {
                    let result = self.files.rename(&old, &target);
                    self.check(&format!("emulators/{id}"), "the old copy cannot be put back", result)?;
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
