//! The registry: the emulator addons a scan found, and which one runs a game.

use super::discovery::Scan;
use super::manifest::*;
use std::collections::BTreeSet;
use std::fmt;

/// The user's choices. Phase 2 stores them with the settings; the registry only reads them.
pub trait Preferences {
    /// The emulator the user chose for one game, by its title id.
    fn game_choice(&self, title_id: &str) -> Option<String>;
    /// The user's emulator for a console's games.
    fn console_choice(&self, console: Console) -> Option<String>;
    /// Whether the user left the emulator on.
    fn enabled(&self, id: &str) -> bool;
}

/// The emulators the launcher can use, as one start found them. It never changes.
#[derive(Clone, Debug, PartialEq)]
pub struct Registry {
    emulators: Vec<Emulator>,
    /// The folder names of the addons the scan rejected.
    rejected: BTreeSet<String>,
    defaults: Defaults,
}

/// Who chose the emulator that cannot run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChosenBy {
    /// The game's own choice.
    Game,
    /// The user's choice for the console.
    User,
    /// The launcher's default for the console.
    Launcher,
}


/// Why a chosen emulator cannot run a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// No addon has this id.
    Missing,
    /// The addon's folder is there, but the scan rejected it.
    Broken,
    /// The user turned it off, or its document says `enabled: false`.
    Off,
    /// It does not run this console's games.
    WrongConsole,
}

/// Why a game has no emulator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// An emulator was chosen and cannot run the game. The launcher never picks another.
    Unavailable { id: String, console: Console, chosen_by: ChosenBy, reason: Unavailable },
    /// Nobody chose an emulator for the console.
    NoDefault(Console),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ResolveError::Unavailable { id, console, chosen_by, reason } => {
                let console = console.key().to_uppercase();
                match chosen_by {
                    ChosenBy::Game => write!(f, "this game's emulator, \"{id}\", cannot run it: ")?,
                    ChosenBy::User => write!(f, "your {console} emulator, \"{id}\", cannot run it: ")?,
                    ChosenBy::Launcher => write!(f, "the default {console} emulator, \"{id}\", cannot run it: ")?,
                }
                match reason {
                    Unavailable::Missing => f.write_str("it is not installed"),
                    Unavailable::Broken => f.write_str("its emulator.yaml has a problem"),
                    Unavailable::Off => f.write_str("it is turned off"),
                    Unavailable::WrongConsole => write!(f, "it does not run {console} games"),
                }
            }
            ResolveError::NoDefault(c) => write!(f, "no emulator is set for {} games", c.key().to_uppercase()),
        }
    }
}

impl Registry {
    /// The addons of one scan, with the console defaults of the launcher's bundle.
    pub fn new(scan: Scan, defaults: Defaults) -> Registry {
        Registry {
            emulators: scan.addons.into_iter().map(|a| a.emulator).collect(),
            rejected: scan.rejected.into_iter().map(|r| r.folder).collect(),
            defaults,
        }
    }

    /// Every emulator found, in scan order, the disabled ones too.
    pub fn emulators(&self) -> &[Emulator] {
        &self.emulators
    }

    pub fn get(&self, id: &str) -> Option<&Emulator> {
        self.emulators.iter().find(|e| e.id.as_str() == id)
    }

    /// The emulator that runs a game: the game's choice, else the user's choice for the
    /// console, else the launcher's default for it. The first choice that exists decides; when
    /// that emulator cannot run the game, the error says so and no other is picked. Scan order
    /// never decides.
    pub fn resolve(&self, console: Console, title_id: &str, prefs: &dyn Preferences) -> Result<&Emulator, ResolveError> {
        let (id, chosen_by) = match (prefs.game_choice(title_id), prefs.console_choice(console)) {
            (Some(id), _) => (id, ChosenBy::Game),
            (None, Some(id)) => (id, ChosenBy::User),
            (None, None) => match self.defaults.get(console) {
                Some(id) => (id.to_string(), ChosenBy::Launcher),
                None => return Err(ResolveError::NoDefault(console)),
            },
        };
        let fail = |reason| Err(ResolveError::Unavailable { id: id.clone(), console, chosen_by, reason });
        let Some(e) = self.get(&id) else {
            return fail(if self.rejected.contains(&id) { Unavailable::Broken } else { Unavailable::Missing });
        };
        if !e.enabled || !prefs.enabled(&id) {
            return fail(Unavailable::Off);
        }
        if !e.consoles.contains(&console) {
            return fail(Unavailable::WrongConsole);
        }
        Ok(e)
    }
}
