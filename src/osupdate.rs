//! PS5 Launcher OS's own state from `bootc status --json`: what runs now, whether an update waits
//! for the next restart, and whether there is a system to roll back to (Phase 6 of
//! docs/plans/ps5-launcher-os.md). Parsing, the command lines and the root helper's answers live
//! here; `system::call` runs them.

use crate::system::{Call, Ran};

/// PS5 Launcher OS's root helper (packaging/os/files/usr/libexec/ps5-launcher/helper).
pub const HELPER: &str = "/usr/libexec/ps5-launcher/helper";

/// One system image on the PC.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Deployment {
    /// The image name with its tag, for example "ghcr.io/owner/ps5-launcher-os:main".
    pub image: String,
    pub digest: String,
    /// The image's version label, for example "44.20261010.0".
    pub version: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Status {
    pub booted: Deployment,
    /// Downloaded and installed at the next restart ("Update and restart").
    pub staged: Option<Deployment>,
    /// The previous system, which "Undo the last system update" starts.
    pub rollback: Option<Deployment>,
    /// The version `bootc upgrade --check` found, not downloaded yet.
    pub available: Option<String>,
}

/// Parse `bootc status --json`. On a terminal bootc prints a progress line first, so the JSON
/// starts at the first `{`.
pub fn parse_status(output: &str) -> Result<Status, String> {
    let json = output.find('{').map(|start| &output[start..]).ok_or("no JSON in bootc's output")?;
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("unexpected bootc status: {e}"))?;
    let status = &v["status"];
    let booted = deployment(&status["booted"]).ok_or("bootc status has no booted system")?;
    Ok(Status {
        booted,
        staged: deployment(&status["staged"]),
        rollback: deployment(&status["rollback"]),
        available: status["booted"]["cachedUpdate"]["version"].as_str().map(String::from),
    })
}

/// A deployment entry (`booted`, `staged` or `rollback`), or None when it is null or incomplete.
fn deployment(v: &serde_json::Value) -> Option<Deployment> {
    let image = &v["image"];
    Some(Deployment {
        image: image["image"]["image"].as_str()?.to_string(),
        digest: image["imageDigest"].as_str()?.to_string(),
        version: image["version"].as_str().map(String::from),
    })
}

/// bootc status needs root on a booted system, so it goes through the helper.
pub fn status_call() -> Call {
    Call::new("pkexec", &[HELPER, "status"], 30)
}

/// A task of the root helper, through pkexec (polkit allows it for the user at the PC).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Task {
    UpdateCheck,
    Update,
    Rollback,
    QueueKey,
}

pub fn helper_call(task: Task) -> Call {
    let (name, secs) = match task {
        Task::UpdateCheck => ("update-check", 120),
        // Downloads the new system image: it may take a long time on a slow line.
        Task::Update => ("update", 2 * 60 * 60),
        Task::Rollback => ("rollback", 120),
        Task::QueueKey => ("queue-key", 60),
    };
    Call::new("pkexec", &[HELPER, name], secs)
}

/// What `helper update` (or `switch`) answered.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Staging {
    /// Staged: it installs at the next restart.
    Done,
    /// The NVIDIA image's Secure Boot key is not enrolled (exit 3): queue it with `queue-key`.
    KeyRequired,
    /// The key waits for the blue MOK screen (exit 4): restart and enroll it first.
    KeyPending,
    Failed(String),
}

pub fn staging(call: &Call, ran: &Ran) -> Staging {
    match ran.code {
        Some(0) => Staging::Done,
        Some(3) if ran.stdout.trim() == "key-required" => Staging::KeyRequired,
        Some(4) if ran.stdout.trim() == "key-pending" => Staging::KeyPending,
        _ => Staging::Failed(ran.error(call)),
    }
}

/// What `helper queue-key` printed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Key {
    /// The 8 digits the blue MOK screen asks for.
    Password(String),
    /// Enrolled already: the update can go ahead.
    Enrolled,
}

