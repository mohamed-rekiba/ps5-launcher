//! What the PC can do, and how the launcher asks it: the mode the launcher runs in, the power
//! actions it offers, and the system commands behind them. The UI reads this module; it never
//! checks the mode itself. See docs/plans/ps5-launcher-os.md, Phase 2.

use std::time::{Duration, Instant};

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

/// The Fedora OS image named in the system marker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Image {
    Main,
}

impl Image {
    pub fn parse(name: &str) -> Option<Self> {
        (name == "main").then_some(Self::Main)
    }
}

fn image_of(marker: &str) -> Option<Image> {
    marker_image(marker).and_then(Image::parse)
}

/// The image this PC runs, in PS5 Launcher OS; None in the other modes.
pub fn os_image() -> Option<Image> {
    if Mode::current() != Mode::Os {
        return None;
    }
    image_of(&std::fs::read_to_string(OS_MARKER).ok()?)
}

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

/// A password or another secret for a tool's standard input. Command-line arguments show in
/// /proc to every process; stdin does not. It never prints: Debug shows dots, and there is no
/// Display.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(secret: &str) -> Secret {
        Secret(secret.to_string())
    }

    /// `text` with every copy of the secret replaced by dots.
    fn hide(&self, text: String) -> String {
        if self.0.is_empty() || !text.contains(&self.0) { text } else { text.replace(&self.0, "••••") }
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(••••)")
    }
}

/// A system tool's command line, as the backends (network, sound, storage, osupdate) build it,
/// so tests can check every argument. A tool can wait forever (bluetoothctl did on Fedora 44), so
/// every call gets `secs` seconds: a user tool runs under coreutils' `timeout`, with the
/// launcher's own deadline as a backstop. A root helper call (pkexec) does not: once pkexec made
/// it root, the user cannot stop it, so the helper keeps its own deadlines and `secs` is a little
/// longer than the helper's.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Call {
    pub program: &'static str,
    pub args: Vec<String>,
    pub secs: u32,
    /// Written to the tool's standard input as one line, then stdin closes. Without it the tool's
    /// stdin is empty.
    pub stdin: Option<Secret>,
}

/// `timeout`'s exit code when the time ran out.
const TIMED_OUT: i32 = 124;

/// `timeout -k`: seconds between SIGTERM and SIGKILL.
const KILL_AFTER: u64 = 5;

/// How long a call waits for the output of a tool that exited, when a process the tool started
/// still holds its output open.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

impl Call {
    pub fn new(program: &'static str, args: &[&str], secs: u32) -> Call {
        Call { program, args: args.iter().map(|a| a.to_string()).collect(), secs, stdin: None }
    }

    /// The same call, with `secret` on its standard input.
    pub fn with_stdin(mut self, secret: &str) -> Call {
        self.stdin = Some(Secret::new(secret));
        self
    }

    /// pkexec runs the tool as root.
    fn privileged(&self) -> bool {
        self.program == "pkexec"
    }

    /// The command. A user tool gets SIGTERM after `secs`, and SIGKILL 5 seconds later if it
    /// ignores it. A root helper call runs as it is.
    pub fn command(&self) -> std::process::Command {
        if self.privileged() {
            let mut cmd = std::process::Command::new(self.program);
            cmd.args(&self.args);
            return cmd;
        }
        let mut cmd = std::process::Command::new("timeout");
        cmd.args(["-k", &KILL_AFTER.to_string(), &self.secs.to_string(), self.program]).args(&self.args);
        cmd
    }

