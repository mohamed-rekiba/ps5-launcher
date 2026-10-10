//! The default addons' lifecycle, on temporary folders.

use super::bundle::{DefaultSource, ShippedAddon};
use super::lifecycle::*;
use super::manifest::Defaults;
use super::Problem;
use std::cell::Cell;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

// The digests of the test defaults below, from outside the code under test:
//
//     python3 -I -c 'import hashlib,struct
//     def d(files):
//         h = hashlib.sha256()
//         for p, c in sorted(files.items(), key=lambda kv: kv[0].encode()):
//             h.update(struct.pack(">Q", len(p.encode())) + p.encode() + struct.pack(">Q", len(c)) + c)
//         return h.hexdigest()
//     print(d({"emulator.yaml": b"kyty v1\n", "media/icon.svg": b"<svg/>"}))'
const V1: &str = "ac246414868d846819f128e5a65d3fdccdaf86f8959f2b5a48c9bfd85f421a71";
const V2: &str = "0fbea35eea7f7b82278e74843241ae03a71688a41de46845a77135c2dbadb3d9";
const V3: &str = "df9b7096ffbd79c62e45b1c5b3c2205a0f263df253201f676bc2700abac53c97";
const SHAD: &str = "52689ee81683f14bce33bc46be66f320f0db10ff71720bb3861a6c05adcb1702";

fn addon(id: &str, files: &[(&str, &str)]) -> ShippedAddon {
    ShippedAddon { id: id.into(), files: files.iter().map(|(p, c)| (p.to_string(), c.as_bytes().to_vec())).collect() }
}

fn kyty_v1() -> ShippedAddon {
    addon("kyty", &[("emulator.yaml", "kyty v1\n"), ("media/icon.svg", "<svg/>")])
}

fn kyty_v2() -> ShippedAddon {
    addon("kyty", &[("emulator.yaml", "kyty v2\n")])
}

fn kyty_v3() -> ShippedAddon {
    addon("kyty", &[("emulator.yaml", "kyty v3\n")])
}

fn shad_v1() -> ShippedAddon {
    addon("shadps4", &[("emulator.yaml", "shad v1\n")])
}

struct Source(Vec<ShippedAddon>);

impl DefaultSource for Source {
    fn emulators(&self) -> Vec<ShippedAddon> {
        self.0.clone()
    }
    fn console_defaults(&self) -> Defaults {
        Defaults::default()
    }
}

fn src(addons: &[ShippedAddon]) -> Source {
    Source(addons.to_vec())
}

fn root() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("addons-").tempdir().unwrap()
}

fn real() -> RealFiles {
    RealFiles { lock_wait: Duration::ZERO, durable: true }
}

