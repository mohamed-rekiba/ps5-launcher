//! User settings, stored in ~/.config/ps5-launcher/config.json (same format as v1, plus the
//! per-emulator preferences of `crate::preferences`).
//!
//! Version 1 of the file keeps each emulator's preferences under `emulators`. The old
//! KytyPS5 and shadPS4 fields stay too: the launch path and Settings still use them (until
//! docs/plans/data-driven-emulators.md phases 3 and 5), and an older launcher can still read the
//! file. Reading a file fills the new form from the old fields where it has no value, then the
//! old fields from the new form; saving stores the old fields' edits in the new form.

use crate::emulators::manifest::Console;
use crate::emulators::registry::Preferences;
use crate::preferences::{choose, join_args, BuildSource, EmulatorPrefs, SettingValue};
use crate::util::{atomic_write, config_dir, expand_home};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The format of config.json this launcher writes.
pub const CONFIG_VERSION: u32 = 1;
/// The ids of the emulators the old fields belong to.
const KYTY: &str = "kyty";
const SHAD: &str = "shadps4";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// 0 (or missing): a file from before the per-emulator preferences.
    pub config_version: u32,
    /// Each emulator's preferences, by emulator id; the ones of a missing addon too.
    pub emulators: BTreeMap<String, EmulatorPrefs>,
    /// The user's emulator for each console's games, by console key ("ps5", "ps4").
    pub console_emulators: BTreeMap<String, String>,
    /// The emulator chosen for single games, by title id.
    pub game_emulators: BTreeMap<String, String>,
    /// KytyPS5's executable, as the launch path uses it this run.
    pub emulator: String,
    /// Your own shadPS4 build; empty = the one the launcher installs and updates.
    pub shad_emulator: String,
    pub game_dirs: Vec<String>,
    pub download_dir: String,
    pub install_dir: String,
    /// Continue sharing completed downloads until stopped or the launcher exits.
    pub seed_after_download: bool,
    pub library_compact: bool,
    pub fullscreen: bool,
    pub width: u32,
    pub height: u32,
    pub present_mode: String,
    /// What the emulator tells a PS5 game its screen is: "Title" (what a PS5 reports for that
    /// game), "FullHd" or "Uhd". Passed as --video-out-resolution.
    pub video_out: String,
    pub amd_cpu: bool,
    pub extra_args: String,
    pub sounds: bool,
    pub return_on_exit: bool,
    pub rawg_key: String,
    /// Output name (e.g. "DP-2"); empty = primary display; `display::ACTIVE` = the display
    /// the mouse pointer is on when the launcher starts.
    pub monitor: String,
    /// Keep the launcher-managed KytyPS5 on the latest official build.
    pub kyty_auto_update: bool,
    /// Install shadPS4 in the background and keep it on the latest official release.
    pub shad_auto_update: bool,
    /// Install new PS5 Launcher releases automatically (applied on the next start).
    pub app_auto_update: bool,
    /// Catalog game ids whose artwork comes from RAWG (chosen per game in the Options menu).
    pub rawg_art: Vec<i64>,
    /// System → Display (PS5 Launcher OS): where the NVIDIA driver's install is. It is kept
    /// across restarts, which the install needs.
    pub nvidia: crate::nvidia::Flow,
    /// System → Display: the screen output for the next session start; None is Automatic. The
    /// session wrapper reads it from session.conf (`screen::save`).
    pub session_output: Option<crate::screen::Output>,
    /// PS5 Launcher OS: the first-start setup ran to its end ("Done"). Settings → About can run
    /// it again.
    pub setup_done: bool,
    /// The setup's step on screen, so it comes back to it after a restart (the NVIDIA driver's
    /// key restarts the PC in the middle of it).
    pub setup_step: Option<crate::setup::Step>,
    /// How this run read the file; None for a Config that was never read. Crate-visible only so
    /// tests elsewhere can build a Config with `..old`.
    #[serde(skip)]
    pub(crate) read: Option<Read>,
}

/// What reading the file left for saving it.
#[derive(Clone, Debug)]
pub(crate) struct Read {
    /// The file's `config_version`, until the first save.
    version: u32,
    /// The managed KytyPS5's folder (`kyty::root`), which tells a managed path from the user's.
    kyty_root: PathBuf,
    /// The old KytyPS5 fields as last read or saved: a save stores only the ones that changed,
    /// so a stored value the launcher cannot use stays as it is.
    synced: KytyFields,
}

/// The old fields that are KytyPS5 settings.
#[derive(Clone, Debug, PartialEq)]
struct KytyFields {
    width: u32,
    height: u32,
    present_mode: String,
    video_out: String,
    amd_cpu: bool,
    extra_args: String,
}