    /// How long the launcher waits for the call before it stops it: `secs`, and for a user
    /// tool, also the time `timeout` takes to stop it.
    pub fn deadline(&self) -> Duration {
        let secs = u64::from(self.secs);
        Duration::from_secs(if self.privileged() { secs } else { secs + KILL_AFTER + 5 })
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

/// Run `call` and wait for it, at most its deadline. Any exit code is Ok; an error means the
/// tool could not start or did not finish. The secret never shows in what it returns. It
/// blocks, so call it off the UI thread.
pub fn call_status(call: &Call) -> Result<Ran, String> {
    let end = run_until(call.command(), call.stdin.as_ref(), call.deadline(), OUTPUT_GRACE)
        .map_err(|e| format!("could not run {}: {e}", call.shown()))?;
    match end {
        End::Exited(ran) => {
            let ran = match &call.stdin {
                Some(secret) => Ran { stdout: secret.hide(ran.stdout), stderr: secret.hide(ran.stderr), ..ran },
                None => ran,
            };
            finished(call, ran)
        }
        End::TimedOut => Err(format!("{} did not answer within {} s", call.shown(), call.secs)),
    }
}

/// How `run_until` ended.
#[derive(PartialEq, Eq, Debug)]
enum End {
    Exited(Ran),
    /// The deadline passed. The process group was told to stop, but a process that became root
    /// may still run.
    TimedOut,
}

/// Run `cmd` in a process group of its own, with `secret` (and a line end) on its stdin, and
/// wait for it at most `deadline`. Its output is read on threads, so a process it left behind
/// that still holds the output open cannot hold the call: after the tool exits, the output
/// waits at most `grace`. At the deadline the group gets SIGTERM, then SIGKILL, and the call
/// ends without waiting for the pipes.
fn run_until(mut cmd: std::process::Command, secret: Option<&Secret>, deadline: Duration, grace: Duration) -> std::io::Result<End> {
    use std::io::{Read, Write};
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::sync::{mpsc, Arc, Mutex};

    let start = Instant::now();
    cmd.stdin(if secret.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = cmd.spawn()?;
    // The group's id is the child's pid.
    let group = child.id() as libc::pid_t;

    let (eof_tx, eof_rx) = mpsc::channel::<()>();
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let (into, eof) = (buf.clone(), eof_tx.clone());
        if let Some(mut pipe) = pipe {
            std::thread::spawn(move || {
                let mut chunk = [0u8; 4096];
                while let Ok(n @ 1..) = pipe.read(&mut chunk) {
                    into.lock().unwrap().extend_from_slice(&chunk[..n]);
                }
                let _ = eof.send(());
            });
        }
        buf
    };
    let stdout = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let stderr = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));

    if let (Some(secret), Some(mut stdin)) = (secret, child.stdin.take()) {
        // A tool that exits without reading closes the pipe: that is its own error to report.
        let _ = stdin.write_all(format!("{}\n", secret.0).as_bytes());
        // Dropping stdin closes it, so a second prompt reads the end of input and fails at once.
    }

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if start.elapsed() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let Some(status) = status else {
        stop_group(group, &mut child);
        return Ok(End::TimedOut);
    };

    // Both readers end at the end of their pipe, unless a process left behind holds it.
    let until = Instant::now() + grace;
    for _ in 0..2 {
        let left = until.saturating_duration_since(Instant::now());
        if eof_rx.recv_timeout(left).is_err() {
            break;
        }
    }
    let text = |buf: &Arc<Mutex<Vec<u8>>>| String::from_utf8_lossy(&buf.lock().unwrap()).into_owned();
    Ok(End::Exited(Ran { code: status.code(), stdout: text(&stdout), stderr: text(&stderr) }))
}

/// SIGTERM to the group, SIGKILL 2 seconds later if the child still runs. A process that pkexec
/// made root does not take the user's signals: then the child is reaped on a thread of its own
/// whenever it ends, and the call does not wait for it.
fn stop_group(group: libc::pid_t, child: &mut std::process::Child) {
    // SAFETY: killpg only sends a signal, to the group the child leads.
    unsafe { libc::killpg(group, libc::SIGTERM) };
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: as above; the child has not been reaped, so the group id is still its own.
    unsafe { libc::killpg(group, libc::SIGKILL) };
    if !matches!(child.try_wait(), Ok(Some(_))) {
        let pid = child.id();
        // SAFETY: waitpid on our own child only reaps it.
        std::thread::spawn(move || unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0) });
    }
}