fn run(root: &Path, addons: &[ShippedAddon]) -> Vec<Problem> {
    reconcile(root, &src(addons), &real())
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn record(root: &Path, id: &str) -> Option<Record> {
    records(root).unwrap().unwrap().get(id).cloned()
}

fn clean(revision: &str) -> Record {
    Record { revision: revision.into(), digest: revision.into(), deleted: false, offered: None }
}

/// The names in a folder, sorted; empty when it does not exist.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// The launcher's own files and folders in the root, after a change: nothing left over.
fn assert_tidy(root: &Path) {
    let leftovers: Vec<String> = names(&root.join("emulators/.staging"));
    assert!(leftovers.is_empty(), "staging: {leftovers:?}");
    let stray: Vec<String> = names(root).into_iter().filter(|n| !["emulators", "proposals", "kept", "addons-state.yaml", "addons.lock"].contains(&n.as_str())).collect();
    assert!(stray.is_empty(), "{stray:?}");
}

// ------------------------------------------------------------------ digest

#[test]
fn the_digest_covers_sorted_paths_and_length_delimited_contents() {
    let r = root();
    let d = r.path();
    fs::create_dir_all(d.join("a")).unwrap();
    fs::create_dir_all(d.join("empty")).unwrap();
    fs::write(d.join("b.txt"), "two").unwrap();
    fs::write(d.join("a/one.txt"), "1").unwrap();
    assert_eq!(digest_folder(d).unwrap(), "14113ec010250a3970da3eb27b5baa5ace946d05618e923ccba0af3717c65357");
    let files = [("b.txt".to_string(), b"two".to_vec()), ("a/one.txt".to_string(), b"1".to_vec())];
    assert_eq!(digest_files(&files), "14113ec010250a3970da3eb27b5baa5ace946d05618e923ccba0af3717c65357");
    fs::write(d.join("c"), "").unwrap();
    assert_eq!(digest_folder(d).unwrap(), "4a231a8272d75109df876f7468e8a25cb0a7a0d1ab8cf5004a3636952b10060f", "an added empty file counts");
    fs::remove_file(d.join("c")).unwrap();
    fs::write(d.join("b.txt"), "tw").unwrap();
    fs::write(d.join("o"), "").unwrap();
    assert_eq!(digest_folder(d).unwrap(), "88ffa4bb98b21b9134f920de3cfab010636bab6d68cad873b077621cb231793b", "lengths keep moved bytes apart");
    assert_eq!(digest_files(&[]), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
}

#[test]
fn the_digest_refuses_links_and_special_files() {
    let r = root();
    fs::write(r.path().join("a"), "1").unwrap();
    std::os::unix::fs::symlink(r.path().join("a"), r.path().join("link")).unwrap();
    assert!(digest_folder(r.path()).unwrap_err().to_string().contains("link"));
    fs::remove_file(r.path().join("link")).unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(r.path().join("socket")).unwrap();
    assert!(digest_folder(r.path()).unwrap_err().to_string().contains("socket"));
}

// ------------------------------------------------------------------ first start

#[test]
fn the_first_start_copies_every_default_and_records_it() {
    let r = root();
    assert_eq!(run(r.path(), &[kyty_v1(), shad_v1()]), []);
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "kyty v1\n");
    assert_eq!(read(r.path(), "emulators/kyty/media/icon.svg"), "<svg/>");
    assert_eq!(read(r.path(), "emulators/shadps4/emulator.yaml"), "shad v1\n");
    assert_eq!(record(r.path(), "kyty"), Some(clean(V1)));
    assert_eq!(record(r.path(), "shadps4"), Some(clean(SHAD)));
    assert_tidy(r.path());
    // The state file is plain YAML with these fields.
    let state: serde_norway::Value = serde_norway::from_str(&read(r.path(), "addons-state.yaml")).unwrap();
    assert_eq!(state["emulators"]["kyty"]["revision"].as_str(), Some(V1));
    assert_eq!(state["emulators"]["kyty"]["deleted"].as_bool(), Some(false));
}

#[test]
fn a_second_start_changes_nothing() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    let before = read(r.path(), "addons-state.yaml");
    assert_eq!(run(r.path(), &[kyty_v1()]), []);
    assert_eq!(read(r.path(), "addons-state.yaml"), before);
}

// ------------------------------------------------------------------ updates

#[test]
fn an_unchanged_copy_is_replaced_by_the_new_default() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    assert_eq!(run(r.path(), &[kyty_v2()]), []);
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "kyty v2\n");
    assert!(!r.path().join("emulators/kyty/media").exists(), "the whole folder is replaced");
    assert_eq!(record(r.path(), "kyty"), Some(clean(V2)));
    assert_tidy(r.path());
}

#[test]
fn an_edited_copy_is_kept_and_the_new_default_offered_once() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    fs::write(r.path().join("emulators/kyty/emulator.yaml"), "kyty v1, my edit\n").unwrap();
    let problems = run(r.path(), &[kyty_v2()]);
    let offer = proposal_folder(r.path(), "kyty", V2);
    assert_eq!(offer, r.path().join("proposals/emulators/kyty/0fbea35eea7f7b82"));
    assert_eq!(problems, [Problem::new("emulators/kyty", "you changed this addon, so the launcher kept it; its new default is in proposals/emulators/kyty/0fbea35eea7f7b82")]);
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "kyty v1, my edit\n");
    assert_eq!(read(&offer, "emulator.yaml"), "kyty v2\n");
    assert_eq!(record(r.path(), "kyty"), Some(Record { offered: Some(V2.into()), ..clean(V1) }));
    assert_tidy(r.path());

    assert_eq!(run(r.path(), &[kyty_v2()]), [], "the same offer is not repeated");
    let problems = run(r.path(), &[kyty_v3()]);
    assert_eq!(problems.len(), 1, "a newer default is offered again");
    assert_eq!(read(&proposal_folder(r.path(), "kyty", V3), "emulator.yaml"), "kyty v3\n");
    assert_eq!(record(r.path(), "kyty").unwrap().offered.as_deref(), Some(V3));
}