impl KytyFields {
    /// Each setting with its value; `extra_args` is the text, split by the caller.
    fn values(&self) -> [(&'static str, SettingValue); 5] {
        [
            ("width", SettingValue::U32(self.width)),
            ("height", SettingValue::U32(self.height)),
            ("present_mode", SettingValue::String(self.present_mode.clone())),
            ("video_out", SettingValue::String(self.video_out.clone())),
            ("amd_cpu", SettingValue::Bool(self.amd_cpu)),
        ]
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            config_version: 0,
            emulators: BTreeMap::new(),
            console_emulators: BTreeMap::new(),
            game_emulators: BTreeMap::new(),
            emulator: String::new(),
            shad_emulator: String::new(),
            game_dirs: vec!["~/Games/PS5".into()],
            download_dir: "~/Downloads/PS5".into(),
            install_dir: "~/Games/PS5".into(),
            seed_after_download: true,
            library_compact: false,
            fullscreen: true,
            width: 1920,
            height: 1080,
            present_mode: "Mailbox".into(),
            video_out: "Title".into(),
            amd_cpu: false,
            extra_args: String::new(),
            sounds: true,
            return_on_exit: true,
            rawg_key: String::new(),
            monitor: String::new(),
            kyty_auto_update: true,
            shad_auto_update: true,
            app_auto_update: true,
            rawg_art: Vec::new(),
            nvidia: crate::nvidia::Flow::default(),
            session_output: None,
            setup_done: false,
            setup_step: None,
            read: None,
        }
    }
}

pub const RESOLUTIONS: [(u32, u32); 5] = [(1280, 720), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160)];
/// The values of --video-out-resolution and the names Settings shows for them.
pub const VIDEO_OUT_MODES: [(&str, &str); 3] =
    [("Title", "Game default"), ("FullHd", "1080p (Full HD)"), ("Uhd", "4K (Ultra HD)")];
