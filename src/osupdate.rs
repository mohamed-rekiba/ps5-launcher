//! PS5 Launcher OS's own state from `bootc status --json`: what runs now, whether an update waits
//! for the next restart, and whether there is a system to roll back to (Phase 6 of
//! docs/plans/ps5-launcher-os.md). Parsing, the command lines and the root helper's answers live
//! here; `system::call` runs them.

use crate::system::Image;
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

/// The launcher waits this much longer than the helper's own deadline for a task (packaging/os:
/// every task runs under `timeout` as root). The user cannot stop the helper once it is root, so
/// the helper's deadline must fire first, and the answer then says what really happened.
const HELPER_MARGIN: u32 = 30;
/// The same for the tasks that may take two hours, where the helper's SIGKILL comes later.
const LONG_HELPER_MARGIN: u32 = 5 * 60;

/// bootc status needs root on a booted system, so it goes through the helper (30 s).
pub fn status_call() -> Call {
    Call::new("pkexec", &[HELPER, "status"], 30 + HELPER_MARGIN)
}

/// A task of the root helper, through pkexec (polkit allows it for the user at the PC).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Task {
    UpdateCheck,
    Update,
    Rollback,
    /// Follow the main image (next restart).
    Switch(Image),
    /// Delete the boot health check's notice: the launcher showed it.
    HealthAck,
}

/// A helper task's call. Its time is the helper's own deadline for the task, plus a margin.
pub fn helper_call(task: Task) -> Call {
    // The helper's steps run one after the other, each under its own deadline: the shared lock
    // 30 s, skopeo 120 s, a download 2 hours.
    let (args, secs): (&[&str], u32) = match task {
        Task::UpdateCheck => (&["update-check"], 120 + HELPER_MARGIN),
        // Downloads the new system image: it may take a long time on a slow line.
        Task::Update => (&["update"], 2 * 60 * 60 + LONG_HELPER_MARGIN),
        Task::Rollback => (&["rollback"], 120 + HELPER_MARGIN),
        Task::Switch(Image::Main) => (&["switch", "main"], 30 + 2 * 60 * 60 + LONG_HELPER_MARGIN),
        // The helper only deletes a file.
        Task::HealthAck => (&["health-ack"], HELPER_MARGIN),
    };
    let mut all = vec![HELPER];
    all.extend_from_slice(args);
    Call::new("pkexec", &all, secs)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum UpdateEnd {
    Staged,
    Failed(String),
}

pub fn update_flow(run: &dyn Fn(&Call) -> Result<Ran, String>) -> UpdateEnd {
    let call = helper_call(Task::Update);
    match run(&call) {
        Ok(ran) if ran.code == Some(0) => UpdateEnd::Staged,
        Ok(ran) => UpdateEnd::Failed(ran.error(&call)),
        Err(e) => UpdateEnd::Failed(e),
    }
}

/// An optional digest from update-check; bootc status normally reports the version.
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
    use crate::system::Image;

    fn ran(code: i32, stdout: &str) -> Ran {
        Ran {
            code: Some(code),
            stdout: stdout.into(),
            stderr: String::new(),
        }
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
    fn helper_command_lines() {
        assert_eq!(helper_call(Task::UpdateCheck).args, [HELPER, "update-check"]);
        assert_eq!(helper_call(Task::Update).args, [HELPER, "update"]);
        assert_eq!(helper_call(Task::Rollback).args, [HELPER, "rollback"]);
        assert_eq!(
            helper_call(Task::Switch(Image::Main)).args,
            [HELPER, "switch", "main"]
        );
        assert_eq!(helper_call(Task::Update).program, "pkexec");
        assert!(
            helper_call(Task::Update).secs >= 3600,
            "a download takes time"
        );
    }

    #[test]
    fn the_helper_stops_itself_before_the_launcher_gives_up() {
        // The user cannot stop the helper once it is root, so the helper's own deadline must
        // fire first. These are the helper's deadlines.
        let helper = [
            (Task::UpdateCheck, 120),
            (Task::Update, 2 * 60 * 60),
            (Task::Rollback, 120),
            // The lock (30 s), then bootc switch.
            (Task::Switch(Image::Main), 30 + 2 * 60 * 60),
        ];
        for (task, secs) in helper {
            assert!(helper_call(task).secs > secs, "{task:?}");
            assert_eq!(helper_call(task).deadline(), std::time::Duration::from_secs(helper_call(task).secs.into()), "{task:?}");
        }
        assert!(status_call().secs > 30);
    }

    #[test]
    fn status_is_read_through_the_root_helper() {
        // bootc status takes root on a booted system (prepare_for_write), so the user asks the
        // helper, which polkit allows without a password.
        assert_eq!(status_call().program, "pkexec");
        assert_eq!(status_call().args, [HELPER, "status"]);
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
