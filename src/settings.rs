//! Settings, a sheet on the right: the Launcher's sections, then the System areas, each on its own
//! sub-sheet; controller-friendly editing, instant save, and search over every row.

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
    // Controllers: a connected controller, the Bluetooth switch, a paired device (Forget), and
    // pairing a new controller (the indices are into `SystemUi`'s lists).
    Pad(usize),
    BtPower,
    BtDevice(usize),
    BtPair,
    BtScan,
    BtFound(usize),
    BtPairClose,
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
    /// PS5 Launcher OS: the first-start setup again.
    RunSetup,
    /// The root sheet's entry row of a System area: it opens the area's sub-sheet. Only on the
    /// sheet, never in `SettingsNav::all`.
    Open(Cat),
}

/// A category of Settings, top to bottom. The Launcher's are sections of the root sheet; the
/// System ones (but the desktop's System) are areas, each with its own sub-sheet.
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
    /// The connected controllers and their battery, in Session and OS mode; and Bluetooth
    /// (pairing, Forget) when bluetoothctl finds an adapter.
    Controllers,
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
    /// The first-start setup's last step; never in Settings. It has no rows, only buttons.
    Setup,
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
            Cat::Controllers => "Controllers",
            Cat::Sound => "Sound",
            Cat::Storage => "Storage",
            Cat::Display => "Display",
            Cat::Time => "Time",
            // Under the SYSTEM heading, so not mixed up with the launcher's Updates.
            Cat::OsUpdates => "Updates",
            Cat::About => "About",
            Cat::Setup => "Setup",
        }
    }

    /// Under the SYSTEM heading.
    pub fn system(self) -> bool {
        matches!(self, Cat::System | Cat::Network | Cat::Controllers | Cat::Sound | Cat::Display | Cat::Storage | Cat::OsUpdates | Cat::Time | Cat::About)
    }

    /// A System area: an entry row on the root sheet, its rows on a sub-sheet. The desktop's
    /// System row stays on the root sheet.
    pub fn area(self) -> bool {
        self.system() && self != Cat::System
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
    /// bluetoothctl, with an adapter.
    pub bluetooth: bool,
}

/// The categories in `mode`, top to bottom, with the System areas whose tools work.
pub fn categories(mode: Mode, tools: Tools) -> Vec<Cat> {
    let mut cats = vec![Cat::Games, Cat::Playing, Cat::Downloads, Cat::Emulators, Cat::Appearance, Cat::Updates, Cat::Advanced];
    if mode == Mode::Desktop {
        // The desktop owns the system's settings.
        cats.push(Cat::System);
        return cats;
    }
    // File sharing joins in its own step of Phase 6 (docs/plans/ps5-launcher-os.md). Controllers
    // is always there: without Bluetooth it still shows the connected controllers and their
    // battery.
    let pages = [
        (Cat::Network, tools.network),
        (Cat::Controllers, true),
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
        Pad(_) | BtPower | BtDevice(_) | BtPair | BtScan | BtFound(_) | BtPairClose => Cat::Controllers,
        Volume | Mute | Output(_) => Cat::Sound,
        Drive(_) | Partition(..) => Cat::Storage,
        OsStatus | OsDownload | OsRollback => Cat::OsUpdates,
        Gpu | NvInstall | NvLater | NvRetry | NvRestart | NvOpenSource | OutResolution | OutRefresh => Cat::Display,
        TimeZone | Ntp | TzBack | TzRegion(_) | TzZone(_) => Cat::Time,
        RestartLauncher | RunSetup => Cat::About,
        Open(cat) => cat,
    })
}

/// The sub-sheet a row shows on: its System area's. None: the root sheet.
pub fn sub_sheet(id: SId) -> Option<Cat> {
    category(id).filter(|c| c.area())
}

/// L1/R1: the previous or next of `n` sections, wrapping at both ends.
pub fn step(n: usize, cur: usize, dir: i32) -> usize {
    if n == 0 {
        return 0;
    }
    (cur as i64 + dir as i64).rem_euclid(n as i64) as usize
}

/// Left changes a row that cycles through values; on any other row it does nothing. Back leaves
/// a sub-sheet or closes Settings.
pub fn left_changes(kind: i32) -> bool {
    kind == 3
}

/// The sheet on screen: its rows, what each one is, and where each section starts (L1/R1).
#[derive(Default)]
pub struct Projection {
    pub ids: Vec<SId>,
    pub rows: Vec<SettingData>,
    /// The first row of each section: a heading, or the top of a sub-sheet.
    pub sections: Vec<usize>,
}

impl Projection {
    fn push(&mut self, id: SId, row: SettingData) {
        self.ids.push(id);
        self.rows.push(row);
    }
}