pub const PRESENT_MODES: [(&str, &str); 3] =
    [("Mailbox", "Mailbox (low latency)"), ("Fifo", "Fifo (V-Sync)"), ("Immediate", "Immediate (uncapped)")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadSource {
    Managed,
    OwnFound,
    OwnMissing,
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.json")
    }

    pub fn load() -> Config {
        let mut cfg = Config::from_json_with(&std::fs::read(Self::path()).unwrap_or_default(), &crate::kyty::root());
        cfg.use_fallback_emulator(|| {
            let managed = crate::kyty::managed_emulator();
            if managed.is_file() { Some(managed) } else { detect_emulator() }
        });
        cfg
    }

    /// The settings in a config.json's bytes, migrated to this version; the defaults when they
    /// are not a config. `kyty_root` is the managed KytyPS5's folder (`kyty::root`).
    pub fn from_json_with(bytes: &[u8], kyty_root: &Path) -> Config {
        let mut cfg: Config = serde_json::from_slice(bytes).unwrap_or_default();
        cfg.one_update_switch();
        cfg.migrate(kyty_root);
        cfg
    }

    /// When KytyPS5's executable is not set, or not there, this run uses the one `find` gives.
    /// The stored choice stays, so a missing path is still the user's until they pick another.
    pub fn use_fallback_emulator(&mut self, find: impl FnOnce() -> Option<PathBuf>) {
        if self.emulator.is_empty() || !self.emulator_path().is_file() {
            if let Some(found) = find() {
                self.emulator = found.to_string_lossy().into_owned();
            }
        }
    }

    /// Fill the per-emulator form from the old fields where it has no value (an older file, or a
    /// key a hand edit left out), then set the old fields from it. Running it again changes
    /// nothing.
    fn migrate(&mut self, kyty_root: &Path) {
        let flat = self.kyty_fields();
        let kyty_source = kyty_source(&self.emulator, kyty_root);
        let shad_source = shad_source(&self.shad_emulator);
        let kyty = self.emulators.entry(KYTY.into()).or_default();
        kyty.source.get_or_insert(kyty_source);
        for (key, value) in flat.values() {
            kyty.settings.entry(key.into()).or_insert(value);
        }
        if !kyty.settings.contains_key("extra_args") {
            kyty.settings.insert("extra_args".into(), SettingValue::Argv(crate::sessions::shell_split(&flat.extra_args)));
            kyty.texts.insert("extra_args".into(), flat.extra_args.clone());
        }
        self.emulators.entry(SHAD.into()).or_default().source.get_or_insert(shad_source);
        self.apply_to_old_fields(kyty_root);
        self.read = Some(Read { version: self.config_version, kyty_root: kyty_root.to_path_buf(), synced: self.kyty_fields() });
        self.config_version = CONFIG_VERSION;
    }

    /// Set the old fields from the per-emulator form, for the launch path and Settings. A value
    /// of the wrong type leaves the old field as it is.
    fn apply_to_old_fields(&mut self, kyty_root: &Path) {
        let kyty = &self.emulators[KYTY];
        match &kyty.source {
            Some(BuildSource::Custom { executable }) => self.emulator = executable.clone(),
            Some(BuildSource::Managed) if kyty_source(&self.emulator, kyty_root) != BuildSource::Managed => self.emulator.clear(),
            _ => {}
        }
        let s = &kyty.settings;
        if let Some(SettingValue::U32(v)) = s.get("width") { self.width = *v; }
        if let Some(SettingValue::U32(v)) = s.get("height") { self.height = *v; }
        if let Some(SettingValue::String(v)) = s.get("present_mode") { self.present_mode = v.clone(); }
        if let Some(SettingValue::String(v)) = s.get("video_out") { self.video_out = v.clone(); }
        if let Some(SettingValue::Bool(v)) = s.get("amd_cpu") { self.amd_cpu = *v; }
        if let Some(SettingValue::Argv(args)) = s.get("extra_args") {
            // The text as typed, unless the arguments were changed without it.
            self.extra_args = match kyty.texts.get("extra_args") {
                Some(text) if crate::sessions::shell_split(text) == *args => text.clone(),
                _ => join_args(args),
            };
        }
        match &self.emulators[SHAD].source {
            Some(BuildSource::Custom { executable }) => self.shad_emulator = executable.clone(),
            Some(BuildSource::Managed) => self.shad_emulator.clear(),
            None => {}
        }
    }

    fn kyty_fields(&self) -> KytyFields {
        KytyFields {
            width: self.width,
            height: self.height,
            present_mode: self.present_mode.clone(),
            video_out: self.video_out.clone(),
            amd_cpu: self.amd_cpu,
            extra_args: self.extra_args.clone(),
        }
    }

    /// Store the edits of the old KytyPS5 fields since the last read or save in the
    /// per-emulator form.
    fn store_edits(&mut self) {
        if self.read.is_none() {
            self.migrate(&crate::kyty::root());
        }
        let now = self.kyty_fields();
        let read = self.read.as_mut().expect("migrate sets it");
        let before = std::mem::replace(&mut read.synced, now.clone());
        let kyty = self.emulators.entry(KYTY.into()).or_default();
        for ((key, value), (_, old)) in now.values().into_iter().zip(before.values()) {
            if value != old {
                kyty.settings.insert(key.into(), value);
            }
        }
        if now.extra_args != before.extra_args {
            kyty.settings.insert("extra_args".into(), SettingValue::Argv(crate::sessions::shell_split(&now.extra_args)));
            kyty.texts.insert("extra_args".into(), now.extra_args);
        }
    }

    /// The file's bytes, with this run's edits stored.
    pub fn to_json(&mut self) -> Vec<u8> {
        self.store_edits();
        serde_json::to_vec_pretty(self).unwrap_or_default()
    }

    /// The user picks KytyPS5's executable ("" or the managed one: the managed build).
    pub fn set_kyty_executable(&mut self, text: &str) {
        let root = self.read.as_ref().map_or_else(crate::kyty::root, |r| r.kyty_root.clone());
        self.emulator = text.to_string();
        self.emulators.entry(KYTY.into()).or_default().source = Some(kyty_source(text, &root));
    }

    /// The user picks shadPS4's executable ("": the managed build).
    pub fn set_shad_executable(&mut self, text: &str) {
        self.shad_emulator = text.to_string();
        self.emulators.entry(SHAD.into()).or_default().source = Some(shad_source(text));
    }

    /// Settings has one "Update automatically" switch for the launcher, shadPS4 and KytyPS5.
    /// Older settings files set them separately (and have no shadPS4 entry, which defaults to
    /// on): if any was turned off, all are off, so the switch shows what actually happens.
    fn one_update_switch(&mut self) {
        let all = self.app_auto_update && self.kyty_auto_update && self.shad_auto_update;
        (self.app_auto_update, self.kyty_auto_update, self.shad_auto_update) = (all, all, all);
    }
}

/// The generic accessors. The launch flow (phase 3) and the generated Settings rows (phase 5)
/// use them; today only the tests do.
#[allow(dead_code, reason = "used from phases 3 and 5 of docs/plans/data-driven-emulators.md")]
impl Config {
    /// One emulator's preferences; None when nothing is stored for it.
    pub fn emulator_prefs(&self, id: &str) -> Option<&EmulatorPrefs> {
        self.emulators.get(id)
    }

    /// One stored setting. KytyPS5's settings are up to date after a read or a save; an edit
    /// of an old field reaches them on the next save.
    pub fn setting(&self, id: &str, key: &str) -> Option<&SettingValue> {
        self.emulators.get(id)?.settings.get(key)
    }

    /// Turn an emulator on or off; its other preferences stay.
    pub fn set_enabled(&mut self, id: &str, on: bool) {
        self.emulators.entry(id.into()).or_default().enabled = on;
    }

