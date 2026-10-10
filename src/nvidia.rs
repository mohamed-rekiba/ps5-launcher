//! The NVIDIA driver offer on System → Display (docs/plans/ps5-launcher-os.md, "NVIDIA: which
//! cards, and how the driver gets installed"). It is a stepper:
//!
//! 1. Install: "Install the NVIDIA driver to play at full speed." Install driver, or Later.
//! 2. Secure Boot key (only with Secure Boot on): the helper queues the key; the launcher shows
//!    the password for the blue MOK screen; the player restarts and enrolls it.
//! 3. Download: after the restart the launcher checks the key by itself (`helper key-state`),
//!    then `helper switch nvidia` downloads the NVIDIA image. With Secure Boot off this comes
//!    right after step 1.
//! 4. Restart to finish.
//!
//! The step lives in the launcher's config, so it survives the restarts; `resume` runs at each
//! start in PS5 Launcher OS. The first-start setup (Phase 7) is to reuse this flow. The same page
//! offers the way back: "Use the open-source driver" on the NVIDIA image, and the switch back when
//! no NVIDIA card drives the screen any more.

use crate::gpu::{Card, Support, NVIDIA};
use crate::osupdate::{self, Task, UpdateEnd};
use crate::system::{Call, Ran};
use serde::{Deserialize, Serialize};

/// The system image of PS5 Launcher OS (the OS marker's IMAGE=).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Image {
    Main,
    Nvidia,
}

impl Image {
    pub fn parse(name: &str) -> Option<Image> {
        match name {
            "main" => Some(Image::Main),
            "nvidia" => Some(Image::Nvidia),
            _ => None,
        }
    }
}

/// The download the offer names. A fixed estimate: CI measures the real size of the NVIDIA
/// image's layers that the main image does not have, and this follows it.
pub const DOWNLOAD_MB: u32 = 600;

/// Where the flow is, kept in the launcher's config.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
#[serde(default)]
pub struct Flow {
    pub step: Step,
    /// "Later": the offer folds to one row on the page; the dot on Display stays.
    pub later: bool,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
#[serde(tag = "step", rename_all = "kebab-case")]
pub enum Step {
    #[default]
    None,
    /// The key was queued during the start with this boot ID: restart and enroll it.
    KeyQueued { boot: String },
    /// After the restart, the key is enrolled: the download comes next.
    KeyEnrolled,
    /// After the restart, the key is not enrolled: try again.
    KeyMissed,
    /// `to` was staged during the start with this boot ID: restart to finish.
    Staged { to: Image, boot: String },
}

/// What a start finds about the flow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resume {
    Nothing,
    /// The PC restarted into the image the flow staged.
    Done(Image),
    /// The PC restarted, but not into that image (the boot health check rolled back, or the
    /// staged image was replaced).
    NotSwitched(Image),
    /// The PC restarted after the key was queued: ask the helper whether it is enrolled now.
    CheckKey,
}

/// At a start: `image` runs now, `boot` is this start's ID.
pub fn resume(flow: &mut Flow, image: Image, boot: &str) -> Resume {
    match &flow.step {
        Step::Staged { to, boot: at } if at != boot => {
            let to = *to;
            flow.step = Step::None;
            if image == to { Resume::Done(to) } else { Resume::NotSwitched(to) }
        }
        Step::KeyQueued { boot: at } if at != boot => Resume::CheckKey,
        _ => Resume::Nothing,
    }
}

/// What `helper key-state` printed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyState {
    Enrolled,
    /// Queued, but the blue screen did not enroll it.
    Pending,
    Missing,
    /// Secure Boot does not check the driver: no key needed.
    SecureBootOff,
}

pub fn parse_key_state(output: &str) -> Result<KeyState, String> {
    match output.trim() {
        "enrolled" => Ok(KeyState::Enrolled),
        "pending" => Ok(KeyState::Pending),
        "missing" => Ok(KeyState::Missing),
        "secure-boot-off" => Ok(KeyState::SecureBootOff),
        other => Err(format!("unexpected answer from key-state: {other:?}")),
    }
}

/// The helper's answer about the key, after a restart.
pub fn key_checked(flow: &mut Flow, state: KeyState) {
    flow.step = match state {
        KeyState::Enrolled | KeyState::SecureBootOff => Step::KeyEnrolled,
        KeyState::Pending | KeyState::Missing => Step::KeyMissed,
    };
}

