//! The first-start setup of PS5 Launcher OS (docs/plans/ps5-launcher-os.md, Phase 7): which steps
//! show, the step machine with Skip, Back and resume, the texts, and the order of the setup and
//! the boot health notice. `setup_ui` shows it with the System pages' own rows.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// A step of the setup, in the order they come. Finish always shows.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Step {
    Network,
    TimeZone,
    Controllers,
    #[serde(alias = "graphics")]
    GameDrive,
    Finish,
}

pub const ORDER: [Step; 5] = [
    Step::Network,
    Step::TimeZone,
    Step::Controllers,
    Step::GameDrive,
    Step::Finish,
];

impl Step {
    /// The stepper's label.
    pub fn label(self) -> &'static str {
        match self {
            Step::Network => "Network",
            Step::TimeZone => "Time zone",
            Step::Controllers => "Controllers",
            Step::GameDrive => "Game drive",
            Step::Finish => "Finish",
        }
    }

    fn rank(self) -> usize {
        ORDER.iter().position(|s| *s == self).unwrap_or(ORDER.len())
    }
}

/// The drives for games, for the Game drive step.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Drive {
    /// No drive but the system's is mounted.
    #[default]
    None,
    /// A mounted drive that is not the system's can become a game folder.
    Offered,
    /// Every such drive is a game folder already.
    Used,
}

/// What the setup knows about the PC. It is read once when the setup starts, and again for
/// whether a step is done.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Facts {
    /// NetworkManager answered.
    pub network: bool,
    /// Online through a cable.
    pub wired: bool,
    /// Online: through a cable or a Wi-Fi network.
    pub online: bool,
    /// timedatectl answered.
    pub time: bool,
    /// The time zone in use; empty when not known.
    pub zone: String,
    /// The connected controllers.
    pub pads: usize,
    pub drive: Drive,
}

/// (online, through a cable) from NetworkManager's devices.
pub fn online(devices: &[crate::network::Device]) -> (bool, bool) {
    let up = |kind: &str| devices.iter().any(|d| d.kind == kind && d.state == "connected");
    let wired = up("ethernet");
    (wired || up("wifi"), wired)
}

/// The Game drive step's fact. `in_use`: whether a folder is one of the game folders.
pub fn drive(drives: &[crate::storage::Drive], in_use: &dyn Fn(&Path) -> bool) -> Drive {
    let parts = || drives.iter().flat_map(|d| d.partitions.iter().map(move |p| (d, p)));
    if parts().any(|(d, p)| crate::storage::offers_games(d, p, in_use)) {
        return Drive::Offered;
    }
    let used = parts().any(|(d, p)| !d.system && p.mountpoint.as_deref().is_some_and(|mp| mp.starts_with('/') && in_use(&crate::storage::games_dir(mp))));
    if used { Drive::Used } else { Drive::None }
}

/// Whether `step` shows for a PC with `facts`.
pub fn shows(step: Step, facts: &Facts) -> bool {
    match step {
        Step::Network => facts.network && !facts.wired,
        Step::TimeZone => facts.time,
        Step::Controllers => facts.pads == 0,
        Step::GameDrive => facts.drive != Drive::None,
        Step::Finish => true,
    }
}

/// The steps that show, in order.
pub fn steps(facts: &Facts) -> Vec<Step> {
    ORDER.into_iter().filter(|s| shows(*s, facts)).collect()
}

/// Whether the player did what `step` asks: then its button says Next, otherwise Skip.
pub fn ready(step: Step, facts: &Facts) -> bool {
    match step {
        Step::Network => facts.online,
        // UTC is what a PC has when nobody chose a zone.
        Step::TimeZone => !facts.zone.is_empty() && !matches!(facts.zone.as_str(), "UTC" | "Etc/UTC" | "Etc/UCT" | "UCT"),
        Step::Controllers => facts.pads > 0,
        Step::GameDrive => facts.drive == Drive::Used,
        Step::Finish => true,
    }
}

/// Where the setup is: the steps it shows, and the one on screen.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Machine {
    steps: Vec<Step>,
    at: usize,
}

impl Machine {
    /// The steps for `facts`. `saved`: the step the setup was on when the PC restarted. The
    /// setup comes back to it; when the new facts skip it, to the next step that shows.
    pub fn start(facts: &Facts, saved: Option<Step>) -> Machine {
        let steps = steps(facts);
        let at = match saved {
            Some(saved) => steps.iter().position(|s| s.rank() >= saved.rank()).unwrap_or(steps.len() - 1),
            None => 0,
        };
        Machine { steps, at }
    }

    pub fn step(&self) -> Step {
        self.steps[self.at]
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// The step on screen, from 0.
    pub fn at(&self) -> usize {
        self.at
    }

    /// Skip or Next: the next step. Finish stays.
    pub fn next(&mut self) -> Step {
        self.at = (self.at + 1).min(self.steps.len() - 1);
        self.step()
    }

    /// Back: the step before. False on the first step.
    pub fn back(&mut self) -> bool {
        if self.at == 0 {
            return false;
        }
        self.at -= 1;
        true
    }
}

/// A button under the step.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Back,
    Skip,
    Next,
    Done,
}

