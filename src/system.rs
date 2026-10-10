//! What the PC can do, and how the launcher asks it: the mode the launcher runs in, the power
//! actions it offers, and the system commands behind them. The UI reads this module; it never
//! checks the mode itself. See docs/plans/ps5-launcher-os.md, Phase 2.

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

/// Exit code that tells ps5-launcher-session to start the launcher again at once (Restart
/// launcher). It does not count as a crash. Exit 0 ends the session.
pub const EXIT_RESTART_LAUNCHER: i32 = 75;

/// The code the process exits with once the event loop ends; 0 unless set.
static EXIT_CODE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Exit with `code` at the next clean quit (`slint::quit_event_loop`).
pub fn set_exit_code(code: i32) {
    EXIT_CODE.store(code, std::sync::atomic::Ordering::Relaxed);
}

pub fn exit_code() -> i32 {
    EXIT_CODE.load(std::sync::atomic::Ordering::Relaxed)
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
    run_output(cmd).map(|_| ())
}

/// Same as `run`, and returns what the command printed on its standard output.
pub fn run_output(cmd: &mut std::process::Command) -> Result<String, String> {
    let name = format!("{cmd:?}");
    let out = cmd.output().map_err(|e| format!("could not run {name}: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(format!("{name} failed ({}): {}", out.status, stderr.trim()))
}

/// A system tool's command line, as the backends (network, sound, storage, osupdate) build it,
/// so tests can check every argument. It runs under coreutils' `timeout`: a tool can wait
/// forever (bluetoothctl did on Fedora 44), so every call gets `secs` seconds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Call {
    pub program: &'static str,
    pub args: Vec<String>,
    pub secs: u32,
}

/// `timeout`'s exit code when the time ran out.
const TIMED_OUT: i32 = 124;

impl Call {
    pub fn new(program: &'static str, args: &[&str], secs: u32) -> Call {
        Call { program, args: args.iter().map(|a| a.to_string()).collect(), secs }
    }

    /// The command: SIGTERM after `secs`, SIGKILL 5 seconds later if the tool ignores it.
    pub fn command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new("timeout");
        cmd.args(["-k", "5", &self.secs.to_string(), self.program]).args(&self.args);
        cmd
    }

    /// The command line for errors and the log. A password never shows.
    pub fn shown(&self) -> String {
        let mut words = vec![self.program.to_string()];
        let mut hide = false;
        for arg in &self.args {
            words.push(if hide { "••••".to_string() } else { arg.clone() });
            hide = arg == "password";
        }
        words.join(" ")
    }
}

/// How a call ended. `code` is None when a signal stopped it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ran {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    /// The error for a call that did not exit with 0: the tool's own message.
    pub fn error(&self, call: &Call) -> String {
        let said = if self.stderr.trim().is_empty() { self.stdout.trim() } else { self.stderr.trim() };
        let code = self.code.map(|c| format!("exit {c}")).unwrap_or_else(|| "stopped".into());
        format!("{} failed ({code}): {said}", call.shown())
    }
}

/// A finished call, or an error when its time ran out.
fn finished(call: &Call, ran: Ran) -> Result<Ran, String> {
    if ran.code == Some(TIMED_OUT) {
        return Err(format!("{} did not answer within {} s", call.shown(), call.secs));
    }
    Ok(ran)
}

/// Run `call` and wait for it, at most its time. Any exit code is Ok; an error means the tool
/// could not start or did not finish. It blocks, so call it off the UI thread.
pub fn call_status(call: &Call) -> Result<Ran, String> {
    let out = call.command().output().map_err(|e| format!("could not run {}: {e}", call.shown()))?;
    let ran = Ran {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    };
    finished(call, ran)
}