/// `helper key-state`: the NVIDIA image's key, read-only.
pub fn key_state_call() -> Call {
    osupdate::helper_call(Task::KeyState)
}

/// How a switch ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SwitchEnd {
    /// Staged: it starts at the next restart.
    Staged,
    /// The key is queued: the blue MOK screen asks for this password at the next start.
    Password(String),
    Failed(String),
}

/// Switch to `to`. To the NVIDIA image: `helper switch nvidia`; when the key is required, or
/// queued earlier but not enrolled, `helper queue-key` gives a (new) password. When queue-key
/// finds the key enrolled already, the switch runs once more. `run` runs a call.
pub fn switch_flow(run: &dyn Fn(&Call) -> Result<Ran, String>, to: Image) -> SwitchEnd {
    match osupdate::stage_flow(run, Task::Switch(to), true) {
        UpdateEnd::Staged => SwitchEnd::Staged,
        UpdateEnd::Password(p) => SwitchEnd::Password(p),
        // The switch to main has no key gate, and the NVIDIA one queues a pending key again.
        UpdateEnd::KeyPending => SwitchEnd::Failed("the key still waits for the blue screen".into()),
        UpdateEnd::Failed(e) => SwitchEnd::Failed(e),
    }
}

/// A switch ended during the start with `boot`.
pub fn switched(flow: &mut Flow, to: Image, end: &SwitchEnd, boot: &str) {
    let step = match end {
        SwitchEnd::Staged => Step::Staged { to, boot: boot.to_string() },
        SwitchEnd::Password(_) => Step::KeyQueued { boot: boot.to_string() },
        SwitchEnd::Failed(_) => return,
    };
    *flow = Flow { step, later: false };
}

/// What the Display page shows about the driver.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Offer {
    Nothing,
    /// An NVIDIA card older than Turing: "Games will be very slow on this card."
    OldCard,
    /// An NVIDIA card not on NVIDIA's list: no offer.
    UnknownCard,
    /// Step 1.
    Install,
    /// Step 1, after "Later": one row.
    Later,
    /// Step 2: the password, then restart.
    KeyWaiting,
    /// Step 3: download.
    KeyEnrolled,
    /// Step 2 again: the key was not enrolled.
    KeyMissed,
    /// Step 4.
    Restart(Image),
    /// The NVIDIA image with an NVIDIA card: "Use the open-source driver".
    UseOpenSource,
    /// The NVIDIA image, and no NVIDIA card drives the screen any more.
    SwitchBack,
}

/// The offer for `image` (None: not PS5 Launcher OS), the card that drives the screen, and the
/// flow.
pub fn offer(image: Option<Image>, card: Option<&Card>, flow: &Flow) -> Offer {
    if let (Some(_), Step::Staged { to, .. }) = (image, &flow.step) {
        return Offer::Restart(*to);
    }
    let support = card.filter(|c| c.vendor == NVIDIA).map(|c| crate::gpu::nvidia_support(c.device));
    match (image, support) {
        // No screen found: not sure which GPU counts, so nothing changes.
        (Some(Image::Nvidia), _) if card.is_none() => Offer::Nothing,
        (Some(Image::Nvidia), Some(_)) => Offer::UseOpenSource,
        (Some(Image::Nvidia), None) => Offer::SwitchBack,
        (_, Some(Support::Old)) => Offer::OldCard,
        (_, Some(Support::Unknown)) => Offer::UnknownCard,
        (Some(Image::Main), Some(Support::Supported)) => match flow.step {
            Step::KeyQueued { .. } => Offer::KeyWaiting,
            Step::KeyEnrolled => Offer::KeyEnrolled,
            Step::KeyMissed => Offer::KeyMissed,
            Step::None | Step::Staged { .. } if flow.later => Offer::Later,
            Step::None | Step::Staged { .. } => Offer::Install,
        },
        _ => Offer::Nothing,
    }
}

/// A dot on Display: the player has something to do there.
pub fn dot(offer: Offer) -> bool {
    !matches!(offer, Offer::Nothing | Offer::OldCard | Offer::UnknownCard | Offer::UseOpenSource)
}

