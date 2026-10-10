//! What the PC can do, and how the launcher asks it: the mode the launcher runs in, the power
//! actions it offers, and the system commands behind them. The UI reads this module; it never
//! checks the mode itself. See docs/plans/ps5-launcher-os.md, Phase 2.
#![cfg_attr(not(test), allow(dead_code))] // the Power menu and Quick Menu use it from Phase 3 on

/// Where the launcher runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// A window on a desktop (KDE, GNOME, macOS). The desktop owns power and settings.
    Desktop,
    /// The "PS5 Launcher" login session on a normal Linux PC: nothing behind the launcher.
    Session,
    /// PS5 Launcher OS: the whole PC.
    Os,
}

impl Mode {
    /// The mode from `PS5_LAUNCHER_SESSION` (set by ps5-launcher-session) and the contents of
    /// the OS marker file, /usr/lib/ps5-launcher/os-release. Without the session variable the
    /// launcher is a desktop app, even on the OS's own fallback desktop.
    pub fn detect(session_env: Option<&str>, os_marker: Option<&str>) -> Mode {
        if session_env != Some("1") {
            return Mode::Desktop;
        }
        match os_marker.and_then(marker_image) {
            Some("main" | "nvidia") => Mode::Os,
            _ => Mode::Session,
        }
    }

    /// The mode of this launcher.
    pub fn current() -> Mode {
        let env = std::env::var("PS5_LAUNCHER_SESSION").ok();
        let marker = std::fs::read_to_string(OS_MARKER).ok();
        Mode::detect(env.as_deref(), marker.as_deref())
    }
}

/// Written into PS5 Launcher OS images (packaging/os/): `IMAGE=main` or `IMAGE=nvidia`.
const OS_MARKER: &str = "/usr/lib/ps5-launcher/os-release";

/// The value of the marker's `IMAGE=` line, without surrounding spaces or double quotes.
fn marker_image(marker: &str) -> Option<&str> {
    let value = marker.lines().find_map(|line| line.trim().strip_prefix("IMAGE="))?;
    match value.strip_prefix('"') {
        Some(quoted) => quoted.strip_suffix('"'),
        None => Some(value),
    }
}

/// Answer of logind's CanSuspend, CanReboot and CanPowerOff.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Can {
    Yes,
    No,
    /// Allowed after a password prompt, which a controller cannot answer.
    Challenge,
    /// Not available on this PC.
    Na,
}

/// Parse a `busctl --system call org.freedesktop.login1 … CanSuspend` (or CanReboot,
/// CanPowerOff) answer, which busctl prints as `s "yes"`.
pub fn parse_logind_can(output: &str) -> Result<Can, String> {
    match output.trim() {
        "s \"yes\"" => Ok(Can::Yes),
        "s \"no\"" => Ok(Can::No),
        "s \"challenge\"" => Ok(Can::Challenge),
        "s \"na\"" => Ok(Can::Na),
        other => Err(format!("unexpected answer from logind: {other:?}")),
    }
}

/// Run a system command and wait for it. An error names the command and carries its exit
/// status and error output, for the UI to show. It blocks, so call it off the UI thread.
pub fn run(cmd: &mut std::process::Command) -> Result<(), String> {
    let name = format!("{cmd:?}");
    let out = cmd.output().map_err(|e| format!("could not run {name}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(format!("{name} failed ({}): {}", out.status, stderr.trim()))
}

/// A row of the Power menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PowerAction {
    CloseGame,
    CloseLauncher,
    Sleep,
    /// `update`: a staged OS update installs during the restart ("Update and restart").
    Restart { update: bool },
    PowerOff,
    LogOut,
    SwitchToDesktop,
}

/// What the system can do, for the Power menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PowerCaps {
    pub can_sleep: Can,
    /// A desktop session (Plasma) is installed to switch to.
    pub has_desktop: bool,
    /// An OS update waits for the next restart.
    pub update_staged: bool,
}

/// An install or a file copy that is running.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub percent: u8,
}

/// Work in progress that a power action can interrupt.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Work {
    /// The running game's name.
    pub game: Option<String>,
    pub jobs: Vec<Job>,
    /// Downloads continue at the next start, so they are never lost.
    pub downloads: usize,
}

/// The Power menu's rows, in order, for this mode, work and system.
pub fn power_actions(mode: Mode, work: &Work, caps: &PowerCaps) -> Vec<PowerAction> {
    let mut rows = Vec::new();
    if work.game.is_some() {
        rows.push(PowerAction::CloseGame);
    }
    if mode == Mode::Desktop {
        // The desktop owns sleep, restart, power off and log out.
        rows.push(PowerAction::CloseLauncher);
        return rows;
    }
    if caps.can_sleep == Can::Yes {
        rows.push(PowerAction::Sleep);
    }
    rows.push(PowerAction::Restart { update: mode == Mode::Os && caps.update_staged });
    rows.push(PowerAction::PowerOff);
    // In the OS, SDDM logs straight back in (Relogin), so Log out would only restart the launcher.
    if mode == Mode::Session {
        rows.push(PowerAction::LogOut);
    }
    if caps.has_desktop {
        rows.push(PowerAction::SwitchToDesktop);
    }
    rows
}

