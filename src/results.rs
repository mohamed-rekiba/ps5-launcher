//! Your own KytyPS5 results, saved on this PC in ~/.config/ps5-launcher/results.json.
//! A result becomes the game's tag here right away, and is shared with the community list
//! (KytyPS5's game status reports) in batches, from Settings.
//!
//! Each result and each skipped prompt records the emulator (its addon id) and the build the
//! game ran on. Older files have only the build, in `kyty`: reading them takes the emulator from
//! the title id. Results stay keyed by title id until compatibility is keyed by emulator
//! (phase 6 of docs/plans/data-driven-emulators.md).

use crate::compat::Status;
use crate::util::{atomic_write, config_dir};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct MyResult {
    pub name: String,
    /// Same spelling as the community list: "InGame", "MainMenu", "Logo", "DoesntBoot".
    pub status: String,
    /// The emulator's addon id; "" when an old result's title id names no known console.
    pub emulator: String,
    /// The emulator build it was played on (older files: `kyty`).
    #[serde(alias = "kyty")]
    pub emulator_version: String,
    pub rated: f64,
    /// When its report was opened for sharing; 0 = not yet.
    pub shared: f64,
    /// The emulator log saved with this result (the crash log, if the game crashed).
    pub log: String,
}

impl MyResult {
    pub fn status(&self) -> Option<Status> {
        Status::parse(&self.status)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Results {
    /// By title ID.
    pub games: BTreeMap<String, MyResult>,
    /// Games whose rating prompt was skipped, by title ID: not asked again on that build.
    pub skipped: BTreeMap<String, Skipped>,
    /// Last "you have results to share" reminder.
    pub reminded: f64,
}

/// The emulator build a game's rating prompt was skipped on.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(from = "SkippedFormat")]
pub struct Skipped {
    /// The emulator's addon id; "" when unresolved.
    pub emulator: String,
    pub emulator_version: String,
}

/// A skip as stored: older files have only the build.
#[derive(Deserialize)]
#[serde(untagged)]
enum SkippedFormat {
    Build(String),
    Full {
        #[serde(default)]
        emulator: String,
        #[serde(default)]
        emulator_version: String,
    },
}

impl From<SkippedFormat> for Skipped {
    fn from(f: SkippedFormat) -> Skipped {
        match f {
            SkippedFormat::Build(emulator_version) => Skipped { emulator: String::new(), emulator_version },
            SkippedFormat::Full { emulator, emulator_version } => Skipped { emulator, emulator_version },
        }
    }
}

/// The emulator an old record without one ran on: before addons, PS5 games (PPSA…) ran on
/// KytyPS5 and PS4 games (CUSA…) on shadPS4. A fact about old files, not today's default.
fn legacy_emulator(title_id: &str) -> &'static str {
    crate::platform::Platform::of_title_id(title_id).map_or("", crate::platform::Platform::emulator_id)
}

/// Remind about unshared results at most this often.
pub const REMIND_EVERY: f64 = 7.0 * 24.0 * 3600.0;
/// Reports opened in one go; the rest wait for the next batch.
pub const BATCH: usize = 10;
/// Saved game logs kept on disk (oldest removed first).
const KEEP_LOGS: usize = 40;

/// Keep a copy of a game's emulator log, which the next launch of that game would overwrite.
/// Returns the saved copy's path.
pub fn save_log(log: &std::path::Path, title_id: &str, crashed: bool) -> Option<std::path::PathBuf> {
    let dir = crate::util::data_dir().join("game-logs");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("{title_id}-{}{}.log", crate::util::now_secs() as u64, if crashed { "-crash" } else { "" });
    let saved = dir.join(name);
    std::fs::copy(log, &saved).ok()?;
    // Oldest first: file names sort by title ID, so sort by modification time.
    let mut logs: Vec<_> = std::fs::read_dir(&dir).ok()?.flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    logs.sort();
    for (_, old) in logs.iter().take(logs.len().saturating_sub(KEEP_LOGS)) {
        let _ = std::fs::remove_file(old);
    }
    Some(saved)
}

fn path() -> std::path::PathBuf {
    config_dir().join("results.json")
}

pub fn load() -> Results {
    Results::from_slice(&std::fs::read(path()).unwrap_or_default())
}

impl Results {
    /// The results in a results.json's bytes, old ones with their emulator; none when the
    /// bytes are not results.
    pub fn from_slice(bytes: &[u8]) -> Results {
        let mut r: Results = serde_json::from_slice(bytes).unwrap_or_default();
        for (tid, game) in &mut r.games {
            if game.emulator.is_empty() {
                game.emulator = legacy_emulator(tid).into();
            }
        }
        for (tid, skip) in &mut r.skipped {
            if skip.emulator.is_empty() {
                skip.emulator = legacy_emulator(tid).into();
            }
        }
        r
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).unwrap_or_default()
    }

    pub fn save(&self) {
        let j = self.to_json();
        if !j.is_empty() {
            let _ = atomic_write(&path(), &j);
        }
    }

    /// Save a result: `emulator` (its addon id) on build `version`.
    #[allow(clippy::too_many_arguments, reason = "one result's fields; a struct would only rename them")]
    pub fn rate(&mut self, title_id: &str, name: &str, status: Status, emulator: &str, version: &str, now: f64, log: &str) {
        self.skipped.remove(title_id);
        self.games.insert(title_id.to_string(), MyResult {
            name: name.into(), status: status.key().into(), emulator: emulator.into(), emulator_version: version.into(), rated: now, shared: 0.0, log: log.into(),
        });
    }

