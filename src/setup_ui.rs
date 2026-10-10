//! The first-start setup and the boot health notice on screen, in PS5 Launcher OS
//! (docs/plans/ps5-launcher-os.md, Phase 7). The setup is a full-screen stepper over the System
//! pages' own rows: Network, Time, Controllers and the drives of Storage. Joining a network
//! and pairing a controller work as they do in Settings. `setup` decides the steps and the texts; `notice` reads the
//! health check's file.
//!
//! The order: the welcome screen first (it prepares the catalog and the Home screen), then the
//! setup when it is due, then the notice when there is one, then Home.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::notice;
use crate::settings::Cat;
use crate::setup::{self, Button, Facts, Machine, Screen, Step};
use crate::system::Mode;
use crate::system_ui::bg;
use std::path::Path;
use std::time::Duration;

#[derive(Default)]
pub struct SetupUi {
    /// The setup shows; it may still wait for the facts.
    pub active: bool,
    /// The System pages were read once (sys_probe ended).
    probed: bool,
    /// The steps, once the facts are read.
    pub machine: Option<Machine>,
}

#[derive(Default)]
pub struct NoticeUi {
    /// A notice that waits for its turn: its key (the time and the outcome, or the error) and its
    /// text.
    pending: Option<(String, notice::Text)>,
    /// The key of the notice on screen, or shown already in this run: not shown again.
    shown: Option<String>,
    /// How many times the file was read.
    reads: u32,
}

/// The health check decides within 180 s of the PC's start, and writes `rolled-back` only after
/// the launcher answered for 10 s: read the file this often for a while after the launcher starts.
const NOTICE_EVERY: Duration = Duration::from_secs(10);
const NOTICE_READS: u32 = 30;

/// The page a step shows.
fn page(step: Step) -> Cat {
    match step {
        Step::Network => Cat::Network,
        Step::TimeZone => Cat::Time,
        Step::Controllers => Cat::Controllers,
        Step::GameDrive => Cat::Storage,
        Step::Finish => Cat::Setup,
    }
}

impl App {
    /// PS5 Launcher OS, and the setup never ran to its end.
    pub fn setup_due(&self) -> bool {
        Mode::current() == Mode::Os && !self.cfg.lock().unwrap().setup_done
    }

    /// At start: read the System pages for the setup while the welcome screen shows.
    pub fn setup_start(&mut self) {
        if self.setup_due() {
            self.sys_probe();
        }
    }

    /// The System pages were read (`sys_probe` ended): a setup that waits for them starts.
    pub fn setup_probed(&mut self) {
        self.setup.probed = true;
        if self.setup.active && self.setup.machine.is_none() {
            self.setup_plan();
        }
    }

    /// The welcome screen is done: the setup, the notice, or Home.
    pub fn after_welcome(&mut self) {
        match setup::after(self.setup_due(), self.notice.pending.is_some()) {
            Screen::Setup => self.setup_open(),
            Screen::Notice => self.notice_show(),
            Screen::Home => {}
        }
    }

    /// Settings → About → Run the setup again.
    pub fn setup_again(&mut self) {
        audio::play(Sound::Select);
        self.save_cfg(|c| {
            c.setup_done = false;
            c.setup_step = None;
        });
        // The setup takes Settings' place: Done goes back to Home.
        if self.overlay == Overlay::Settings {
            self.back();
        }
        self.setup_open();
        // Fresh facts: a drive or a controller may have come since Settings opened.
        self.sys_probe();
    }

    fn setup_open(&mut self) {
        self.end_editing();
        self.setup = SetupUi { active: true, probed: self.setup.probed, ..SetupUi::default() };
        self.sys_close_pages();
        self.settings_nav = Default::default();
        self.ui().set_settings_y(0.0);
        self.push_overlay(Overlay::Setup, Z_SETUP_NAV, 0);
        if self.setup.probed {
            self.setup_plan();
        } else {
            // "Getting ready…" until the pages are read.
            self.push_setup();
        }
    }

    /// What the setup knows now, from the System pages.
    fn setup_facts(&self) -> Facts {
        let (online, wired) = setup::online(&self.sys.net.devices);
        Facts {
            network: self.sys.tools.network,
            wired,
            online,
            time: self.sys.tools.time,
            zone: self.sys.time.clock.as_ref().map(|c| c.zone.clone()).unwrap_or_default(),
            pads: self.controllers().len(),
            drive: setup::drive(&self.sys.drives, &|dir| self.is_game_dir(dir)),
        }
    }