#[test]
fn a_copy_the_user_brought_up_to_date_is_recorded() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    fs::remove_dir_all(r.path().join("emulators/kyty/media")).unwrap();
    fs::write(r.path().join("emulators/kyty/emulator.yaml"), "kyty v2\n").unwrap();
    assert_eq!(run(r.path(), &[kyty_v2()]), []);
    assert_eq!(record(r.path(), "kyty"), Some(clean(V2)));
}

#[test]
fn an_older_launcher_puts_back_its_own_default_on_an_unchanged_copy() {
    // The default belongs to the adapters compiled with it: after a rollback, the older one.
    let r = root();
    run(r.path(), &[kyty_v1()]);
    run(r.path(), &[kyty_v2()]);
    assert_eq!(run(r.path(), &[kyty_v1()]), []);
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "kyty v1\n");
    assert_eq!(record(r.path(), "kyty"), Some(clean(V1)));
}

// ------------------------------------------------------------------ deletions and new defaults

#[test]
fn a_deleted_copy_is_never_restored_by_itself() {
    let r = root();
    run(r.path(), &[kyty_v1(), shad_v1()]);
    fs::remove_dir_all(r.path().join("emulators/kyty")).unwrap();
    assert_eq!(run(r.path(), &[kyty_v1(), shad_v1()]), []);
    assert!(!r.path().join("emulators/kyty").exists());
    assert_eq!(record(r.path(), "kyty"), Some(Record { deleted: true, ..clean(V1) }));
    run(r.path(), &[kyty_v2(), shad_v1()]);
    assert!(!r.path().join("emulators/kyty").exists(), "not after an update either");
}

#[test]
fn a_new_shipped_default_is_copied_once() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    assert_eq!(run(r.path(), &[kyty_v1(), shad_v1()]), []);
    assert_eq!(read(r.path(), "emulators/shadps4/emulator.yaml"), "shad v1\n");
    fs::remove_dir_all(r.path().join("emulators/shadps4")).unwrap();
    run(r.path(), &[kyty_v1(), shad_v1()]);
    assert!(!r.path().join("emulators/shadps4").exists());
}

#[test]
fn a_users_folder_with_a_new_defaults_name_is_kept() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    fs::create_dir_all(r.path().join("emulators/shadps4")).unwrap();
    fs::write(r.path().join("emulators/shadps4/emulator.yaml"), "mine\n").unwrap();
    let problems = run(r.path(), &[kyty_v1(), shad_v1()]);
    assert_eq!(problems, [Problem::new("emulators/shadps4", "this folder was already there, so the launcher kept it; the default is in proposals/emulators/shadps4/52689ee81683f14b")]);
    assert_eq!(read(r.path(), "emulators/shadps4/emulator.yaml"), "mine\n");
    assert_eq!(read(&proposal_folder(r.path(), "shadps4", SHAD), "emulator.yaml"), "shad v1\n");
    assert_eq!(run(r.path(), &[kyty_v1(), shad_v1()]), [], "offered once");
}

#[test]
fn a_users_own_addon_is_never_touched() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    fs::create_dir_all(r.path().join("emulators/ps4-lab")).unwrap();
    fs::write(r.path().join("emulators/ps4-lab/emulator.yaml"), "mine\n").unwrap();
    run(r.path(), &[kyty_v2()]);
    assert_eq!(read(r.path(), "emulators/ps4-lab/emulator.yaml"), "mine\n");
    assert_eq!(record(r.path(), "ps4-lab"), None);
}

// ------------------------------------------------------------------ a missing or broken state

#[test]
fn a_missing_state_file_keeps_every_folder_as_it_is() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    fs::remove_file(r.path().join("addons-state.yaml")).unwrap();
    let problems = run(r.path(), &[kyty_v2(), shad_v1()]);
    assert_eq!(problems, [Problem::new("addons-state.yaml", "the file is missing, so the launcher left the addon folders as they are. Restore missing defaults to record them again")]);
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "kyty v1\n");
    assert!(!r.path().join("emulators/shadps4").exists());
    assert_eq!(records(r.path()), Ok(None));
}

