//! The boot health check's notice in PS5 Launcher OS (packaging/os/README.md, "The boot health
//! check"): what happened at a start that failed, in plain words. The check writes
//! `notice.json`; the launcher shows it once and deletes it with `helper health-ack`.

use crate::osupdate::{self, Task};
use crate::system::Call;
use serde::Deserialize;
use std::path::Path;

/// Written by /usr/libexec/ps5-launcher-os/boot-health, mode 0644.
pub const PATH: &str = "/var/lib/ps5-launcher-os/health/notice.json";

/// How the check ended.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// The start failed, and the check did not roll back (the reason says why).
    Unhealthy,
    /// The start failed: the check rolled back and restarts the PC.
    RollingBack,
    /// The previous system started after the rollback, and works.
    RolledBack,
    /// The previous system, after the rollback, failed too.
    RollbackUnhealthy,
    /// A rollback was started once and did not finish; it is not tried again.
    RollbackInterrupted,
    /// The rollback could not be queued: the PC stays on the failed system.
    RollbackFailed,
}

#[derive(Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Notice {
    pub version: u32,
    pub outcome: Outcome,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub failed_digest: String,
    #[serde(default)]
    pub destination_digest: String,
}

/// The file's text, read. A file that is not a version-1 notice is an error.
pub fn parse(text: &str) -> Result<Notice, String> {
    let notice: Notice = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if notice.version != 1 {
        return Err(format!("notice version {} is not 1", notice.version));
    }
    Ok(notice)
}

/// What the dialog shows: a title, the text, and one short line of details.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Text {
    pub title: String,
    pub lines: Vec<String>,
    pub details: String,
}

/// Every text ends with this.
const SAFE: &str = "Your games and saves are safe.";
/// How to start an older system without the launcher. The GRUB menu shows for 5 seconds
/// (packaging/os/README.md, "Recovery mode"); bootc's second entry is the other system.
const BOOT_MENU: &str =
    "Choose an older system in the boot menu: restart the PC, and while the menu shows (5 seconds), pick the second entry with the arrow keys of a USB keyboard.";
/// The longest Details line, in characters.
const DETAILS_MAX: usize = 140;

/// The dialog's text for `notice`. No digests, apart from short ones on the Details line.
pub fn text(notice: &Notice) -> Text {
    let update = "The last system update didn't start properly";
    let (title, first, second): (&str, String, Option<&str>) = match notice.outcome {
        Outcome::Unhealthy => (
            "The system didn't start properly",
            format!("PS5 Launcher OS found a problem at this start, and it did not go back to the previous version by itself. {SAFE}"),
            Some("If something doesn't work, restart the PC. If the problem stays, choose an older system in the boot menu when the PC starts."),
        ),
        Outcome::RollingBack => (
            "Going back to the previous version",
            format!("{update}, so PS5 Launcher OS is going back to the previous version. {SAFE}"),
            Some("The PC restarts by itself. If it doesn't, restart it from the Power menu."),
        ),
        Outcome::RolledBack => ("Back on the previous version", format!("{update}, so PS5 Launcher OS went back to the previous version. {SAFE}"), None),
        Outcome::RollbackUnhealthy => (
            "The previous version has a problem too",
            format!("{update}, so PS5 Launcher OS went back to the previous version, but that one didn't start properly either. {SAFE}"),
            Some("Restart the PC. If the problem stays, choose an older system in the boot menu when the PC starts."),
        ),
        Outcome::RollbackInterrupted => (
            "Going back was interrupted",
            format!("PS5 Launcher OS started to go back to the previous version, but the PC stopped before it finished. It does not try again by itself. {SAFE}"),
            Some("If something doesn't work, choose an older system in the boot menu when the PC starts."),
        ),
        Outcome::RollbackFailed => (
            "The system could not go back by itself",
            format!("{update}, and PS5 Launcher OS could not go back to the previous version by itself. {SAFE}"),
            Some(BOOT_MENU),
        ),
    };
    let mut lines = vec![first];
    lines.extend(second.map(String::from));
    Text { title: title.into(), lines, details: details(notice) }
}

/// "Details: the reason · failed → destination", cut to `DETAILS_MAX` characters.
fn details(notice: &Notice) -> String {
    let what = if notice.reason.is_empty() { &notice.action } else { &notice.reason };
    let mut line = format!("Details: {what}");
    if !notice.failed_digest.is_empty() && !notice.destination_digest.is_empty() {
        line += &format!(" · {} → {}", osupdate::short_digest(&notice.failed_digest), osupdate::short_digest(&notice.destination_digest));
    }
    cut(line)
}

fn cut(line: String) -> String {
    if line.chars().count() <= DETAILS_MAX {
        return line;
    }
    let mut short: String = line.chars().take(DETAILS_MAX - 1).collect();
    short.push('…');
    short
}

/// The dialog for a file the launcher cannot read.
pub fn unreadable(error: &str) -> Text {
    Text {
        title: "The system check left a note".into(),
        lines: vec![format!("PS5 Launcher OS checked the last start, but the launcher can't read what it found. {SAFE}")],
        details: cut(format!("Details: {error}")),
    }
}

/// The notice at `path`: None when there is none, an error when it cannot be read.
pub fn read(path: &Path) -> Option<Result<Notice, String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(parse(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(Err(e.to_string())),
    }
}