    /// The steps for this PC, from the step saved before a restart.
    fn setup_plan(&mut self) {
        let saved = self.cfg.lock().unwrap().setup_step;
        self.setup.machine = Some(Machine::start(&self.setup_facts(), saved));
        self.setup_enter();
    }

    fn setup_step(&self) -> Option<Step> {
        self.setup.machine.as_ref().map(Machine::step)
    }

    /// Show the step the machine is on, and keep it for a restart.
    fn setup_enter(&mut self) {
        let Some(step) = self.setup_step() else { return };
        if self.cfg.lock().unwrap().setup_step != Some(step) {
            self.save_cfg(|c| c.setup_step = Some(step));
        }
        self.sys_close_pages();
        let cat = page(step);
        self.settings_nav.cats = vec![cat];
        self.settings_nav.sub = Some(cat);
        self.ui().set_settings_y(0.0);
        self.build_settings();
        let first = self.settings_rows.iter().position(|r| r.kind != 0);
        match first {
            // Done already: focus waits on Next.
            Some(i) if !setup::ready(step, &self.setup_facts()) => self.set_focus(Z_SETTINGS, i as i32),
            _ => self.set_focus(Z_SETUP_NAV, self.setup_main_button()),
        }
        self.push_settings();
        if cat != Cat::Setup {
            // Read the page again: the Network page scans for networks.
            self.sys_page_opened(cat);
        }
    }

    fn setup_buttons(&self) -> Vec<Button> {
        let Some(m) = &self.setup.machine else { return Vec::new() };
        let step = m.step();
        setup::buttons(
            step,
            m.at() == 0,
            setup::ready(step, &self.setup_facts()),
        )
    }

    /// The main button (Skip, Next, Done) comes last.
    pub fn setup_main_button(&self) -> i32 {
        self.setup_buttons().len().saturating_sub(1) as i32
    }

    /// The stepper, the title, the text and the buttons over the page's rows.
    pub fn push_setup(&mut self) {
        if self.overlay != Overlay::Setup {
            return;
        }
        let buttons = self.setup_buttons();
        let view = match &self.setup.machine {
            None => crate::SetupView {
                kicker: "SET UP PS5 LAUNCHER OS".into(),
                title: "Getting ready…".into(),
                text: "Looking at the network, the controllers and the drives.".into(),
                ..Default::default()
            },
            Some(m) => {
                let (title, text) = setup::text(m.step());
                crate::SetupView {
                    kicker: format!("SET UP PS5 LAUNCHER OS · STEP {} OF {}", m.at() + 1, m.steps().len()).into(),
                    steps: model(m.steps().iter().map(|s| s.label().into()).collect()),
                    at: m.at() as i32,
                    title: title.into(),
                    text: if m.step() == Step::Finish
                        && self.sys.display.state.as_ref().is_some_and(|d| {
                            d.image.is_some() && d.screen.card.vendor == crate::gpu::NVIDIA
                        }) {
                        format!("{text} NVIDIA graphics use Fedora's nouveau driver and Mesa NVK. Updates arrive with system updates.").into()
                    } else {
                        text.into()
                    },
                    buttons: model(buttons.iter().map(|b| b.label().into()).collect()),
                }
            }
        };
        // The buttons change as a step becomes ready (Skip becomes Next).
        if self.zone == Z_SETUP_NAV && self.idx >= buttons.len() as i32 {
            self.set_focus(Z_SETUP_NAV, buttons.len().saturating_sub(1) as i32);
        }
        self.ui().set_setup(view);
    }