/// `run_output` with the call's time limit: what the tool printed, when it exited with 0.
pub fn call(call: &Call) -> Result<String, String> {
    let ran = call_status(call)?;
    if ran.code == Some(0) { Ok(ran.stdout) } else { Err(ran.error(call)) }
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

impl PowerAction {
    /// The row's label in the Power menu.
    pub fn label(self) -> &'static str {
        match self {
            PowerAction::CloseGame => "Close game",
            PowerAction::CloseLauncher => "Close launcher",
            PowerAction::Sleep => "Sleep",
            PowerAction::Restart { update: false } => "Restart",
            PowerAction::Restart { update: true } => "Update and restart",
            PowerAction::PowerOff => "Power off",
            PowerAction::LogOut => "Log out",
            PowerAction::SwitchToDesktop => "Switch to desktop",
        }
    }

    /// The countdown's text before the seconds: "Powering off in" 3….
    pub fn counting(self) -> &'static str {
        match self {
            PowerAction::CloseGame => "Closing the game in",
            PowerAction::CloseLauncher => "Closing the launcher in",
            PowerAction::Sleep => "Going to sleep in",
            PowerAction::Restart { update: false } => "Restarting in",
            PowerAction::Restart { update: true } => "Restarting to update in",
            PowerAction::PowerOff => "Powering off in",
            PowerAction::LogOut => "Logging out in",
            PowerAction::SwitchToDesktop => "Switching to desktop in",
        }
    }
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
pub const COUNTDOWN_SECS: u8 = 3;

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

/// A button's label in the loss dialog for `action`.
pub fn choice_label(action: PowerAction, choice: &Choice) -> String {
    match choice {
        Choice::Cancel => "Cancel".into(),
        Choice::WhenDone(_) => format!("{} when done", action.label()),
        // "Close game now" reads oddly next to the game's own warning.
        Choice::Now if action == PowerAction::CloseGame => action.label().into(),
        Choice::Now => format!("{} now", action.label()),
    }
}

/// A line of the loss dialog.
pub fn loss_text(loss: &Loss) -> String {
    match loss {
        Loss::GameProgress(game) => format!("{game} is still running. Progress you have not saved is lost."),
        Loss::Job(job) => format!("Installing {} ({}%). Stopping now leaves a broken install.", job.name, job.percent),
    }
}

/// How a job that "Power off when done" waits for stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobEnd {
    Running,
    Done,
    /// Failed or cancelled.
    Failed,
}

/// Where "Power off when done" stands.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WaitState {
    /// These jobs (by name) still run.
    Waiting(Vec<String>),
    /// Every job finished: power off now.
    Ready,
    /// This job (by name) did not finish: the PC stays on.
    Failed(String),
}