#[test]
fn a_broken_state_file_keeps_every_folder_as_it_is() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    fs::write(r.path().join("addons-state.yaml"), "emulators: [\n").unwrap();
    let problems = run(r.path(), &[kyty_v2()]);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].subject, "addons-state.yaml");
    assert!(problems[0].message.starts_with("the file cannot be read, so the launcher left the addon folders as they are. Restore missing defaults to record them again ("), "{}", problems[0].message);
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "kyty v1\n");
    assert_eq!(read(r.path(), "addons-state.yaml"), "emulators: [\n", "the broken file stays for the user to see");
}

// ------------------------------------------------------------------ the Settings actions

#[test]
fn restore_missing_defaults_brings_back_deleted_copies() {
    let r = root();
    run(r.path(), &[kyty_v1(), shad_v1()]);
    fs::remove_dir_all(r.path().join("emulators/kyty")).unwrap();
    run(r.path(), &[kyty_v1(), shad_v1()]);
    fs::write(r.path().join("emulators/shadps4/emulator.yaml"), "edited\n").unwrap();
    assert_eq!(restore_missing_defaults(r.path(), &src(&[kyty_v1(), shad_v1()]), &real()), []);
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "kyty v1\n");
    assert_eq!(record(r.path(), "kyty"), Some(clean(V1)));
    assert_eq!(read(r.path(), "emulators/shadps4/emulator.yaml"), "edited\n", "existing folders stay");
    assert_tidy(r.path());
}

#[test]
fn restore_missing_defaults_rebuilds_a_lost_state_file() {
    let r = root();
    run(r.path(), &[kyty_v1(), shad_v1()]);
    fs::remove_file(r.path().join("addons-state.yaml")).unwrap();
    fs::remove_dir_all(r.path().join("emulators/shadps4")).unwrap();
    fs::write(r.path().join("emulators/kyty/emulator.yaml"), "edited\n").unwrap();
    assert_eq!(restore_missing_defaults(r.path(), &src(&[kyty_v1(), shad_v1()]), &real()), []);
    assert_eq!(read(r.path(), "emulators/shadps4/emulator.yaml"), "shad v1\n");
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "edited\n");
    assert_eq!(record(r.path(), "shadps4"), Some(clean(SHAD)));
    // The kept copy is recorded as changed: a newer default is offered, never written over it.
    assert_eq!(record(r.path(), "kyty"), Some(clean(V1)));
    assert_eq!(run(r.path(), &[kyty_v2(), shad_v1()]).len(), 1);
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "edited\n");
}

#[test]
fn reset_to_default_replaces_an_edited_copy() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    fs::write(r.path().join("emulators/kyty/emulator.yaml"), "edited\n").unwrap();
    fs::write(r.path().join("emulators/kyty/extra.txt"), "mine\n").unwrap();
    run(r.path(), &[kyty_v2()]);
    assert_eq!(reset_to_default(r.path(), &src(&[kyty_v2()]), &real(), "kyty"), []);
    assert_eq!(names(&r.path().join("emulators/kyty")), ["emulator.yaml"]);
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "kyty v2\n");
    assert_eq!(record(r.path(), "kyty"), Some(clean(V2)));
    assert_tidy(r.path());
}

#[test]
fn reset_to_default_brings_back_a_deleted_copy_and_refuses_other_ids() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    fs::remove_dir_all(r.path().join("emulators/kyty")).unwrap();
    run(r.path(), &[kyty_v1()]);
    assert_eq!(reset_to_default(r.path(), &src(&[kyty_v1()]), &real(), "kyty"), []);
    assert_eq!(record(r.path(), "kyty"), Some(clean(V1)));
    assert_eq!(reset_to_default(r.path(), &src(&[kyty_v1()]), &real(), "ps4-lab"), [Problem::new("emulators/ps4-lab", "the launcher has no default for this addon")]);
}

// ------------------------------------------------------------------ the lock

#[test]
fn one_lock_serializes_changes() {
    let r = root();
    let held = real().lock(&r.path().join("addons.lock")).unwrap();
    assert!(held.is_some());
    assert!(real().lock(&r.path().join("addons.lock")).unwrap().is_none(), "a second lock waits");
    assert_eq!(run(r.path(), &[kyty_v1()]), [Problem::new("addons", "another launcher is changing the addons; the defaults were not checked this time")]);
    assert!(!r.path().join("emulators").exists());
    drop(held);
    assert_eq!(run(r.path(), &[kyty_v1()]), []);
}

