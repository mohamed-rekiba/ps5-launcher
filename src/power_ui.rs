//! The Power menu, its loss dialog and countdown, and "Power off when done". The rules live in
//! `system`; this file only shows them and runs the chosen action off the UI thread.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::system::{self, Can, Choice, Guard, JobEnd, Mode, PowerAction, PowerCaps, WaitState, Work};
use crate::PowerRow;
use std::process::Command;
use std::time::Duration;

/// Installed when Plasma is there to switch to ("Switch to desktop").
const DESKTOP_SESSION: &str = "/usr/share/wayland-sessions/plasma.desktop";
/// Root helper of PS5 Launcher OS (packaging/os/); it does not exist yet.
const HELPER: &str = "/usr/libexec/ps5-launcher/helper";

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
}

impl Default for PowerUi {
    fn default() -> Self {
        Self { rows: Vec::new(), can_sleep: Can::Na, action: None, choices: Vec::new(), seconds: 0, timer: None, wait: None, note: String::new() }
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
pub fn can_switch_to_desktop(desktop_session: bool, helper: bool) -> bool {
    desktop_session && helper
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
        // Phase 6 (system pages) fills in update_staged from `bootc status`.
        PowerCaps { can_sleep: self.power.can_sleep, has_desktop: can_switch_to_desktop(
                std::path::Path::new(DESKTOP_SESSION).exists(),
                std::path::Path::new(HELPER).exists(),
            ), update_staged: false }
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
            let mut cmd = Command::new("busctl");
            cmd.args(["--system", "call", "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "CanSuspend"]);
            let can = system::run_output(&mut cmd).and_then(|out| system::parse_logind_can(&out)).unwrap_or_else(|e| {
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
    fn guard_power(&mut self, action: PowerAction) {
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
        let (program, args): (&str, &[&str]) = match action {
            PowerAction::CloseGame => return self.stop_game(None),
            // The normal quit path: downloads and installs shut down, and the process exits 0,
            // which ends ps5-launcher-session (Log out).
            PowerAction::CloseLauncher | PowerAction::LogOut => {
                let _ = slint::quit_event_loop();
                return;
            }
            PowerAction::Sleep => ("systemctl", &["suspend"]),
            PowerAction::Restart { .. } => ("systemctl", &["reboot"]),
            PowerAction::PowerOff => ("systemctl", &["poweroff"]),
            PowerAction::SwitchToDesktop => ("pkexec", &[HELPER, "set-next-session", "plasma"]),
        };
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        std::thread::spawn(move || {
            let result = system::run(Command::new(program).args(&args));
            post(move |app| match result {
                // The next login goes to the desktop: end this session.
                Ok(()) if action == PowerAction::SwitchToDesktop => {
                    let _ = slint::quit_event_loop();
                }
                Ok(()) => {}
                Err(e) => {
                    crate::log!("{}: {e}", action.label());
                    audio::play(Sound::Error);
                    app.toast(&format!("Couldn't {}", action.label().to_lowercase()), &e, 2);
                }
            });
        });
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
    fn switching_to_the_desktop_needs_the_helper_too() {
        assert!(can_switch_to_desktop(true, true));
        assert!(!can_switch_to_desktop(true, false), "a normal Linux PC: no helper");
        assert!(!can_switch_to_desktop(false, true));
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