impl Button {
    pub fn label(self) -> &'static str {
        match self {
            Button::Back => "Back",
            Button::Skip => "Skip",
            Button::Next => "Next",
            Button::Done => "Done",
        }
    }
}

/// The buttons of `step`, the main one last. `first`: no step before it; `ready`: the player did
/// what it asks.
pub fn buttons(step: Step, first: bool, ready: bool) -> Vec<Button> {
    let mut b = Vec::new();
    if !first {
        b.push(Button::Back);
    }
    match step {
        Step::Finish => b.push(Button::Done),
        _ if ready => b.push(Button::Next),
        _ => b.push(Button::Skip),
    }
    b
}

/// The title and the text over a step's rows.
pub fn text(step: Step) -> (&'static str, &'static str) {
    match step {
        Step::Network => ("Connect to the internet", "Games, artwork and updates need it. Choose your Wi-Fi network, or plug in a network cable."),
        Step::TimeZone => ("Choose your time zone", "The clock follows it. The installer may have set it already: check it here."),
        Step::Controllers => ("Connect a controller", "Plug it in with a USB cable, or pair it over Bluetooth."),
        Step::GameDrive => ("Use a drive for games", "Choose a drive to keep games on. The launcher makes a Games folder on it and deletes nothing."),
        Step::Finish => ("You're all set", "You can change all of this later in Settings."),
    }
}

/// What shows after the welcome screen or after the setup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Setup,
    Notice,
    Home,
}

/// The screen after the welcome screen: the setup first when it is due, then the boot health
/// notice when there is one, then Home. After the setup, `setup_due` is false.
pub fn after(setup_due: bool, notice: bool) -> Screen {
    match (setup_due, notice) {
        (true, _) => Screen::Setup,
        (false, true) => Screen::Notice,
        (false, false) => Screen::Home,
    }
}

