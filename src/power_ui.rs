//! The Power menu, its loss dialog and countdown, and "Power off when done". The rules live in
//! `system`; this file only shows them and runs the chosen action off the UI thread.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::osupdate::HELPER;
use crate::system::{self, Can, Choice, Guard, JobEnd, Mode, PowerAction, PowerCaps, WaitState, Work};
use crate::PowerRow;
use std::time::{Duration, Instant};

/// Installed when Plasma is there to switch to ("Switch to desktop").
const DESKTOP_SESSION: &str = "/usr/share/wayland-sessions/plasma.desktop";
/// systemd sends SIGTERM to the session within its stop timeout (90 s by default). A launcher
/// still running after this did not go down, so its resume intent goes: a crash later must not
/// resume downloads.
const RESUME_GRACE: Duration = Duration::from_secs(120);

/// A row of the Power menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Row {
    /// Stop waiting to power off after the installs ("Power off when done").
    CancelWait,
    Action(PowerAction),
}

pub struct PowerUi {
    pub rows: Vec<Row>,
    /// logind's answer to CanSuspend, asked in the background each time the menu opens.
    pub can_sleep: Can,
    /// The action of the open dialog or countdown, and the dialog's buttons.
    pub action: Option<PowerAction>,
    pub choices: Vec<Choice>,
    pub seconds: u8,
    pub timer: Option<slint::Timer>,
    /// "Power off when done": the install ids it waits for.
    pub wait: Option<Vec<String>>,
    pub note: String,
    /// When the downloads' resume intent was written for the action that runs now.
    pub resume_written: Option<Instant>,
}

impl Default for PowerUi {
    fn default() -> Self {
        Self { rows: Vec::new(), can_sleep: Can::Na, action: None, choices: Vec::new(), seconds: 0, timer: None, wait: None, note: String::new(), resume_written: None }
    }
}

fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

fn icon(action: PowerAction) -> &'static str {
    match action {
        PowerAction::CloseGame => "close",
        PowerAction::CloseLauncher | PowerAction::LogOut => "logout",
        PowerAction::Sleep => "moon",
        PowerAction::Restart { update: false } => "restart",
        PowerAction::Restart { update: true } => "update",
        PowerAction::PowerOff => "power",
        PowerAction::SwitchToDesktop => "desktop",
    }
}

/// A row's label and icon, for the Power menu and the Quick Menu.
pub fn label_icon(row: Row) -> (&'static str, &'static str) {
    match row {
        Row::CancelWait => ("Cancel power off", "close"),
        Row::Action(a) => (a.label(), icon(a)),
    }
}

/// A divider sits after Close game (and Cancel power off), and before Log out / Switch to desktop.
/// After the rows change (logind's answer added Sleep, say): the index of the row that was
/// selected, so focus stays on the same action. When it is gone, the nearest index.
pub fn keep_selection(old: &[Row], new: &[Row], idx: usize) -> usize {
    let same = old.get(idx).and_then(|row| new.iter().position(|r| r == row));
    same.unwrap_or(idx).min(new.len().saturating_sub(1))
}

/// "Switch to desktop" needs a desktop session and PS5 Launcher OS's root helper, which sets the
/// next login. On another Linux PC there is no helper, so the row would only fail.
/// The system command behind a power action, with its time limit; None when the launcher does
/// it itself (Close game, Close launcher, Log out).
pub fn power_call(action: PowerAction) -> Option<system::Call> {
    use system::Call;
    match action {
        PowerAction::CloseGame | PowerAction::CloseLauncher | PowerAction::LogOut => None,
        // systemctl only asks logind and returns; logind does the rest.
        PowerAction::Sleep => Some(Call::new("systemctl", &["suspend"], 30)),
        PowerAction::Restart { .. } => Some(Call::new("systemctl", &["reboot"], 30)),
        PowerAction::PowerOff => Some(Call::new("systemctl", &["poweroff"], 30)),
        // The helper only writes one small file.
        PowerAction::SwitchToDesktop => Some(Call::new("pkexec", &[HELPER, "set-next-session", "plasma"], 60)),
    }
}