/// The rows of `all` that the sheet shows. The root sheet (`sub` None): each launcher category
/// of `cats` under its heading, then SYSTEM: the desktop's own row, or one entry row per System
/// area. A sub-sheet: the rows of its area. `dot`: something waits in that category.
pub fn project(all: &[(Cat, SId, SettingData)], cats: &[Cat], sub: Option<Cat>, dot: &dyn Fn(Cat) -> bool) -> Projection {
    let mut p = Projection::default();
    let rows_of = |cat: Cat| all.iter().filter(move |(c, _, _)| *c == cat).map(|(_, id, r)| (*id, r.clone()));
    if let Some(cat) = sub {
        for (id, r) in rows_of(cat) {
            if id == SId::Header || p.ids.is_empty() {
                p.sections.push(p.ids.len());
            }
            p.push(id, r);
        }
        return p;
    }
    let heading = |label: &str, dot: bool| SettingData { dot, ..row(0, &label.to_uppercase()) };
    for cat in cats.iter().copied().filter(|c| !c.system()) {
        p.sections.push(p.ids.len());
        p.push(SId::Header, heading(cat.label(), dot(cat)));
        rows_of(cat).for_each(|(id, r)| p.push(id, r));
    }
    let mut system = Projection::default();
    for cat in cats.iter().copied().filter(|c| c.system()) {
        if cat.area() {
            system.push(SId::Open(cat), SettingData { more: true, dot: dot(cat), ..row(4, cat.label()) });
        } else {
            rows_of(cat).for_each(|(id, r)| system.push(id, r));
        }
    }
    if !system.ids.is_empty() {
        p.sections.push(p.ids.len());
        p.push(SId::Header, heading("System", false));
        p.ids.extend(system.ids);
        p.rows.extend(system.rows);
    }
    p
}

/// Where L1/R1 goes, and whether the row being typed in is dropped on the way.
#[derive(Debug, PartialEq, Eq)]
pub struct Jump {
    pub drop_edit: bool,
    pub to: Option<usize>,
}

/// L1/R1 from row `idx` (-1: the search field): the first row of the previous or next section,
/// wrapping around. An edit is dropped first, as Escape drops it: its row leaves the screen.
pub fn jump(sections: &[usize], rows: &[SettingData], idx: i32, dir: i32, editing: bool) -> Jump {
    let n = sections.len();
    let cur = sections.iter().rposition(|s| *s as i32 <= idx);
    let to = match cur {
        _ if n == 0 => None,
        Some(cur) => Some(step(n, cur, dir)),
        // Above the first section: R1 goes to the first one, L1 to the last.
        None => Some(if dir > 0 { 0 } else { n - 1 }),
    };
    let to = to.and_then(|s| {
        let end = sections.get(s + 1).copied().unwrap_or(rows.len()).min(rows.len());
        (sections[s]..end).find(|i| rows[*i].kind != 0)
    });
    Jump { drop_edit: editing, to }
}

/// Where the root sheet was when a sub-sheet opened: the entry row (its index on the sheet then)
/// and the scroll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Return {
    pub entry: SId,
    pub idx: usize,
    pub y: f32,
}

/// What Back does, innermost first.
#[derive(Debug, PartialEq)]
pub enum Back {
    /// Drop the row being typed in.
    Edit,
    /// Leave what the sub-sheet has open: pairing, the time zone list.
    Flow,
    /// Back to the root sheet, where it was.
    Root(Option<Return>),
    /// Close Settings.
    Close,
}

