//! Settings, full screen: the rail of categories, each category's rows, controller-friendly
//! editing, instant save, and search over every row.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::config::{ShadSource, PRESENT_MODES, RESOLUTIONS, VIDEO_OUT_MODES};
use crate::system::Mode;
use crate::util;
use crate::SettingData;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SId {
    Header,
    // Games
    Dirs,
    InstallDir,
    DownloadDir,
    Rescan,
    // Playing
    Fullscreen,
    ReturnOnExit,
    Controls,
    // Downloads
    SeedCompleted,
    Downloads,
    // Emulators
    KytyUpdate,
    Emulator,
    Resolution,
    VideoOut,
    Present,
    Amd,
    Extra,
    KytyRollback,
    ShadUpdate,
    ShadEmulator,
    ShadRollback,
    AutoUpdate,
    // Appearance
    Display,
    Sounds,
    Share,
    // Updates
    AppUpdate,
    // Advanced
    Rawg,
    RawgRemove,
    Refresh,
    // System (Desktop mode)
    OpenSystemSettings,
    // Network: the indices are into `SystemUi`'s lists.
    WifiOn,
    Wired,
    Network(usize),
    /// The password of the network being joined; only while it is typed.
    WifiPassword,
    WifiScan,
    SavedNetwork(usize),
    // Sound
    Volume,
    Mute,
    Output(usize),
    // Storage: a drive, and a partition of it.
    Drive(usize),
    Partition(usize, usize),
    // Updates (OS)
    OsStatus,
    OsDownload,
    OsRollback,
    // Display: the screen's GPU, the NVIDIA driver's steps, and the output for the next session.
    Gpu,
    NvInstall,
    NvLater,
    /// The key was not enrolled, or its password is gone: queue it again.
    NvRetry,
    NvRestart,
    /// "Use the open-source driver" (on the NVIDIA image).
    NvOpenSource,
    OutResolution,
    OutRefresh,
    // Time: the zone, the NTP switch, and the zone picker (a region, then its zones; the indices
    // are into `SystemUi`'s lists).
    TimeZone,
    Ntp,
    TzBack,
    TzRegion(usize),
    TzZone(usize),
    // About (Session and OS mode)
    RestartLauncher,
}

/// A category of the Settings rail, top to bottom. The Launcher group comes first; the System
/// group sits below a divider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cat {
    Games,
    Playing,
    Downloads,
    Emulators,
    Appearance,
    Updates,
    Advanced,
    /// Desktop mode: the desktop's own settings app.
    System,
    /// Session and OS mode, each when its tool works: nmcli, wpctl, lsblk.
    Network,
    Sound,
    /// The screen's GPU and driver, the NVIDIA driver, and the screen output: when a connected
    /// screen shows in sysfs.
    Display,
    Storage,
    /// OS mode: the system image's updates (bootc and the root helper). The launcher's own
    /// update stays under Updates.
    OsUpdates,
    /// The time zone and automatic time: when timedatectl answers.
    Time,
    /// Session and OS mode: version, mode, Restart launcher.
    About,
}

impl Cat {
    pub fn label(self) -> &'static str {
        match self {
            Cat::Games => "Games",
            Cat::Playing => "Playing",
            Cat::Downloads => "Downloads",
            Cat::Emulators => "Emulators",
            Cat::Appearance => "Appearance",
            Cat::Updates => "Updates",
            Cat::Advanced => "Advanced",
            Cat::System => "System",
            Cat::Network => "Network",
            Cat::Sound => "Sound",
            Cat::Storage => "Storage",
            Cat::Display => "Display",
            Cat::Time => "Time",
            // Under the SYSTEM heading, so not mixed up with the launcher's Updates.
            Cat::OsUpdates => "Updates",
            Cat::About => "About",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Cat::Games => "grid",
            Cat::Playing => "play",
            Cat::Downloads => "download",
            Cat::Emulators => "chip",
            Cat::Appearance => "palette",
            Cat::Updates | Cat::OsUpdates => "update",
            Cat::Advanced => "tune",
            Cat::System => "desktop",
            Cat::Network => "wifi",
            Cat::Sound => "sound",
            Cat::Storage => "disk",
            Cat::Display => "desktop",
            Cat::Time => "cal",
            Cat::About => "info",
        }
    }

    /// In the System group, below the divider.
    pub fn system(self) -> bool {
        matches!(self, Cat::System | Cat::Network | Cat::Sound | Cat::Display | Cat::Storage | Cat::OsUpdates | Cat::Time | Cat::About)
    }
}

/// The system tools that answered on this PC, for the System group's pages.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Tools {
    pub network: bool,
    pub sound: bool,
    /// A connected screen in sysfs.
    pub display: bool,
    pub storage: bool,
    /// bootc and the root helper (PS5 Launcher OS only).
    pub os_updates: bool,
    /// timedatectl.
    pub time: bool,
}

/// The rail's categories in `mode`, with the System pages whose tools work.
pub fn categories(mode: Mode, tools: Tools) -> Vec<Cat> {
    let mut cats = vec![Cat::Games, Cat::Playing, Cat::Downloads, Cat::Emulators, Cat::Appearance, Cat::Updates, Cat::Advanced];
    if mode == Mode::Desktop {
        // The desktop owns the system's settings.
        cats.push(Cat::System);
        return cats;
    }
    // Controllers and File sharing join in their own steps of Phase 6
    // (docs/plans/ps5-launcher-os.md).
    let pages = [
        (Cat::Network, tools.network),
        (Cat::Sound, tools.sound),
        (Cat::Display, tools.display),
        (Cat::Storage, tools.storage),
        (Cat::OsUpdates, tools.os_updates && mode == Mode::Os),
        (Cat::Time, tools.time),
    ];
    cats.extend(pages.into_iter().filter(|(_, found)| *found).map(|(cat, _)| cat));
    cats.push(Cat::About);
    cats
}

/// The category a row lives in; none for a section header.
pub fn category(id: SId) -> Option<Cat> {
    use SId::*;
    Some(match id {
        Header => return None,
        Dirs | InstallDir | DownloadDir | Rescan => Cat::Games,
        Fullscreen | ReturnOnExit | Controls => Cat::Playing,
        SeedCompleted | Downloads => Cat::Downloads,
        KytyUpdate | Emulator | Resolution | VideoOut | Present | Amd | Extra | KytyRollback | ShadUpdate | ShadEmulator | ShadRollback | AutoUpdate => Cat::Emulators,
        Display | Sounds | Share => Cat::Appearance,
        AppUpdate => Cat::Updates,
        Rawg | RawgRemove | Refresh => Cat::Advanced,
        OpenSystemSettings => Cat::System,
        WifiOn | Wired | Network(_) | WifiPassword | WifiScan | SavedNetwork(_) => Cat::Network,
        Volume | Mute | Output(_) => Cat::Sound,
        Drive(_) | Partition(..) => Cat::Storage,
        OsStatus | OsDownload | OsRollback => Cat::OsUpdates,
        Gpu | NvInstall | NvLater | NvRetry | NvRestart | NvOpenSource | OutResolution | OutRefresh => Cat::Display,
        TimeZone | Ntp | TzBack | TzRegion(_) | TzZone(_) => Cat::Time,
        RestartLauncher => Cat::About,
    })
}

/// L1/R1: the previous or next of `n` categories, wrapping at both ends.
pub fn step(n: usize, cur: usize, dir: i32) -> usize {
    if n == 0 {
        return 0;
    }
    (cur as i64 + dir as i64).rem_euclid(n as i64) as usize
}

