//! Emulators as addons (docs/plans/addons.md, which amends docs/plans/data-driven-emulators.md),
//! phase 1: read-only. Each emulator is a folder in the user's data folder with its own
//! emulator.yaml; the defaults built into the launcher are copied there. Nothing in the app uses
//! the registry yet: kyty.rs, shad.rs, sessions.rs and the rest still do all the work.

pub mod bundle;
pub mod discovery;
pub mod document;
pub mod lifecycle;
pub mod manifest;
pub mod registry;
pub mod schema;
pub mod startup;
mod yaml;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod lifecycle_tests;

use std::fmt;

/// Something the launcher could not do or use, or that the user should know, for the app to
/// show: what it is about ("emulators/kyty", "addons-state.yaml") and what happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub subject: String,
    pub message: String,
}

impl Problem {
    pub fn new(subject: impl Into<String>, message: impl Into<String>) -> Problem {
        Problem { subject: subject.into(), message: message.into() }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.subject, self.message)
    }
}