/// `pkexec helper health-ack`: delete the notice once the player saw it.
pub fn ack_call() -> Call {
    osupdate::helper_call(Task::HealthAck)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAILED: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
    const GOOD: &str = "sha256:2222222222222222222222222222222222222222222222222222222222222222";

    fn file(outcome: &str, reason: &str, action: &str, digests: bool) -> String {
        let (failed, dest) = if digests { (FAILED, GOOD) } else { ("", "") };
        format!(
            r#"{{"version": 1, "time": "2026-10-10T08:00:00Z", "outcome": "{outcome}", "reason": "{reason}", "action": "{action}",
               "booted_digest": "{GOOD}", "failed_digest": "{failed}", "destination_digest": "{dest}", "last_good_digest": "{GOOD}"}}"#
        )
    }

    #[test]
    fn notice_parses_every_outcome() {
        for (name, outcome) in [
            ("unhealthy", Outcome::Unhealthy),
            ("rolling-back", Outcome::RollingBack),
            ("rolled-back", Outcome::RolledBack),
            ("rollback-unhealthy", Outcome::RollbackUnhealthy),
            ("rollback-interrupted", Outcome::RollbackInterrupted),
            ("rollback-failed", Outcome::RollbackFailed),
        ] {
            let n = parse(&file(name, "gamescope is not running", "x", true)).unwrap();
            assert_eq!(n.outcome, outcome, "{name}");
            assert_eq!(n.reason, "gamescope is not running");
            assert_eq!(n.failed_digest, FAILED);
        }
    }

    #[test]
    fn notice_a_bad_file_is_an_error() {
        assert!(parse("").is_err());
        assert!(parse("not json").is_err());
        assert!(parse("[]").is_err());
        assert!(parse(r#"{"version": 1}"#).is_err(), "no outcome");
        assert!(parse(r#"{"version": 1, "outcome": "exploded"}"#).is_err(), "an outcome the launcher does not know");
        assert!(parse(r#"{"version": 2, "outcome": "rolled-back"}"#).is_err(), "a newer format");
        assert!(parse(r#"{"outcome": "rolled-back"}"#).is_err(), "no version");
        let n = parse(r#"{"version": 1, "outcome": "unhealthy"}"#).unwrap();
        assert_eq!((n.reason.as_str(), n.failed_digest.as_str()), ("", ""), "the rest may be missing");
        let t = unreadable("expected value at line 1 column 1");
        assert_eq!(t.title, "The system check left a note");
        assert_eq!(t.lines, ["PS5 Launcher OS checked the last start, but the launcher can't read what it found. Your games and saves are safe."]);
        assert_eq!(t.details, "Details: expected value at line 1 column 1");
    }

    #[test]
    fn notice_texts_for_every_outcome() {
        let t = |outcome: &str| text(&parse(&file(outcome, "the launcher did not answer", "x", true)).unwrap());
        let rolled = t("rolled-back");
        assert_eq!(rolled.title, "Back on the previous version");
        assert_eq!(rolled.lines, ["The last system update didn't start properly, so PS5 Launcher OS went back to the previous version. Your games and saves are safe."]);
        let failed = t("rollback-failed");
        assert_eq!(failed.title, "The system could not go back by itself");
        assert_eq!(failed.lines[0], "The last system update didn't start properly, and PS5 Launcher OS could not go back to the previous version by itself. Your games and saves are safe.");
        assert!(failed.lines[1].starts_with("Choose an older system in the boot menu"), "{:?}", failed.lines);
        let rolling = t("rolling-back");
        assert_eq!(rolling.title, "Going back to the previous version");
        assert!(rolling.lines[1].contains("restart it from the Power menu"));
        let again = t("rollback-unhealthy");
        assert_eq!(again.title, "The previous version has a problem too");
        assert!(again.lines[1].contains("boot menu"));
        let stopped = t("rollback-interrupted");
        assert_eq!(stopped.title, "Going back was interrupted");
        assert!(stopped.lines[0].contains("does not try again by itself"));
        let unhealthy = t("unhealthy");
        assert_eq!(unhealthy.title, "The system didn't start properly");
        assert!(unhealthy.lines[0].contains("did not go back to the previous version by itself"));
        for o in ["unhealthy", "rolling-back", "rolled-back", "rollback-unhealthy", "rollback-interrupted", "rollback-failed"] {
            let x = t(o);
            assert!(x.lines[0].ends_with("Your games and saves are safe."), "{o}");
            assert!(!x.lines.iter().any(|l| l.contains("sha256")), "no digests in the text: {o}");
        }
    }

    #[test]
    fn notice_details_are_one_short_line() {
        let n = parse(&file("rolled-back", "the launcher did not answer", "the previous system started and works", true)).unwrap();
        assert_eq!(text(&n).details, "Details: the launcher did not answer · 111111111111 → 222222222222");
        let n = parse(&file("unhealthy", "gamescope is not running", "none: not the first start of this system", false)).unwrap();
        assert_eq!(text(&n).details, "Details: gamescope is not running");
        let n = parse(&file("unhealthy", "", "none: bootc status is unknown", false)).unwrap();
        assert_eq!(text(&n).details, "Details: none: bootc status is unknown", "the action when there is no reason");
        let long = "x".repeat(400);
        let n = parse(&file("unhealthy", &long, "", false)).unwrap();
        let d = text(&n).details;
        assert!(d.chars().count() <= 140 && d.ends_with('…'), "{d}");
    }

    #[test]
    fn notice_is_read_from_its_file_and_acknowledged_by_the_helper() {
        let dir = std::env::temp_dir().join(format!("ps5l-notice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notice.json");
        assert_eq!(read(&path), None, "no file: nothing to show");
        std::fs::write(&path, file("rolled-back", "r", "a", true)).unwrap();
        assert_eq!(read(&path).unwrap().unwrap().outcome, Outcome::RolledBack);
        std::fs::write(&path, "{").unwrap();
        assert!(read(&path).unwrap().is_err());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(ack_call().args, [osupdate::HELPER, "health-ack"]);
        assert_eq!(ack_call().program, "pkexec");
        assert!(ack_call().secs <= 60, "the helper only deletes a file");
    }
}