/// Run `call` within its time limit: what the tool printed, when it exited with 0.
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
    fn the_image_from_the_marker() {
        assert_eq!(image_of("IMAGE=main\n"), Some(Image::Main));
        assert_eq!(image_of("# PS5 Launcher OS\n IMAGE=\"nvidia\" \n"), None);
        assert_eq!(image_of("IMAGE=bazzite"), None);
        assert_eq!(image_of(""), None);
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
    fn a_secret_never_shows_in_the_args_the_debug_output_or_an_error() {
        let call = Call::new("nmcli", &["--ask", "device", "wifi", "connect", "Home"], 60).with_stdin("hunter22");
        assert!(!call.args.iter().any(|a| a.contains("hunter22")), "{:?}", call.args);
        let debug = format!("{call:?}");
        assert!(!debug.contains("hunter22"), "{debug}");
        assert_eq!(call.shown(), "nmcli --ask device wifi connect Home");
        let ran = Ran { code: Some(1), stdout: String::new(), stderr: "Error: Secrets were required.".into() };
        assert!(!ran.error(&call).contains("hunter22"));
    }

    // macOS has no `timeout`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_secret_goes_to_stdin_and_never_comes_back() {
        // The tool gets the secret as its first line, and stdin closes after it.
        let check = Call::new("sh", &["-c", "read p; [ \"$p\" = hunter22 ] && echo match; read q || echo eof"], 5);
        assert_eq!(call(&check.with_stdin("hunter22")), Ok("match\neof\n".into()));
        // A tool that prints what it read: the secret is hidden in what the launcher keeps.
        let echo = Call::new("sh", &["-c", "read p; echo \"Password: $p\"; echo \"bad $p\" >&2; exit 4"], 5).with_stdin("hunter22");
        let ran = call_status(&echo).unwrap();
        assert!(!ran.stdout.contains("hunter22") && !ran.stderr.contains("hunter22"), "{ran:?}");
        let error = call(&echo).unwrap_err();
        assert!(!error.contains("hunter22") && error.contains("exit 4"), "{error}");
    }

    // macOS has no `timeout`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_call_without_a_secret_has_no_stdin() {
        // The launcher's own stdin is never handed to a tool.
        assert_eq!(call(&Call::new("sh", &["-c", "read x || echo none"], 5)), Ok("none\n".into()));
    }

    #[test]
    fn a_root_helper_call_does_not_run_under_timeout() {
        // `timeout` runs as the user and cannot stop the helper once pkexec made it root: the
        // helper has its own deadlines, and the launcher only waits a little longer.
        let call = Call::new("pkexec", &["/usr/libexec/ps5-launcher/helper", "status"], 60);
        let cmd = call.command();
        assert_eq!(cmd.get_program(), "pkexec");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, ["/usr/libexec/ps5-launcher/helper", "status"]);
        assert_eq!(call.deadline(), Duration::from_secs(60));
        // A user tool runs under timeout; the launcher's own deadline is the backstop after it.
        assert!(Call::new("nmcli", &["radio"], 10).deadline() > Duration::from_secs(15));
    }

    #[test]
    fn the_runner_writes_the_secret_and_closes_stdin() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "read p; echo \"[$p]\"; read q || echo eof"]);
        let secret = Secret::new("p@ss word'\"");
        let end = run_until(cmd, Some(&secret), Duration::from_secs(5), Duration::from_secs(1)).unwrap();
        let End::Exited(ran) = end else { panic!("{end:?}") };
        assert_eq!(ran.stdout, "[p@ss word'\"]\neof\n");
    }

    #[test]
    fn the_deadline_stops_a_tool_that_never_ends() {
        let started = Instant::now();
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        let end = run_until(cmd, None, Duration::from_millis(300), Duration::from_millis(200)).unwrap();
        assert_eq!(end, End::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn a_child_left_behind_cannot_hold_the_call() {
        // The tool exits, but a process it started keeps its output open.
        let started = Instant::now();
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 20 & echo hi"]);
        let end = run_until(cmd, None, Duration::from_secs(30), Duration::from_millis(300)).unwrap();
        let End::Exited(ran) = end else { panic!("{end:?}") };
        assert_eq!((ran.code, ran.stdout.as_str()), (Some(0), "hi\n"));
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn the_deadline_stops_the_tools_children_too() {
        // A grandchild that holds the pipes after the deadline does not keep the call waiting.
        let started = Instant::now();
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 30 & sleep 30"]);
        let end = run_until(cmd, None, Duration::from_millis(300), Duration::from_millis(200)).unwrap();
        assert_eq!(end, End::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn an_error_carries_what_the_tool_said() {
        let call = Call::new("helper", &["update"], 60);
        let said_on_stdout = Ran { code: Some(3), stdout: "update-unavailable\n".into(), stderr: "  ".into() };
        assert_eq!(said_on_stdout.error(&call), "helper update failed (exit 3): update-unavailable");
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
        assert!(call(&Call::new("ps5-launcher-no-such-program", &[], 5)).is_err());
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