    pub fn act_setup(&mut self, a: Act) {
        if self.setup.machine.is_none() {
            return;
        }
        if a == Act::Back {
            // Back first leaves what a page has open: pairing, or a row being typed in.
            if self.sys_back() {
                return;
            }
            if self.edit_index >= 0 {
                return self.finish_edit(None);
            }
            return self.setup_back();
        }
        let n = self.settings_rows.len() as i32;
        let focusable = |rows: &Vec<crate::SettingData>, i: i32| i >= 0 && i < rows.len() as i32 && rows[i as usize].kind != 0;
        if self.zone == Z_SETUP_NAV {
            let buttons = self.setup_buttons();
            match a {
                Act::Left if self.idx > 0 => self.move_focus(Z_SETUP_NAV, self.idx - 1),
                Act::Right if self.idx + 1 < buttons.len() as i32 => self.move_focus(Z_SETUP_NAV, self.idx + 1),
                Act::Up => {
                    if let Some(i) = (0..n).rev().find(|i| focusable(&self.settings_rows, *i)) {
                        self.move_focus(Z_SETTINGS, i);
                        self.scroll_settings();
                    }
                }
                Act::Confirm => {
                    if let Some(b) = buttons.get(self.idx as usize).copied() {
                        self.setup_press(b);
                    }
                }
                _ => {}
            }
            return;
        }
        match a {
            Act::Down if !(self.idx + 1..n).any(|i| focusable(&self.settings_rows, i)) => self.move_focus(Z_SETUP_NAV, self.setup_main_button()),
            Act::Up | Act::Down | Act::Left | Act::Right | Act::Confirm => self.act_settings_page(a),
            _ => {}
        }
    }

    fn setup_press(&mut self, b: Button) {
        match b {
            Button::Back => self.setup_back(),
            Button::Skip | Button::Next => {
                audio::play(Sound::Select);
                if let Some(m) = self.setup.machine.as_mut() {
                    m.next();
                }
                self.setup_enter();
            }
            Button::Done => self.setup_finish(),
        }
    }

    fn setup_back(&mut self) {
        if self.setup.machine.as_mut().is_some_and(Machine::back) {
            audio::play(Sound::Back);
            self.setup_enter();
        }
    }

    /// Done: the setup never shows again by itself. The notice comes next, if there is one.
    fn setup_finish(&mut self) {
        self.save_cfg(|c| {
            c.setup_done = true;
            c.setup_step = None;
        });
        self.sys_close_pages();
        self.setup = SetupUi { probed: self.setup.probed, ..SetupUi::default() };
        if self.overlay == Overlay::Setup {
            self.back();
        }
        self.notice_show();
    }

    // ------------------------------------------------------------------ the boot health notice

    /// At start in PS5 Launcher OS: read the health check's notice, now and for a while after.
    pub fn notice_start(&mut self) {
        if Mode::current() == Mode::Os {
            self.notice_read();
        }
    }

    /// Read the notice off the UI thread; the next read follows a while later.
    fn notice_read(&mut self) {
        self.notice.reads += 1;
        bg(|| notice::read(Path::new(notice::PATH)), |app, found| app.notice_found(found));
        if self.notice.reads < NOTICE_READS {
            slint::Timer::single_shot(NOTICE_EVERY, || with_app(|app| app.notice_read()));
        }
    }

    fn notice_found(&mut self, found: Option<Result<notice::Notice, String>>) {
        let Some(found) = found else { return };
        let (key, text) = match found {
            Ok(n) => (format!("{} {:?}", n.time, n.outcome), notice::text(&n)),
            Err(e) => {
                crate::log!("boot health notice: {e}");
                (e.clone(), notice::unreadable(&e))
            }
        };
        if self.notice.shown.as_deref() == Some(key.as_str()) {
            return;
        }
        if self.overlay == Overlay::Notice {
            // A newer notice while one shows ("rolling back", then "rolled back"): the newer one.
            self.notice.shown = Some(key);
            return self.push_notice(&text);
        }
        self.notice.pending = Some((key, text));
        self.notice_show();
    }

    /// Show the notice that waits, unless the welcome screen or the setup shows.
    fn notice_show(&mut self) {
        if self.notice.pending.is_none() || !setup::notice_now(self.boot.active, self.setup.active, self.overlay == Overlay::Notice) {
            return;
        }
        let Some((key, text)) = self.notice.pending.take() else { return };
        self.notice.shown = Some(key);
        self.end_editing();
        self.push_notice(&text);
        self.push_overlay(Overlay::Notice, Z_NOTICE, 0);
    }

    fn push_notice(&self, text: &notice::Text) {
        self.ui().set_notice(crate::NoticeView {
            title: text.title.clone().into(),
            lines: model(text.lines.iter().map(|l| l.into()).collect()),
            details: text.details.clone().into(),
        });
    }

    pub fn act_notice(&mut self, a: Act) {
        if matches!(a, Act::Confirm | Act::Back) {
            self.notice_ok();
        }
    }

    /// OK: the helper deletes the file, so it shows once.
    fn notice_ok(&mut self) {
        self.back();
        self.sys_run(notice::ack_call(), "Couldn't clear the system notice", |_, _| {});
    }
}
