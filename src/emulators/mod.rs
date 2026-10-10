//! Emulators from a configuration file (docs/plans/data-driven-emulators.md), phase 1: the
//! registry is read-only. It loads the embedded manifest (assets/emulators.yaml) and the user's
//! override (~/.config/ps5-launcher/emulators.yaml), validates both and resolves which emulator
//! runs a console's games. Nothing in the app uses it yet: kyty.rs, shad.rs, sessions.rs and the
//! rest still do all the work.

pub mod manifest;
pub mod registry;
pub mod schema;
mod yaml;

#[cfg(test)]
mod tests;