// ------------------------------------------------------------------ interrupted changes

/// The kinds of file change, to say which ones a test stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Op {
    CreateDir,
    WriteNew,
    Rename,
    SyncDir,
    RemoveFile,
    RemoveDirAll,
}

type Hook<'a> = Box<dyn Fn(usize, Op, &Path) -> io::Result<()> + 'a>;

/// The real file system with a hook before every change. The hook can fail the change, as a
/// crash would, or change the disk first, as another program would.
struct Injected<'a> {
    real: RealFiles,
    count: Cell<usize>,
    failed: Cell<Option<Op>>,
    hook: Hook<'a>,
}

impl<'a> Injected<'a> {
    fn new(real: RealFiles, hook: impl Fn(usize, Op, &Path) -> io::Result<()> + 'a) -> Injected<'a> {
        Injected { real, count: Cell::new(0), failed: Cell::new(None), hook: Box::new(hook) }
    }

    /// The `fail_at`-th change (0-based) fails.
    fn crash_at(fail_at: usize, real: RealFiles) -> Injected<'a> {
        Injected::new(real, move |n, _, _| if n == fail_at { Err(io::Error::other("the power went off")) } else { Ok(()) })
    }

    fn step(&self, op: Op, path: &Path) -> io::Result<()> {
        let n = self.count.get();
        self.count.set(n + 1);
        let result = (self.hook)(n, op, path);
        if result.is_err() && self.failed.get().is_none() {
            self.failed.set(Some(op));
        }
        result
    }

    /// The change a run stopped at, if it got that far.
    fn crashed(&self) -> Option<Op> {
        self.failed.get()
    }
}

impl FileOps for Injected<'_> {
    fn create_dir(&self, path: &Path) -> io::Result<()> {
        self.step(Op::CreateDir, path)?;
        self.real.create_dir(path)
    }
    fn write_new(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        self.step(Op::WriteNew, path)?;
        self.real.write_new(path, data)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.step(Op::Rename, from)?;
        self.real.rename(from, to)
    }
    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        self.step(Op::SyncDir, path)?;
        self.real.sync_dir(path)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.step(Op::RemoveFile, path)?;
        self.real.remove_file(path)
    }
    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.step(Op::RemoveDirAll, path)?;
        self.real.remove_dir_all(path)
    }
    fn lock(&self, path: &Path) -> io::Result<Option<Lock>> {
        self.real.lock(path)
    }
}

/// No flush to the disk: the tests cannot see it, and the nested crash tests run hundreds of
/// changes.
fn fast() -> RealFiles {
    RealFiles { lock_wait: Duration::ZERO, durable: false }
}

/// Stop `change` at every step in turn; after each stop, the next start must end where an
/// uninterrupted run ends, and no part of a copy may ever be in place. Returns the kinds of
/// change that were stopped.
fn survives_a_crash_at_every_step(prepare: impl Fn(&Path), change: impl Fn(&Path, &dyn FileOps) -> Vec<Problem>, end: impl Fn(&Path)) -> BTreeSet<Op> {
    let mut stopped = BTreeSet::new();
    for fail_at in 0.. {
        let r = root();
        prepare(r.path());
        let crash = Injected::crash_at(fail_at, real());
        change(r.path(), &crash);
        let Some(op) = crash.crashed() else {
            assert!(fail_at > 3, "the change took {fail_at} steps");
            break;
        };
        stopped.insert(op);
        for folder in names(&r.path().join("emulators")).iter().filter(|n| !n.starts_with('.')) {
            let digest = digest_folder(&r.path().join("emulators").join(folder)).unwrap();
            assert!([V1, V2, SHAD].contains(&digest.as_str()), "step {fail_at}: a partial emulators/{folder}");
        }
        assert_eq!(run(r.path(), &[kyty_v2(), shad_v1()]), [], "step {fail_at}");
        end(r.path());
        assert_tidy(r.path());
    }
    stopped
}