/// logind's answer to "can this PC sleep?".
pub fn can_sleep_call() -> system::Call {
    system::Call::new(
        "busctl",
        &["--system", "call", "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "CanSuspend"],
        10,
    )
}

pub fn can_switch_to_desktop(desktop_session: bool, helper: bool) -> bool {
    desktop_session && helper
}

/// The planned actions that end the launcher: the running downloads resume at the next start.
/// Sleep keeps the process and its downloads running. Close launcher (desktop) is quitting an
/// app, which stops its work, as Pause does; the window's close button quits the same way.
pub fn resumes_downloads(action: PowerAction) -> bool {
    matches!(action, PowerAction::Restart { .. } | PowerAction::PowerOff | PowerAction::LogOut | PowerAction::SwitchToDesktop)
}

pub fn divider(prev: Option<Row>, row: Row) -> bool {
    let leaving = |r: Row| matches!(r, Row::Action(PowerAction::LogOut | PowerAction::SwitchToDesktop));
    match prev {
        None => false,
        Some(Row::CancelWait | Row::Action(PowerAction::CloseGame)) => true,
        Some(p) => leaving(row) && !leaving(p),
    }
}

pub fn is_power(ov: Overlay) -> bool {
    matches!(ov, Overlay::Power | Overlay::PowerDialog | Overlay::PowerCountdown)
}

impl App {
    /// What a power action would interrupt now.
    pub fn power_work(&self) -> Work {
        let jobs = self.installer.snapshot().into_iter().filter(|r| r.state.active())
            .map(|r| system::Job { percent: (r.progress() * 100.0).round() as u8, id: r.key, name: r.name })
            .collect();
        Work {
            game: self.live.first().map(|s| s.name.clone()),
            jobs,
            downloads: self.downloads.snapshot().iter().filter(|j| j.state.active()).count(),
        }
    }

    fn power_caps(&self) -> PowerCaps {
        PowerCaps { can_sleep: self.power.can_sleep, has_desktop: can_switch_to_desktop(
                std::path::Path::new(DESKTOP_SESSION).exists(),
                std::path::Path::new(HELPER).exists(),
            ), update_staged: self.sys.os.status.as_ref().is_some_and(|s| s.staged.is_some()) }
    }

    /// Ask bootc whether an OS update waits for the next restart; the Restart row of the Power
    /// menu and the Quick Menu follows ("Update and restart").
    pub fn check_staged(&self) {
        if Mode::current() != Mode::Os {
            return;
        }
        std::thread::spawn(|| {
            let status = crate::system_ui::load_os();
            post(move |app| {
                let status = match status {
                    Ok(status) => status,
                    Err(e) => return crate::log!("bootc status: {e}"),
                };
                let before = app.power.rows.clone();
                app.sys.os.status = Some(status);
                if app.overlay == Overlay::Power {
                    app.push_power_rows();
                    let idx = keep_selection(&before, &app.power.rows, app.idx.max(0) as usize);
                    app.set_focus(Z_POWER, idx as i32);
                } else if app.overlay == Overlay::Quick {
                    app.push_quick();
                }
            });
        });
    }

    /// PS button held, or the top bar's power button: open the Power menu from anywhere.
    pub fn open_power_menu(&mut self) {
        if self.boot.active || is_power(self.overlay) {
            return;
        }
        self.end_editing();
        audio::play(Sound::Select);
        self.push_power_rows();
        self.push_overlay(Overlay::Power, Z_POWER, 0);
        self.check_sleep();
        self.check_staged();
    }

    /// The PC's power key (PS5 Launcher OS, see `power_key`): the Power menu, as a held PS button
    /// opens it. During a game the launcher comes forward first.
    #[cfg(target_os = "linux")]
    pub fn on_power_key(&mut self) {
        if !self.live.is_empty() {
            crate::sessions::show_launcher();
        }
        self.open_power_menu();
    }

    /// Before a menu opens over the screen: stop editing text there.
    pub fn end_editing(&mut self) {
        if self.search_editing {
            self.stop_search_edit();
        }
        if self.edit_index >= 0 {
            self.finish_edit(None);
        }
        self.settings_find_stop();
        if self.overlay == Overlay::Downloads && self.ui().get_download_editing() {
            self.ui().invoke_focus_root();
        }
    }

    /// Ask logind whether the PC can sleep; the rows of the Power menu and the Quick Menu follow
    /// the answer.
    pub fn check_sleep(&self) {
        if !cfg!(target_os = "linux") || Mode::current() == Mode::Desktop {
            return;
        }
        std::thread::spawn(|| {
            let can = system::call(&can_sleep_call()).and_then(|out| system::parse_logind_can(&out)).unwrap_or_else(|e| {
                crate::log!("CanSuspend: {e}");
                Can::Na
            });
            post(move |app| {
                if app.power.can_sleep != can {
                    let before = app.power.rows.clone();
                    app.power.can_sleep = can;
                    if app.overlay == Overlay::Power {
                        app.push_power_rows();
                        let idx = keep_selection(&before, &app.power.rows, app.idx.max(0) as usize);
                        app.set_focus(Z_POWER, idx as i32);
                    } else if app.overlay == Overlay::Quick {
                        app.push_quick();
                    }
                }
            });
        });
    }

    /// The Power menu's rows, in order.
    pub fn power_rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = Vec::new();
        if self.power.wait.is_some() {
            rows.push(Row::CancelWait);
        }
        rows.extend(system::power_actions(Mode::current(), &self.power_work(), &self.power_caps()).into_iter().map(Row::Action));
        rows
    }

    fn push_power_rows(&mut self) {
        let rows = self.power_rows();
        let data = rows.iter().enumerate().map(|(i, r)| {
            let (label, icon) = label_icon(*r);
            let sep = divider(i.checked_sub(1).map(|p| rows[p]), *r);
            PowerRow { label: label.into(), icon: icon.into(), danger: *r == Row::Action(PowerAction::CloseGame), sep }
        }).collect();
        let ui = self.ui();
        ui.set_power_rows(model(data));
        ui.set_power_note(self.power.note.clone().into());
        self.power.rows = rows;
    }

    pub fn act_power_menu(&mut self, a: Act) {
        let n = self.power.rows.len() as i32;
        match a {
            Act::Up if self.idx > 0 => self.move_focus(Z_POWER, self.idx - 1),
            Act::Down if self.idx + 1 < n => self.move_focus(Z_POWER, self.idx + 1),
            Act::Back => self.back(),
            Act::Confirm => {
                if let Some(row) = self.power.rows.get(self.idx as usize).copied() {
                    self.choose_power_row(row);
                }
            }
            _ => {}
        }
    }

    /// A row chosen in the Power menu or the Quick Menu.
    pub fn choose_power_row(&mut self, row: Row) {
        match row {
            Row::CancelWait => {
                self.power.wait = None;
                self.power.note.clear();
                self.close_power();
                self.toast("Power off cancelled", "The PC stays on after the install.", 0);
            }
            Row::Action(action) => self.guard_power(action),
        }
    }

    /// Confirm `action` the way `system::guard` says: a countdown, or a dialog of what is lost.
    pub(crate) fn guard_power(&mut self, action: PowerAction) {
        audio::play(Sound::Select);
        match system::guard(action, &self.power_work()) {
            Guard::Countdown(secs) => self.start_countdown(action, secs),
            Guard::Dialog { losses, choices } => {
                let ui = self.ui();
                ui.set_power_title(format!("{}?", action.label()).into());
                ui.set_power_losses(model(losses.iter().map(|l| system::loss_text(l).into()).collect()));
                let note = if choices.iter().any(|c| matches!(c, Choice::WhenDone(_))) {
                    format!("“{}” waits for the install. It does not wait for games or downloads.", system::choice_label(action, &Choice::WhenDone(Vec::new())))
                } else {
                    String::new()
                };
                ui.set_power_dialog_note(note.into());
                ui.set_power_choices(model(choices.iter().map(|c| PowerRow {
                    label: system::choice_label(action, c).into(),
                    danger: *c == Choice::Now,
                    ..Default::default()
                }).collect()));
                self.power.action = Some(action);
                self.power.choices = choices;
                // The safe choice comes first, and focus starts on it.
                self.push_overlay(Overlay::PowerDialog, Z_POWER, 0);
            }
        }
    }

    pub fn act_power_dialog(&mut self, a: Act) {
        let n = self.power.choices.len() as i32;
        match a {
            Act::Left | Act::Up if self.idx > 0 => self.move_focus(Z_POWER, self.idx - 1),
            Act::Right | Act::Down if self.idx + 1 < n => self.move_focus(Z_POWER, self.idx + 1),
            Act::Back => self.back(),
            Act::Confirm => {
                let (Some(action), Some(choice)) = (self.power.action, self.power.choices.get(self.idx as usize).cloned()) else { return };
                match choice {
                    Choice::Cancel => self.back(),
                    Choice::WhenDone(ids) => {
                        audio::play(Sound::Select);
                        self.power.wait = Some(ids);
                        self.close_power();
                        self.check_power_wait();
                        let note = self.power.note.clone();
                        self.toast(&note, "Open the Power menu to cancel. Games and downloads are not waited for.", 0);
                    }
                    Choice::Now => {
                        self.close_power();
                        self.run_power(action);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn act_power_countdown(&mut self, a: Act) {
        if a == Act::Back {
            self.back();
        }
    }

    fn start_countdown(&mut self, action: PowerAction, secs: u8) {
        self.power.action = Some(action);
        self.power.seconds = secs;
        let ui = self.ui();
        ui.set_power_counting(action.counting().into());
        ui.set_power_seconds(secs as i32);
        self.push_overlay(Overlay::PowerCountdown, Z_POWER, 0);
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), || with_app(|app| app.countdown_tick()));
        self.power.timer = Some(timer);
    }

    fn countdown_tick(&mut self) {
        if self.overlay != Overlay::PowerCountdown {
            self.power.timer = None;
            return;
        }
        self.power.seconds = self.power.seconds.saturating_sub(1);
        if self.power.seconds > 0 {
            self.ui().set_power_seconds(self.power.seconds as i32);
            return;
        }
        self.power.timer = None;
        let action = self.power.action;
        self.close_power();
        if let Some(action) = action {
            self.run_power(action);
        }
    }

    /// Close the Power menu, its dialog and its countdown, and the Quick Menu they came from, back
    /// to where they were opened.
    fn close_power(&mut self) {
        while is_power(self.overlay) || self.overlay == Overlay::Quick {
            self.back();
        }
    }

    /// Do it. System commands run off the UI thread; a failure shows as a toast and the
    /// launcher stays open.
    fn run_power(&mut self, action: PowerAction) {
        if resumes_downloads(action) {
            match self.downloads.keep_running_after_restart() {
                Ok(n) => self.power.resume_written = (n > 0).then(Instant::now),
                Err(e) => crate::log!("Could not save the downloads to resume: {e}"),
            }
        }
        let Some(call) = power_call(action) else {
            match action {
                PowerAction::CloseGame => self.stop_game(None),
                // The normal quit path: downloads and installs shut down, and the process exits
                // 0, which ends ps5-launcher-session (Log out). Log out keeps its resume intent:
                // the process ends now.
                _ => {
                    let _ = slint::quit_event_loop();
                }
            }
            return;
        };
        std::thread::spawn(move || {
            let result = system::call(&call).map(|_| ());
            post(move |app| match result {
                // The next login goes to the desktop: end this session.
                Ok(()) if action == PowerAction::SwitchToDesktop => {
                    // pkexec can wait on a password for longer than RESUME_GRACE: save it again.
                    if let Err(e) = app.downloads.keep_running_after_restart() {
                        crate::log!("Could not save the downloads to resume: {e}");
                    }
                    let _ = slint::quit_event_loop();
                }
                Ok(()) => {}
                Err(e) => {
                    app.forget_resume();
                    crate::log!("{}: {e}", action.label());
                    audio::play(Sound::Error);
                    app.toast(&format!("Couldn't {}", action.label().to_lowercase()), &e, 2);
                }
            });
        });
    }

    fn forget_resume(&mut self) {
        if self.power.resume_written.take().is_some() {
            self.downloads.forget_resume();
        }
    }

    /// Every second: a planned action that did not end the launcher leaves no resume intent.
    pub fn expire_resume(&mut self) {
        if self.power.resume_written.is_some_and(|t| t.elapsed() > RESUME_GRACE) {
            crate::log!("still running after a power action: downloads will not resume");
            self.forget_resume();
        }
    }

    /// "Power off when done": every second, check the installs it waits for.
    pub fn check_power_wait(&mut self) {
        let Some(waited) = self.power.wait.clone() else { return };
        let jobs: Vec<(String, String, JobEnd)> = self.installer.snapshot().into_iter().map(|r| {
            let end = match r.state {
                crate::installer::State::Installed => JobEnd::Done,
                crate::installer::State::Failed | crate::installer::State::Cancelled => JobEnd::Failed,
                _ => JobEnd::Running,
            };
            (r.key, r.name, end)
        }).collect();
        match system::when_done(&waited, &jobs) {
            WaitState::Waiting(names) => {
                let note = format!("Powering off after {} install{}", names.join(" and "), if names.len() == 1 { "s" } else { "" });
                if note != self.power.note {
                    self.power.note = note;
                    if self.overlay == Overlay::Power {
                        self.ui().set_power_note(self.power.note.clone().into());
                    }
                }
            }
            WaitState::Ready => {
                self.power.wait = None;
                self.power.note.clear();
                self.close_power();
                if !self.live.is_empty() {
                    crate::sessions::show_launcher();
                }
                self.start_countdown(PowerAction::PowerOff, system::COUNTDOWN_SECS);
            }
            WaitState::Failed(name) => {
                self.power.wait = None;
                self.power.note.clear();
                if self.overlay == Overlay::Power {
                    self.close_power();
                }
                audio::play(Sound::Error);
                self.toast("The PC stays on", &format!("{name} did not finish installing, so Power off was cancelled."), 2);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PowerAction::*;

    fn seps(rows: &[Row]) -> Vec<bool> {
        rows.iter().enumerate().map(|(i, r)| divider(i.checked_sub(1).map(|p| rows[p]), *r)).collect()
    }

    #[test]
    fn focus_stays_on_the_same_action_when_rows_change() {
        let before = [Row::Action(Restart { update: false }), Row::Action(PowerOff), Row::Action(LogOut)];
        let after = [Row::Action(Sleep), Row::Action(Restart { update: false }), Row::Action(PowerOff), Row::Action(LogOut)];
        assert_eq!(keep_selection(&before, &after, 1), 2, "Power off moved down one row");
        assert_eq!(keep_selection(&after, &before, 0), 0, "Sleep is gone: the nearest row");
        assert_eq!(keep_selection(&after, &before, 3), 2, "Log out moved up one row");
        assert_eq!(keep_selection(&[], &before, 0), 0);
        assert_eq!(keep_selection(&after, &[], 2), 0);
    }

    #[test]
    fn power_actions_run_with_a_time_limit() {
        let restart = power_call(Restart { update: true }).unwrap();
        assert_eq!((restart.program, restart.args.clone()), ("systemctl", vec!["reboot".to_string()]));
        assert_eq!(power_call(PowerOff).unwrap().args, ["poweroff"]);
        assert_eq!(power_call(Sleep).unwrap().args, ["suspend"]);
        let desktop = power_call(SwitchToDesktop).unwrap();
        assert_eq!(desktop.program, "pkexec");
        assert_eq!(desktop.args, [HELPER, "set-next-session", "plasma"]);
        for action in [Sleep, Restart { update: false }, PowerOff, SwitchToDesktop] {
            let secs = power_call(action).unwrap().secs;
            assert!((10..=120).contains(&secs), "{action:?}: {secs} s");
        }
        for action in [CloseGame, CloseLauncher, LogOut] {
            assert!(power_call(action).is_none(), "{action:?} is the launcher's own");
        }
    }

    #[test]
    fn the_sleep_check_asks_logind_with_a_time_limit() {
        let call = can_sleep_call();
        assert_eq!(call.program, "busctl");
        assert_eq!(call.args.last().map(String::as_str), Some("CanSuspend"));
        assert!(call.secs > 0 && call.secs <= 15);
    }

    #[test]
    fn switching_to_the_desktop_needs_the_helper_too() {
        assert!(can_switch_to_desktop(true, true));
        assert!(!can_switch_to_desktop(true, false), "a normal Linux PC: no helper");
        assert!(!can_switch_to_desktop(false, true));
    }

    #[test]
    fn only_actions_that_end_the_launcher_on_purpose_resume_downloads() {
        for action in [Restart { update: false }, Restart { update: true }, PowerOff, LogOut, SwitchToDesktop] {
            assert!(resumes_downloads(action), "{action:?}");
        }
        assert!(!resumes_downloads(Sleep), "the launcher keeps running, and so do its downloads");
        assert!(!resumes_downloads(CloseLauncher), "quitting the app stops its downloads");
        assert!(!resumes_downloads(CloseGame));
    }

    #[test]
    fn dividers_follow_close_game_and_lead_the_session_rows() {
        let session = [Row::Action(CloseGame), Row::Action(Sleep), Row::Action(PowerOff), Row::Action(LogOut), Row::Action(SwitchToDesktop)];
        assert_eq!(seps(&session), [false, true, false, true, false]);
        let desktop = [Row::Action(CloseGame), Row::Action(CloseLauncher)];
        assert_eq!(seps(&desktop), [false, true]);
        let waiting = [Row::CancelWait, Row::Action(Restart { update: false }), Row::Action(PowerOff)];
        assert_eq!(seps(&waiting), [false, true, false]);
    }
}