/// The focus and scroll on the rebuilt root sheet `ids` for `ret`: its row with its scroll when
/// the row did not move, else the entry row (scroll to it). None: the area is gone.
pub fn restore(ret: &Return, ids: &[SId]) -> Option<(usize, Option<f32>)> {
    if ids.get(ret.idx) == Some(&ret.entry) {
        return Some((ret.idx, Some(ret.y)));
    }
    ids.iter().position(|id| *id == ret.entry).map(|i| (i, None))
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

/// Sizes of the Settings sheet on the 1920×1080 canvas (ui/settings.slint), for the scroll.
/// The sheet's content is 648 wide: 760 less 56 each side.
const SHEET_W: f32 = 648.0;
/// The root sheet's top: the padding, Back, the title, the subtitle and the search field.
const ROOT_TOP: f32 = 64.0 + 68.0 + 53.0 + 22.0 + 78.0;
/// A sub-sheet's top: the same, with a space instead of the search field.
const SUB_TOP: f32 = 64.0 + 68.0 + 53.0 + 22.0 + 20.0;
/// A search hit under the search field, and the space over the first one.
const HIT: f32 = 72.0;
const HITS_TOP: f32 = 12.0;
/// The About card under the rows, with its space or heading.
const ABOUT_CARD: f32 = 36.0 + 30.0 + 120.0;
/// The Secure Boot key's steps (Updates, Display), with the space under them: the setup's wide
/// page, and the sheet, where the steps wrap more.
const KEY_CARD: f32 = 510.0;
const KEY_CARD_NARROW: f32 = 560.0;
/// The NVIDIA driver's card on Display (the stepper, a title and its text).
const NV_CARD: f32 = 300.0;
const NV_CARD_NARROW: f32 = 400.0;
/// The pairing card on Controllers: the stepper, a title, its text and the pictures of the
/// buttons to hold; and without the pictures.
const PAIR_CARD: f32 = 560.0;
const PAIR_CARD_NARROW: f32 = 830.0;
const PAIR_CARD_TEXT: f32 = 280.0;
const PAIR_CARD_TEXT_NARROW: f32 = 360.0;
/// The setup's page (ui/setup.slint): its rows start under the stepper, the title and the text,
/// and end over the buttons.
const SETUP_TOP: f32 = 330.0;
const SETUP_BOTTOM: f32 = 150.0;

/// The height of a row of `SettingRows` (ui/settings.slint) whose hints wrap at `per_line`
/// characters. `gap`: the space over a heading.
fn row_height(r: &SettingData, gap: f32, per_line: usize) -> f32 {
    let hint = match r.hint.chars().count().div_ceil(per_line.max(1)) {
        0 => 0.0,
        lines => 6.0 + lines as f32 * 20.0,
    };
    match r.kind {
        0 if r.label.is_empty() => 20.0 + 10.0,
        0 => gap + 20.0 + 10.0,
        1 => 8.0 + 23.0 + 6.0 + 54.0 + hint + 4.0,
        _ => 8.0 + 64.0 + hint + 4.0,
    }
}

/// Settings' state: the categories, the sheet on screen, every row of every category (for
/// search), and the search field.
#[derive(Default)]
pub struct SettingsNav {
    pub cats: Vec<Cat>,
    /// The open sub-sheet's area; None: the root sheet. The setup shows its step's area.
    pub sub: Option<Cat>,
    /// Where the root sheet was when the sub-sheet opened.
    pub ret: Option<Return>,
    /// The sheet's sections: indices of their first rows (see `Projection`).
    pub sections: Vec<usize>,
    /// Every row of every category, in `cats` order. The sheet shows a projection of them.
    pub all: Vec<(Cat, SId, SettingData)>,
    pub query: String,
    /// Search hits: indices into `all`.
    pub hits: Vec<usize>,
    /// The search field has the keyboard.
    pub find_editing: bool,
    /// Desktop mode: the desktop's settings app, looked up when Settings opens.
    pub system_app: Option<(&'static str, &'static [&'static str])>,
}

impl SettingsNav {
    /// Open the sub-sheet of `area`; Back comes back to `from`.
    pub fn open_sub(&mut self, area: Cat, from: Return) {
        self.sub = Some(area);
        self.ret = Some(from);
    }

    /// Back, innermost first: the row being typed in, what the sub-sheet has open (`flow`), the
    /// sub-sheet, then Settings. Leaving the sub-sheet gives where the root sheet was.
    pub fn back(&mut self, editing: bool, flow: bool) -> Back {
        if editing {
            Back::Edit
        } else if flow {
            Back::Flow
        } else if self.sub.take().is_some() {
            Back::Root(self.ret.take())
        } else {
            Back::Close
        }
    }
}

/// Is `program` installed: an absolute path that exists, or a file in a `PATH` folder.
pub(crate) fn installed(program: &str) -> bool {
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
            if Mode::current() == Mode::Os {
                let mut r = row(4, "Run the setup again");
                r.hint = "Network, time zone, controllers, the graphics driver and a drive for games".into();
                add(&mut rows, SId::RunSetup, r);
            }
        }

        self.settings_nav.all = rows;
        // Rows come and go between builds: hits point into `all`, so find them again.
        self.settings_nav.hits = search(&self.settings_nav.all, &self.settings_nav.query);
        self.settings_page();
    }

    /// The sheet on screen: the root sheet or the open sub-sheet, from every row.
    fn settings_page(&mut self) {
        let (app_update, emulator_update, display) = (self.upd.installed.is_some() || self.app_update_available(), self.kyty_update_available(), self.display_dot());
        let nav = &self.settings_nav;
        let p = project(&nav.all, &nav.cats, nav.sub, &|cat| waiting(cat, app_update, emulator_update, display));
        self.settings_nav.sections = p.sections;
        self.settings_ids = p.ids;
        self.settings_rows = p.rows;
    }

    /// The first row that takes focus on the sheet; -1: it has none.
    fn settings_first_row(&self) -> i32 {
        self.settings_rows.iter().position(|r| r.kind != 0).map_or(-1, |i| i as i32)
    }

    /// Open Settings on "Share your game ratings".
    pub fn open_share_setting(&mut self) {
        self.open_settings();
        self.settings_show_row(SId::Share);
    }

    /// Focus on row `id`: on the root sheet, or on its area's sub-sheet. Back from that sub-sheet
    /// goes to the area's entry row.
    pub fn settings_show_row(&mut self, id: SId) {
        let Some(cat) = category(id) else { return };
        if !self.settings_nav.cats.contains(&cat) {
            return;
        }
        let sub = sub_sheet(id);
        if sub != self.settings_nav.sub {
            if self.edit_index >= 0 {
                self.finish_edit(None);
            }
            match sub {
                Some(area) => {
                    self.settings_nav.open_sub(area, Return { entry: SId::Open(area), idx: usize::MAX, y: 0.0 });
                    self.sys_page_opened(area);
                }
                None => (self.settings_nav.sub, self.settings_nav.ret) = (None, None),
            }
        }
        self.settings_page();
        let i = self.settings_ids.iter().position(|s| *s == id).map_or(self.settings_first_row(), |i| i as i32);
        self.set_focus(Z_SETTINGS, i);
        self.push_settings();
        self.scroll_settings();
    }

    /// An entry row: open its area's sub-sheet, with focus on its first row. Back comes back to
    /// this row and this scroll.
    fn settings_open_sub(&mut self, area: Cat) {
        if self.edit_index >= 0 {
            self.finish_edit(None);
        }
        self.settings_find_stop();
        audio::play(Sound::Select);
        let ui = self.ui();
        let from = Return { entry: SId::Open(area), idx: self.idx.max(0) as usize, y: ui.get_settings_y() };
        self.settings_nav.open_sub(area, from);
        self.settings_page();
        self.set_focus(Z_SETTINGS, self.settings_first_row());
        self.push_settings();
        ui.set_settings_y(0.0);
        self.sys_page_opened(area);
    }

    /// Back on the root sheet from a sub-sheet: focus and scroll as they were.
    pub(crate) fn settings_root(&mut self, ret: Option<Return>) {
        self.settings_page();
        self.push_settings();
        match ret.and_then(|r| restore(&r, &self.settings_ids)) {
            Some((i, y)) => {
                self.set_focus(Z_SETTINGS, i as i32);
                match y {
                    Some(y) => self.ui().set_settings_y(y),
                    None => self.scroll_settings(),
                }
            }
            None => {
                self.set_focus(Z_SETTINGS, self.settings_first_row());
                self.scroll_settings();
            }
        }
    }

    /// The search hits show under the search field while it or a hit has focus.
    fn settings_hits_shown(&self) -> bool {
        self.settings_nav.sub.is_none() && !self.settings_nav.query.is_empty() && matches!(self.zone, Z_SETTINGS_FIND | Z_SETTINGS_HITS)
    }

    pub fn push_settings(&mut self) {
        let nav = &self.settings_nav;
        let hits: Vec<crate::SettingsHit> = nav.hits.iter().filter_map(|i| nav.all.get(*i)).map(|(cat, _, r)| crate::SettingsHit {
            label: r.label.clone(),
            place: if cat.area() { format!("System · {}", cat.label()) } else { cat.label().to_string() }.into(),
        }).collect();
        let sub = nav.sub;
        let ui = self.ui();
        ui.set_settings_title(sub.map(|c| c.label()).unwrap_or_default().into());
        ui.set_settings_hits(model(hits));
        // The About card: on About's sub-sheet, or at the root sheet's end on a desktop.
        ui.set_settings_about(sub == Some(Cat::About) || (sub.is_none() && nav.cats.contains(&Cat::System)));
        let key = match sub {
            Some(Cat::OsUpdates) => self.os_key_digits(),
            Some(Cat::Display) => self.nv_key_digits(),
            _ => Vec::new(),
        };
        ui.set_settings_key(model(key));
        ui.set_settings_key_last(
            if self.overlay == Overlay::Setup {
                "Choose \"Reboot\". The setup comes back, checks the key by itself and downloads the driver."
            } else if sub == Some(Cat::Display) {
                "Choose \"Reboot\". Back here, the launcher checks the key by itself; then download the driver."
            } else {
                "Choose \"Reboot\". Back here, choose Download update again."
            }
            .into(),
        );
        ui.set_settings_nv(if sub == Some(Cat::Display) { self.nv_card() } else { crate::NvCard::default() });
        ui.set_settings_pair(if sub == Some(Cat::Controllers) { self.pair_card() } else { crate::PairCard::default() });
        ui.set_settings_mode(mode_label(Mode::current()).into());
        ui.set_settings(model(self.settings_rows.clone()));
        ui.set_edit_index(self.edit_index);
        self.push_setup();
    }

    /// Keep the focused row, or the focused search hit, in view.
    pub fn scroll_settings(&mut self) {
        // Estimated heights matching ui/settings.slint and ui/setup.slint. Hints are 16 px text,
        // about 8.5 px a character.
        let (w, h) = self.logical_size();
        let setup = self.overlay == Overlay::Setup;
        let sub = self.settings_nav.sub;
        let ui = self.ui();
        // The setup's page is centred, at most 1100 wide, under its title; all of the sheet
        // scrolls, from its top.
        let (width, view, mut y) = if setup {
            ((w - 192.0).min(1100.0), h - SETUP_TOP - SETUP_BOTTOM, 0.0)
        } else if sub.is_some() {
            (SHEET_W, h, SUB_TOP)
        } else {
            let hits = if self.settings_hits_shown() { HITS_TOP + HIT * self.settings_nav.hits.len().max(1) as f32 } else { 0.0 };
            (SHEET_W, h, ROOT_TOP + hits)
        };
        let per_line = (width / 8.5).max(20.0) as usize;
        // The cards over the rows of a System area.
        let (key, nv, pair) = if setup { (KEY_CARD, NV_CARD, (PAIR_CARD, PAIR_CARD_TEXT)) } else { (KEY_CARD_NARROW, NV_CARD_NARROW, (PAIR_CARD_NARROW, PAIR_CARD_TEXT_NARROW)) };
        y += if slint::Model::row_count(&ui.get_settings_key()) > 0 { key } else { 0.0 };
        y += if ui.get_settings_nv().show { nv } else { 0.0 };
        let p = ui.get_settings_pair();
        y += match (p.show, p.pictures) {
            (false, _) => 0.0,
            (true, true) => pair.0,
            (true, false) => pair.1,
        };
        let mut target = match self.zone {
            Z_SETTINGS_HITS => ROOT_TOP + HITS_TOP + HIT * self.idx.max(0) as f32,
            Z_SETTINGS => y,
            _ => 0.0,
        };
        for (i, r) in self.settings_rows.iter().enumerate() {
            if i as i32 == self.idx && self.zone == Z_SETTINGS {
                target = y;
            }
            // The setup's first heading sits close under its text.
            y += row_height(r, if i == 0 && setup { 8.0 } else { 36.0 }, per_line);
        }
        if ui.get_settings_about() {
            y += ABOUT_CARD;
        }
        y += if setup { 24.0 } else { 120.0 };
        let max = (y - view).max(0.0);
        ui.set_settings_y(-(target - view * 0.4).clamp(0.0, max) * self.scale);
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
            SId::WifiOn | SId::Mute | SId::Volume | SId::OutResolution | SId::OutRefresh | SId::Ntp | SId::BtPower => return self.sys_change(id, dir),
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
            SId::Fullscreen | SId::Amd | SId::ReturnOnExit | SId::Sounds | SId::AutoUpdate | SId::SeedCompleted | SId::WifiOn | SId::Mute | SId::Ntp | SId::BtPower => {
                let on = self.settings_rows[i].on;
                self.settings_change(i, if on { -1 } else { 1 });
            }
            SId::Resolution | SId::Present | SId::VideoOut | SId::Display | SId::OutResolution | SId::OutRefresh => self.settings_change(i, 1),
            SId::Volume | SId::Wired | SId::Drive(_) | SId::Gpu | SId::Pad(_) => {}
            SId::Network(_) | SId::WifiScan | SId::SavedNetwork(_) | SId::Output(_) | SId::Partition(..) | SId::OsStatus | SId::OsDownload | SId::OsRollback => self.sys_activate(id),
            SId::NvInstall | SId::NvLater | SId::NvRetry | SId::NvRestart | SId::NvOpenSource | SId::TimeZone | SId::TzBack | SId::TzRegion(_) | SId::TzZone(_) => {
                self.sys_activate(id)
            }
            SId::BtDevice(_) | SId::BtPair | SId::BtScan | SId::BtFound(_) | SId::BtPairClose => self.sys_activate(id),
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
            SId::RunSetup => self.setup_again(),
            SId::Downloads => self.open_downloads(None),
            SId::Share => self.share_results(),
            SId::Controls => self.open_controls(),
            SId::Open(area) => self.settings_open_sub(area),
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

    // ------------------------------------------------------------------ the sheet

    /// Open Settings: the root sheet, with focus on its first row.
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
        self.push_overlay(Overlay::Settings, Z_SETTINGS, self.settings_first_row());
        self.push_settings();
        self.sys_probe();
    }

    pub fn act_settings(&mut self, a: Act) {
        // L1/R1 (Tab on a keyboard) from anywhere: the previous or next section.
        if let Act::TabPrev | Act::TabNext = a {
            let from = if self.zone == Z_SETTINGS { self.idx } else { -1 };
            let j = jump(&self.settings_nav.sections, &self.settings_rows, from, if a == Act::TabPrev { -1 } else { 1 }, self.edit_index >= 0);
            // Dropping the edit can take a row away (a Wi-Fi password): find the target again.
            let to = j.to.and_then(|i| self.settings_ids.get(i).copied());
            if j.drop_edit {
                self.finish_edit(None);
            }
            self.settings_find_stop();
            if let Some(i) = to.and_then(|id| self.settings_ids.iter().position(|x| *x == id)) {
                self.move_focus(Z_SETTINGS, i as i32);
                self.scroll_settings();
            }
            return;
        }
        if a == Act::Search {
            return self.settings_find_start();
        }
        match self.zone {
            Z_SETTINGS => self.act_settings_page(a),
            Z_SETTINGS_FIND => match a {
                Act::Confirm => self.settings_find_start(),
                Act::Down if !self.settings_nav.hits.is_empty() && !self.settings_nav.query.is_empty() => {
                    self.move_focus(Z_SETTINGS_HITS, 0);
                    self.scroll_settings();
                }
                Act::Down => {
                    self.move_focus(Z_SETTINGS, self.settings_first_row());
                    self.scroll_settings();
                }
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
            _ => {}
        }
        if self.zone == Z_SETTINGS_HITS {
            self.scroll_settings();
        }
    }

    pub(crate) fn act_settings_page(&mut self, a: Act) {
        let n = self.settings_rows.len() as i32;
        let focusable = |rows: &Vec<crate::SettingData>, i: i32| i >= 0 && i < rows.len() as i32 && rows[i as usize].kind != 0;
        match a {
            // Back, innermost first: the row being typed in, what the sub-sheet has open, the
            // sub-sheet, Settings.
            Act::Back => match self.settings_nav.back(self.edit_index >= 0, self.sys_flow()) {
                Back::Edit => {
                    audio::play(Sound::Back);
                    self.finish_edit(None);
                }
                Back::Flow => {
                    self.sys_back();
                }
                Back::Root(ret) => {
                    audio::play(Sound::Back);
                    self.settings_root(ret);
                }
                Back::Close => self.back(),
            },
            Act::Up | Act::Down => {
                let step = if a == Act::Up { -1 } else { 1 };
                let mut j = self.idx + step;
                while j >= 0 && j < n && !focusable(&self.settings_rows, j) {
                    j += step;
                }
                if focusable(&self.settings_rows, j) {
                    self.move_focus(Z_SETTINGS, j);
                    self.scroll_settings();
                } else if a == Act::Up && self.overlay == Overlay::Settings && self.settings_nav.sub.is_none() {
                    // Over the root sheet's first row: the search field.
                    self.move_focus(Z_SETTINGS_FIND, 0);
                    self.scroll_settings();
                }
            }
            // Left changes a value; on any other row it stays.
            Act::Left if !self.settings_rows.get(self.idx as usize).is_some_and(|r| left_changes(r.kind)) => {}
            Act::Left | Act::Right => self.settings_change(self.idx as usize, if a == Act::Left { -1 } else { 1 }),
            Act::Confirm => self.settings_activate(self.idx as usize),
            _ => {}
        }
    }

    /// The search field takes the keyboard. It is on the root sheet: a sub-sheet closes first.
    pub fn settings_find_start(&mut self) {
        if self.edit_index >= 0 {
            self.finish_edit(None);
        }
        if self.settings_nav.sub.is_some() {
            self.sys_close_pages();
            (self.settings_nav.sub, self.settings_nav.ret) = (None, None);
            self.settings_page();
            self.push_settings();
        }
        self.settings_nav.find_editing = true;
        self.set_focus(Z_SETTINGS_FIND, 0);
        let ui = self.ui();
        ui.set_settings_y(0.0);
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
        assert_eq!(categories(Mode::Session, none), [&launcher[..], &[Cat::Controllers, Cat::About]].concat());
        assert_eq!(categories(Mode::Os, none), [&launcher[..], &[Cat::Controllers, Cat::About]].concat());
        assert!(Cat::System.system() && Cat::About.system());
        assert!(launcher.iter().all(|c| !c.system()));
    }

    #[test]
    fn system_pages_show_in_session_and_os_mode_when_their_tool_works() {
        let all = Tools { network: true, sound: true, display: true, storage: true, os_updates: true, time: true, bluetooth: true };
        let system = |mode| categories(mode, all).into_iter().filter(|c| c.system()).collect::<Vec<_>>();
        assert_eq!(system(Mode::Desktop), [Cat::System], "the desktop owns these");
        assert_eq!(system(Mode::Session), [Cat::Network, Cat::Controllers, Cat::Sound, Cat::Display, Cat::Storage, Cat::Time, Cat::About], "OS updates need the OS");
        assert_eq!(system(Mode::Os), [Cat::Network, Cat::Controllers, Cat::Sound, Cat::Display, Cat::Storage, Cat::OsUpdates, Cat::Time, Cat::About]);
        let sound_only = Tools { sound: true, ..Tools::default() };
        let cats = categories(Mode::Os, sound_only);
        assert_eq!(cats[cats.len() - 2..], [Cat::Sound, Cat::About]);
        assert!([Cat::Network, Cat::Controllers, Cat::Sound, Cat::Display, Cat::Storage, Cat::OsUpdates, Cat::Time].iter().all(|c| c.system()));
        assert_eq!(Cat::OsUpdates.label(), "Updates");
        assert_eq!((Cat::Display.label(), Cat::Time.label()), ("Display", "Time"));
        let time_only = Tools { time: true, ..Tools::default() };
        assert_eq!(categories(Mode::Session, time_only).iter().filter(|c| c.system()).copied().collect::<Vec<_>>(), [Cat::Controllers, Cat::Time, Cat::About]);
    }

    #[test]
    fn controllers_show_without_bluetooth_but_not_on_a_desktop() {
        // The connected controllers and their battery need no tool; Bluetooth only adds rows.
        let none = Tools::default();
        assert!(categories(Mode::Session, none).contains(&Cat::Controllers));
        assert!(categories(Mode::Os, none).contains(&Cat::Controllers));
        assert!(!categories(Mode::Desktop, Tools { bluetooth: true, ..none }).contains(&Cat::Controllers), "the desktop owns Bluetooth");
        assert_eq!(Cat::Controllers.label(), "Controllers");
        assert!(Cat::Controllers.area() && Cat::About.area());
        assert!(!Cat::System.area() && !Cat::Games.area(), "the desktop's row and the launcher stay on the root sheet");
    }

    #[test]
    fn every_row_has_a_home() {
        use SId::*;
        assert_eq!(category(Header), None);
        // An entry row of the root sheet stands for its area.
        assert_eq!(category(Open(Cat::Network)), Some(Cat::Network));
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
            (Cat::Controllers, &[Pad(0), BtPower, BtDevice(1), BtPair, BtScan, BtFound(2), BtPairClose]),
            (Cat::Sound, &[Volume, Mute, Output(2)]),
            (Cat::Storage, &[Drive(0), Partition(0, 3)]),
            (Cat::OsUpdates, &[OsStatus, OsDownload, OsRollback]),
            (Cat::Display, &[Gpu, NvInstall, NvLater, NvRetry, NvRestart, NvOpenSource, OutResolution, OutRefresh]),
            (Cat::Time, &[TimeZone, Ntp, TzBack, TzRegion(3), TzZone(40)]),
            (Cat::About, &[RestartLauncher, RunSetup]),
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
    fn left_cycles_a_value_and_otherwise_stays_on_the_row() {
        // No rail to go to: Back leaves a sub-sheet or closes Settings.
        assert!(left_changes(3));
        assert!(!left_changes(1));
        assert!(!left_changes(2));
        assert!(!left_changes(4));
    }

    /// One row of each launcher category, the Emulators' own headers, and a few System rows.
    fn model_rows() -> Vec<(Cat, SId, SettingData)> {
        vec![
            (Cat::Games, SId::Dirs, data(1, "Game folders", "")),
            (Cat::Games, SId::Rescan, data(4, "Rescan game folders", "")),
            (Cat::Playing, SId::Fullscreen, data(2, "Play games in fullscreen", "")),
            (Cat::Downloads, SId::SeedCompleted, data(2, "Keep sharing finished downloads", "")),
            (Cat::Emulators, SId::AutoUpdate, data(2, "Update automatically", "")),
            (Cat::Emulators, SId::Header, data(0, "PS5 GAMES · KYTYPS5", "")),
            (Cat::Emulators, SId::Resolution, data(3, "Resolution", "")),
            (Cat::Emulators, SId::Header, data(0, "PS4 GAMES · SHADPS4", "")),
            (Cat::Emulators, SId::ShadUpdate, data(4, "shadPS4", "")),
            (Cat::Appearance, SId::Sounds, data(2, "Interface sounds", "")),
            (Cat::Updates, SId::AppUpdate, data(4, "PS5 Launcher", "")),
            (Cat::Advanced, SId::Rawg, data(1, "RAWG API key", "")),
            (Cat::System, SId::OpenSystemSettings, data(4, "Your desktop manages these", "")),
            (Cat::Network, SId::WifiOn, data(2, "Wi-Fi", "")),
            (Cat::Network, SId::Header, data(0, "NETWORKS", "")),
            (Cat::Network, SId::Network(0), data(4, "Home", "")),
            (Cat::Controllers, SId::Header, data(0, "CONNECTED", "")),
            (Cat::Controllers, SId::Pad(0), data(4, "DualSense", "")),
            (Cat::Display, SId::Gpu, data(4, "Graphics card", "")),
            (Cat::Time, SId::TimeZone, data(4, "Time zone", "")),
            (Cat::About, SId::RestartLauncher, data(4, "Restart launcher", "")),
        ]
    }

    fn headings(p: &Projection) -> Vec<String> {
        p.sections.iter().map(|i| p.rows[*i].label.to_string()).collect()
    }

    #[test]
    fn the_root_sheet_shows_the_launcher_first_then_one_entry_row_per_system_area() {
        let all = model_rows();
        let cats = categories(Mode::Os, Tools { network: true, display: true, time: true, ..Tools::default() });
        let p = project(&all, &cats, None, &|c| c == Cat::Display);
        assert_eq!(headings(&p), ["GAMES", "PLAYING", "DOWNLOADS", "EMULATORS", "APPEARANCE", "UPDATES", "ADVANCED", "SYSTEM"]);
        assert_eq!(p.ids.len(), p.rows.len());
        // The Emulators keep their two sub-headings; they are not sections of their own.
        let emu = p.sections[3];
        assert_eq!(p.ids[emu + 1..emu + 6], [SId::AutoUpdate, SId::Header, SId::Resolution, SId::Header, SId::ShadUpdate]);
        assert_eq!(p.rows[emu + 2].label, "PS5 GAMES · KYTYPS5");
        // SYSTEM: an entry row per area, in the old rail's order.
        let system = p.sections[7];
        assert_eq!(p.ids[system + 1..], [SId::Open(Cat::Network), SId::Open(Cat::Controllers), SId::Open(Cat::Display), SId::Open(Cat::Time), SId::Open(Cat::About)]);
        let display = &p.rows[system + 3];
        assert_eq!((display.kind, display.label.as_str(), display.more, display.dot), (4, "Display", true, true), "the NVIDIA driver waits there");
        assert!(!p.rows[system + 1].dot);
        assert!(!p.ids.contains(&SId::Network(0)) && !p.ids.contains(&SId::RestartLauncher), "System rows live in their sub-sheet");
        assert!(!p.ids.contains(&SId::OpenSystemSettings), "not a desktop");
    }

    #[test]
    fn a_dot_shows_on_the_section_where_an_update_waits() {
        let all = model_rows();
        let cats = categories(Mode::Session, Tools::default());
        let p = project(&all, &cats, None, &|c| waiting(c, true, true, false));
        let dotted: Vec<String> = p.rows.iter().filter(|r| r.dot).map(|r| r.label.to_string()).collect();
        assert_eq!(dotted, ["EMULATORS", "UPDATES"]);
    }

    #[test]
    fn desktop_mode_keeps_its_system_row_on_the_root_sheet() {
        let all = model_rows();
        let p = project(&all, &categories(Mode::Desktop, Tools::default()), None, &|_| false);
        assert_eq!(headings(&p).last().map(String::as_str), Some("SYSTEM"));
        assert_eq!(p.ids[p.sections[7] + 1..], [SId::OpenSystemSettings]);
        // Without the desktop's settings app the row is not built: no empty SYSTEM section.
        let no_app: Vec<_> = all.iter().filter(|(_, id, _)| *id != SId::OpenSystemSettings).cloned().collect();
        let p = project(&no_app, &categories(Mode::Desktop, Tools::default()), None, &|_| false);
        assert_eq!(headings(&p).last().map(String::as_str), Some("ADVANCED"));
    }

    #[test]
    fn a_sub_sheet_shows_only_its_area() {
        let all = model_rows();
        let cats = categories(Mode::Os, Tools { network: true, ..Tools::default() });
        let p = project(&all, &cats, Some(Cat::Network), &|_| false);
        assert_eq!(p.ids, [SId::WifiOn, SId::Header, SId::Network(0)]);
        // Its sections: the top, then each heading.
        assert_eq!(p.sections, [0, 1]);
        let p = project(&all, &cats, Some(Cat::Controllers), &|_| false);
        assert_eq!((p.ids.as_slice(), p.sections.as_slice()), (&[SId::Header, SId::Pad(0)][..], &[0][..]));
    }

    #[test]
    fn l1_and_r1_jump_to_the_first_row_of_a_section_and_wrap_around() {
        let all = model_rows();
        let cats = categories(Mode::Os, Tools { network: true, ..Tools::default() });
        let p = project(&all, &cats, None, &|_| false);
        let first = |s: usize| p.sections[s] + 1;
        assert_eq!(jump(&p.sections, &p.rows, first(0) as i32, 1, false).to, Some(first(1)));
        // From inside the Emulators, past their sub-headings: the next section.
        let shad = p.ids.iter().position(|id| *id == SId::ShadUpdate).unwrap() as i32;
        assert_eq!(jump(&p.sections, &p.rows, shad, 1, false).to, Some(first(4)));
        assert_eq!(jump(&p.sections, &p.rows, shad, -1, false).to, Some(first(2)), "back to Downloads, not the Emulators' top");
        assert_eq!(jump(&p.sections, &p.rows, first(7) as i32, 1, false).to, Some(first(0)), "SYSTEM wraps to GAMES");
        assert_eq!(jump(&p.sections, &p.rows, first(0) as i32, -1, false).to, Some(first(7)), "GAMES wraps to SYSTEM");
        // From the search field (no row): R1 goes to the first section, L1 to the last.
        assert_eq!(jump(&p.sections, &p.rows, -1, 1, false).to, Some(first(0)));
        assert_eq!(jump(&p.sections, &p.rows, -1, -1, false).to, Some(first(7)));
        assert_eq!(jump(&[], &[], 0, 1, false).to, None);
    }

    #[test]
    fn a_section_jump_drops_the_edit_of_the_row_it_leaves() {
        let all = model_rows();
        let p = project(&all, &categories(Mode::Session, Tools::default()), None, &|_| false);
        let rawg = p.ids.iter().position(|id| *id == SId::Rawg).unwrap();
        // Typing in the RAWG key, R1: the typing is dropped, as Escape drops it, and focus moves on.
        let j = jump(&p.sections, &p.rows, rawg as i32, 1, true);
        assert!(j.drop_edit);
        assert_eq!(j.to, Some(p.sections[7] + 1));
        assert!(!jump(&p.sections, &p.rows, rawg as i32, 1, false).drop_edit);
        // An edit is dropped even where there is nowhere to go.
        assert_eq!(jump(&[], &[], 0, 1, true), Jump { drop_edit: true, to: None });
    }

    #[test]
    fn a_search_hit_opens_where_its_row_lives() {
        assert_eq!(sub_sheet(SId::Resolution), None, "a launcher row: the root sheet");
        assert_eq!(sub_sheet(SId::OpenSystemSettings), None, "the desktop's row: the root sheet");
        assert_eq!(sub_sheet(SId::Network(2)), Some(Cat::Network));
        assert_eq!(sub_sheet(SId::Gpu), Some(Cat::Display));
        assert_eq!(sub_sheet(SId::OsDownload), Some(Cat::OsUpdates));
        assert_eq!(sub_sheet(SId::RestartLauncher), Some(Cat::About));
        assert_eq!(sub_sheet(SId::Header), None);
        // A hit keeps its category; entry rows are never hits.
        let all = model_rows();
        let hits = search(&all, "wi-fi");
        assert_eq!(hits.iter().map(|i| (all[*i].0, all[*i].1)).collect::<Vec<_>>(), [(Cat::Network, SId::WifiOn)]);
        assert!(all.iter().all(|(_, id, _)| !matches!(id, SId::Open(_))));
    }

    #[test]
    fn back_leaves_an_edit_then_a_flow_then_the_sub_sheet_then_settings() {
        let mut nav = SettingsNav::default();
        let from = Return { entry: SId::Open(Cat::Controllers), idx: 31, y: -840.0 };
        nav.open_sub(Cat::Controllers, from);
        assert_eq!(nav.sub, Some(Cat::Controllers));
        assert_eq!(nav.back(true, true), Back::Edit, "typing first");
        assert_eq!(nav.back(false, true), Back::Flow, "then pairing");
        assert_eq!(nav.sub, Some(Cat::Controllers), "both stay in the sub-sheet");
        assert_eq!(nav.back(false, false), Back::Root(Some(from)));
        assert_eq!(nav.sub, None);
        assert_eq!(nav.back(false, false), Back::Close);
        // At the root, an edit is still dropped before Settings closes.
        assert_eq!(nav.back(true, false), Back::Edit);
    }

    #[test]
    fn back_puts_focus_and_scroll_where_they_were_on_the_root_sheet() {
        let ids = [SId::Header, SId::Dirs, SId::Header, SId::Open(Cat::Network), SId::Open(Cat::Sound)];
        let ret = Return { entry: SId::Open(Cat::Sound), idx: 4, y: -900.0 };
        assert_eq!(restore(&ret, &ids), Some((4, Some(-900.0))), "the same row: the same scroll");
        // An area came or went above it while the sub-sheet was open: the row, scrolled to.
        let moved = [SId::Header, SId::Dirs, SId::Header, SId::Open(Cat::Sound)];
        assert_eq!(restore(&ret, &moved), Some((3, None)));
        // Opened from search: no row index to go back to.
        let from_search = Return { entry: SId::Open(Cat::Network), idx: usize::MAX, y: 0.0 };
        assert_eq!(restore(&from_search, &ids), Some((3, None)));
        assert_eq!(restore(&ret, &[SId::Header, SId::Dirs]), None, "its area is gone");
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