/// What a power action would lose.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Loss {
    /// Progress the player has not saved in this game.
    GameProgress(String),
    /// An install or file copy that would be left broken.
    Job(Job),
}

/// A button of the loss dialog.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Choice {
    Cancel,
    /// Wait for these jobs (by id), then power off. Work started later is not waited for.
    WhenDone(Vec<String>),
    /// Go ahead now and lose what the dialog lists.
    Now,
}

/// How the launcher confirms a power action before it runs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Guard {
    /// Nothing is lost: a countdown of this many seconds, which Back cancels.
    Countdown(u8),
    /// Something is lost: a dialog that lists it, with its buttons in this order.
    Dialog { losses: Vec<Loss>, choices: Vec<Choice> },
}

/// Seconds of the countdown when nothing is lost.
const COUNTDOWN_SECS: u8 = 3;

/// How to confirm `action` while `work` runs. Downloads never count: they continue at the next
/// start.
pub fn guard(action: PowerAction, work: &Work) -> Guard {
    let game = work.game.clone().map(Loss::GameProgress);
    let jobs = work.jobs.iter().cloned().map(Loss::Job);
    let losses: Vec<Loss> = match action {
        PowerAction::Sleep => Vec::new(),
        // The game is separate from the launcher's own work.
        PowerAction::CloseGame => game.into_iter().collect(),
        PowerAction::CloseLauncher => jobs.collect(),
        PowerAction::Restart { .. } | PowerAction::PowerOff | PowerAction::LogOut | PowerAction::SwitchToDesktop => {
            game.into_iter().chain(jobs).collect()
        }
    };
    if losses.is_empty() {
        return Guard::Countdown(COUNTDOWN_SECS);
    }
    let mut choices = vec![Choice::Cancel];
    if action == PowerAction::PowerOff && !work.jobs.is_empty() {
        choices.push(Choice::WhenDone(work.jobs.iter().map(|j| j.id.clone()).collect()));
    }
    choices.push(Choice::Now);
    Guard::Dialog { losses, choices }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PowerAction::*;

    #[test]
    fn mode_needs_the_session_variable() {
        assert_eq!(Mode::detect(None, None), Mode::Desktop);
        assert_eq!(Mode::detect(None, Some("IMAGE=main\n")), Mode::Desktop);
        assert_eq!(Mode::detect(Some("0"), Some("IMAGE=main\n")), Mode::Desktop);
        assert_eq!(Mode::detect(Some("yes"), None), Mode::Desktop);
    }

    #[test]
    fn mode_is_the_os_with_a_valid_marker() {
        assert_eq!(Mode::detect(Some("1"), None), Mode::Session);
        assert_eq!(Mode::detect(Some("1"), Some("IMAGE=main\n")), Mode::Os);
        assert_eq!(Mode::detect(Some("1"), Some("IMAGE=nvidia")), Mode::Os);
        assert_eq!(Mode::detect(Some("1"), Some("# PS5 Launcher OS\n IMAGE=\"nvidia\" \n")), Mode::Os);
    }

    #[test]
    fn a_broken_marker_means_the_session() {
        for marker in ["", "IMAGE=", "IMAGE=bazzite", "image=main", "IMAGE=main extra", "IMAGE=\"main"] {
            assert_eq!(Mode::detect(Some("1"), Some(marker)), Mode::Session, "{marker:?}");
        }
    }

    #[test]
    fn logind_answers() {
        assert_eq!(parse_logind_can("s \"yes\"\n"), Ok(Can::Yes));
        assert_eq!(parse_logind_can("s \"no\"\n"), Ok(Can::No));
        assert_eq!(parse_logind_can("s \"challenge\"\n"), Ok(Can::Challenge));
        assert_eq!(parse_logind_can("s \"na\""), Ok(Can::Na));
    }

    #[test]
    fn unknown_logind_answers_are_errors() {
        for output in ["", "yes", "s yes", "s \"maybe\"", "b true", "s \"yes\" extra"] {
            assert!(parse_logind_can(output).is_err(), "{output:?}");
        }
    }

    #[test]
    fn run_reports_the_exit_status() {
        use std::process::Command;
        assert_eq!(run(&mut Command::new("true")), Ok(()));
        let failed = run(Command::new("sh").args(["-c", "echo refused >&2; exit 3"])).unwrap_err();
        assert!(failed.contains("sh") && failed.contains('3') && failed.contains("refused"), "{failed}");
        let missing = run(&mut Command::new("ps5-launcher-no-such-program")).unwrap_err();
        assert!(missing.contains("ps5-launcher-no-such-program"), "{missing}");
    }

    fn installing() -> Job {
        Job { id: "astro".into(), name: "Astro Bot".into(), percent: 64 }
    }

    #[test]
    fn nothing_to_lose_counts_down() {
        let downloading = Work { downloads: 3, ..Work::default() };
        assert_eq!(guard(PowerOff, &downloading), Guard::Countdown(3));
        assert_eq!(guard(Restart { update: true }, &Work::default()), Guard::Countdown(3));
    }

    #[test]
    fn sleep_loses_nothing() {
        let busy = Work { jobs: vec![installing()], ..playing() };
        assert_eq!(guard(Sleep, &busy), Guard::Countdown(3));
    }

    #[test]
    fn power_off_during_an_install_can_wait_for_it() {
        let busy = Work { jobs: vec![installing()], ..Work::default() };
        assert_eq!(
            guard(PowerOff, &busy),
            Guard::Dialog {
                losses: vec![Loss::Job(installing())],
                choices: vec![Choice::Cancel, Choice::WhenDone(vec!["astro".into()]), Choice::Now],
            }
        );
    }

    #[test]
    fn only_power_off_waits_for_installs() {
        let busy = Work { jobs: vec![installing()], ..Work::default() };
        for action in [Restart { update: false }, LogOut, SwitchToDesktop, CloseLauncher] {
            assert_eq!(
                guard(action, &busy),
                Guard::Dialog { losses: vec![Loss::Job(installing())], choices: vec![Choice::Cancel, Choice::Now] },
                "{action:?}"
            );
        }
    }

    #[test]
    fn power_off_lists_the_game_first_and_cannot_wait_for_it() {
        let busy = Work { jobs: vec![installing()], ..playing() };
        assert_eq!(
            guard(PowerOff, &busy),
            Guard::Dialog {
                losses: vec![Loss::GameProgress("Dreaming Sarah".into()), Loss::Job(installing())],
                choices: vec![Choice::Cancel, Choice::WhenDone(vec!["astro".into()]), Choice::Now],
            }
        );
    }

    #[test]
    fn closing_a_game_mentions_only_the_game() {
        let busy = Work { jobs: vec![installing()], ..playing() };
        assert_eq!(
            guard(CloseGame, &busy),
            Guard::Dialog {
                losses: vec![Loss::GameProgress("Dreaming Sarah".into())],
                choices: vec![Choice::Cancel, Choice::Now],
            }
        );
    }

    fn caps() -> PowerCaps {
        PowerCaps { can_sleep: Can::Yes, has_desktop: true, update_staged: false }
    }

    fn playing() -> Work {
        Work { game: Some("Dreaming Sarah".into()), ..Work::default() }
    }

    #[test]
    fn desktop_offers_only_closing() {
        assert_eq!(power_actions(Mode::Desktop, &Work::default(), &caps()), vec![CloseLauncher]);
        assert_eq!(power_actions(Mode::Desktop, &playing(), &caps()), vec![CloseGame, CloseLauncher]);
    }

    #[test]
    fn session_offers_power_log_out_and_desktop() {
        assert_eq!(
            power_actions(Mode::Session, &Work::default(), &caps()),
            vec![Sleep, Restart { update: false }, PowerOff, LogOut, SwitchToDesktop]
        );
    }

    #[test]
    fn os_hides_log_out_and_names_the_update() {
        let staged = PowerCaps { update_staged: true, ..caps() };
        assert_eq!(
            power_actions(Mode::Os, &playing(), &staged),
            vec![CloseGame, Sleep, Restart { update: true }, PowerOff, SwitchToDesktop]
        );
    }

    #[test]
    fn a_staged_update_changes_restart_only_in_the_os() {
        let staged = PowerCaps { update_staged: true, ..caps() };
        assert!(power_actions(Mode::Session, &Work::default(), &staged).contains(&Restart { update: false }));
    }

    #[test]
    fn sleep_needs_a_plain_yes() {
        for can in [Can::No, Can::Challenge, Can::Na] {
            let c = PowerCaps { can_sleep: can, ..caps() };
            assert!(!power_actions(Mode::Os, &Work::default(), &c).contains(&Sleep), "{can:?}");
        }
    }

    #[test]
    fn switch_to_desktop_needs_a_desktop() {
        let c = PowerCaps { has_desktop: false, ..caps() };
        assert_eq!(
            power_actions(Mode::Os, &Work::default(), &c),
            vec![Sleep, Restart { update: false }, PowerOff]
        );
    }
}