/// In the page, Left changes a row that cycles through values; on any other row it goes back
/// to the rail.
pub fn left_changes(kind: i32) -> bool {
    kind == 3
}

/// Every word of `query` is in the label or the hint, ignoring case. An empty query matches
/// nothing.
pub fn matches(query: &str, label: &str, hint: &str) -> bool {
    let text = format!("{label}\n{hint}").to_lowercase();
    let mut words = query.split_whitespace().peekable();
    words.peek().is_some() && words.all(|w| text.contains(&w.to_lowercase()))
}

/// At most this many search hits show under the search field.
pub const MAX_HITS: usize = 8;

/// The rows of `rows` (any category, headers skipped) that match `query`, at most `MAX_HITS`.
pub fn search(rows: &[(Cat, SId, SettingData)], query: &str) -> Vec<usize> {
    rows.iter().enumerate()
        .filter(|(_, (_, id, r))| *id != SId::Header && matches(query, &r.label, &r.hint))
        .map(|(i, _)| i)
        .take(MAX_HITS)
        .collect()
}

/// The desktop's settings app: `systemsettings` on KDE, `gnome-control-center` on GNOME (from
/// `XDG_CURRENT_DESKTOP`, a list split by `:`), System Settings on macOS. None when it is not
/// installed; `exists` looks a program up.
pub fn system_settings_command(desktop: Option<&str>, macos: bool, exists: &dyn Fn(&str) -> bool) -> Option<(&'static str, &'static [&'static str])> {
    let found: Option<(&'static str, &'static [&'static str])> = if macos {
        Some(("/usr/bin/open", &["-b", "com.apple.systempreferences"]))
    } else {
        desktop.unwrap_or("").split(':').find_map(|d| match d.to_ascii_uppercase().as_str() {
            "KDE" => Some(("systemsettings", &[][..])),
            "GNOME" => Some(("gnome-control-center", &[][..])),
            _ => None,
        })
    };
    found.filter(|(program, _)| exists(program))
}

/// A dot on a category when something waits there: a launcher update on Updates, an emulator
/// update on Emulators, a step of the NVIDIA driver on Display.
pub fn waiting(cat: Cat, app_update: bool, emulator_update: bool, display: bool) -> bool {
    match cat {
        Cat::Updates => app_update,
        Cat::Emulators => emulator_update,
        Cat::Display => display,
        _ => false,
    }
}

/// How About names the mode.
pub fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Desktop => "Desktop app",
        Mode::Session => "PS5 Launcher session",
        Mode::Os => "PS5 Launcher OS",
    }
}

