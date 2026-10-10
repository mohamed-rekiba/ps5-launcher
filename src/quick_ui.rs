//! The Quick Menu: a drawer on the right edge with a card for each quick setting
//! (docs/plans/ps5-launcher-os.md, Phase 3). Its power cards use the Power menu's guard, dialog
//! and countdown in `power_ui`.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::battery::{self, Controller};
use crate::downloads::State;
use crate::power_ui::{self, Row};
use crate::system::{PowerAction, Work};
use crate::{util, QuickCard};

#[derive(Default)]
pub struct QuickUi {
    pub cards: Vec<Card>,
    /// The Controllers card is open and lists the controllers.
    pub open: bool,
    /// What the drawer shows, so the clock tick only touches the UI when something changed.
    shown: (Vec<QuickCard>, Vec<(String, String)>),
}

/// A card of the Quick Menu, top to bottom.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Card {
    CloseGame,
    /// The volume of the output in use (Session and OS mode, with PipeWire).
    Sound,
    Controllers,
    Downloads,
    Power(Row),
}

/// What a short press of the PS button does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ps {
    /// Bring the launcher forward with the Quick Menu open.
    OpenQuick,
    CloseQuick,
    /// Close the Quick Menu if it is open, and bring the game forward.
    BackToGame,
}

/// The cards for the Power menu's `power` rows. Close game leads while a game runs; it has its own
/// card, so it is not repeated with the power rows. `sound`: there is an output whose volume the
/// Sound card sets.
pub fn cards(game: bool, sound: bool, power: &[Row]) -> Vec<Card> {
    let mut out = Vec::new();
    if game {
        out.push(Card::CloseGame);
    }
    if sound {
        out.push(Card::Sound);
    }
    // A Wi-Fi card goes here (Phase 6); the Network page in Settings has the networks for now.
    out.extend([Card::Controllers, Card::Downloads]);
    out.extend(power.iter().filter(|r| **r != Row::Action(PowerAction::CloseGame)).map(|r| Card::Power(*r)));
    out
}

/// A divider above each card: above the first power card, and where the Power menu has one.
pub fn seps(cards: &[Card]) -> Vec<bool> {
    let mut prev: Option<Row> = None;
    cards.iter().map(|c| match c {
        Card::Power(row) => {
            let sep = prev.is_none_or(|p| power_ui::divider(Some(p), *row));
            prev = Some(*row);
            sep
        }
        _ => false,
    }).collect()
}

/// The Sound card's status: "65%", or "Muted · 65%".
pub fn sound_status(output: &crate::sound::Output) -> String {
    let volume = crate::sound::percent(output.volume);
    if output.muted { format!("Muted · {volume}") } else { volume }
}

/// The Controllers card's status: each player's battery, "P1 80% · P2 15%, charging", or
/// "2 connected" when no battery level is known.
pub fn controllers_status(pads: &[Controller]) -> String {
    let levels: Vec<String> = pads.iter().map(|c| c.battery.as_ref().map(battery::text).unwrap_or_default()).collect();
    if pads.is_empty() {
        return "None connected".into();
    }
    if levels.iter().all(String::is_empty) {
        return format!("{} connected", pads.len());
    }
    let players = levels.iter().enumerate().map(|(i, level)| if level.is_empty() { format!("P{}", i + 1) } else { format!("P{} {level}", i + 1) });
    players.collect::<Vec<_>>().join(" · ")
}

/// The open Controllers card's lines: each controller's name and battery.
pub fn controller_items(pads: &[Controller]) -> Vec<(String, String)> {
    pads.iter().map(|c| (c.pad.name.clone(), c.battery.as_ref().map(battery::text).unwrap_or_default())).collect()
}

/// What the top bar's progress shows, in one line: the install first, then the downloads.
/// `waiting`: downloads that are paused or not started; `eta`: the first download's time left.
pub fn downloads_status(work: &Work, waiting: usize, eta: &str) -> String {
    if let Some(job) = work.jobs.first() {
        return format!("Installing {} · {}%", job.name, job.percent);
    }
    let time = if eta.is_empty() { String::new() } else { format!(" · {eta}") };
    match (work.downloads, waiting) {
        (0, 0) => "Nothing downloading".into(),
        (0, w) => format!("{w} paused"),
        (n, 0) => format!("{n} downloading{time}"),
        (n, w) => format!("{n} of {}{time}", n + w),
    }
}