pub fn parse_key(output: &str) -> Result<Key, String> {
    match output.trim() {
        "key-enrolled" => Ok(Key::Enrolled),
        p if p.len() == 8 && p.bytes().all(|b| b.is_ascii_digit()) => Ok(Key::Password(p.to_string())),
        other => Err(format!("unexpected answer from queue-key: {other:?}")),
    }
}

/// How "Download update" ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum UpdateEnd {
    Staged,
    /// The key is queued: the blue MOK screen asks for this password at the next start. The
    /// update is not staged yet; it is downloaded again once the key is enrolled.
    Password(String),
    /// A queued key waits for the blue screen: restart and enroll it first.
    KeyPending,
    Failed(String),
}

/// `helper update`, and when the key is required, `helper queue-key`. When queue-key finds the
/// key enrolled already, the update runs once more. `run` runs a call (`system::call_status`).
pub fn update_flow(run: &dyn Fn(&Call) -> Result<Ran, String>) -> UpdateEnd {
    let update = helper_call(Task::Update);
    for _ in 0..2 {
        let ran = match run(&update) {
            Ok(ran) => ran,
            Err(e) => return UpdateEnd::Failed(e),
        };
        match staging(&update, &ran) {
            Staging::Done => return UpdateEnd::Staged,
            Staging::KeyPending => return UpdateEnd::KeyPending,
            Staging::Failed(e) => return UpdateEnd::Failed(e),
            Staging::KeyRequired => {}
        }
        let queue = helper_call(Task::QueueKey);
        let key = run(&queue).and_then(|ran| if ran.code == Some(0) { parse_key(&ran.stdout) } else { Err(ran.error(&queue)) });
        match key {
            Ok(Key::Password(p)) => return UpdateEnd::Password(p),
            Ok(Key::Enrolled) => {}
            Err(e) => return UpdateEnd::Failed(e),
        }
    }
    UpdateEnd::Failed("the key is enrolled, but the helper still asks for it".into())
}

/// The digest `helper update-check` found on the NVIDIA image (`update-available <digest>`).
/// The main image prints bootc's own text instead; bootc status then has the version.
pub fn parse_check(output: &str) -> Option<String> {
    output.trim().strip_prefix("update-available ").map(|d| d.trim().to_string()).filter(|d| !d.is_empty())
}

/// "sha256:feda9c45f80a…" → "feda9c45f80a".
pub fn short_digest(digest: &str) -> String {
    digest.strip_prefix("sha256:").unwrap_or(digest).chars().take(12).collect()
}