pub(crate) fn row(kind: i32, label: &str) -> SettingData {
    SettingData { kind, label: label.into(), ..Default::default() }
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// Widths and heights of the Settings page on the 1920×1080 canvas (ui/settings.slint).
const RAIL_FOLDED: f32 = 116.0;
const PAGE_TOP: f32 = 170.0;
const PAGE_BOTTOM: f32 = 110.0;
/// The Secure Boot key's steps on the Updates page, with the space under them.
const KEY_CARD: f32 = 510.0;
/// The NVIDIA driver's card on the Display page (the stepper, a title and its text).
const NV_CARD: f32 = 300.0;

/// The rail and the page: which category is open, every row of every category (for search), and
/// the search field.
#[derive(Default)]
pub struct SettingsNav {
    pub cats: Vec<Cat>,
    /// The open category, an index into `cats`.
    pub cat: usize,
    /// Every row of every category, in rail order. The page shows those of `cats[cat]`.
    pub all: Vec<(Cat, SId, SettingData)>,
    pub query: String,
    /// Search hits: indices into `all`.
    pub hits: Vec<usize>,
    /// The search field has the keyboard.
    pub find_editing: bool,
    /// Desktop mode: the desktop's settings app, looked up when Settings opens.
    pub system_app: Option<(&'static str, &'static [&'static str])>,
}

/// Is `program` installed: an absolute path that exists, or a file in a `PATH` folder.
fn installed(program: &str) -> bool {
    let path = std::path::Path::new(program);
    if path.is_absolute() {
        return path.is_file();
    }
    std::env::var_os("PATH").is_some_and(|dirs| std::env::split_paths(&dirs).any(|d| d.join(program).is_file()))
}

impl App {
    /// Every row of every category. The page shows the open category's rows.
    pub fn build_settings(&mut self) {
        let cfg = self.cfg.lock().unwrap().clone();
        let mut rows: Vec<(Cat, SId, SettingData)> = Vec::new();
        let add = |rows: &mut Vec<(Cat, SId, SettingData)>, id: SId, r: SettingData| rows.push((category(id).expect("a row, not a header"), id, r));
        let header = |rows: &mut Vec<(Cat, SId, SettingData)>, cat: Cat, label: &str| rows.push((cat, SId::Header, row(0, label)));

        // Games
        let mut r = row(1, "Game folders");
        r.value = cfg.game_dirs.iter().map(|d| util::display_path(d)).collect::<Vec<_>>().join("; ").into();
        r.hint = format!("{} found · separate several folders with ;", plural(self.locals.len(), "installed game")).into();
        add(&mut rows, SId::Dirs, r);
        let mut r = row(1, "Install new games to");
        r.value = util::display_path(&cfg.install_dir).into();
        r.hint = "Downloaded games are set up here when you press Install".into();
        add(&mut rows, SId::InstallDir, r);
        let mut r = row(1, "Save downloads to");
        r.value = util::display_path(&cfg.download_dir).into();
        r.hint = "Each download gets its own folder".into();
        add(&mut rows, SId::DownloadDir, r);
        add(&mut rows, SId::Rescan, row(4, "Rescan game folders"));

        // Playing
        let mut r = row(2, "Play games in fullscreen");
        r.on = cfg.fullscreen;
        add(&mut rows, SId::Fullscreen, r);
        let mut r = row(2, "Return to the launcher when a game closes");
        r.on = cfg.return_on_exit;
        add(&mut rows, SId::ReturnOnExit, r);
        let connected = !crate::gamepad::connected().is_empty();
        let mut r = row(4, "Controller & keyboard");
        r.value = crate::gamepad::status().into();
        r.value_kind = if connected { 1 } else { 0 };
        r.hint = if connected { "Your controller works in games with no setup" } else { "Games play best with a controller · open to see the keyboard keys" }.into();
        add(&mut rows, SId::Controls, r);

        // Downloads
        let mut r = row(2, "Keep sharing finished downloads");
        r.on = cfg.seed_after_download;
        r.hint = "Helps other players download faster · uploads at most 128 KiB/s".into();
        add(&mut rows, SId::SeedCompleted, r);
        let mut r = row(4, "Open downloads");
        r.value = "Ctrl+D".into();
        add(&mut rows, SId::Downloads, r);

        // Emulators: both emulators' updates, then each emulator's own rows.
        let mut r = row(2, "Update automatically");
        r.on = cfg.app_auto_update && cfg.kyty_auto_update && cfg.shad_auto_update;
        r.hint = "Checks every few hours · updates wait until you close your game".into();
        add(&mut rows, SId::AutoUpdate, r);
        let rollback_hint = "For when an update breaks a game";

        header(&mut rows, Cat::Emulators, "PS5 GAMES · KYTYPS5");
        let ok = cfg.emulator_ok();
        let managed = self.kyty_managed();
        let kyty_state = crate::kyty::load_state();
        let mut r = row(4, "KytyPS5");
        r.hint = "Runs PS5 games".into();
        let (value, kind) = if self.kyty.busy {
            (self.kyty.progress.replace("Downloading KytyPS5 ", ""), 0)
        } else if self.kyty.checking {
            ("Checking…".to_string(), 0)
        } else if !managed && !ok {
            ("Install".to_string(), 3)
        } else if !managed {
            r.hint = format!("Runs PS5 games · you use your own build{} · switching keeps your saves",
                self.kyty.version.strip_prefix("Self-built").map(|v| format!(" ({})", v.trim())).unwrap_or_default()).into();
            ("Switch to official builds".to_string(), 3)
        } else if self.kyty_update_available() {
            (self.kyty.latest.as_ref().map(|l| format!("Update to {}", crate::kyty::pretty(&l.tag))).unwrap_or_default(), 3)
        } else if self.kyty.latest.is_some() {
            (format!("{} · Up to date", crate::kyty::pretty(&kyty_state.installed)), 1)
        } else {
            (crate::kyty::pretty(&kyty_state.installed), 0)
        };
        (r.value, r.value_kind) = (value.into(), kind);
        if !self.kyty.error.is_empty() {
            (r.hint, r.hint_kind) = (self.kyty.error.clone().into(), 2);
        }
        add(&mut rows, SId::KytyUpdate, r);
        let mut r = row(1, "KytyPS5 location");
        r.value = util::display_path(&cfg.emulator).into();
        (r.hint, r.hint_kind) = if ok { ("Found".into(), 1) } else { ("Nothing runnable at this path".into(), 2) };
        add(&mut rows, SId::Emulator, r);
        let mut r = row(3, "Resolution");
        r.value = format!("{} × {}", cfg.width, cfg.height).into();
        r.hint = "The size of the window. What the game is told is Game output resolution".into();
        add(&mut rows, SId::Resolution, r);
        let mut r = row(3, "Game output resolution");
        r.value = VIDEO_OUT_MODES.iter().find(|(k, _)| *k == cfg.video_out).map(|(_, l)| *l).unwrap_or(VIDEO_OUT_MODES[0].1).into();
        r.hint = "The screen resolution the game is told it runs on. The game renders at this size.".into();
        add(&mut rows, SId::VideoOut, r);
        let mut r = row(3, "Present mode");
        r.value = PRESENT_MODES.iter().find(|(k, _)| *k == cfg.present_mode).map(|(_, l)| *l).unwrap_or(cfg.present_mode.as_str()).into();
        r.hint = "Try V-Sync if the picture tears".into();
        add(&mut rows, SId::Present, r);
        let mut r = row(2, "AMD CPU patches");
        r.on = cfg.amd_cpu;
        r.hint = "Only for AMD processors".into();
        add(&mut rows, SId::Amd, r);
        let mut r = row(1, "Extra KytyPS5 arguments");
        r.value = cfg.extra_args.clone().into();
        r.hint = "For example --vblank-frequency 60 --tessellation".into();
        add(&mut rows, SId::Extra, r);
        // Rollbacks only when there is something to go back to.
        if managed && !kyty_state.previous.is_empty() && crate::kyty::root().join("versions").join(&kyty_state.previous).is_dir() {
            let mut r = row(4, "Go back to the previous KytyPS5");
            r.value = crate::kyty::pretty(&kyty_state.previous).into();
            r.hint = rollback_hint.into();
            add(&mut rows, SId::KytyRollback, r);
        }

        header(&mut rows, Cat::Emulators, "PS4 GAMES · SHADPS4");
        let shad_state = crate::shad::load_state();
        let shad_installed = crate::shad::installed();
        let mut r = row(4, "shadPS4");
        r.hint = if cfg.shad_custom() {
            "Runs PS4 games · using your own build · no automatic updates"
        } else if shad_installed {
            "Runs PS4 games"
        } else if cfg.shad_auto_update {
            "Runs PS4 games · installs by itself in the background"
        } else {
            "Runs PS4 games · installs when you first play one"
        }
        .into();
        let (value, kind) = if self.shad.busy {
            (self.shad.progress.clone(), 0)
        } else if cfg.shad_custom() {
            ("Own build".to_string(), if cfg.shad_ok() { 1 } else { 2 })
        } else if !shad_installed {
            ("Install".to_string(), 3)
        } else if shad_state.latest == shad_state.installed {
            (format!("{} · Up to date", crate::shad::pretty(&shad_state.installed)), 1)
        } else {
            (crate::shad::pretty(&shad_state.installed), 0)
        };
        (r.value, r.value_kind) = (value.into(), kind);
        if !self.shad.error.is_empty() {
            (r.hint, r.hint_kind) = (self.shad.error.clone().into(), 2);
        }
        add(&mut rows, SId::ShadUpdate, r);
        let mut r = row(1, "shadPS4 location");
        r.value = util::display_path(&cfg.shad_emulator).into();
        (r.hint, r.hint_kind) = match cfg.shad_source() {
            ShadSource::Managed => ("Empty = the one the launcher installs and updates".into(), 0),
            ShadSource::OwnFound => ("Found · no automatic updates".into(), 1),
            ShadSource::OwnMissing => ("Nothing runnable at this path".into(), 2),
        };
        add(&mut rows, SId::ShadEmulator, r);
        if !cfg.shad_custom() && !shad_state.previous.is_empty() && crate::shad::root().join("versions").join(&shad_state.previous).is_dir() {
            let mut r = row(4, "Go back to the previous shadPS4");
            r.value = crate::shad::pretty(&shad_state.previous).into();
            r.hint = rollback_hint.into();
            add(&mut rows, SId::ShadRollback, r);
        }

        // Appearance
        let mut r = row(3, "Display");
        r.value = if cfg.monitor == crate::display::ACTIVE {
            "Active display".into()
        } else {
            match self.monitors.iter().find(|m| m.name == cfg.monitor) {
                Some(m) => format!("{} · {}×{}", m.name, m.w, m.h),
                None => "Primary display".into(),
            }
        }
        .into();
        add(&mut rows, SId::Display, r);
        let mut r = row(2, "Interface sounds");
        r.on = cfg.sounds;
        add(&mut rows, SId::Sounds, r);
        let unshared = self.my_results.unshared().len();
        let mut r = row(4, "Share your game ratings");
        (r.value, r.value_kind) = if unshared > 0 { (format!("{unshared} to share").into(), 3) } else { ("Nothing new".into(), 0) };
        r.hint = if self.my_results.games.is_empty() {
            "Rate a game from its Options menu after you play it, then share it here".into()
        } else {
            "Tells the emulator teams which games work on Linux · needs a free GitHub account".into()
        };
        add(&mut rows, SId::Share, r);

        // Updates: PS5 Launcher
        let cur = crate::update::current_version();
        let latest = self.upd.latest.as_ref().map(|r| r.version.clone());
        let mut r = row(4, "PS5 Launcher");
        r.hint = "This app".into();
        let (value, kind) = if let Some(v) = &self.upd.installed {
            (format!("Restart to finish · {v}"), 3)
        } else if self.upd.busy {
            (self.upd.progress.replace("Downloading PS5 Launcher ", ""), 0)
        } else if self.upd.checking {
            ("Checking…".to_string(), 0)
        } else if self.app_update_available() {
            (latest.map(|l| format!("Update to {l}")).unwrap_or_default(), 3)
        } else if latest.is_some() {
            (format!("{cur} · Up to date"), 1)
        } else {
            (cur.to_string(), 0)
        };
        (r.value, r.value_kind) = (value.into(), kind);
        if !self.upd.error.is_empty() {
            (r.hint, r.hint_kind) = (self.upd.error.clone().into(), 2);
        }
        add(&mut rows, SId::AppUpdate, r);

        // Advanced
        let mut r = row(1, "RAWG API key");
        r.secret = true;
        let key = cfg.rawg_key.trim();
        r.value = if key.is_empty() { "".into() } else if key.len() >= 8 { format!("••••••••{}", &key[key.len() - 4..]).into() } else { "••••".into() };
        let rawg_games = self.games.iter().filter(|g| g.info.as_ref().and_then(|i| i.source.as_deref()) == Some("rawg")).count();
        if !self.rawg_status.is_empty() {
            (r.hint, r.hint_kind) = (self.rawg_status.clone().into(), self.rawg_status_kind);
        } else if key.is_empty() {
            r.hint = "Optional · fills in artwork for games missing from the PlayStation Store".into();
        } else {
            (r.hint, r.hint_kind) = (format!("Key saved · artwork for {}", plural(rawg_games, "game")).into(), 1);
        }
        add(&mut rows, SId::Rawg, r);
        if !key.is_empty() {
            add(&mut rows, SId::RawgRemove, row(4, "Remove RAWG key"));
        }
        let mut r = row(4, "Reload game catalog");
        r.value = if self.syncing {
            "Reloading…".into()
        } else {
            let snapshot = self.games.first().map(|game| util::fmt_date(util::parse_iso_date(&game.g.peers_observed))).unwrap_or_default();
            format!("{} · {}", plural(self.games.len(), "release"), if snapshot.is_empty() { "date unknown" } else { &snapshot }).into()
        };
        r.hint = "Seeder counts are from that date, not live".into();
        add(&mut rows, SId::Refresh, r);

        // The System group.
        if self.settings_nav.cats.contains(&Cat::System) && self.settings_nav.system_app.is_some() {
            let mut r = row(4, "Your desktop manages these");
            (r.value, r.value_kind) = ("Open system settings".into(), 3);
            r.hint = "Network, Bluetooth, sound, displays and power".into();
            add(&mut rows, SId::OpenSystemSettings, r);
        }
        rows.extend(self.system_rows(&self.settings_nav.cats));
        if self.settings_nav.cats.contains(&Cat::About) {
            let mut r = row(4, "Restart launcher");
            r.hint = "Starts PS5 Launcher again, for when something looks stuck".into();
            add(&mut rows, SId::RestartLauncher, r);
        }

        self.settings_nav.all = rows;
        // Rows come and go between builds: hits point into `all`, so find them again.
        self.settings_nav.hits = search(&self.settings_nav.all, &self.settings_nav.query);
        self.settings_page();
    }

    /// The open category's rows become the page.
    fn settings_page(&mut self) {
        let cat = self.settings_nav.cats.get(self.settings_nav.cat).copied().unwrap_or(Cat::Games);
        let mut rows: Vec<(SId, SettingData)> = self.settings_nav.all.iter().filter(|(c, _, _)| *c == cat).map(|(_, id, r)| (*id, r.clone())).collect();
        // Rows between two headers form one card.
        for i in 0..rows.len() {
            let first = i == 0 || rows[i - 1].1.kind == 0;
            let last = i + 1 == rows.len() || rows[i + 1].1.kind == 0;
            (rows[i].1.group_first, rows[i].1.group_last) = (first, last);
        }
        self.settings_ids = rows.iter().map(|(id, _)| *id).collect();
        self.settings_rows = rows.into_iter().map(|(_, r)| r).collect();
    }

    /// Open Settings on "Share your game ratings".
    pub fn open_share_setting(&mut self) {
        self.open_settings();
        self.settings_show_row(SId::Share);
    }

    /// Open the category of `id`, with focus on that row.
    pub fn settings_show_row(&mut self, id: SId) {
        let Some(cat) = category(id) else { return };
        let Some(c) = self.settings_nav.cats.iter().position(|x| *x == cat) else { return };
        self.settings_nav.cat = c;
        self.settings_page();
        let i = self.settings_ids.iter().position(|s| *s == id).unwrap_or(0);
        self.set_focus(Z_SETTINGS, i as i32);
        self.push_settings();
        self.scroll_settings();
    }

    pub fn push_settings(&mut self) {
        let nav = &self.settings_nav;
        let app_update = self.upd.installed.is_some() || self.app_update_available();
        let emulator_update = self.kyty_update_available();
        let display = self.display_dot();
        let cats: Vec<crate::SettingsCat> = nav.cats.iter().enumerate().map(|(i, c)| crate::SettingsCat {
            label: c.label().into(),
            icon: c.icon().into(),
            dot: waiting(*c, app_update, emulator_update, display),
            system: c.system(),
            sep: c.system() && i > 0 && !nav.cats[i - 1].system(),
        }).collect();
        let hits: Vec<crate::SettingsHit> = nav.hits.iter().filter_map(|i| nav.all.get(*i)).map(|(cat, _, r)| crate::SettingsHit {
            label: r.label.clone(),
            place: cat.label().into(),
        }).collect();
        let cat = nav.cats.get(nav.cat).copied().unwrap_or(Cat::Games);
        let ui = self.ui();
        ui.set_settings_cats(model(cats));
        ui.set_settings_cat(nav.cat as i32);
        ui.set_settings_hits(model(hits));
        ui.set_settings_about(matches!(cat, Cat::System | Cat::About));
        let key = match cat {
            Cat::OsUpdates => self.os_key_digits(),
            Cat::Display => self.nv_key_digits(),
            _ => Vec::new(),
        };
        ui.set_settings_key(model(key));
        ui.set_settings_key_last(
            if cat == Cat::Display {
                "Choose \"Reboot\". Back here, the launcher checks the key by itself; then download the driver."
            } else {
                "Choose \"Reboot\". Back here, choose Download update again."
            }
            .into(),
        );
        ui.set_settings_nv(if cat == Cat::Display { self.nv_card() } else { crate::NvCard::default() });
        ui.set_settings_mode(mode_label(Mode::current()).into());
        ui.set_settings(model(self.settings_rows.clone()));
        ui.set_edit_index(self.edit_index);
    }

    /// Keep the focused row of the page in view.
    pub fn scroll_settings(&mut self) {
        // Estimated heights matching ui/settings.slint.
        let (w, h) = self.logical_size();
        // The page is at most 1100 wide, right of the folded rail; hints wrap at about 8 px a
        // character (15 px text).
        let hint_w = (w - RAIL_FOLDED - 72.0 - 96.0).min(1100.0) - 36.0;
        let per_line = (hint_w / 8.0).max(20.0) as usize;
        // The key's steps on the Updates page come before the rows.
        let cat = self.settings_nav.cats.get(self.settings_nav.cat).copied();
        let mut y = if cat == Some(Cat::OsUpdates) && self.sys.os.key.is_some() { KEY_CARD } else { 0.0 };
        if cat == Some(Cat::Display) {
            let ui = self.ui();
            y += if ui.get_settings_nv().show { NV_CARD } else { 0.0 } + if self.nv_key_digits().is_empty() { 0.0 } else { KEY_CARD };
        }
        let mut target = y;
        for (i, r) in self.settings_rows.iter().enumerate() {
            if i as i32 == self.idx && self.zone == Z_SETTINGS {
                target = y;
            }
            let hint_lines = r.hint.chars().count().div_ceil(per_line) as f32;
            y += match r.kind {
                0 => (if i == 0 { 8.0 } else { 32.0 }) + 20.0 + 12.0,
                _ => 64.0 + if hint_lines > 0.0 { 4.0 + hint_lines * 19.0 } else { 0.0 } + if self.edit_index == i as i32 { 54.0 } else { 0.0 },
            };
        }
        if self.ui().get_settings_about() {
            y += 36.0 + 120.0; // the About card under the rows
        }
        let view = h - PAGE_TOP - PAGE_BOTTOM;
        let max = (y - view).max(0.0);
        self.ui().set_settings_y(-(target - view * 0.4).clamp(0.0, max) * self.scale);
    }

    pub(crate) fn save_cfg(&mut self, f: impl FnOnce(&mut crate::config::Config)) {
        let mut c = self.cfg.lock().unwrap();
        f(&mut c);
        c.save();
    }

    pub fn refresh_settings(&mut self) {
        self.build_settings();
        self.push_settings();
    }

    pub fn settings_change(&mut self, i: usize, dir: i32) {
        let Some(id) = self.settings_ids.get(i).copied() else { return };
        match id {
            SId::Resolution => {
                let cur = { let c = self.cfg.lock().unwrap(); (c.width, c.height) };
                let pos = RESOLUTIONS.iter().position(|r| *r == cur).unwrap_or(2) as i32;
                let (w, h) = RESOLUTIONS[(pos + dir).rem_euclid(RESOLUTIONS.len() as i32) as usize];
                self.save_cfg(|c| {
                    c.width = w;
                    c.height = h;
                });
            }
            SId::Present => {
                let cur = self.cfg.lock().unwrap().present_mode.clone();
                let pos = PRESENT_MODES.iter().position(|(k, _)| *k == cur).unwrap_or(0) as i32;
                let next = PRESENT_MODES[(pos + dir).rem_euclid(PRESENT_MODES.len() as i32) as usize].0.to_string();
                self.save_cfg(|c| c.present_mode = next);
            }
            SId::VideoOut => {
                let cur = self.cfg.lock().unwrap().video_out.clone();
                let pos = VIDEO_OUT_MODES.iter().position(|(k, _)| *k == cur).unwrap_or(0) as i32;
                let next = VIDEO_OUT_MODES[(pos + dir).rem_euclid(VIDEO_OUT_MODES.len() as i32) as usize].0.to_string();
                self.save_cfg(|c| c.video_out = next);
            }
            SId::Display => {
                let names: Vec<String> = [String::new(), crate::display::ACTIVE.to_string()].into_iter().chain(self.monitors.iter().map(|m| m.name.clone())).collect();
                let cur = self.cfg.lock().unwrap().monitor.clone();
                let pos = names.iter().position(|n| *n == cur).unwrap_or(0) as i32;
                let next = names[(pos + dir).rem_euclid(names.len() as i32) as usize].clone();
                self.save_cfg(|c| c.monitor = next.clone());
                self.move_to_monitor(&next);
            }
            SId::WifiOn | SId::Mute | SId::Volume | SId::OutResolution | SId::OutRefresh | SId::Ntp => return self.sys_change(id, dir),
            SId::SeedCompleted => {
                let on = dir > 0;
                if let Err(error) = self.downloads.set_seed_after_download(on) {
                    self.toast("Seeding setting unavailable", &error.to_string(), 2);
                    return;
                }
                self.save_cfg(|c| c.seed_after_download = on);
                self.push_downloads();
            }
            SId::Fullscreen | SId::Amd | SId::ReturnOnExit | SId::Sounds | SId::AutoUpdate => {
                let on = dir > 0;
                self.save_cfg(|c| match id {
                    SId::Fullscreen => c.fullscreen = on,
                    SId::Amd => c.amd_cpu = on,
                    SId::ReturnOnExit => c.return_on_exit = on,
                    SId::AutoUpdate => (c.app_auto_update, c.kyty_auto_update, c.shad_auto_update) = (on, on, on),
                    _ => c.sounds = on,
                });
                if id == SId::AutoUpdate && on {
                    self.kyty_check(false);
                    self.shad_tick();
                }
                if id == SId::Sounds {
                    audio::set_enabled(on);
                }
            }
            _ => return,
        }
        audio::play(Sound::Move);
        self.refresh_settings();
    }

    pub fn settings_activate(&mut self, i: usize) {
        let Some(id) = self.settings_ids.get(i).copied() else { return };
        match id {
            SId::Emulator | SId::ShadEmulator | SId::Dirs | SId::Extra | SId::Rawg | SId::DownloadDir | SId::InstallDir | SId::WifiPassword => {
                let cfg = self.cfg.lock().unwrap().clone();
                let text = match id {
                    SId::Emulator => cfg.emulator,
                    SId::ShadEmulator => cfg.shad_emulator,
                    SId::Dirs => cfg.game_dirs.join("; "),
                    SId::Extra => cfg.extra_args,
                    SId::DownloadDir => cfg.download_dir,
                    SId::InstallDir => cfg.install_dir,
                    _ => String::new(),
                };
                audio::play(Sound::Select);
                self.edit_index = i as i32;
                let ui = self.ui();
                ui.set_edit_caret(-1);
                if self.last_input_pad {
                    self.osk_open(crate::osk_ui::Field::Row(i, id), &text);
                }
                ui.set_edit_text(text.into());
                ui.set_edit_index(i as i32);
            }
            SId::Fullscreen | SId::Amd | SId::ReturnOnExit | SId::Sounds | SId::AutoUpdate | SId::SeedCompleted | SId::WifiOn | SId::Mute | SId::Ntp => {
                let on = self.settings_rows[i].on;
                self.settings_change(i, if on { -1 } else { 1 });
            }
            SId::Resolution | SId::Present | SId::VideoOut | SId::Display | SId::OutResolution | SId::OutRefresh => self.settings_change(i, 1),
            SId::Volume | SId::Wired | SId::Drive(_) | SId::Gpu => {}
            SId::Network(_) | SId::WifiScan | SId::SavedNetwork(_) | SId::Output(_) | SId::Partition(..) | SId::OsStatus | SId::OsDownload | SId::OsRollback => self.sys_activate(id),
            SId::NvInstall | SId::NvLater | SId::NvRetry | SId::NvRestart | SId::NvOpenSource | SId::TimeZone | SId::TzBack | SId::TzRegion(_) | SId::TzZone(_) => {
                self.sys_activate(id)
            }
            SId::RawgRemove => {
                self.save_cfg(|c| c.rawg_key.clear());
                self.rawg_status.clear();
                self.toast("RAWG key removed", "Artwork from RAWG stays until the next refresh.", 1);
                self.refresh_settings();
                let first = self.settings_ids.iter().position(|s| *s == SId::Rawg).unwrap_or(0);
                self.set_focus(Z_SETTINGS, first as i32);
            }
            SId::KytyUpdate => {
                audio::play(Sound::Select);
                if self.kyty.busy || self.kyty.checking {
                    return;
                }
                let managed = self.kyty_managed();
                if !managed {
                    self.kyty_switch_to_managed();
                } else if self.kyty_update_available() {
                    if let Some(rel) = self.kyty.latest.clone() {
                        self.kyty_install(rel, true);
                    }
                } else {
                    self.kyty_check(true);
                }
                self.refresh_settings();
            }
            SId::AppUpdate => {
                audio::play(Sound::Select);
                if self.upd.installed.is_some() {
                    self.app_restart();
                } else if self.upd.busy || self.upd.checking {
                    return;
                } else if self.app_update_available() {
                    if let Some(rel) = self.upd.latest.clone() {
                        self.app_update_install(rel);
                    }
                } else {
                    self.app_update_check(true);
                }
                self.refresh_settings();
            }
            SId::KytyRollback => {
                audio::play(Sound::Select);
                self.kyty_rollback();
                self.refresh_settings();
            }
            SId::ShadUpdate => {
                audio::play(Sound::Select);
                if self.cfg.lock().unwrap().shad_custom() {
                    self.toast("Using your own shadPS4", "Clear Settings → Emulators → shadPS4 location to use the launcher's copy.", 0);
                    return;
                }
                // Installs when shadPS4 isn't there yet, otherwise updates it if there's a newer release.
                self.shad_check(true);
            }
            SId::ShadRollback => {
                audio::play(Sound::Select);
                self.shad_rollback();
            }
            SId::Refresh => {
                audio::play(Sound::Select);
                self.start_sync();
                self.toast("Refreshing the catalog…", "New games appear as soon as it's done.", 0);
                self.refresh_settings();
            }
            SId::Rescan => {
                audio::play(Sound::Select);
                self.rescan_library();
                let n = self.locals.len();
                self.toast("Installed games rescanned", &format!("{n} game{} found.", if n == 1 { "" } else { "s" }), 1);
                self.refresh_settings();
            }
            SId::OpenSystemSettings => {
                audio::play(Sound::Select);
                self.open_system_settings();
            }
            SId::RestartLauncher => {
                audio::play(Sound::Select);
                self.restart_launcher();
            }
            SId::Downloads => self.open_downloads(None),
            SId::Share => self.share_results(),
            SId::Controls => self.open_controls(),
            SId::Header => {}
        }
    }

    pub fn settings_commit_text(&mut self, i: usize, text: String) {
        let Some(id) = self.settings_ids.get(i).copied() else { return };
        let t = text.trim().to_string();
        match id {
            SId::Emulator => {
                self.save_cfg(|c| c.emulator = t);
                self.kyty_refresh_version();
                let ok = self.cfg.lock().unwrap().emulator_ok();
                if ok { self.toast("Emulator path saved", "Games will start with this KytyPS5.", 1) } else { self.toast("Emulator not found", "There's no runnable kyty_emulator at that path.", 2) }
            }
            SId::ShadEmulator => {
                self.save_cfg(|c| c.shad_emulator = t);
                let source = self.cfg.lock().unwrap().shad_source();
                match source {
                    ShadSource::Managed => self.toast("shadPS4 location cleared", "PS4 games use the shadPS4 the launcher installs.", 1),
                    ShadSource::OwnFound => self.toast("shadPS4 path saved", "PS4 games will start with this shadPS4. It is not updated automatically.", 1),
                    ShadSource::OwnMissing => self.toast("shadPS4 not found", "There's no runnable shadPS4 at that path.", 2),
                }
                self.refresh_settings();
            }
            SId::Dirs => {
                let dirs: Vec<String> = t.split([';', '\n']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                self.save_cfg(|c| c.game_dirs = dirs);
                self.rescan_library();
                let n = self.locals.len();
                self.toast("Installed games rescanned", &format!("{n} game{} found.", if n == 1 { "" } else { "s" }), 1);
            }
            SId::Extra => self.save_cfg(|c| c.extra_args = t),
            // Spaces at either end can be part of a Wi-Fi password.
            SId::WifiPassword => self.net_password(text),
            SId::DownloadDir => {
                if t.is_empty() || !util::expand_home(&t).is_absolute() {
                    self.toast("Invalid download folder", "Use an absolute path or ~/Downloads/PS5.", 2);
                } else {
                    self.save_cfg(|c| c.download_dir = t);
                }
            }
            SId::InstallDir => {
                if t.is_empty() || !util::expand_home(&t).is_absolute() {
                    self.toast("Invalid installation folder", "Use an absolute path or ~/Games/PS5.", 2);
                } else { self.save_cfg(|c| c.install_dir = t); }
            }
            SId::Rawg => {
                if t.is_empty() {
                    return; // empty keeps the saved key
                }
                self.rawg_status = "Checking key with RAWG…".into();
                self.rawg_status_kind = 0;
                std::thread::spawn(move || {
                    let err = crate::psn::check_rawg_key(&t);
                    let _ = slint::invoke_from_event_loop(move || {
                        with_app(move |app| {
                            if err.is_empty() {
                                app.save_cfg(|c| c.rawg_key = t);
                                app.rawg_status.clear();
                                app.toast("RAWG key saved", "Missing artwork is downloading in the background.", 1);
                                app.start_enrich();
                            } else {
                                app.rawg_status = err.clone();
                                app.rawg_status_kind = 2;
                                app.toast("RAWG key not saved", &format!("{err}."), 2);
                                audio::play(Sound::Error);
                            }
                            if app.overlay == Overlay::Settings {
                                app.build_settings();
                                app.push_settings();
                            }
                        })
                    });
                });
            }
            _ => {}
        }
    }

    /// Switch to another display. The UI scale is fixed per window at startup, so the
    /// launcher restarts itself on the new display (about a second).
    pub fn move_to_monitor(&mut self, name: &str) {
        let label = match name {
            "" => "the primary display".to_string(),
            crate::display::ACTIVE => "the active display".to_string(),
            _ => name.to_string(),
        };
        if !self.live.is_empty() {
            self.toast(&format!("Moves to {label} next time"), "The launcher can't restart while a game is running.", 0);
            return;
        }
        self.toast(&format!("Moving to {label}…"), "The launcher restarts on that display.", 0);
        slint::Timer::single_shot(std::time::Duration::from_millis(600), || with_app(|app| app.app_restart()));
    }

    // ------------------------------------------------------------------ the rail and the page

    /// Open Settings full screen, with focus on the rail and its first category.
    pub fn open_settings(&mut self) {
        audio::play(Sound::Select);
        let mode = Mode::current();
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
        self.settings_nav = SettingsNav {
            cats: categories(mode, self.sys.tools),
            system_app: if mode == Mode::Desktop { system_settings_command(desktop.as_deref(), cfg!(target_os = "macos"), &installed) } else { None },
            ..Default::default()
        };
        self.build_settings();
        let ui = self.ui();
        ui.set_settings_query("".into());
        ui.set_settings_find_editing(false);
        ui.set_settings_y(0.0);
        self.push_overlay(Overlay::Settings, Z_SETTINGS_RAIL, 0);
        self.push_settings();
        self.sys_probe();
    }

    pub fn act_settings(&mut self, a: Act) {
        let nav = &self.settings_nav;
        let (n, cat) = (nav.cats.len(), nav.cat);
        // L1/R1 (Tab on a keyboard) from anywhere; focus stays in the page or on the rail.
        if let Act::TabPrev | Act::TabNext = a {
            let into_page = self.zone == Z_SETTINGS;
            return self.settings_open_cat(step(n, cat, if a == Act::TabPrev { -1 } else { 1 }), into_page);
        }
        if a == Act::Search {
            return self.settings_find_start();
        }
        match self.zone {
            Z_SETTINGS => self.act_settings_page(a),
            Z_SETTINGS_FIND => match a {
                Act::Confirm => self.settings_find_start(),
                Act::Down if !self.settings_nav.hits.is_empty() => self.move_focus(Z_SETTINGS_HITS, 0),
                Act::Down => self.move_focus(Z_SETTINGS_RAIL, cat as i32),
                Act::Back if !self.settings_nav.query.is_empty() => {
                    audio::play(Sound::Back);
                    self.settings_find_edited(String::new());
                    self.ui().set_settings_query("".into());
                }
                Act::Back => self.back(),
                _ => {}
            },
            Z_SETTINGS_HITS => match a {
                Act::Up if self.idx > 0 => self.move_focus(Z_SETTINGS_HITS, self.idx - 1),
                Act::Up | Act::Back | Act::Left => self.move_focus(Z_SETTINGS_FIND, 0),
                Act::Down if (self.idx as usize) + 1 < self.settings_nav.hits.len() => self.move_focus(Z_SETTINGS_HITS, self.idx + 1),
                Act::Confirm => {
                    let hit = self.settings_nav.hits.get(self.idx as usize).and_then(|i| self.settings_nav.all.get(*i)).map(|(_, id, _)| *id);
                    if let Some(id) = hit {
                        audio::play(Sound::Select);
                        self.settings_show_row(id);
                    }
                }
                _ => {}
            },
            _ => match a {
                Act::Up if cat > 0 => self.settings_open_cat(cat - 1, false),
                Act::Up => self.move_focus(Z_SETTINGS_FIND, 0),
                Act::Down if cat + 1 < n => self.settings_open_cat(cat + 1, false),
                Act::Right | Act::Confirm => {
                    if let Some(i) = self.settings_rows.iter().position(|r| r.kind != 0) {
                        self.move_focus(Z_SETTINGS, i as i32);
                        self.scroll_settings();
                    }
                }
                Act::Back => self.back(),
                _ => {}
            },
        }
    }

    fn act_settings_page(&mut self, a: Act) {
        let n = self.settings_rows.len() as i32;
        let focusable = |rows: &Vec<crate::SettingData>, i: i32| i >= 0 && i < rows.len() as i32 && rows[i as usize].kind != 0;
        let to_rail = a == Act::Back || (a == Act::Left && !self.settings_rows.get(self.idx as usize).is_some_and(|r| left_changes(r.kind)));
        // Leaving the row drops an unsaved edit, as Escape does.
        if to_rail && self.edit_index >= 0 {
            self.finish_edit(None);
        }
        match a {
            Act::Back => {
                audio::play(Sound::Back);
                self.set_focus(Z_SETTINGS_RAIL, self.settings_nav.cat as i32);
            }
            Act::Up | Act::Down => {
                let step = if a == Act::Up { -1 } else { 1 };
                let mut j = self.idx + step;
                while j >= 0 && j < n && !focusable(&self.settings_rows, j) {
                    j += step;
                }
                if focusable(&self.settings_rows, j) {
                    self.move_focus(Z_SETTINGS, j);
                    self.scroll_settings();
                }
            }
            Act::Left if !self.settings_rows.get(self.idx as usize).is_some_and(|r| left_changes(r.kind)) => {
                self.move_focus(Z_SETTINGS_RAIL, self.settings_nav.cat as i32);
            }
            Act::Left | Act::Right => self.settings_change(self.idx as usize, if a == Act::Left { -1 } else { 1 }),
            Act::Confirm => self.settings_activate(self.idx as usize),
            _ => {}
        }
    }

    /// Open category `c`, with focus on its first row or on the rail.
    fn settings_open_cat(&mut self, c: usize, into_page: bool) {
        if self.edit_index >= 0 {
            self.finish_edit(None);
        }
        self.settings_find_stop();
        self.settings_nav.cat = c.min(self.settings_nav.cats.len().saturating_sub(1));
        self.settings_page();
        self.push_settings();
        self.ui().set_settings_y(0.0);
        let first = self.settings_rows.iter().position(|r| r.kind != 0);
        match first {
            Some(i) if into_page => self.move_focus(Z_SETTINGS, i as i32),
            _ => self.move_focus(Z_SETTINGS_RAIL, self.settings_nav.cat as i32),
        }
        if let Some(cat) = self.settings_nav.cats.get(self.settings_nav.cat).copied() {
            self.sys_page_opened(cat);
        }
    }

    /// The search field takes the keyboard.
    pub fn settings_find_start(&mut self) {
        if self.edit_index >= 0 {
            self.finish_edit(None);
        }
        self.settings_nav.find_editing = true;
        self.set_focus(Z_SETTINGS_FIND, 0);
        let ui = self.ui();
        ui.set_edit_caret(-1);
        if self.last_input_pad {
            let query = self.settings_nav.query.clone();
            self.osk_open(crate::osk_ui::Field::SettingsFind, &query);
        }
        ui.set_settings_find_editing(true);
    }

    pub fn settings_find_stop(&mut self) {
        self.osk_close_if(|f| f == crate::osk_ui::Field::SettingsFind);
        if !self.settings_nav.find_editing {
            return;
        }
        self.settings_nav.find_editing = false;
        let ui = self.ui();
        ui.set_settings_find_editing(false);
        ui.invoke_focus_root();
    }

    /// The search text changed.
    pub fn settings_find_edited(&mut self, query: String) {
        self.settings_nav.hits = search(&self.settings_nav.all, &query);
        self.settings_nav.query = query;
        self.push_settings();
    }

    /// Enter in the search field: focus goes to the first hit.
    pub fn settings_find_done(&mut self) {
        self.settings_find_stop();
        if !self.settings_nav.hits.is_empty() {
            self.move_focus(Z_SETTINGS_HITS, 0);
        }
    }

    /// Desktop mode: start the desktop's settings app. It runs on its own; only a failure to
    /// start it shows.
    fn open_system_settings(&mut self) {
        let Some((program, args)) = self.settings_nav.system_app else { return };
        std::thread::spawn(move || match std::process::Command::new(program).args(args).spawn() {
            // Wait, so the finished app does not stay a zombie process.
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(e) => {
                crate::log!("{program}: {e}");
                let _ = slint::invoke_from_event_loop(move || with_app(move |app| {
                    audio::play(Sound::Error);
                    app.toast("Couldn't open system settings", &format!("{program}: {e}"), 2);
                }));
            }
        });
    }

    /// About → Restart launcher: the normal quit path, then exit code 75, which tells
    /// ps5-launcher-session to start the launcher again.
    fn restart_launcher(&mut self) {
        if !self.live.is_empty() {
            self.toast_app("Finish your game first", "The launcher can restart once no game is running.", 0);
            return;
        }
        crate::system::set_exit_code(crate::system::EXIT_RESTART_LAUNCHER);
        let _ = slint::quit_event_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(kind: i32, label: &str, hint: &str) -> SettingData {
        SettingData { kind, label: label.into(), hint: hint.into(), ..Default::default() }
    }

    #[test]
    fn desktop_mode_has_system_and_the_others_have_about() {
        let launcher = [Cat::Games, Cat::Playing, Cat::Downloads, Cat::Emulators, Cat::Appearance, Cat::Updates, Cat::Advanced];
        let none = Tools::default();
        assert_eq!(categories(Mode::Desktop, none), [&launcher[..], &[Cat::System]].concat());
        assert_eq!(categories(Mode::Session, none), [&launcher[..], &[Cat::About]].concat());
        assert_eq!(categories(Mode::Os, none), [&launcher[..], &[Cat::About]].concat());
        assert!(Cat::System.system() && Cat::About.system());
        assert!(launcher.iter().all(|c| !c.system()));
    }

    #[test]
    fn system_pages_show_in_session_and_os_mode_when_their_tool_works() {
        let all = Tools { network: true, sound: true, display: true, storage: true, os_updates: true, time: true };
        let system = |mode| categories(mode, all).into_iter().filter(|c| c.system()).collect::<Vec<_>>();
        assert_eq!(system(Mode::Desktop), [Cat::System], "the desktop owns these");
        assert_eq!(system(Mode::Session), [Cat::Network, Cat::Sound, Cat::Display, Cat::Storage, Cat::Time, Cat::About], "OS updates need the OS");
        assert_eq!(system(Mode::Os), [Cat::Network, Cat::Sound, Cat::Display, Cat::Storage, Cat::OsUpdates, Cat::Time, Cat::About]);
        let sound_only = Tools { sound: true, ..Tools::default() };
        let cats = categories(Mode::Os, sound_only);
        assert_eq!(cats[cats.len() - 2..], [Cat::Sound, Cat::About]);
        assert!([Cat::Network, Cat::Sound, Cat::Display, Cat::Storage, Cat::OsUpdates, Cat::Time].iter().all(|c| c.system()));
        assert_eq!(Cat::OsUpdates.label(), "Updates");
        assert_eq!((Cat::Display.label(), Cat::Time.label()), ("Display", "Time"));
        let time_only = Tools { time: true, ..Tools::default() };
        assert_eq!(categories(Mode::Session, time_only).iter().filter(|c| c.system()).copied().collect::<Vec<_>>(), [Cat::Time, Cat::About]);
    }

    #[test]
    fn every_row_has_a_home() {
        use SId::*;
        assert_eq!(category(Header), None);
        for (cat, ids) in [
            (Cat::Games, &[Dirs, InstallDir, DownloadDir, Rescan][..]),
            (Cat::Playing, &[Fullscreen, ReturnOnExit, Controls]),
            (Cat::Downloads, &[SeedCompleted, Downloads]),
            (Cat::Emulators, &[KytyUpdate, Emulator, Resolution, VideoOut, Present, Amd, Extra, KytyRollback, ShadUpdate, ShadEmulator, ShadRollback, AutoUpdate]),
            (Cat::Appearance, &[Display, Sounds, Share]),
            (Cat::Updates, &[AppUpdate]),
            (Cat::Advanced, &[Rawg, RawgRemove, Refresh]),
            (Cat::System, &[OpenSystemSettings]),
            (Cat::Network, &[WifiOn, Wired, Network(0), WifiPassword, WifiScan, SavedNetwork(1)]),
            (Cat::Sound, &[Volume, Mute, Output(2)]),
            (Cat::Storage, &[Drive(0), Partition(0, 3)]),
            (Cat::OsUpdates, &[OsStatus, OsDownload, OsRollback]),
            (Cat::Display, &[Gpu, NvInstall, NvLater, NvRetry, NvRestart, NvOpenSource, OutResolution, OutRefresh]),
            (Cat::Time, &[TimeZone, Ntp, TzBack, TzRegion(3), TzZone(40)]),
            (Cat::About, &[RestartLauncher]),
        ] {
            for id in ids {
                assert_eq!(category(*id), Some(cat), "{id:?}");
            }
        }
    }

    #[test]
    fn l1_and_r1_wrap_around() {
        assert_eq!(step(8, 0, 1), 1);
        assert_eq!(step(8, 7, 1), 0);
        assert_eq!(step(8, 0, -1), 7);
        assert_eq!(step(8, 3, -1), 2);
        assert_eq!(step(0, 0, 1), 0);
    }

    #[test]
    fn left_cycles_a_value_and_otherwise_goes_to_the_rail() {
        assert!(left_changes(3));
        assert!(!left_changes(1));
        assert!(!left_changes(2));
        assert!(!left_changes(4));
    }

    #[test]
    fn search_matches_every_word_in_the_label_or_the_hint() {
        assert!(matches("res", "Resolution", ""));
        assert!(matches("RESOL", "Resolution", ""));
        assert!(matches("tear", "Present mode", "Try V-Sync if the picture tears"));
        assert!(matches("game folder", "Game folders", ""));
        assert!(matches("folders rescan", "Rescan game folders", ""));
        assert!(!matches("folders zzz", "Game folders", ""));
        assert!(!matches("", "Game folders", ""));
        assert!(!matches("   ", "Game folders", ""));
    }

    #[test]
    fn search_skips_headers_and_keeps_the_order() {
        let rows = vec![
            (Cat::Emulators, SId::Header, data(0, "PS5 GAMES · KYTYPS5", "")),
            (Cat::Emulators, SId::Resolution, data(3, "Resolution", "The size of the window")),
            (Cat::Emulators, SId::VideoOut, data(3, "Game output resolution", "")),
            (Cat::Appearance, SId::Display, data(3, "Display", "")),
        ];
        assert_eq!(search(&rows, "resolution"), [1, 2]);
        assert_eq!(search(&rows, "games"), Vec::<usize>::new());
        let many: Vec<_> = (0..20).map(|_| (Cat::Games, SId::Rescan, data(4, "Rescan", ""))).collect();
        assert_eq!(search(&many, "rescan").len(), MAX_HITS);
    }

    #[test]
    fn the_desktop_settings_app_follows_the_desktop() {
        let all = |_: &str| true;
        let none = |_: &str| false;
        assert_eq!(system_settings_command(Some("KDE"), false, &all), Some(("systemsettings", &[][..])));
        assert_eq!(system_settings_command(Some("ubuntu:GNOME"), false, &all), Some(("gnome-control-center", &[][..])));
        assert_eq!(system_settings_command(Some("GNOME-Classic:GNOME"), false, &all), Some(("gnome-control-center", &[][..])));
        assert_eq!(system_settings_command(Some("KDE"), false, &none), None);
        assert_eq!(system_settings_command(Some("XFCE"), false, &all), None);
        assert_eq!(system_settings_command(None, false, &all), None);
        assert_eq!(system_settings_command(None, true, &all), Some(("/usr/bin/open", &["-b", "com.apple.systempreferences"][..])));
        assert_eq!(system_settings_command(None, true, &none), None);
    }

    #[test]
    fn dots_show_where_an_update_waits() {
        assert!(waiting(Cat::Updates, true, false, false));
        assert!(!waiting(Cat::Updates, false, true, true));
        assert!(waiting(Cat::Emulators, false, true, false));
        assert!(!waiting(Cat::Emulators, true, false, true));
        assert!(!waiting(Cat::Games, true, true, true));
        assert!(waiting(Cat::Display, false, false, true), "the NVIDIA driver waits");
        assert!(!waiting(Cat::Display, true, true, false));
    }

    #[test]
    fn about_names_the_mode() {
        assert_eq!(mode_label(Mode::Desktop), "Desktop app");
        assert_eq!(mode_label(Mode::Session), "PS5 Launcher session");
        assert_eq!(mode_label(Mode::Os), "PS5 Launcher OS");
    }
}
