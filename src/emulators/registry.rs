//! The registry: the emulators the launcher can use, and which one runs a game.

use super::bundle::{DefaultSource, Embedded};
use super::document::{self, Issue, Version};
use super::manifest::*;
use std::fmt;

/// The emulators the launcher knows.
#[derive(Clone, Debug, PartialEq)]
pub struct Registry {
    defaults: Defaults,
    emulators: Vec<Emulator>,
}

/// Why a game has no emulator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    Unknown(String),
    Disabled(EmulatorId),
    WrongConsole(EmulatorId, Console),
    NoDefault(Console),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ResolveError::Unknown(id) => write!(f, "there is no emulator \"{id}\""),
            ResolveError::Disabled(id) => write!(f, "the emulator \"{id}\" is disabled"),
            ResolveError::WrongConsole(id, c) => write!(f, "the emulator \"{id}\" does not run {} games", c.key().to_uppercase()),
            ResolveError::NoDefault(c) => write!(f, "no emulator is set for {} games", c.key().to_uppercase()),
        }
    }
}

impl Registry {
    /// The default addons built into the launcher, read as they ship.
    pub fn embedded() -> Result<Registry, (String, Vec<Issue>)> {
        let source = Embedded;
        let mut emulators = Vec::new();
        for addon in source.emulators() {
            let text = addon.document().unwrap_or_default();
            emulators.push(document::parse(text, Version::current()).map_err(|e| (addon.id.clone(), e))?);
        }
        Ok(Registry { defaults: source.console_defaults(), emulators })
    }

    /// Every emulator, the disabled ones too.
    pub fn emulators(&self) -> &[Emulator] {
        &self.emulators
    }

    pub fn get(&self, id: &str) -> Option<&Emulator> {
        self.emulators.iter().find(|e| e.id.as_str() == id)
    }

    pub fn default_for(&self, console: Console) -> Option<&EmulatorId> {
        self.defaults.get(console)
    }

    /// The emulator that runs a game: the game's own choice when it has one, else the
    /// console's default. A choice that is unknown, disabled or for another console is an
    /// error, never a quiet switch to another emulator.
    pub fn resolve(&self, console: Console, game_choice: Option<&str>) -> Result<&Emulator, ResolveError> {
        let id = match game_choice {
            Some(id) => id,
            None => self.default_for(console).ok_or(ResolveError::NoDefault(console))?.as_str(),
        };
        let e = self.get(id).ok_or_else(|| ResolveError::Unknown(id.to_string()))?;
        if !e.enabled {
            return Err(ResolveError::Disabled(e.id.clone()));
        }
        if !e.consoles.contains(&console) {
            return Err(ResolveError::WrongConsole(e.id.clone(), console));
        }
        Ok(e)
    }
}