    /// The user's emulator for a console's games; None: the launcher's default.
    pub fn choose_for_console(&mut self, console: Console, id: Option<&str>) {
        choose(&mut self.console_emulators, console.key(), id);
    }

    /// The emulator for one game; None: the console's.
    pub fn choose_for_game(&mut self, title_id: &str, id: Option<&str>) {
        choose(&mut self.game_emulators, title_id, id);
    }
}

impl Config {
    pub fn save(&mut self) {
        self.save_at(&Self::path());
    }

    /// Save to `path`. The first save over a file from before version 1 keeps that file as
    /// `<path>.legacy`, once.
    pub fn save_at(&mut self, path: &Path) {
        let json = self.to_json();
        if json.is_empty() {
            return;
        }
        if let Some(read) = self.read.as_mut().filter(|r| r.version == 0) {
            let mut backup = path.as_os_str().to_owned();
            backup.push(".legacy");
            let backup = PathBuf::from(backup);
            match path.exists() && !backup.exists() {
                true => match std::fs::copy(path, &backup) {
                    Ok(_) => read.version = CONFIG_VERSION,
                    // The next save tries again.
                    Err(e) => crate::log!("could not keep the old config as {}: {e}", backup.display()),
                },
                false => read.version = CONFIG_VERSION,
            }
        }
        if let Err(e) = atomic_write(path, &json) {
            crate::log!("could not save config: {e}");
        }
    }

    pub fn emulator_path(&self) -> PathBuf {
        expand_home(self.emulator.trim())
    }

    pub fn emulator_ok(&self) -> bool {
        runnable(&self.emulator_path())
    }

    /// Whether the user chose their own shadPS4 instead of the managed one.
    pub fn shad_custom(&self) -> bool {
        !self.shad_emulator.trim().is_empty()
    }

    /// The shadPS4 to launch: the user's own build when set, else the managed one.
    pub fn shad_path(&self) -> PathBuf {
        if self.shad_custom() { expand_home(self.shad_emulator.trim()) } else { crate::shad::emulator() }
    }

    /// Where PS4 games get their shadPS4: the launcher's copy, or one the user set (found or not).
    pub fn shad_source(&self) -> ShadSource {
        match (self.shad_custom(), self.shad_ok()) {
            (false, _) => ShadSource::Managed,
            (true, true) => ShadSource::OwnFound,
            (true, false) => ShadSource::OwnMissing,
        }
    }

    pub fn shad_ok(&self) -> bool {
        if self.shad_custom() { runnable(&self.shad_path()) } else { crate::shad::installed() }
    }

    pub fn game_dir_paths(&self) -> Vec<PathBuf> {
        self.game_dirs.iter().filter(|d| !d.trim().is_empty()).map(|d| expand_home(d.trim())).collect()
    }
}

impl Preferences for Config {
    fn game_choice(&self, title_id: &str) -> Option<String> {
        self.game_emulators.get(title_id).cloned()
    }

    fn console_choice(&self, console: Console) -> Option<String> {
        self.console_emulators.get(console.key()).cloned()
    }

    /// On unless the user turned it off: a new addon starts on.
    fn enabled(&self, id: &str) -> bool {
        self.emulators.get(id).is_none_or(|p| p.enabled)
    }
}

/// The old `emulator` text as a build: blank, or a path in the managed folder, is the managed
/// KytyPS5 (blank looks for one, as before); anything else is the user's, found or not.
/// The user's text is kept exactly, spaces too; the launcher trims it where it uses it.
fn kyty_source(text: &str, kyty_root: &Path) -> BuildSource {
    let trimmed = text.trim();
    if trimmed.is_empty() || crate::kyty::is_managed_in(&expand_home(trimmed), kyty_root) {
        BuildSource::Managed
    } else {
        BuildSource::Custom { executable: text.to_string() }
    }
}

/// The old `shad_emulator` text as a build: blank is the managed shadPS4.
fn shad_source(text: &str) -> BuildSource {
    match text.trim() {
        "" => BuildSource::Managed,
        _ => BuildSource::Custom { executable: text.to_string() },
    }
}

fn runnable(path: &std::path::Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file() && is_executable(&m)).unwrap_or(false)
}

