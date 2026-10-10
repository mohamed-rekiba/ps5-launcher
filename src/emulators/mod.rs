//! Emulators as addons (docs/plans/addons.md, which amends docs/plans/data-driven-emulators.md),
//! phase 1: read-only. Each emulator is a folder in the user's data folder with its own
//! emulator.yaml; the defaults built into the launcher are copied there. Nothing in the app uses
//! the registry yet: kyty.rs, shad.rs, sessions.rs and the rest still do all the work.

pub mod bundle;
pub mod discovery;
pub mod document;
pub mod manifest;
pub mod registry;
pub mod schema;
mod yaml;

#[cfg(test)]
mod tests;
