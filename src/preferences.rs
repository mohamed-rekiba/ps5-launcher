//! The user's emulator preferences, stored in config.json next to the other settings
//! (docs/plans/data-driven-emulators.md §6, with the choices of docs/plans/addons.md):
//! each emulator's on/off switch, build and settings, the user's emulator for each console,
//! and the emulator chosen for single games.
//!
//! The emulator addons define what a setting means; this only stores values. A value the
//! launcher cannot use, or one for an emulator that is off, missing or broken, is kept as it
//! is, so nothing the user set is lost while an addon is away.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One emulator's preferences.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct EmulatorPrefs {
    /// The user's on/off switch. "Reset to default" on the addon's files does not touch it.
    pub enabled: bool,
    /// The build to run; None until the user, or the migration, chose one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<BuildSource>,
    /// Setting values by the addon's setting key.
    pub settings: BTreeMap<String, SettingValue>,
    /// The text the user typed for an argument-list setting (today: KytyPS5's extra arguments),
    /// by setting key: the same arguments as in `settings`, as they were written.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub texts: BTreeMap<String, String>,
}

impl Default for EmulatorPrefs {
    fn default() -> Self {
        EmulatorPrefs { enabled: true, source: None, settings: BTreeMap::new(), texts: BTreeMap::new() }
    }
}

/// Which build of an emulator runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuildSource {
    /// The one the launcher installs and updates.
    Managed,
    /// The user's own executable, as the user wrote it (it can start with ~). It is kept when
    /// the file is missing: the launcher says so instead of picking another build.
    Custom { executable: String },
}

/// One setting's stored value. JSON keeps its type: true, 1920, "Mailbox", ["--a", "b"].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    U32(u32),
    String(String),
    Argv(Vec<String>),
    /// Anything else, kept as it is: a hand-edited value, or one from a newer launcher.
    Other(serde_json::Value),
}

/// Sets or clears one choice of emulator (`id` None: no choice).
#[allow(dead_code, reason = "used by Config::choose_for_console and choose_for_game, which wait for phases 3 and 5")]
pub fn choose(map: &mut BTreeMap<String, String>, key: &str, id: Option<&str>) {
    match id {
        Some(id) => map.insert(key.to_string(), id.to_string()),
        None => map.remove(key),
    };
}

/// Arguments as text that `sessions::shell_split` splits back into the same arguments.
pub fn join_args(args: &[String]) -> String {
    let plain = |a: &str| !a.is_empty() && !a.chars().any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '\\'));
    let quote = |a: &str| format!("'{}'", a.replace('\\', "\\\\").replace('\'', "\\'"));
    args.iter().map(|a| if plain(a) { a.clone() } else { quote(a) }).collect::<Vec<_>>().join(" ")
}