/// `quick_open`: the Quick Menu is open, maybe under a power dialog.
pub fn ps_press(game_running: bool, game_in_front: bool, quick_open: bool) -> Ps {
    if game_in_front || (!game_running && !quick_open) {
        Ps::OpenQuick
    } else if game_running {
        Ps::BackToGame
    } else {
        Ps::CloseQuick
    }
}

impl App {
    /// The Quick Menu is open, maybe under a Power menu, dialog or countdown opened from it.
    pub fn quick_open(&self) -> bool {
        self.overlay == Overlay::Quick || self.stack.iter().any(|s| s.0 == Overlay::Quick)
    }

    /// A short press of the PS button.
    pub fn ps_pressed(&mut self) {
        if !self.pad_hints {
            self.pad_hints = true;
            self.ui().set_pad_hints(true);
        }
        let game = !self.live.is_empty();
        match ps_press(game, game && self.sessions.game_in_front(), self.quick_open()) {
            Ps::OpenQuick => {
                crate::sessions::show_launcher();
                self.open_quick_menu();
            }
            Ps::CloseQuick => self.close_quick(),
            Ps::BackToGame => {
                self.close_quick();
                self.sessions.resume();
            }
        }
    }

    pub fn open_quick_menu(&mut self) {
        // Not over the setup or the notice: its cards open Settings pages, which the setup uses.
        if self.boot.active || self.quick_open() || power_ui::is_power(self.overlay) || self.setup.active || self.overlay == Overlay::Notice {
            return;
        }
        self.end_editing();
        audio::play(Sound::Select);
        self.quick.open = false;
        self.push_quick();
        self.push_overlay(Overlay::Quick, Z_QUICK, 0);
        self.check_sleep();
        self.check_staged();
        if crate::system::Mode::current() != crate::system::Mode::Desktop {
            self.sound_load(false);
        }
        self.pads_load();
    }

    /// Close the Quick Menu and anything opened over it.
    fn close_quick(&mut self) {
        while self.quick_open() && self.overlay != Overlay::None {
            self.back();
        }
    }

    /// Build the cards from the launcher's state. Focus stays on the same card when the cards
    /// change (a game ends, an install starts).
    pub fn push_quick(&mut self) {
        let focused = (self.overlay == Overlay::Quick).then(|| self.quick.cards.get(self.idx as usize).copied()).flatten();
        let work = self.power_work();
        let output = crate::sound::default_output(&self.sys.outputs).cloned();
        let list = cards(work.game.is_some(), output.is_some(), &self.power_rows());
        let jobs = self.downloads.snapshot();
        let waiting = jobs.iter().filter(|j| matches!(j.state, State::Paused | State::Ready)).count();
        let eta = jobs.iter().find(|j| j.state.active()).map(|j| j.eta.as_str()).unwrap_or("");
        let pads = self.controllers();
        let data: Vec<QuickCard> = list.iter().zip(seps(&list)).map(|(card, sep)| match card {
            Card::CloseGame => {
                let status = self.live.first().map(|s| format!("{} · {}", s.name, util::fmt_clock(util::now_secs() - s.since))).unwrap_or_default();
                QuickCard { label: "Close game".into(), status: status.into(), icon: "close".into(), danger: true, sep, ..Default::default() }
            }
            Card::Sound => QuickCard {
                label: "Sound".into(),
                status: output.as_ref().map(sound_status).unwrap_or_default().into(),
                icon: "sound".into(),
                sep,
                adjust: true,
                ..Default::default()
            },
            Card::Controllers => QuickCard {
                label: "Controllers".into(),
                status: controllers_status(&pads).into(),
                icon: "pad".into(),
                sep,
                more: true,
                open: self.quick.open,
                ..Default::default()
            },
            Card::Downloads => QuickCard {
                label: "Downloads".into(),
                status: downloads_status(&work, waiting, eta).into(),
                icon: "download".into(),
                sep,
                more: true,
                ..Default::default()
            },
            Card::Power(row) => {
                let (label, icon) = power_ui::label_icon(*row);
                // "Powering off after Astro Bot installs" on its Cancel card.
                let status = if *row == Row::CancelWait { self.power.note.clone() } else { String::new() };
                QuickCard { label: label.into(), status: status.into(), icon: icon.into(), sep, ..Default::default() }
            }
        }).collect();
        let items = if self.quick.open { controller_items(&pads) } else { Vec::new() };
        if (&data, &items) != (&self.quick.shown.0, &self.quick.shown.1) {
            let ui = self.ui();
            ui.set_quick_cards(model(data.clone()));
            ui.set_quick_items(model(items.iter().map(|(label, status)| crate::QuickItem { label: label.into(), status: status.into() }).collect()));
            self.quick.shown = (data, items);
        }
        if let Some(card) = focused {
            let idx = list.iter().position(|c| *c == card).unwrap_or((self.idx as usize).min(list.len().saturating_sub(1)));
            if idx as i32 != self.idx {
                self.set_focus(Z_QUICK, idx as i32);
            }
        }
        self.quick.cards = list;
    }