/// Check the jobs that "Power off when done" waits for (`waited`, by id) against the jobs
/// there are now (id, name, state). A job that left the list did not finish. Jobs started
/// later do not count.
pub fn when_done(waited: &[String], jobs: &[(String, String, JobEnd)]) -> WaitState {
    let mut running = Vec::new();
    for id in waited {
        match jobs.iter().find(|(job, _, _)| job == id) {
            None => return WaitState::Failed(id.clone()),
            Some((_, name, JobEnd::Failed)) => return WaitState::Failed(name.clone()),
            Some((_, name, JobEnd::Running)) => running.push(name.clone()),
            Some((_, _, JobEnd::Done)) => {}
        }
    }
    if running.is_empty() { WaitState::Ready } else { WaitState::Waiting(running) }
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

    #[test]
    fn run_output_returns_what_the_command_prints() {
        use std::process::Command;
        assert_eq!(run_output(Command::new("sh").args(["-c", "echo 's \"yes\"'"])), Ok("s \"yes\"\n".into()));
        let failed = run_output(Command::new("sh").args(["-c", "echo half; echo refused >&2; exit 3"])).unwrap_err();
        assert!(failed.contains('3') && failed.contains("refused"), "{failed}");
        assert!(run_output(&mut Command::new("ps5-launcher-no-such-program")).is_err());
    }

    #[test]
    fn a_call_runs_under_timeout() {
        let call = Call::new("nmcli", &["radio", "wifi"], 10);
        let cmd = call.command();
        assert_eq!(cmd.get_program(), "timeout");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, ["-k", "5", "10", "nmcli", "radio", "wifi"]);
    }

    #[test]
    fn a_password_never_shows() {
        let call = Call::new("nmcli", &["device", "wifi", "connect", "Home", "password", "hunter22"], 60);
        assert_eq!(call.shown(), "nmcli device wifi connect Home password ••••");
        let ran = Ran { code: Some(4), stdout: String::new(), stderr: "Error: Secrets were required.\n".into() };
        let error = ran.error(&call);
        assert!(!error.contains("hunter22"), "{error}");
        assert_eq!(error, "nmcli device wifi connect Home password •••• failed (exit 4): Error: Secrets were required.");
    }

    #[test]
    fn an_error_carries_what_the_tool_said() {
        let call = Call::new("helper", &["update"], 60);
        let said_on_stdout = Ran { code: Some(3), stdout: "key-required\n".into(), stderr: "  ".into() };
        assert_eq!(said_on_stdout.error(&call), "helper update failed (exit 3): key-required");
        let killed = Ran { code: None, stdout: String::new(), stderr: String::new() };
        assert_eq!(killed.error(&call), "helper update failed (stopped): ");
    }

    #[test]
    fn running_out_of_time_is_an_error() {
        let call = Call::new("bluetoothctl", &["list"], 10);
        let ran = |code| Ran { code: Some(code), stdout: String::new(), stderr: String::new() };
        assert_eq!(finished(&call, ran(124)), Err("bluetoothctl list did not answer within 10 s".into()));
        assert_eq!(finished(&call, ran(3)), Ok(ran(3)));
    }

    // macOS has no `timeout`.
    #[cfg(target_os = "linux")]
    #[test]
    fn calls_run_and_stop_in_time() {
        assert_eq!(call(&Call::new("sh", &["-c", "echo hi"], 5)), Ok("hi\n".into()));
        let failed = call(&Call::new("sh", &["-c", "echo refused >&2; exit 3"], 5)).unwrap_err();
        assert!(failed.contains("exit 3") && failed.contains("refused"), "{failed}");
        assert_eq!(call_status(&Call::new("sh", &["-c", "exit 4"], 5)).unwrap().code, Some(4));
        let slow = call(&Call::new("sleep", &["5"], 1)).unwrap_err();
        assert!(slow.contains("did not answer within 1 s"), "{slow}");
    }

    fn end(id: &str, state: JobEnd) -> (String, String, JobEnd) {
        (id.into(), format!("{id} game"), state)
    }

    #[test]
    fn waiting_lasts_while_a_job_runs() {
        let waited = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            when_done(&waited, &[end("a", JobEnd::Done), end("b", JobEnd::Running)]),
            WaitState::Waiting(vec!["b game".into()])
        );
    }

    #[test]
    fn waiting_ends_when_every_job_is_done() {
        let waited = vec!["a".to_string(), "b".to_string()];
        assert_eq!(when_done(&waited, &[end("a", JobEnd::Done), end("b", JobEnd::Done)]), WaitState::Ready);
    }

    #[test]
    fn waiting_ignores_work_started_later() {
        let waited = vec!["a".to_string()];
        assert_eq!(when_done(&waited, &[end("a", JobEnd::Done), end("later", JobEnd::Running)]), WaitState::Ready);
    }

    #[test]
    fn a_failed_or_missing_job_stops_the_wait() {
        let waited = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            when_done(&waited, &[end("a", JobEnd::Running), end("b", JobEnd::Failed)]),
            WaitState::Failed("b game".into())
        );
        // A job that left the list (removed from Downloads) did not finish.
        assert_eq!(when_done(&waited, &[end("a", JobEnd::Done)]), WaitState::Failed("b".into()));
    }

    #[test]
    fn labels_name_each_action() {
        assert_eq!(PowerOff.label(), "Power off");
        assert_eq!(Restart { update: true }.label(), "Update and restart");
        assert_eq!(Restart { update: false }.label(), "Restart");
        assert_eq!(SwitchToDesktop.label(), "Switch to desktop");
        assert_eq!(PowerOff.counting(), "Powering off in");
        assert_eq!(Sleep.counting(), "Going to sleep in");
    }

    #[test]
    fn choice_labels_follow_the_action() {
        assert_eq!(choice_label(PowerOff, &Choice::Cancel), "Cancel");
        assert_eq!(choice_label(PowerOff, &Choice::WhenDone(vec!["a".into()])), "Power off when done");
        assert_eq!(choice_label(PowerOff, &Choice::Now), "Power off now");
        assert_eq!(choice_label(Restart { update: false }, &Choice::Now), "Restart now");
        assert_eq!(choice_label(CloseGame, &Choice::Now), "Close game");
    }

    #[test]
    fn losses_say_what_is_lost() {
        assert_eq!(
            loss_text(&Loss::GameProgress("Dreaming Sarah".into())),
            "Dreaming Sarah is still running. Progress you have not saved is lost."
        );
        assert_eq!(loss_text(&Loss::Job(installing())), "Installing Astro Bot (64%). Stopping now leaves a broken install.");
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