/// The stepper's step for `offer` (0 to 3), or None when it shows no stepper. `downloading`:
/// `helper switch nvidia` runs.
pub fn stepper(offer: Offer, downloading: bool) -> Option<usize> {
    match offer {
        Offer::Install | Offer::KeyEnrolled | Offer::KeyMissed if downloading => Some(2),
        Offer::Install => Some(0),
        Offer::KeyWaiting | Offer::KeyMissed => Some(1),
        Offer::KeyEnrolled => Some(2),
        Offer::Restart(Image::Nvidia) => Some(3),
        _ => None,
    }
}


/// The card's title and text, for the card named `name`.
pub fn card_text(offer: Offer, name: &str, downloading: bool) -> Option<(String, Vec<String>)> {
    let text = |title: &str, lines: &[&str]| Some((title.to_string(), lines.iter().map(|l| l.to_string()).collect()));
    if downloading && matches!(offer, Offer::Install | Offer::KeyEnrolled | Offer::KeyMissed) {
        return text(
            "Downloading the NVIDIA driver",
            &[&format!("About {DOWNLOAD_MB} MB. You can keep playing; the PC does not restart by itself."), "This page says when it is ready."],
        );
    }
    match offer {
        Offer::Install => text(
            &format!("{name} found"),
            &[
                &format!("Install the NVIDIA driver to play at full speed. Download: about {DOWNLOAD_MB} MB, needs a restart."),
                "With Secure Boot on, the PC first learns the driver's key: that takes one more restart and a USB keyboard.",
                "Today the card runs on the open-source driver. You can go back to it later on this page.",
            ],
        ),
        Offer::OldCard => text(
            &format!("{name} found"),
            &[
                "Games will be very slow on this card. The NVIDIA driver for it is not supported.",
                "The card runs on the open-source driver, which cannot raise its clock speed.",
            ],
        ),
        Offer::UnknownCard => text(
            &format!("{name} found"),
            &["Not sure this card is supported. The launcher does not know it, so it does not offer the NVIDIA driver.", "The card runs on the open-source driver."],
        ),
        Offer::KeyWaiting => text("Enroll the driver's key first", &["Secure Boot is on, so the PC must learn the driver's key before the download. The steps and the password are below."]),
        Offer::KeyMissed => text(
            "The key was not enrolled",
            &["Nothing was changed. Try again: the launcher queues the key with a new password, then you restart and enroll it on the blue screen."],
        ),
        Offer::KeyEnrolled => text(
            "Key enrolled",
            &[&format!("The PC accepted the driver's key. Next, download the NVIDIA driver: about {DOWNLOAD_MB} MB, then one more restart.")],
        ),
        Offer::Restart(Image::Nvidia) => text("Restart to finish", &["The PC starts with the NVIDIA driver after this restart. No keyboard is needed this time."]),
        Offer::Restart(Image::Main) => text("Restart to finish", &["The PC starts with the open-source driver after this restart."]),
        Offer::SwitchBack => text(
            "No NVIDIA card drives the screen",
            &["This PC runs the NVIDIA version of the system, but the screen's graphics card is not NVIDIA. Switch to the open-source driver."],
        ),
        Offer::Nothing | Offer::Later | Offer::UseOpenSource => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvidia(device: u16) -> Card {
        Card { vendor: NVIDIA, device, slot: None, driver: Some("nouveau".into()) }
    }

    fn amd() -> Card {
        Card { vendor: crate::gpu::AMD, device: 0x73df, slot: None, driver: Some("amdgpu".into()) }
    }

    const RTX_3070: u16 = 0x2484;
    const GTX_1060: u16 = 0x1C03;

    fn at(step: Step) -> Flow {
        Flow { step, later: false }
    }

    #[test]
    fn a_supported_card_on_the_main_image_gets_the_offer() {
        let card = nvidia(RTX_3070);
        assert_eq!(offer(Some(Image::Main), Some(&card), &Flow::default()), Offer::Install);
        let later = Flow { later: true, ..Flow::default() };
        assert_eq!(offer(Some(Image::Main), Some(&card), &later), Offer::Later);
        assert_eq!(offer(Some(Image::Main), Some(&card), &at(Step::KeyQueued { boot: "a".into() })), Offer::KeyWaiting);
        assert_eq!(offer(Some(Image::Main), Some(&card), &at(Step::KeyEnrolled)), Offer::KeyEnrolled);
        assert_eq!(offer(Some(Image::Main), Some(&card), &at(Step::KeyMissed)), Offer::KeyMissed);
        let staged = at(Step::Staged { to: Image::Nvidia, boot: "a".into() });
        assert_eq!(offer(Some(Image::Main), Some(&card), &staged), Offer::Restart(Image::Nvidia));
    }

    #[test]
    fn old_and_unknown_cards_get_no_offer() {
        assert_eq!(offer(Some(Image::Main), Some(&nvidia(GTX_1060)), &Flow::default()), Offer::OldCard);
        assert_eq!(offer(Some(Image::Main), Some(&nvidia(0x3FFF)), &Flow::default()), Offer::UnknownCard);
        // The messages show outside the OS too; the offer does not.
        assert_eq!(offer(None, Some(&nvidia(GTX_1060)), &Flow::default()), Offer::OldCard);
        assert_eq!(offer(None, Some(&nvidia(RTX_3070)), &Flow::default()), Offer::Nothing);
        assert_eq!(offer(Some(Image::Main), Some(&amd()), &Flow::default()), Offer::Nothing);
        assert_eq!(offer(Some(Image::Main), None, &Flow::default()), Offer::Nothing, "no screen");
    }

    #[test]
    fn the_nvidia_image_offers_the_way_back() {
        assert_eq!(offer(Some(Image::Nvidia), Some(&nvidia(RTX_3070)), &Flow::default()), Offer::UseOpenSource);
        assert_eq!(offer(Some(Image::Nvidia), Some(&amd()), &Flow::default()), Offer::SwitchBack, "the card changed");
        assert_eq!(offer(Some(Image::Nvidia), None, &Flow::default()), Offer::Nothing, "no screen: not sure");
        let staged = at(Step::Staged { to: Image::Main, boot: "a".into() });
        assert_eq!(offer(Some(Image::Nvidia), Some(&amd()), &staged), Offer::Restart(Image::Main));
    }

    #[test]
    fn a_dot_where_the_player_has_something_to_do() {
        for o in [Offer::Install, Offer::Later, Offer::KeyWaiting, Offer::KeyEnrolled, Offer::KeyMissed, Offer::Restart(Image::Nvidia), Offer::SwitchBack] {
            assert!(dot(o), "{o:?}");
        }
        for o in [Offer::Nothing, Offer::OldCard, Offer::UnknownCard, Offer::UseOpenSource] {
            assert!(!dot(o), "{o:?}");
        }
    }

    #[test]
    fn the_numbered_steps() {
        assert_eq!(stepper(Offer::Install, false), Some(0));
        assert_eq!(stepper(Offer::KeyWaiting, false), Some(1));
        assert_eq!(stepper(Offer::KeyMissed, false), Some(1));
        assert_eq!(stepper(Offer::KeyEnrolled, false), Some(2));
        assert_eq!(stepper(Offer::Install, true), Some(2), "downloading");
        assert_eq!(stepper(Offer::KeyEnrolled, true), Some(2));
        assert_eq!(stepper(Offer::Restart(Image::Nvidia), false), Some(3));
        for o in [Offer::Nothing, Offer::Later, Offer::OldCard, Offer::UnknownCard, Offer::UseOpenSource, Offer::SwitchBack, Offer::Restart(Image::Main)] {
            assert_eq!(stepper(o, false), None, "{o:?}");
        }
    }

    #[test]
    fn the_card_texts() {
        let (title, lines) = card_text(Offer::Install, "NVIDIA GeForce RTX 3070", false).unwrap();
        assert_eq!(title, "NVIDIA GeForce RTX 3070 found");
        assert_eq!(lines[0], "Install the NVIDIA driver to play at full speed. Download: about 600 MB, needs a restart.");
        let (_, lines) = card_text(Offer::OldCard, "NVIDIA GeForce GTX 1060", false).unwrap();
        assert_eq!(lines[0], "Games will be very slow on this card. The NVIDIA driver for it is not supported.");
        let (_, lines) = card_text(Offer::UnknownCard, "NVIDIA graphics card (10de:3fff)", false).unwrap();
        assert!(lines[0].starts_with("Not sure this card is supported"));
        assert_eq!(card_text(Offer::Restart(Image::Nvidia), "x", false).unwrap().0, "Restart to finish");
        assert_eq!(card_text(Offer::Install, "x", true).unwrap().0, "Downloading the NVIDIA driver");
        assert_eq!(card_text(Offer::Nothing, "x", false), None);
        assert_eq!(card_text(Offer::Later, "x", false), None, "folded to a row");
        assert_eq!(card_text(Offer::UseOpenSource, "x", false), None, "a row");
    }

    #[test]
    fn a_restart_into_the_staged_image_finishes_the_flow() {
        let mut flow = at(Step::Staged { to: Image::Nvidia, boot: "one".into() });
        assert_eq!(resume(&mut flow, Image::Main, "one"), Resume::Nothing, "not restarted yet");
        assert_eq!(flow.step, Step::Staged { to: Image::Nvidia, boot: "one".into() });
        assert_eq!(resume(&mut flow, Image::Nvidia, "two"), Resume::Done(Image::Nvidia));
        assert_eq!(flow, Flow::default());

        let mut flow = at(Step::Staged { to: Image::Nvidia, boot: "one".into() });
        assert_eq!(resume(&mut flow, Image::Main, "two"), Resume::NotSwitched(Image::Nvidia), "rolled back");
        assert_eq!(flow.step, Step::None, "the offer shows again");
    }

    #[test]
    fn a_restart_after_the_key_asks_whether_it_is_enrolled() {
        let mut flow = at(Step::KeyQueued { boot: "one".into() });
        assert_eq!(resume(&mut flow, Image::Main, "one"), Resume::Nothing, "the launcher restarted, not the PC");
        assert_eq!(resume(&mut flow, Image::Main, "two"), Resume::CheckKey);
        let mut enrolled = flow.clone();
        key_checked(&mut enrolled, KeyState::Enrolled);
        assert_eq!(enrolled.step, Step::KeyEnrolled);
        let mut off = flow.clone();
        key_checked(&mut off, KeyState::SecureBootOff);
        assert_eq!(off.step, Step::KeyEnrolled, "nothing to enroll");
        for state in [KeyState::Pending, KeyState::Missing] {
            let mut missed = flow.clone();
            key_checked(&mut missed, state);
            assert_eq!(missed.step, Step::KeyMissed, "{state:?}");
        }
        assert_eq!(resume(&mut at(Step::KeyEnrolled), Image::Main, "three"), Resume::Nothing);
        assert_eq!(resume(&mut Flow::default(), Image::Main, "three"), Resume::Nothing);
    }

    #[test]
    fn the_key_state() {
        assert_eq!(parse_key_state("enrolled\n"), Ok(KeyState::Enrolled));
        assert_eq!(parse_key_state("pending\n"), Ok(KeyState::Pending));
        assert_eq!(parse_key_state("missing\n"), Ok(KeyState::Missing));
        assert_eq!(parse_key_state("secure-boot-off\n"), Ok(KeyState::SecureBootOff));
        assert!(parse_key_state("").is_err());
        assert!(parse_key_state("yes").is_err());
        assert_eq!(key_state_call().args, [osupdate::HELPER, "key-state"]);
        assert!(key_state_call().secs > 120, "the helper reads the registry for the image's key");
    }

    #[test]
    fn a_switch_moves_the_flow_on() {
        let mut flow = Flow { step: Step::None, later: true };
        switched(&mut flow, Image::Nvidia, &SwitchEnd::Password("04718263".into()), "one");
        assert_eq!(flow, at(Step::KeyQueued { boot: "one".into() }), "Later is forgotten once the player installs");
        switched(&mut flow, Image::Nvidia, &SwitchEnd::Failed("no network".into()), "one");
        assert_eq!(flow, at(Step::KeyQueued { boot: "one".into() }), "a failure changes nothing");
        let mut flow = at(Step::KeyEnrolled);
        switched(&mut flow, Image::Nvidia, &SwitchEnd::Staged, "two");
        assert_eq!(flow, at(Step::Staged { to: Image::Nvidia, boot: "two".into() }));
        let mut back = Flow::default();
        switched(&mut back, Image::Main, &SwitchEnd::Staged, "two");
        assert_eq!(back, at(Step::Staged { to: Image::Main, boot: "two".into() }));
    }

    #[test]
    fn the_flow_survives_in_the_config() {
        for flow in [
            Flow::default(),
            Flow { step: Step::KeyQueued { boot: "b1".into() }, later: false },
            Flow { step: Step::KeyEnrolled, later: false },
            Flow { step: Step::KeyMissed, later: false },
            Flow { step: Step::Staged { to: Image::Nvidia, boot: "b2".into() }, later: false },
            Flow { step: Step::None, later: true },
        ] {
            let json = serde_json::to_string(&flow).unwrap();
            assert_eq!(serde_json::from_str::<Flow>(&json).unwrap(), flow, "{json}");
        }
        let json = serde_json::to_string(&Flow { step: Step::Staged { to: Image::Nvidia, boot: "b2".into() }, later: false }).unwrap();
        assert_eq!(json, r#"{"step":{"step":"staged","to":"nvidia","boot":"b2"},"later":false}"#);
        assert_eq!(serde_json::from_str::<Flow>("{}").unwrap(), Flow::default(), "an older config");
    }

    /// Answers each helper task with the next answer given for it, and records the tasks.
    fn helper(answers: &[(&'static str, i32, &'static str)]) -> (impl Fn(&Call) -> Result<Ran, String>, std::rc::Rc<std::cell::RefCell<Vec<String>>>) {
        let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let left = std::cell::RefCell::new(answers.to_vec());
        let log = asked.clone();
        let run = move |call: &Call| {
            let task = call.args[1..].join(" ");
            log.borrow_mut().push(task.clone());
            let mut left = left.borrow_mut();
            let i = left.iter().position(|(t, _, _)| *t == task).expect("an answer for each call");
            let (_, code, stdout) = left.remove(i);
            Ok(Ran { code: Some(code), stdout: stdout.into(), stderr: String::new() })
        };
        (run, asked)
    }

    #[test]
    fn switching_to_nvidia_with_secure_boot_off_or_the_key_enrolled() {
        let (run, asked) = helper(&[("switch nvidia", 0, "")]);
        assert_eq!(switch_flow(&run, Image::Nvidia), SwitchEnd::Staged);
        assert_eq!(*asked.borrow(), ["switch nvidia"]);
    }

    #[test]
    fn switching_to_nvidia_queues_the_key_first() {
        let (run, asked) = helper(&[("switch nvidia", 3, "key-required\n"), ("queue-key", 0, "04718263\n")]);
        assert_eq!(switch_flow(&run, Image::Nvidia), SwitchEnd::Password("04718263".into()));
        assert_eq!(*asked.borrow(), ["switch nvidia", "queue-key"]);
        // A key queued before (the installer's, or a missed blue screen) gets a new password.
        let (run, asked) = helper(&[("switch nvidia", 4, "key-pending\n"), ("queue-key", 0, "11112222\n")]);
        assert_eq!(switch_flow(&run, Image::Nvidia), SwitchEnd::Password("11112222".into()));
        assert_eq!(*asked.borrow(), ["switch nvidia", "queue-key"]);
        let (run, asked) = helper(&[("switch nvidia", 3, "key-required\n"), ("queue-key", 0, "key-enrolled\n"), ("switch nvidia", 0, "")]);
        assert_eq!(switch_flow(&run, Image::Nvidia), SwitchEnd::Staged);
        assert_eq!(*asked.borrow(), ["switch nvidia", "queue-key", "switch nvidia"]);
    }

    #[test]
    fn switching_back_needs_no_key() {
        let (run, asked) = helper(&[("switch main", 0, "")]);
        assert_eq!(switch_flow(&run, Image::Main), SwitchEnd::Staged);
        assert_eq!(*asked.borrow(), ["switch main"]);
        let (run, _) = helper(&[("switch main", 1, "")]);
        assert!(matches!(switch_flow(&run, Image::Main), SwitchEnd::Failed(e) if e.contains("switch main failed (exit 1)")));
        let (run, _) = helper(&[("switch nvidia", 1, "")]);
        assert!(matches!(switch_flow(&run, Image::Nvidia), SwitchEnd::Failed(_)));
        let (run, _) = helper(&[("switch nvidia", 3, "key-required\n"), ("queue-key", 0, "key-enrolled\n"), ("switch nvidia", 3, "key-required\n"), ("queue-key", 0, "key-enrolled\n")]);
        assert!(matches!(switch_flow(&run, Image::Nvidia), SwitchEnd::Failed(_)), "no endless loop");
    }
}