#[test]
fn an_interrupted_first_copy_is_finished_or_undone_at_the_next_start() {
    survives_a_crash_at_every_step(
        |_| {},
        |root, files| reconcile(root, &src(&[kyty_v2(), shad_v1()]), files),
        |root| {
            assert_eq!(read(root, "emulators/kyty/emulator.yaml"), "kyty v2\n");
            assert_eq!(read(root, "emulators/shadps4/emulator.yaml"), "shad v1\n");
            assert_eq!((record(root, "kyty"), record(root, "shadps4")), (Some(clean(V2)), Some(clean(SHAD))));
        },
    );
}

#[test]
fn an_interrupted_update_is_finished_or_undone_at_the_next_start() {
    let stopped = survives_a_crash_at_every_step(
        |root| assert_eq!(run(root, &[kyty_v1(), shad_v1()]), []),
        |root, files| reconcile(root, &src(&[kyty_v2(), shad_v1()]), files),
        |root| {
            assert_eq!(names(&root.join("emulators/kyty")), ["emulator.yaml"]);
            assert_eq!(read(root, "emulators/kyty/emulator.yaml"), "kyty v2\n");
            assert_eq!(record(root, "kyty"), Some(clean(V2)), "never counted as deleted or edited");
        },
    );
    // Inside the atomic writes too (a temporary file written, not yet renamed), and a flush
    // that fails after a rename succeeded.
    let all = [Op::CreateDir, Op::WriteNew, Op::Rename, Op::SyncDir, Op::RemoveFile, Op::RemoveDirAll];
    assert_eq!(stopped, BTreeSet::from(all));
}

#[test]
fn an_interrupted_recovery_is_finished_at_the_next_start() {
    let prepare = |root: &Path| assert_eq!(reconcile(root, &src(&[kyty_v1(), shad_v1()]), &fast()), []);
    let mut recoveries = 0;
    for first in 0.. {
        let r = root();
        prepare(r.path());
        let crash = Injected::crash_at(first, fast());
        reconcile(r.path(), &src(&[kyty_v2(), shad_v1()]), &crash);
        if crash.crashed().is_none() {
            break;
        }
        if !r.path().join("addons-journal.yaml").exists() {
            continue;
        }
        // The next start stops again, at each step of its own recovery and update in turn.
        for second in 0.. {
            let r = root();
            prepare(r.path());
            reconcile(r.path(), &src(&[kyty_v2(), shad_v1()]), &Injected::crash_at(first, fast()));
            let again = Injected::crash_at(second, fast());
            reconcile(r.path(), &src(&[kyty_v2(), shad_v1()]), &again);
            if again.crashed().is_none() {
                break;
            }
            recoveries += 1;
            assert_eq!(reconcile(r.path(), &src(&[kyty_v2(), shad_v1()]), &fast()), [], "steps {first}, {second}");
            assert_eq!(names(&r.path().join("emulators/kyty")), ["emulator.yaml"], "steps {first}, {second}");
            assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "kyty v2\n");
            assert_eq!(record(r.path(), "kyty"), Some(clean(V2)), "steps {first}, {second}");
            assert_tidy(r.path());
        }
    }
    assert!(recoveries > 20, "{recoveries}");
}

#[test]
fn an_interrupted_offer_is_finished_or_undone_at_the_next_start() {
    let edited = |root: &Path| {
        assert_eq!(run(root, &[kyty_v1(), shad_v1()]), []);
        fs::write(root.join("emulators/kyty/emulator.yaml"), "edited\n").unwrap();
    };
    for fail_at in 0.. {
        let r = root();
        edited(r.path());
        let crash = Injected::crash_at(fail_at, real());
        reconcile(r.path(), &src(&[kyty_v2(), shad_v1()]), &crash);
        if crash.crashed().is_none() {
            break;
        }
        // Finished, the offer was made; undone, the next start makes it.
        let again = run(r.path(), &[kyty_v2(), shad_v1()]);
        assert!(again.len() <= 1, "step {fail_at}: {again:?}");
        assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "edited\n");
        assert_eq!(read(&proposal_folder(r.path(), "kyty", V2), "emulator.yaml"), "kyty v2\n", "step {fail_at}");
        assert_eq!(record(r.path(), "kyty").unwrap().offered.as_deref(), Some(V2));
        assert_eq!(run(r.path(), &[kyty_v2(), shad_v1()]), []);
        assert_tidy(r.path());
    }
}

