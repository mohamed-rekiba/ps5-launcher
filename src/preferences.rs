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
    /// Fields this launcher does not know (from a newer one), kept as they are.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for EmulatorPrefs {
    fn default() -> Self {
        EmulatorPrefs { enabled: true, source: None, settings: BTreeMap::new(), texts: BTreeMap::new(), extra: BTreeMap::new() }
    }
}

/// Every emulator's preferences, by emulator id. It reads as a map of the readable ones; an
/// entry this launcher cannot read (a build kind from a newer launcher, a hand edit) is kept
/// as raw JSON and written back unchanged, so one entry never costs the rest of the config.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Emulators {
    readable: BTreeMap<String, EmulatorPrefs>,
    unreadable: BTreeMap<String, serde_json::Value>,
}

impl Emulators {
    /// Whether the entry for `id` could not be read.
    pub fn is_unreadable(&self, id: &str) -> bool {
        self.unreadable.contains_key(id)
    }

    /// The entry for `id` to change, made when missing; None when it could not be read.
    pub fn readable_mut(&mut self, id: &str) -> Option<&mut EmulatorPrefs> {
        (!self.is_unreadable(id)).then(|| self.readable.entry(id.to_string()).or_default())
    }
}

impl std::ops::Deref for Emulators {
    type Target = BTreeMap<String, EmulatorPrefs>;
    fn deref(&self) -> &Self::Target {
        &self.readable
    }
}

impl Serialize for Emulators {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error, SerializeMap};
        let mut all: BTreeMap<&String, serde_json::Value> = BTreeMap::new();
        for (id, prefs) in &self.readable {
            all.insert(id, serde_json::to_value(prefs).map_err(S::Error::custom)?);
        }
        for (id, raw) in &self.unreadable {
            all.insert(id, raw.clone());
        }
        let mut map = s.serialize_map(Some(all.len()))?;
        for (id, value) in all {
            map.serialize_entry(id, &value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Emulators {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut out = Emulators::default();
        for (id, raw) in BTreeMap::<String, serde_json::Value>::deserialize(d)? {
            match serde_json::from_value::<EmulatorPrefs>(raw.clone()) {
                Ok(prefs) => {
                    out.readable.insert(id, prefs);
                }
                Err(e) => {
                    crate::log!("config.json: the preferences of emulator \"{id}\" cannot be read ({e}); they are kept as they are");
                    out.unreadable.insert(id, raw);
                }
            }
        }
        Ok(out)
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