fn is_executable(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_update_opt_out_turns_every_automatic_update_off() {
        let mut old: Config = serde_json::from_str(r#"{"kyty_auto_update": false, "app_auto_update": false}"#).unwrap();
        assert!(old.shad_auto_update, "a missing shadPS4 entry defaults to on");
        old.one_update_switch();
        assert!(!old.app_auto_update && !old.kyty_auto_update && !old.shad_auto_update);
        let mut partial: Config = serde_json::from_str(r#"{"app_auto_update": false}"#).unwrap();
        partial.one_update_switch();
        assert!(!partial.kyty_auto_update && !partial.shad_auto_update);
        let mut fresh = Config::default();
        fresh.one_update_switch();
        assert!(fresh.app_auto_update && fresh.kyty_auto_update && fresh.shad_auto_update);
    }

    #[test]
    fn seeding_defaults_on_for_old_configs_and_preserves_explicit_opt_out() {
        let old: Config = serde_json::from_str(r#"{"sounds":false}"#).unwrap();
        assert!(old.seed_after_download);
        let opted_out: Config = serde_json::from_str(r#"{"seed_after_download":false}"#).unwrap();
        assert!(!opted_out.seed_after_download);
        let restored: Config = serde_json::from_slice(&serde_json::to_vec(&opted_out).unwrap()).unwrap();
        assert!(!restored.seed_after_download);
    }

    #[test]
    fn the_display_choices_survive_a_restart() {
        let old: Config = serde_json::from_str(r#"{"sounds":false}"#).unwrap();
        assert_eq!(old.nvidia, crate::nvidia::Flow::default(), "an older config has no NVIDIA step");
        assert_eq!(old.session_output, None, "and the automatic output");
        let mut c = Config::default();
        c.nvidia.step = crate::nvidia::Step::KeyQueued { boot: "b1".into() };
        c.session_output = Some(crate::screen::Output { width: 2560, height: 1440, refresh: Some(144) });
        let back: Config = serde_json::from_slice(&serde_json::to_vec(&c).unwrap()).unwrap();
        assert_eq!(back.nvidia, c.nvidia);
        assert_eq!(back.session_output, c.session_output);
    }

    #[test]
    fn shad_path_is_managed_until_the_user_sets_one() {
        let mut c = Config::default();
        assert!(!c.shad_custom());
        assert_eq!(c.shad_path(), crate::shad::emulator());
        c.shad_emulator = "  ".into();
        assert!(!c.shad_custom(), "blank text is not a path");
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("shadps4");
        c.shad_emulator = exe.to_string_lossy().into_owned();
        assert!(c.shad_custom() && !c.shad_ok(), "a missing file is not runnable");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        assert!(!c.shad_ok(), "not executable yet");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(c.shad_ok());
        assert_eq!(c.shad_path(), exe);
    }

    // ---------------------------------------------------------- per-emulator preferences

    use crate::preferences::{BuildSource, SettingValue};
    use std::collections::BTreeMap;

    const KYTY_ROOT: &str = "/home/u/.local/share/ps5-launcher/kyty";

    fn read(json: &str) -> Config {
        Config::from_json_with(json.as_bytes(), Path::new(KYTY_ROOT))
    }

    /// Save, then start again from what was saved.
    fn again(c: &mut Config) -> Config {
        Config::from_json_with(&c.to_json(), Path::new(KYTY_ROOT))
    }

    fn custom(path: &str) -> Option<BuildSource> {
        Some(BuildSource::Custom { executable: path.into() })
    }

    fn settings(pairs: &[(&str, SettingValue)]) -> BTreeMap<String, SettingValue> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn argv(a: &[&str]) -> SettingValue {
        SettingValue::Argv(a.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn an_old_config_moves_every_kyty_and_shad_value_into_its_emulator() {
        let c = read(r#"{"emulator": "~/kyty/kyty_emulator", "shad_emulator": "/opt/shad/AppRun",
            "width": 7680, "height": 0, "present_mode": "Relaxed", "video_out": "Bogus", "amd_cpu": true,
            "extra_args": "--a 'b c'  \"d\\\"e\"", "fullscreen": false}"#);
        assert_eq!(c.config_version, 1);
        let kyty = &c.emulators["kyty"];
        assert!(kyty.enabled);
        assert_eq!(kyty.source, custom("~/kyty/kyty_emulator"), "the text as stored, ~ and all");
        assert_eq!(kyty.settings, settings(&[
            ("width", SettingValue::U32(7680)),
            ("height", SettingValue::U32(0)),
            ("present_mode", SettingValue::String("Relaxed".into())),
            ("video_out", SettingValue::String("Bogus".into())),
            ("amd_cpu", SettingValue::Bool(true)),
            ("extra_args", argv(&["--a", "b c", "d\"e"])),
        ]));
        assert_eq!(kyty.texts, BTreeMap::from([("extra_args".to_string(), "--a 'b c'  \"d\\\"e\"".to_string())]), "the typed text, for recovery");
        let shad = &c.emulators["shadps4"];
        assert!(shad.enabled);
        assert_eq!(shad.source, custom("/opt/shad/AppRun"));
        assert_eq!(shad.settings, BTreeMap::new(), "shadPS4 keeps its settings in its own config");
        // The old launch path still reads the same values.
        assert_eq!((c.width, c.height, c.present_mode.as_str(), c.video_out.as_str(), c.amd_cpu, c.fullscreen), (7680, 0, "Relaxed", "Bogus", true, false));
        assert_eq!(c.extra_args, "--a 'b c'  \"d\\\"e\"");
        assert_eq!((c.emulator.as_str(), c.shad_emulator.as_str()), ("~/kyty/kyty_emulator", "/opt/shad/AppRun"));
    }

    #[test]
    fn a_minimal_config_gets_the_default_values_and_the_managed_builds() {
        for text in ["{}", "", "not json"] {
            let c = read(text);
            assert_eq!(c.config_version, 1, "{text}");
            assert_eq!(c.emulators["kyty"].source, Some(BuildSource::Managed), "{text}");
            assert_eq!(c.emulators["shadps4"].source, Some(BuildSource::Managed), "{text}");
            assert_eq!(c.emulators["kyty"].settings, settings(&[
                ("width", SettingValue::U32(1920)),
                ("height", SettingValue::U32(1080)),
                ("present_mode", SettingValue::String("Mailbox".into())),
                ("video_out", SettingValue::String("Title".into())),
                ("amd_cpu", SettingValue::Bool(false)),
                ("extra_args", argv(&[])),
            ]), "{text}");
        }
    }

    #[test]
    fn the_managed_kyty_path_and_blank_paths_mean_the_managed_build() {
        let c = read(r#"{"emulator": "/home/u/.local/share/ps5-launcher/kyty/current/kyty_emulator", "shad_emulator": "   "}"#);
        assert_eq!(c.emulators["kyty"].source, Some(BuildSource::Managed));
        assert_eq!(c.emulators["shadps4"].source, Some(BuildSource::Managed));
        assert_eq!(c.emulator, "/home/u/.local/share/ps5-launcher/kyty/current/kyty_emulator", "the old field keeps its text");
    }

    #[test]
    fn in_a_mixed_config_the_new_keys_win_and_the_old_keys_fill_the_gaps() {
        let c = read(r#"{"config_version": 1, "width": 1280, "height": 720, "emulator": "/old/kyty_emulator",
            "emulators": {"kyty": {"source": {"custom": {"executable": "/new/kyty_emulator"}}, "settings": {"width": 2560}}}}"#);
        let kyty = &c.emulators["kyty"];
        assert_eq!(kyty.source, custom("/new/kyty_emulator"));
        assert_eq!(kyty.settings["width"], SettingValue::U32(2560));
        assert_eq!(kyty.settings["height"], SettingValue::U32(720));
        assert_eq!((c.width, c.height), (2560, 720), "the old launch path uses the new value");
        assert_eq!(c.emulator, "/new/kyty_emulator");
        assert_eq!(c.emulators["shadps4"].source, Some(BuildSource::Managed), "a missing emulator is filled in too");
    }

    #[test]
    fn a_missing_kyty_path_stays_stored_while_the_launcher_uses_another() {
        let mut c = read(r#"{"emulator": "/gone/kyty_emulator"}"#);
        c.use_fallback_emulator(|| Some(PathBuf::from("/found/kyty_emulator")));
        assert_eq!(c.emulator, "/found/kyty_emulator", "this run launches the one it found");
        let mut back = again(&mut c);
        assert_eq!(back.emulators["kyty"].source, custom("/gone/kyty_emulator"), "the user's choice survives a save");
        assert_eq!(back.emulator, "/gone/kyty_emulator");
        // The user picks a new one: that is stored.
        back.set_kyty_executable("/home/u/kyty/kyty_emulator");
        assert_eq!(again(&mut back).emulators["kyty"].source, custom("/home/u/kyty/kyty_emulator"));
        // Picking the managed path stores the managed build.
        back.set_kyty_executable("/home/u/.local/share/ps5-launcher/kyty/current/kyty_emulator");
        assert_eq!(again(&mut back).emulators["kyty"].source, Some(BuildSource::Managed));
        back.set_shad_executable("/opt/shad/AppRun");
        assert_eq!(back.shad_emulator, "/opt/shad/AppRun");
        assert_eq!(again(&mut back).emulators["shadps4"].source, custom("/opt/shad/AppRun"));
        back.set_shad_executable("");
        assert_eq!(again(&mut back).emulators["shadps4"].source, Some(BuildSource::Managed));
    }

    #[test]
    fn the_update_opt_out_and_fullscreen_stay_shared_by_every_emulator() {
        let c = read(r#"{"shad_auto_update": false, "fullscreen": false}"#);
        assert!(!c.app_auto_update && !c.kyty_auto_update && !c.shad_auto_update, "any old opt-out turns every update off");
        assert!(!c.fullscreen);
        let json: serde_json::Value = serde_json::from_slice(&Config::from_json_with(b"{}", Path::new(KYTY_ROOT)).to_json()).unwrap();
        assert_eq!(json["emulators"]["kyty"].get("fullscreen"), None);
        assert_eq!(json["fullscreen"], true);
    }

    #[test]
    fn settings_edits_reach_the_emulator_on_save() {
        let mut c = read(r#"{"width": 1920, "height": 1080, "extra_args": "--tessellation"}"#);
        (c.width, c.height, c.present_mode, c.video_out, c.amd_cpu) = (3840, 2160, "Fifo".into(), "Uhd".into(), true);
        c.extra_args = "--vblank-frequency 60 'a b'".into();
        let back = again(&mut c);
        let kyty = &back.emulators["kyty"];
        assert_eq!(kyty.settings, settings(&[
            ("width", SettingValue::U32(3840)),
            ("height", SettingValue::U32(2160)),
            ("present_mode", SettingValue::String("Fifo".into())),
            ("video_out", SettingValue::String("Uhd".into())),
            ("amd_cpu", SettingValue::Bool(true)),
            ("extra_args", argv(&["--vblank-frequency", "60", "a b"])),
        ]));
        assert_eq!(kyty.texts["extra_args"], "--vblank-frequency 60 'a b'");
        assert_eq!(back.extra_args, "--vblank-frequency 60 'a b'", "the text comes back exactly as typed");
    }

    #[test]
    fn extra_arguments_stored_only_as_argv_come_back_as_text_that_splits_the_same() {
        let tricky = ["--x", "", "a b", "it's", "back\\slash", "q\"uote", "tab\there"];
        let json = serde_json::json!({"emulators": {"kyty": {"settings": {"extra_args": tricky}}}}).to_string();
        let c = read(&json);
        assert_eq!(crate::sessions::shell_split(&c.extra_args), tricky, "{}", c.extra_args);
        assert_eq!(c.emulators["kyty"].settings["extra_args"], argv(&tricky));
    }

    #[test]
    fn values_the_launcher_cannot_use_are_kept_as_they_are() {
        let mut c = read(r#"{"width": 1280, "emulators": {
            "kyty": {"settings": {"width": "wide", "height": -5, "future": [1, {"a": 2}]}},
            "ghost": {"enabled": false, "source": {"custom": {"executable": "/x"}}, "settings": {"speed": 3}}}}"#);
        assert_eq!(c.width, 1280, "an unusable stored value leaves the old value in effect");
        assert_eq!(c.height, 1080);
        let back = again(&mut c);
        let kyty = &back.emulators["kyty"];
        assert_eq!(kyty.settings["width"], SettingValue::String("wide".into()));
        assert_eq!(kyty.settings["height"], SettingValue::Other(serde_json::json!(-5)));
        assert_eq!(kyty.settings["future"], SettingValue::Other(serde_json::json!([1, {"a": 2}])));
        let ghost = &back.emulators["ghost"];
        assert!(!ghost.enabled);
        assert_eq!(ghost.source, custom("/x"));
        assert_eq!(ghost.settings["speed"], SettingValue::U32(3));
    }

    #[test]
    fn the_migration_can_run_again_and_changes_nothing() {
        for text in [
            "{}",
            r#"{"emulator": "/gone/kyty_emulator", "width": 4294967295, "present_mode": "", "extra_args": "'unclosed"}"#,
            r#"{"config_version": 1, "emulators": {"kyty": {"enabled": false, "settings": {"width": 2560}}}, "console_emulators": {"ps4": "ps4-lab"}, "game_emulators": {"CUSA1": "shadps4"}}"#,
        ] {
            let mut first = read(text);
            let first_json = first.to_json();
            let mut second = again(&mut first);
            let second_json = second.to_json();
            assert_eq!(String::from_utf8(second_json).unwrap(), String::from_utf8(first_json).unwrap(), "{text}");
            let third = again(&mut second);
            assert_eq!(third.emulators, first.emulators, "{text}");
            assert_eq!((third.width, &third.extra_args, &third.emulator), (first.width, &first.extra_args, &first.emulator), "{text}");
        }
    }

    #[test]
    fn the_choices_and_the_on_off_switch_are_stored_with_the_settings() {
        let mut c = read("{}");
        assert_eq!((c.game_choice("CUSA00001"), c.console_choice(Console::Ps4)), (None, None));
        assert!(c.enabled("kyty") && c.enabled("never-seen"), "an emulator is on until the user turns it off");
        c.choose_for_console(Console::Ps4, Some("ps4-lab"));
        c.choose_for_game("CUSA00001", Some("shadps4"));
        c.set_enabled("kyty", false);
        let json: serde_json::Value = serde_json::from_slice(&c.to_json()).unwrap();
        assert_eq!(json["console_emulators"], serde_json::json!({"ps4": "ps4-lab"}));
        assert_eq!(json["game_emulators"], serde_json::json!({"CUSA00001": "shadps4"}));
        assert_eq!(json["emulators"]["kyty"]["enabled"], false);
        let mut back = again(&mut c);
        assert_eq!(back.console_choice(Console::Ps4).as_deref(), Some("ps4-lab"));
        assert_eq!(back.console_choice(Console::Ps5), None);
        assert_eq!(back.game_choice("CUSA00001").as_deref(), Some("shadps4"));
        assert!(!back.enabled("kyty") && back.enabled("shadps4"));
        assert_eq!(back.setting("kyty", "width"), Some(&SettingValue::U32(1920)), "an emulator that is off keeps its settings");
        assert_eq!(back.emulator_prefs("kyty").map(|p| &p.source), Some(&Some(BuildSource::Managed)));
        back.choose_for_game("CUSA00001", None);
        back.choose_for_console(Console::Ps4, None);
        back.set_enabled("kyty", true);
        let back = again(&mut back);
        assert_eq!((back.game_choice("CUSA00001"), back.console_choice(Console::Ps4)), (None, None));
        assert!(back.enabled("kyty"));
    }

    #[test]
    fn the_registry_picks_the_emulator_from_the_users_choices() {
        use crate::emulators::registry::{ChosenBy, ResolveError, Unavailable};
        let root = tempfile::tempdir().unwrap();
        let loaded = crate::emulators::startup::load(root.path(), &crate::emulators::bundle::Embedded,
            &crate::emulators::lifecycle::RealFiles::default(), crate::emulators::document::Version::current());
        let registry = loaded.registry;
        let mut c = read("{}");
        assert_eq!(registry.resolve(Console::Ps5, "PPSA00001", &c).map(|e| e.id.as_str()), Ok("kyty"));
        c.choose_for_game("PPSA00001", Some("shadps4"));
        assert_eq!(registry.resolve(Console::Ps5, "PPSA00001", &c).map(|e| e.id.as_str()),
            Err(ResolveError::Unavailable { id: "shadps4".into(), console: Console::Ps5, chosen_by: ChosenBy::Game, reason: Unavailable::WrongConsole }));
        c.set_enabled("kyty", false);
        assert_eq!(registry.resolve(Console::Ps5, "PPSA00002", &c).map(|e| e.id.as_str()),
            Err(ResolveError::Unavailable { id: "kyty".into(), console: Console::Ps5, chosen_by: ChosenBy::Launcher, reason: Unavailable::Off }));
        c.choose_for_console(Console::Ps4, Some("ps4-lab"));
        assert_eq!(registry.resolve(Console::Ps4, "CUSA00001", &c).map(|e| e.id.as_str()),
            Err(ResolveError::Unavailable { id: "ps4-lab".into(), console: Console::Ps4, chosen_by: ChosenBy::User, reason: Unavailable::Missing }));
    }

    #[test]
    fn the_first_save_after_the_migration_keeps_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let old = br#"{"emulator": "/k/kyty_emulator", "width": 1280}"#;
        std::fs::write(&path, old).unwrap();
        let mut c = Config::from_json_with(old, Path::new(KYTY_ROOT));
        c.save_at(&path);
        let backup = dir.path().join("config.json.legacy");
        assert_eq!(std::fs::read(&backup).unwrap(), old);
        let saved = std::fs::read(&path).unwrap();
        let mut c = Config::from_json_with(&saved, Path::new(KYTY_ROOT));
        c.width = 640;
        c.save_at(&path);
        assert_eq!(std::fs::read(&backup).unwrap(), old, "a later save never replaces the backup");
        assert!(Config::from_json_with(&std::fs::read(&path).unwrap(), Path::new(KYTY_ROOT)).width == 640);
    }
}

/// Look for kyty_emulator in PATH and a few common build locations.
pub fn detect_emulator() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join("kyty_emulator")));
    }
    let home = expand_home("~");
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf()));
    let mut roots = vec![home.clone(), home.join("projects"), home.join("Games"), home.join("Applications"), home.join("src")];
    if let Some(d) = &exe_dir {
        roots.extend(d.ancestors().take(4).map(|p| p.to_path_buf()));
    }
    for root in roots {
        for sub in ["KytyPS5", "kyty", "Kyty"] {
            for build in ["_Build/release-linux", "_Build/linux/install", "_Build/linux", "build", ""] {
                candidates.push(root.join(sub).join(build).join("kyty_emulator"));
            }
        }
        // One level deeper, e.g. ~/projects/PS5/KytyPS5
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten().take(200) {
                let p = e.path();
                if p.is_dir() {
                    for build in ["KytyPS5/_Build/release-linux", "KytyPS5/_Build/linux/install"] {
                        candidates.push(p.join(build).join("kyty_emulator"));
                    }
                }
            }
        }
    }
    candidates.into_iter().find(|p| std::fs::metadata(p).map(|m| m.is_file() && is_executable(&m)).unwrap_or(false))
}