#[test]
fn an_interrupted_reset_leaves_the_edit_or_the_default() {
    for fail_at in 0.. {
        let r = root();
        run(r.path(), &[kyty_v2()]);
        fs::write(r.path().join("emulators/kyty/emulator.yaml"), "edited\n").unwrap();
        let crash = Injected::crash_at(fail_at, real());
        reset_to_default(r.path(), &src(&[kyty_v2()]), &crash, "kyty");
        if crash.crashed().is_none() {
            break;
        }
        run(r.path(), &[kyty_v2()]);
        let text = read(r.path(), "emulators/kyty/emulator.yaml");
        assert!(text == "edited\n" || text == "kyty v2\n", "step {fail_at}: {text}");
        assert_eq!(record(r.path(), "kyty").unwrap().revision, V2);
        assert_tidy(r.path());
    }
}

#[test]
fn an_edit_made_during_an_update_is_kept() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    let folder = r.path().join("emulators/kyty");
    // The user saves an edit just before the launcher moves the checked copy aside.
    let editor = Injected::new(real(), |_, op, path| {
        if op == Op::Rename && path == folder {
            fs::write(folder.join("emulator.yaml"), "edited meanwhile\n").unwrap();
        }
        Ok(())
    });
    let problems = reconcile(r.path(), &src(&[kyty_v2()]), &editor);
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "edited meanwhile\n");
    assert_eq!(read(r.path(), "emulators/kyty/media/icon.svg"), "<svg/>");
    assert_eq!(problems, [Problem::new("emulators/kyty", "you changed this addon, so the launcher kept it; its new default is in proposals/emulators/kyty/0fbea35eea7f7b82")]);
    assert_eq!(read(&proposal_folder(r.path(), "kyty", V2), "emulator.yaml"), "kyty v2\n");
    assert_eq!(record(r.path(), "kyty"), Some(Record { offered: Some(V2.into()), ..clean(V1) }));
    assert_tidy(r.path());
}

#[test]
fn a_recovery_that_finds_another_folder_keeps_both() {
    let r = root();
    run(r.path(), &[kyty_v1()]);
    // The update stops after the old copy went aside, before the default is in place ...
    let staged = r.path().join("emulators/.staging/kyty");
    let crash = Injected::new(real(), |_, op, path| if op == Op::Rename && path == staged { Err(io::Error::other("the power went off")) } else { Ok(()) });
    reconcile(r.path(), &src(&[kyty_v2()]), &crash);
    assert_eq!(crash.crashed(), Some(Op::Rename));
    // ... and the user makes a new folder with that name before the next start.
    fs::create_dir(r.path().join("emulators/kyty")).unwrap();
    fs::write(r.path().join("emulators/kyty/emulator.yaml"), "mine\n").unwrap();
    let problems = run(r.path(), &[kyty_v2()]);
    assert_eq!(problems[0], Problem::new("emulators/kyty", "an interrupted update found another folder here; the launcher kept both, and the earlier copy is in kept/emulators/kyty/1"));
    assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "mine\n");
    assert_eq!(read(r.path(), "kept/emulators/kyty/1/emulator.yaml"), "kyty v1\n");
    assert_eq!(read(r.path(), "kept/emulators/kyty/1/media/icon.svg"), "<svg/>");
    assert!(!r.path().join("addons-journal.yaml").exists());
    assert_tidy(r.path());
}

#[test]
fn a_damaged_journal_leaves_every_folder_as_it_is() {
    for journal in ["step: offer\nid: kyty\nrevision: \"ééééééééééééééééé\"\n", "step: copy\nid: ../kyty\nrevision: aaaa\n", "step: [\n"] {
        let r = root();
        run(r.path(), &[kyty_v1()]);
        fs::write(r.path().join("addons-journal.yaml"), journal).unwrap();
        let problems = run(r.path(), &[kyty_v2()]);
        assert_eq!(problems.len(), 1, "{journal}");
        assert_eq!(problems[0].subject, "addons-journal.yaml");
        assert!(problems[0].message.contains("the launcher left the addon folders as they are"), "{}", problems[0].message);
        assert_eq!(read(r.path(), "emulators/kyty/emulator.yaml"), "kyty v1\n");
    }
}