    pub fn skip(&mut self, title_id: &str, emulator: &str, version: &str) {
        self.skipped.insert(title_id.to_string(), Skipped { emulator: emulator.into(), emulator_version: version.into() });
    }

    /// Ask for a rating after playing, unless this game was rated or skipped on this emulator
    /// and build. A rating on an unknown build (`version` "") counts for the emulator.
    pub fn should_ask(&self, title_id: &str, emulator: &str, version: &str) -> bool {
        !title_id.is_empty()
            && self.games.get(title_id).is_none_or(|r| r.emulator != emulator || (!version.is_empty() && r.emulator_version != version))
            && self.skipped.get(title_id).is_none_or(|k| k.emulator != emulator || k.emulator_version != version)
    }

    pub fn unshared(&self) -> Vec<(&String, &MyResult)> {
        self.games.iter().filter(|(_, r)| r.shared == 0.0 && r.status().is_some()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rating_skipping_and_sharing() {
        let mut r = Results::default();
        assert!(r.should_ask("PPSA1", "kyty", "build-1"));
        r.skip("PPSA1", "kyty", "build-1");
        assert!(!r.should_ask("PPSA1", "kyty", "build-1"));
        assert!(r.should_ask("PPSA1", "kyty", "build-2"), "a new KytyPS5 build asks again");
        assert!(r.should_ask("PPSA1", "other-ps5", "build-1"), "another emulator asks again");
        r.rate("PPSA1", "Game", Status::InGame, "kyty", "build-2", 1.0, "");
        assert!(!r.should_ask("PPSA1", "kyty", "build-2"));
        assert!(!r.should_ask("PPSA1", "kyty", ""), "an unknown build does not ask again");
        assert!(r.should_ask("PPSA1", "other-ps5", "build-2"));
        assert_eq!(r.unshared().len(), 1);
        r.games.get_mut("PPSA1").unwrap().shared = 2.0;
        assert!(r.unshared().is_empty());
        r.rate("PPSA1", "Game", Status::Logo, "kyty", "build-3", 3.0, "");
        assert_eq!(r.unshared().len(), 1, "a new rating is shared again");
        assert!(!r.should_ask("", "kyty", "build-3"));
    }

    #[test]
    fn old_results_load_with_their_emulator() {
        let old = br#"{"games": {
            "PPSA01234": {"name": "A", "status": "InGame", "kyty": "KytyPS5-2026-09-29-59a1760", "rated": 5.0, "shared": 6.0, "log": "/l/a.log"},
            "CUSA00001": {"name": "B", "status": "Logo", "kyty": "0.9.0"},
            "NPUB30000": {"name": "C", "status": "Weird", "kyty": "x"}},
            "skipped": {"PPSA09999": "build-1", "CUSA00002": "custom build", "XXXX00003": ""},
            "reminded": 3.0}"#;
        let r = Results::from_slice(old);
        let game = |tid: &str| r.games[tid].clone();
        assert_eq!(game("PPSA01234"), MyResult { name: "A".into(), status: "InGame".into(), emulator: "kyty".into(),
            emulator_version: "KytyPS5-2026-09-29-59a1760".into(), rated: 5.0, shared: 6.0, log: "/l/a.log".into() });
        assert_eq!((game("CUSA00001").emulator.as_str(), game("CUSA00001").emulator_version.as_str()), ("shadps4", "0.9.0"));
        assert_eq!((game("NPUB30000").emulator.as_str(), game("NPUB30000").status.as_str()), ("", "Weird"), "an unknown console is kept, unresolved");
        assert_eq!(r.skipped["PPSA09999"], Skipped { emulator: "kyty".into(), emulator_version: "build-1".into() });
        assert_eq!(r.skipped["CUSA00002"], Skipped { emulator: "shadps4".into(), emulator_version: "custom build".into() });
        assert_eq!(r.skipped["XXXX00003"], Skipped { emulator: "".into(), emulator_version: "".into() });
        assert_eq!(r.reminded, 3.0);
        assert!(!r.should_ask("PPSA09999", "kyty", "build-1"), "an old skip still counts");
        assert!(!r.should_ask("PPSA01234", "kyty", "KytyPS5-2026-09-29-59a1760"), "an old rating still counts");
        let json: serde_json::Value = serde_json::from_slice(&r.to_json()).unwrap();
        assert_eq!(json["games"]["PPSA01234"]["emulator_version"], "KytyPS5-2026-09-29-59a1760");
        assert_eq!(json["games"]["PPSA01234"].get("kyty"), None);
        assert_eq!(json["skipped"]["CUSA00002"], serde_json::json!({"emulator": "shadps4", "emulator_version": "custom build"}));
        assert_eq!(Results::from_slice(&r.to_json()), r, "loading again changes nothing");
        assert_eq!(Results::from_slice(b"not json"), Results::default());
    }

    #[test]
    fn a_rating_keeps_the_emulator_it_was_given() {
        let mut r = Results::default();
        r.rate("CUSA00001", "B", Status::InGame, "ps4-lab", "1.2", 1.0, "");
        r.skip("PPSA00001", "other-ps5", "7");
        let back = Results::from_slice(&r.to_json());
        assert_eq!(back.games["CUSA00001"].emulator, "ps4-lab", "a recorded emulator is never replaced by the title's guess");
        assert_eq!(back.skipped["PPSA00001"], Skipped { emulator: "other-ps5".into(), emulator_version: "7".into() });
    }
}