/// The Updates page's status line. `found`: the update `update-check` found, by version or
/// digest; `checking`: the check runs now.
pub fn status_text(status: &Status, found: Option<&str>, checking: bool) -> String {
    if status.staged.is_some() {
        return "Update ready: restart to install".into();
    }
    if let Some(found) = found.map(String::from).or_else(|| status.available.clone()) {
        let shown = if found.starts_with("sha256:") { short_digest(&found) } else { found };
        return format!("Update available ({shown})");
    }
    if checking { "Checking for updates…".into() } else { "System: up to date".into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ran(code: i32, stdout: &str) -> Ran {
        Ran { code: Some(code), stdout: stdout.into(), stderr: String::new() }
    }

    #[test]
    fn the_helpers_exit_codes() {
        let call = helper_call(Task::Update);
        assert_eq!(staging(&call, &ran(0, "")), Staging::Done);
        assert_eq!(staging(&call, &ran(3, "key-required\n")), Staging::KeyRequired);
        assert_eq!(staging(&call, &ran(4, "key-pending\n")), Staging::KeyPending);
        let failed = Ran { code: Some(1), stdout: String::new(), stderr: "helper: could not read ghcr.io/x\n".into() };
        assert_eq!(
            staging(&call, &failed),
            Staging::Failed("pkexec /usr/libexec/ps5-launcher/helper update failed (exit 1): helper: could not read ghcr.io/x".into())
        );
        // pkexec's own refusal (126, 127) or a stray 3 is a failure, not a key step.
        assert!(matches!(staging(&call, &ran(126, "")), Staging::Failed(_)));
        assert!(matches!(staging(&call, &ran(3, "")), Staging::Failed(_)));
    }

    /// Answers each helper task with the next answer given for it, and records the tasks.
    fn helper(answers: &[(&'static str, i32, &'static str)]) -> (impl Fn(&Call) -> Result<Ran, String>, std::rc::Rc<std::cell::RefCell<Vec<String>>>) {
        let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let left = std::cell::RefCell::new(answers.to_vec());
        let log = asked.clone();
        let run = move |call: &Call| {
            let task = call.args[1].clone();
            log.borrow_mut().push(task.clone());
            let mut left = left.borrow_mut();
            let i = left.iter().position(|(t, _, _)| *t == task).expect("an answer for each call");
            let (_, code, stdout) = left.remove(i);
            Ok(ran(code, stdout))
        };
        (run, asked)
    }

    #[test]
    fn an_update_that_stages() {
        let (run, asked) = helper(&[("update", 0, "")]);
        assert_eq!(update_flow(&run), UpdateEnd::Staged);
        assert_eq!(*asked.borrow(), ["update"]);
    }

    #[test]
    fn a_required_key_is_queued_and_its_password_shown() {
        let (run, asked) = helper(&[("update", 3, "key-required\n"), ("queue-key", 0, "04718263\n")]);
        assert_eq!(update_flow(&run), UpdateEnd::Password("04718263".into()));
        assert_eq!(*asked.borrow(), ["update", "queue-key"]);
    }

    #[test]
    fn a_key_enrolled_already_lets_the_update_run_again() {
        let (run, asked) = helper(&[("update", 3, "key-required\n"), ("queue-key", 0, "key-enrolled\n"), ("update", 0, "")]);
        assert_eq!(update_flow(&run), UpdateEnd::Staged);
        assert_eq!(*asked.borrow(), ["update", "queue-key", "update"]);
    }

    #[test]
    fn a_pending_key_or_a_failure_stops_the_update() {
        let (run, _) = helper(&[("update", 4, "key-pending\n")]);
        assert_eq!(update_flow(&run), UpdateEnd::KeyPending);
        let (run, _) = helper(&[("update", 3, "key-required\n"), ("queue-key", 1, "")]);
        assert!(matches!(update_flow(&run), UpdateEnd::Failed(e) if e.contains("queue-key failed (exit 1)")));
        let (run, _) = helper(&[("update", 3, "key-required\n"), ("queue-key", 0, "key-enrolled\n"), ("update", 3, "key-required\n"), ("queue-key", 0, "key-enrolled\n")]);
        assert!(matches!(update_flow(&run), UpdateEnd::Failed(_)), "no endless loop");
        let failing = |_: &Call| Err("could not run pkexec".to_string());
        assert_eq!(update_flow(&failing), UpdateEnd::Failed("could not run pkexec".into()));
    }

    #[test]
    fn helper_command_lines() {
        assert_eq!(helper_call(Task::UpdateCheck).args, [HELPER, "update-check"]);
        assert_eq!(helper_call(Task::Update).args, [HELPER, "update"]);
        assert_eq!(helper_call(Task::Rollback).args, [HELPER, "rollback"]);
        assert_eq!(helper_call(Task::QueueKey).args, [HELPER, "queue-key"]);
        assert_eq!(helper_call(Task::Update).program, "pkexec");
        assert!(helper_call(Task::Update).secs >= 3600, "a download takes time");
    }

    #[test]
    fn status_is_read_through_the_root_helper() {
        // bootc status takes root on a booted system (prepare_for_write), so the user asks the
        // helper, which polkit allows without a password.
        assert_eq!(status_call().program, "pkexec");
        assert_eq!(status_call().args, [HELPER, "status"]);
    }

    #[test]
    fn the_key_password() {
        assert_eq!(parse_key("04718263\n"), Ok(Key::Password("04718263".into())));
        assert_eq!(parse_key("key-enrolled\n"), Ok(Key::Enrolled));
        for out in ["", "1234567", "123456789", "abcdefgh"] {
            assert!(parse_key(out).is_err(), "{out:?}");
        }
    }

    #[test]
    fn the_nvidia_check() {
        assert_eq!(parse_check("update-available sha256:0123abcd\n"), Some("sha256:0123abcd".into()));
        assert_eq!(parse_check("up-to-date\n"), None);
        assert_eq!(parse_check("No update available.\n"), None);
    }

    #[test]
    fn the_status_line() {
        let status = parse_status(BOOTED).unwrap();
        assert_eq!(status_text(&status, None, false), "System: up to date");
        assert_eq!(status_text(&status, None, true), "Checking for updates…");
        assert_eq!(status_text(&status, Some("sha256:0123456789abcdef"), false), "Update available (0123456789ab)");
        let found = Status { available: Some("44.20261012.0".into()), ..status.clone() };
        assert_eq!(status_text(&found, None, false), "Update available (44.20261012.0)");
        let staged = Status { staged: Some(booted()), ..found };
        assert_eq!(status_text(&staged, Some("sha256:0123"), true), "Update ready: restart to install");
    }

    const BOOTED: &str = include_str!("testdata/fedora44-vm-bootc-status.txt");

    /// The captured status, with `field` of "status" replaced by `value` (same shape as `booted`).
    fn with(field: &str, value: serde_json::Value) -> String {
        let json = &BOOTED[BOOTED.find('{').unwrap()..];
        let mut v: serde_json::Value = serde_json::from_str(json).unwrap();
        v["status"][field] = value;
        v.to_string()
    }

    fn booted() -> Deployment {
        Deployment {
            image: "localhost/ps5-launcher-os:boottest".into(),
            digest: "sha256:feda9c45f80a4bfe4541a133edbe69410aba4f0d03003fec7ddfdc3c7e7f20d6".into(),
            version: Some("44.20261010.0".into()),
        }
    }

    #[test]
    fn the_running_system_from_real_output() {
        assert_eq!(
            parse_status(BOOTED),
            Ok(Status { booted: booted(), staged: None, rollback: None, available: None })
        );
    }

    #[test]
    fn a_staged_update_and_a_rollback() {
        let json = &BOOTED[BOOTED.find('{').unwrap()..];
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let mut newer = v["status"]["booted"].clone();
        newer["image"]["imageDigest"] = "sha256:0123".into();
        newer["image"]["version"] = "44.20261011.0".into();
        let staged = parse_status(&with("staged", newer.clone())).unwrap().staged.unwrap();
        assert_eq!(staged.digest, "sha256:0123");
        assert_eq!(staged.version.as_deref(), Some("44.20261011.0"));
        assert_eq!(parse_status(&with("rollback", newer)).unwrap().rollback.unwrap().digest, "sha256:0123");
    }

    #[test]
    fn an_update_found_by_check() {
        let mut cached = serde_json::json!({});
        cached["version"] = "44.20261012.0".into();
        cached["imageDigest"] = "sha256:4567".into();
        let mut booted = serde_json::from_str::<serde_json::Value>(&BOOTED[BOOTED.find('{').unwrap()..]).unwrap();
        booted["status"]["booted"]["cachedUpdate"] = cached;
        assert_eq!(parse_status(&booted.to_string()).unwrap().available.as_deref(), Some("44.20261012.0"));
    }

    #[test]
    fn output_that_is_not_bootc_status_is_an_error() {
        assert!(parse_status("").is_err());
        assert!(parse_status("error: Not running on a bootc system").is_err());
        assert!(parse_status(r#"{"status": {}}"#).is_err(), "no booted system");
    }
}