/// A notice read later may show now: not over the welcome screen, the setup or another notice.
pub fn notice_now(welcome: bool, setup: bool, notice_shown: bool) -> bool {
    !welcome && !setup && !notice_shown
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PC on Wi-Fi with nothing set up: every step but Game drive.
    fn fresh() -> Facts {
        Facts { network: true, time: true, zone: "UTC".into(), ..Facts::default() }
    }

    #[test]
    fn setup_steps_follow_the_facts() {
        assert_eq!(
            steps(&fresh()),
            [
                Step::Network,
                Step::TimeZone,
                Step::Controllers,
                Step::Finish
            ]
        );
        let all = Facts {
            drive: Drive::Offered,
            ..fresh()
        };
        assert_eq!(steps(&all), ORDER);
        let cable = Facts { wired: true, online: true, ..fresh() };
        assert!(!steps(&cable).contains(&Step::Network), "online by cable");
        let wifi = Facts { online: true, ..fresh() };
        assert!(steps(&wifi).contains(&Step::Network), "only a cable skips the step");
        let no_nm = Facts { network: false, ..fresh() };
        assert!(!steps(&no_nm).contains(&Step::Network), "no NetworkManager");
        let no_time = Facts { time: false, ..fresh() };
        assert!(!steps(&no_time).contains(&Step::TimeZone));
        let pad = Facts { pads: 1, ..fresh() };
        assert!(
            !steps(&pad).contains(&Step::Controllers),
            "a controller is connected"
        );
        let used = Facts {
            drive: Drive::Used,
            ..fresh()
        };
        assert!(
            steps(&used).contains(&Step::GameDrive),
            "a second run shows the drive in use"
        );
        let bare = Facts::default();
        assert_eq!(
            steps(&bare),
            [Step::Controllers, Step::Finish],
            "Controllers and Finish always can show"
        );
    }

    #[test]
    fn setup_a_step_is_ready_when_its_work_is_done() {
        let f = fresh();
        assert!(!ready(Step::Network, &f));
        assert!(ready(Step::Network, &Facts { online: true, ..f.clone() }));
        assert!(!ready(Step::TimeZone, &f), "UTC: nobody chose a zone");
        assert!(!ready(Step::TimeZone, &Facts { zone: String::new(), ..f.clone() }));
        assert!(ready(Step::TimeZone, &Facts { zone: "Europe/Berlin".into(), ..f.clone() }));
        assert!(!ready(Step::Controllers, &f));
        assert!(ready(
            Step::Controllers,
            &Facts {
                pads: 2,
                ..f.clone()
            }
        ));
        assert!(!ready(
            Step::GameDrive,
            &Facts {
                drive: Drive::Offered,
                ..f.clone()
            }
        ));
        assert!(ready(
            Step::GameDrive,
            &Facts {
                drive: Drive::Used,
                ..f.clone()
            }
        ));
        assert!(ready(Step::Finish, &f));
    }

    #[test]
    fn setup_skip_next_and_back_move_through_the_steps() {
        let mut m = Machine::start(&fresh(), None);
        assert_eq!((m.step(), m.at()), (Step::Network, 0));
        assert!(!m.back(), "nothing before the first step");
        assert_eq!(m.next(), Step::TimeZone);
        assert_eq!(m.next(), Step::Controllers);
        assert!(m.back());
        assert_eq!(m.step(), Step::TimeZone);
        assert_eq!(m.next(), Step::Controllers);
        assert_eq!(m.next(), Step::Finish);
        assert_eq!(m.next(), Step::Finish, "Finish stays until Done");
        assert_eq!(m.steps(), [Step::Network, Step::TimeZone, Step::Controllers, Step::Finish]);
    }

    #[test]
    fn setup_resumes_at_the_saved_step_after_a_restart() {
        let facts = Facts {
            drive: Drive::Offered,
            ..fresh()
        };
        let m = Machine::start(&facts, Some(Step::GameDrive));
        assert_eq!(m.step(), Step::GameDrive);
        assert_eq!(m.at(), 3, "the steps before it show as done");
        // A controller came, or the cable: the saved step is gone, so the next one that shows.
        let pad = Facts { pads: 1, ..fresh() };
        assert_eq!(
            Machine::start(&pad, Some(Step::Controllers)).step(),
            Step::Finish
        );
        let cable = Facts {
            wired: true,
            online: true,
            ..fresh()
        };
        assert_eq!(
            Machine::start(&cable, Some(Step::Network)).step(),
            Step::TimeZone
        );
        assert_eq!(
            Machine::start(&fresh(), Some(Step::Finish)).step(),
            Step::Finish
        );
    }

    #[test]
    fn setup_buttons_say_skip_until_the_step_is_done() {
        assert_eq!(
            buttons(Step::Network, true, false),
            [Button::Skip]
        );
        assert_eq!(
            buttons(Step::Network, true, true),
            [Button::Next]
        );
        assert_eq!(
            buttons(Step::TimeZone, false, false),
            [Button::Back, Button::Skip]
        );
        assert_eq!(
            buttons(Step::Finish, false, true),
            [Button::Back, Button::Done]
        );
    }

    #[test]
    fn setup_comes_before_the_notice_and_both_before_home() {
        assert_eq!(after(true, true), Screen::Setup, "the notice waits for the setup");
        assert_eq!(after(true, false), Screen::Setup);
        assert_eq!(after(false, true), Screen::Notice, "instead of the setup when it is done");
        assert_eq!(after(false, false), Screen::Home);
        // A notice the health check writes later.
        assert!(notice_now(false, false, false));
        assert!(!notice_now(true, false, false), "not over the welcome screen");
        assert!(!notice_now(false, true, false), "not over the setup");
        assert!(!notice_now(false, false, true), "one at a time");
    }

    fn device(kind: &str, state: &str) -> crate::network::Device {
        crate::network::Device { name: "x".into(), kind: kind.into(), state: state.into(), connection: None }
    }

    #[test]
    fn setup_online_by_cable_or_wifi() {
        assert_eq!(online(&[]), (false, false));
        assert_eq!(online(&[device("ethernet", "unavailable"), device("wifi", "disconnected")]), (false, false));
        assert_eq!(online(&[device("ethernet", "connected"), device("wifi", "disconnected")]), (true, true));
        assert_eq!(online(&[device("ethernet", "unavailable"), device("wifi", "connected")]), (true, false));
        assert_eq!(online(&[device("loopback", "connected (externally)")]), (false, false), "lo is not a network");
    }

    #[test]
    fn setup_game_drive_from_the_drives() {
        use crate::storage::{Drive as Disk, Partition};
        let part = |mp: Option<&str>| Partition { path: "/dev/x1".into(), bytes: 1, fstype: Some("ext4".into()), label: None, mountpoint: mp.map(String::from) };
        let disk = |system: bool, mp: Option<&str>| Disk { path: "/dev/x".into(), name: "x".into(), bytes: 1, removable: !system, system, partitions: vec![part(mp)] };
        let none = |_: &std::path::Path| false;
        let games = |p: &std::path::Path| p == std::path::Path::new("/run/media/u/USB/Games");
        assert_eq!(drive(&[disk(true, Some("/"))], &none), Drive::None, "only the system's drive");
        assert_eq!(drive(&[disk(true, Some("/")), disk(false, None)], &none), Drive::None, "not mounted");
        assert_eq!(drive(&[disk(true, Some("/")), disk(false, Some("/run/media/u/USB"))], &none), Drive::Offered);
        assert_eq!(drive(&[disk(false, Some("/run/media/u/USB"))], &games), Drive::Used);
        let two = [disk(false, Some("/run/media/u/USB")), disk(false, Some("/run/media/u/HDD"))];
        assert_eq!(drive(&two, &games), Drive::Offered, "one drive is still free");
    }

    #[test]
    fn setup_step_names_survive_in_the_config() {
        for step in ORDER {
            let json = serde_json::to_string(&step).unwrap();
            assert_eq!(serde_json::from_str::<Step>(&json).unwrap(), step, "{json}");
        }
        assert_eq!(serde_json::to_string(&Step::TimeZone).unwrap(), r#""time-zone""#);
    }
}