    pub fn act_quick(&mut self, a: Act) {
        let n = self.quick.cards.len() as i32;
        match a {
            Act::Up if self.idx > 0 => self.move_focus(Z_QUICK, self.idx - 1),
            Act::Down if self.idx + 1 < n => self.move_focus(Z_QUICK, self.idx + 1),
            Act::Back => self.back(),
            Act::Left | Act::Right if self.quick.cards.get(self.idx as usize) == Some(&Card::Sound) => {
                self.sound_step(if a == Act::Left { -1 } else { 1 });
            }
            Act::Confirm => match self.quick.cards.get(self.idx as usize).copied() {
                // The Power menu's flow: the same guard, loss dialog and countdown.
                Some(Card::CloseGame) => self.choose_power_row(Row::Action(PowerAction::CloseGame)),
                Some(Card::Power(row)) => self.choose_power_row(row),
                Some(Card::Controllers) => {
                    audio::play(Sound::Select);
                    self.quick.open = !self.quick.open;
                    self.push_quick();
                    if self.quick.open {
                        self.pads_load();
                    }
                }
                Some(Card::Downloads) => {
                    self.close_quick();
                    self.open_downloads(None);
                }
                // Left and Right set it.
                Some(Card::Sound) | None => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::Level;
    use crate::system::Job;
    use PowerAction::*;

    #[test]
    fn close_game_leads_while_a_game_runs_and_is_not_repeated() {
        let power = [Row::Action(CloseGame), Row::Action(Sleep), Row::Action(Restart { update: false }), Row::Action(PowerOff)];
        assert_eq!(cards(true, false, &power), [
            Card::CloseGame, Card::Controllers, Card::Downloads,
            Card::Power(Row::Action(Sleep)), Card::Power(Row::Action(Restart { update: false })), Card::Power(Row::Action(PowerOff)),
        ]);
        let power = [Row::Action(Restart { update: true }), Row::Action(PowerOff)];
        assert_eq!(cards(false, false, &power), [
            Card::Controllers, Card::Downloads, Card::Power(Row::Action(Restart { update: true })), Card::Power(Row::Action(PowerOff)),
        ]);
    }

    #[test]
    fn the_sound_card_follows_close_game_when_there_is_an_output() {
        let power = [Row::Action(PowerOff)];
        assert_eq!(cards(true, true, &power), [Card::CloseGame, Card::Sound, Card::Controllers, Card::Downloads, Card::Power(Row::Action(PowerOff))]);
        assert_eq!(cards(false, true, &power)[0], Card::Sound);
        assert_eq!(seps(&cards(false, true, &power)), [false, false, false, true]);
    }

    #[test]
    fn the_sound_card_shows_the_volume() {
        let out = |volume, muted| crate::sound::Output { id: 1, name: "TV".into(), volume, muted, default: true };
        assert_eq!(sound_status(&out(0.65, false)), "65%");
        assert_eq!(sound_status(&out(0.65, true)), "Muted · 65%");
    }

    #[test]
    fn cancel_power_off_comes_with_the_power_cards() {
        let power = [Row::CancelWait, Row::Action(Restart { update: false }), Row::Action(PowerOff)];
        assert_eq!(cards(false, false, &power)[2..], [Card::Power(Row::CancelWait), Card::Power(Row::Action(Restart { update: false })), Card::Power(Row::Action(PowerOff))]);
    }

    #[test]
    fn dividers_set_the_power_cards_apart() {
        let c = cards(true, false, &[Row::Action(CloseGame), Row::Action(Sleep), Row::Action(PowerOff), Row::Action(LogOut), Row::Action(SwitchToDesktop)]);
        assert_eq!(seps(&c), [false, false, false, true, false, true, false]);
        let c = cards(false, false, &[Row::CancelWait, Row::Action(PowerOff)]);
        assert_eq!(seps(&c), [false, false, true, true]);
        assert_eq!(seps(&cards(false, false, &[])), [false, false]);
    }

    fn pad(battery: Option<(Level, bool)>) -> Controller {
        let pad = crate::gamepad::InputPad { name: "DualSense Wireless Controller".into(), bus: None, uniq: String::new(), sysfs: String::new() };
        let battery = battery.map(|(level, charging)| crate::battery::Battery::fake(level, charging));
        Controller { pad, battery }
    }

    #[test]
    fn controllers_say_how_many_without_a_battery() {
        assert_eq!(controllers_status(&[]), "None connected");
        assert_eq!(controllers_status(&[pad(None)]), "1 connected");
        assert_eq!(controllers_status(&[pad(None), pad(None)]), "2 connected");
        assert_eq!(controllers_status(&[pad(Some((Level::Unknown, false)))]), "1 connected", "no level read yet");
    }

    #[test]
    fn controllers_show_each_players_battery() {
        assert_eq!(controllers_status(&[pad(Some((Level::Percent(80), false)))]), "P1 80%");
        assert_eq!(controllers_status(&[pad(Some((Level::Percent(80), false))), pad(Some((Level::Percent(15), true)))]), "P1 80% · P2 15%, charging");
        assert_eq!(controllers_status(&[pad(None), pad(Some((Level::Coarse("Low".into()), false)))]), "P1 · P2 Low", "a pad without a battery keeps its number");
    }

    #[test]
    fn the_open_card_lists_each_controller_with_its_battery() {
        assert_eq!(
            controller_items(&[pad(Some((Level::Percent(80), true))), pad(None)]),
            [("DualSense Wireless Controller".to_string(), "80%, charging".to_string()), ("DualSense Wireless Controller".to_string(), String::new())]
        );
    }

    #[test]
    fn downloads_show_the_install_first_then_the_downloads() {
        let job = |name: &str, percent| Job { id: name.into(), name: name.into(), percent };
        let idle = Work::default();
        assert_eq!(downloads_status(&idle, 0, ""), "Nothing downloading");
        assert_eq!(downloads_status(&idle, 2, ""), "2 paused");
        let one = Work { downloads: 1, ..Work::default() };
        assert_eq!(downloads_status(&one, 2, "12m"), "1 of 3 · 12m");
        assert_eq!(downloads_status(&one, 0, ""), "1 downloading");
        let installing = Work { jobs: vec![job("Astro Bot", 64)], downloads: 1, ..Work::default() };
        assert_eq!(downloads_status(&installing, 0, "12m"), "Installing Astro Bot · 64%");
    }

    #[test]
    fn the_ps_button_opens_the_quick_menu_over_a_game_and_goes_back_from_the_launcher() {
        // A game has focus: the launcher comes forward with the Quick Menu.
        assert_eq!(ps_press(true, true, false), Ps::OpenQuick);
        assert_eq!(ps_press(true, true, true), Ps::OpenQuick);
        // The launcher is in front while a game runs: back to the game, as before.
        assert_eq!(ps_press(true, false, false), Ps::BackToGame);
        assert_eq!(ps_press(true, false, true), Ps::BackToGame);
        // No game: the PS button opens and closes the Quick Menu.
        assert_eq!(ps_press(false, false, false), Ps::OpenQuick);
        assert_eq!(ps_press(false, false, true), Ps::CloseQuick);
    }
}
