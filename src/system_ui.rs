//! The System pages of Settings in Session and OS mode: Network, Controllers, Sound, Display,
//! Storage, Updates and Time (docs/plans/ps5-launcher-os.md, Phase 6), and the Quick Menu's volume
//! and controller batteries. The backends (network, bluetooth, battery, sound, gpu, nvidia, screen,
//! storage, osupdate, timezone) build the command lines and read the answers; this file runs them
//! off the UI thread and shows the result. A page shows only when its tool answered; Controllers
//! always shows, and gains its Bluetooth rows when bluetoothctl finds an adapter.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::battery::{self, Controller};
use crate::bluetooth::{self, PairEnd, Step};
use crate::network::{self, Join};
use crate::nvidia::{self, Image, Offer, SwitchEnd};
use crate::osupdate::{self, Task, UpdateEnd};
use crate::settings::{categories, installed, row, Cat, SId, Tools};
use crate::system::{self, Call, Mode, PowerAction};
use crate::{gpu, screen, sound, storage, timezone, util, SettingData};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct SystemUi {
    pub tools: Tools,
    pub net: Net,
    pub outputs: Vec<sound::Output>,
    pub drives: Vec<storage::Drive>,
    /// Free bytes of each mounted partition, by its device path.
    pub free: HashMap<String, u64>,
    pub os: Os,
    pub display: Disp,
    pub time: Time,
    pub ctl: Ctl,
    /// When each page was last read on opening.
    opened: Vec<(Cat, Instant)>,
    gen: Gens,
    /// The sound writes that run or wait.
    writes: sound::Writes,
    /// Read the outputs again once the sound writes are done.
    sound_reload: bool,
}

/// A page opened again within this time is not read again.
const REREAD: Duration = Duration::from_secs(20);
/// The Updates page asks the registry at most this often; "PS5 Launcher OS" checks at once.
const RECHECK: Duration = Duration::from_secs(10 * 60);

/// Which read of a domain (network, sound, storage, OS) is the latest. A read takes a ticket
/// when it starts; a newer read or a change the player makes moves the count on, and a read
/// whose ticket is old is dropped when it ends, so an older answer never overwrites a newer
/// state.
#[derive(Default)]
pub struct Gen(u64);

impl Gen {
    /// A read starts, or the state changes. Returns the read's ticket.
    fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }

    /// The read with `ticket` is the latest, and nothing changed since it started.
    fn current(&self, ticket: u64) -> bool {
        self.0 == ticket
    }
}

/// The generation of each domain.
#[derive(Default)]
pub struct Gens {
    net: Gen,
    sound: Gen,
    storage: Gen,
    os: Gen,
    display: Gen,
    time: Gen,
    /// The connected controllers and their batteries.
    pads: Gen,
    /// The adapter and the paired devices.
    bt: Gen,
    /// A scan or a pairing. Cancel moves it on, so a late answer does not reopen the pairing.
    pair: Gen,
}

/// Whether something done at `last` is due again at `now`.
fn due(last: Option<Instant>, now: Instant, wait: Duration) -> bool {
    last.is_none_or(|at| now.saturating_duration_since(at) >= wait)
}

#[derive(Default)]
pub struct Net {
    pub devices: Vec<network::Device>,
    pub radio: bool,
    pub wifi: Vec<network::Wifi>,
    pub saved: Vec<network::Saved>,
    pub scanning: bool,
    /// The ticket of the read that scans. It ends the scan, even when a newer read drops it.
    scan: Option<u64>,
    /// The network being joined.
    pub joining: Option<String>,
    /// The secured network whose password is being typed.
    pub asking: Option<String>,
}

#[derive(Default)]
pub struct Os {
    pub status: Option<osupdate::Status>,
    /// What update-check found on the NVIDIA image (a digest).
    pub found: Option<String>,
    pub checking: bool,
    /// The ticket of the read that checks for an update. It ends the check, even when a newer
    /// read drops it.
    check: Option<u64>,
    /// "Downloading…" or "Undoing…" while the helper works, and until the state is read again
    /// after it: a retry needs the real state, not the one before the task.
    pub busy: Option<&'static str>,
    /// The password of the queued Secure Boot key, for the blue MOK screen.
    pub key: Option<String>,
    /// When the Updates page last checked on opening.
    pub checked: Option<Instant>,
}

/// The Display page: the screen, its GPU and the NVIDIA driver's flow.
#[derive(Default)]
pub struct Disp {
    pub state: Option<DisplayState>,
    /// The helper works on the NVIDIA driver: "Downloading…", "Switching…", "Queuing the key…".
    pub busy: Option<&'static str>,
    /// The password of the key queued in this run, for the blue MOK screen. It is not kept: after
    /// a launcher restart the page offers a new one.
    pub key: Option<String>,
}

/// What the Display page reads.
#[derive(Clone)]
pub struct DisplayState {
    pub screen: gpu::Screen,
    /// The card's name from lspci, if it answered.
    pub name: Option<String>,
    pub nvidia_version: Option<String>,
    /// PS5 Launcher OS's image; None in a session on another Linux PC.
    pub image: Option<Image>,
}

/// The Time page.
#[derive(Default)]
pub struct Time {
    pub clock: Option<timezone::Clock>,
    /// The zones by region, for the picker.
    pub regions: Vec<(String, Vec<String>)>,
    pub picker: Option<Picker>,
    /// A change runs.
    pub setting: bool,
}

/// The Controllers page: the connected controllers, then Bluetooth.
#[derive(Default)]
pub struct Ctl {
    /// The connected controllers with their battery; None until the first read.
    pub pads: Option<Vec<Controller>>,
    /// When the controllers were last read.
    read_at: Option<Instant>,
    pub adapter: Option<bluetooth::Adapter>,
    pub paired: Vec<bluetooth::Info>,
    /// "Turning on…" or "Turning off…" while the switch changes.
    pub busy: Option<&'static str>,
    /// The MAC of the device being forgotten.
    pub forgetting: Option<String>,
    pub pairing: Option<Pairing>,
}

/// Pairing a new controller: where it stands, and what the last scan found.
#[derive(Clone, Default)]
pub struct Pairing {
    pub stage: Stage,
    pub found: Vec<bluetooth::Info>,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub enum Stage {
    /// The buttons to hold, before the scan.
    #[default]
    Ready,
    Scanning,
    /// The scan ended: `found` lists the controllers.
    Results,
    Running { mac: String, step: Step },
    /// `step` None: the scan failed.
    Failed { step: Option<Step>, what: &'static str, reason: &'static str, detail: String },
    Done { name: String },
}

/// The batteries are read again this often while the Quick Menu or the Controllers page shows.
const BATTERY_EVERY: Duration = Duration::from_secs(30);
/// At most this many devices from a scan are asked what they are.
const SCAN_INFOS: usize = 40;

/// The time zone picker: the regions, or the zones of one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Picker {
    Regions,
    Region(usize),
}

/// What the Network page reads: devices, the Wi-Fi switch, networks and saved networks.
struct NetState {
    devices: Vec<network::Device>,
    radio: bool,
    wifi: Vec<network::Wifi>,
    saved: Vec<network::Saved>,
}

fn load_net(rescan: bool) -> Result<NetState, String> {
    let devices = network::parse_devices(&system::call(&network::devices_call())?);
    let radio = network::has_wifi(&devices) && network::parse_radio(&system::call(&network::radio_call())?)?;
    let wifi = if !radio {
        Vec::new()
    } else {
        // NetworkManager refuses a scan right after another one: then the last list it has.
        let listed = system::call(&network::wifi_call(rescan)).or_else(|e| if rescan { system::call(&network::wifi_call(false)) } else { Err(e) });
        network::parse_wifi(&listed?)
    };
    let saved = network::parse_saved(&system::call(&network::saved_call())?);
    Ok(NetState { devices, radio, wifi, saved })
}

fn load_sound() -> Result<Vec<sound::Output>, String> {
    Ok(sound::parse_outputs(&system::call(&sound::status_call())?))
}

fn load_storage() -> Result<(Vec<storage::Drive>, HashMap<String, u64>), String> {
    let drives = storage::parse_drives(&system::call(&storage::lsblk_call())?)?;
    let mut free = HashMap::new();
    for p in drives.iter().flat_map(|d| &d.partitions) {
        if let Some(mp) = p.mountpoint.as_deref().filter(|mp| mp.starts_with('/')) {
            match crate::downloads::available_space(Path::new(mp)) {
                Ok(bytes) => {
                    free.insert(p.path.clone(), bytes);
                }
                Err(e) => crate::log!("free space of {mp}: {e}"),
            }
        }
    }
    Ok((drives, free))
}

/// The screen and its GPU. The name comes from lspci when it is there.
fn load_display() -> Result<DisplayState, String> {
    let screen = gpu::read_screen(Path::new(gpu::DRM_DIR)).ok_or("no connected screen in sysfs")?;
    let name = screen.card.slot.as_deref().and_then(|slot| system::call(&gpu::lspci_call(slot)).ok()).and_then(|out| gpu::parse_lspci(&out));
    let nvidia_version = gpu::nvidia_version(Path::new(gpu::NVIDIA_MODULE));
    Ok(DisplayState { screen, name, nvidia_version, image: system::os_image() })
}

/// The clock, and the zones by region.
fn load_time() -> Result<(timezone::Clock, Vec<(String, Vec<String>)>), String> {
    let clock = timezone::parse_show(&system::call(&timezone::show_call())?)?;
    let zones = timezone::parse_list(&system::call(&timezone::list_call())?);
    Ok((clock, timezone::regions(&zones)))
}

/// The connected controllers, each with its battery.
fn load_pads() -> Vec<Controller> {
    battery::controllers(crate::gamepad::pads(), Path::new(battery::SYS))
}

/// `info` on a device; what `devices` said when info has no answer.
fn device_info(device: bluetooth::Device, paired: bool) -> bluetooth::Info {
    let asked = bluetooth::info_call(&device.mac).and_then(|call| system::call(&call)).ok().and_then(|out| bluetooth::parse_info(&out));
    asked.unwrap_or(bluetooth::Info { mac: device.mac, name: device.name, paired, ..bluetooth::Info::default() })
}

/// The adapter and the paired devices. An error when there is no adapter (`list` prints none) or
/// no bluetoothd (the call runs out of time).
fn load_bt() -> Result<(bluetooth::Adapter, Vec<bluetooth::Info>), String> {
    if bluetooth::parse_list(&system::call(&bluetooth::list_call())?).is_empty() {
        return Err("no Bluetooth adapter".into());
    }
    let adapter = bluetooth::parse_show(&system::call(&bluetooth::show_call())?)?;
    let paired = bluetooth::parse_devices(&system::call(&bluetooth::paired_call())?);
    Ok((adapter, paired.into_iter().map(|d| device_info(d, true)).collect()))
}

/// Scan, then the controllers it found that are not paired yet.
fn scan_bt() -> Result<Vec<bluetooth::Info>, String> {
    let call = bluetooth::scan_call();
    let ran = system::call_status(&call)?;
    if let Some(error) = bluetooth::scan_error(&ran.stdout) {
        return Err(error);
    }
    if ran.code != Some(0) {
        return Err(ran.error(&call));
    }
    let devices = bluetooth::parse_devices(&system::call(&bluetooth::devices_call())?);
    Ok(bluetooth::found(devices.into_iter().take(SCAN_INFOS).map(|d| device_info(d, false)).collect()))
}

/// `bootc status`. An OS update needs bootc and the root helper.
pub fn load_os() -> Result<osupdate::Status, String> {
    if !Path::new(osupdate::HELPER).exists() {
        return Err(format!("{} is missing", osupdate::HELPER));
    }
    osupdate::parse_status(&system::call(&osupdate::status_call())?)
}

/// Run `work` off the UI thread, then `done` with its result on it.
fn bg<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(&mut App, T) + Send + 'static) {
    std::thread::spawn(move || {
        let out = work();
        let _ = slint::invoke_from_event_loop(move || with_app(move |app| done(app, out)));
    });
}

/// Where focus goes once a page's rows changed under it (`headers`: which rows are headers): the
/// same row when it is still a row, else the nearest row above it, else the first row. None: the
/// page has no rows.
fn settle(headers: &[bool], idx: usize) -> Option<usize> {
    let idx = idx.min(headers.len().checked_sub(1)?);
    (0..=idx).rev().find(|i| !headers[*i]).or_else(|| (idx..headers.len()).find(|i| !headers[*i]))
}

fn header(rows: &mut Vec<(Cat, SId, SettingData)>, cat: Cat, label: &str) {
    rows.push((cat, SId::Header, row(0, label)));
}

impl App {
    /// Find which tools work and read every page, when Settings opens. The rail gains the pages
    /// whose tool answered.
    pub fn sys_probe(&mut self) {
        let mode = Mode::current();
        if mode == Mode::Desktop {
            return;
        }
        let os = mode == Mode::Os;
        let g = &mut self.sys.gen;
        let tickets = (g.net.next(), g.sound.next(), g.storage.next(), g.os.next(), g.display.next(), g.time.next());
        bg(
            move || {
                let os_status = if os { load_os() } else { Err("not the OS".into()) };
                (load_net(false), load_sound(), load_storage(), os_status, load_display(), load_time())
            },
            move |app, (net, sound, storage, os, display, time)| {
                let (net_t, sound_t, storage_t, os_t, display_t, time_t) = tickets;
                let pages = [
                    ("network", net.as_ref().err()),
                    ("sound", sound.as_ref().err()),
                    ("storage", storage.as_ref().err()),
                    ("display", display.as_ref().err()),
                    ("time", time.as_ref().err()),
                ];
                for (page, e) in pages {
                    if let Some(e) = e {
                        crate::log!("System page {page} hidden: {e}");
                    }
                }
                app.sys.tools = Tools {
                    network: net.is_ok(),
                    // Without an output there is nothing to set.
                    sound: sound.as_ref().is_ok_and(|o| !o.is_empty()),
                    storage: storage.is_ok(),
                    os_updates: os.is_ok(),
                    display: display.is_ok(),
                    time: time.is_ok(),
                    // Read on its own: bluetoothctl can take its whole time limit.
                    bluetooth: app.sys.tools.bluetooth,
                };
                // A page read or a change since the probe started is newer: keep it.
                let g = &app.sys.gen;
                let sound_t = g.sound.current(sound_t) && app.sys.writes.idle();
                let current = (g.net.current(net_t), sound_t, g.storage.current(storage_t), g.os.current(os_t));
                if let (true, Ok(net)) = (current.0, net) {
                    app.sys_set_net(net);
                }
                if current.1 {
                    app.sys.outputs = sound.unwrap_or_default();
                }
                if let (true, Ok((drives, free))) = (current.2, storage) {
                    (app.sys.drives, app.sys.free) = (drives, free);
                }
                if current.3 {
                    app.sys.os.status = os.ok();
                }
                if let (true, Ok(display)) = (app.sys.gen.display.current(display_t), display) {
                    app.sys.display.state = Some(display);
                }
                if let (true, Ok((clock, regions))) = (app.sys.gen.time.current(time_t), time) {
                    (app.sys.time.clock, app.sys.time.regions) = (Some(clock), regions);
                }
                app.sys_show();
            },
        );
        self.pads_load();
        self.bt_load();
    }

    fn sys_set_net(&mut self, net: NetState) {
        let n = &mut self.sys.net;
        (n.devices, n.radio, n.wifi, n.saved) = (net.devices, net.radio, net.wifi, net.saved);
    }

    /// A System page opened: read it again. The Network page scans for networks; the Updates
    /// page checks for an update. Moving along the rail passes pages quickly, so a page read a
    /// moment ago is not read again.
    pub fn sys_page_opened(&mut self, cat: Cat) {
        let now = Instant::now();
        let last = self.sys.opened.iter().find(|(c, _)| *c == cat).map(|(_, at)| *at);
        if !due(last, now, REREAD) {
            return;
        }
        self.sys.opened.retain(|(c, _)| *c != cat);
        self.sys.opened.push((cat, now));
        match cat {
            Cat::Network => self.net_load(true),
            Cat::Controllers => {
                self.pads_load();
                self.bt_load();
            }
            Cat::Sound => self.sound_load(true),
            Cat::Storage => self.storage_load(),
            Cat::Display => self.display_load(),
            Cat::Time => self.time_load(),
            Cat::OsUpdates => {
                // No check while the helper works: the next opening checks instead.
                let check = due(self.sys.os.checked, now, RECHECK) && self.sys.os.busy.is_none();
                if check {
                    self.sys.os.checked = Some(now);
                }
                self.os_load(check);
            }
            _ => {}
        }
    }

    /// Show what changed: the rail's pages and the open page. A row being typed in keeps the page
    /// as it is until the typing ends.
    fn sys_show(&mut self) {
        if self.overlay != Overlay::Settings {
            return;
        }
        let cats = categories(Mode::current(), self.sys.tools);
        let nav = &mut self.settings_nav;
        if cats != nav.cats {
            let open = nav.cats.get(nav.cat).copied();
            let kept = open.and_then(|c| cats.iter().position(|x| *x == c));
            nav.cat = kept.unwrap_or(0);
            nav.cats = cats;
            if kept.is_none() || self.zone == Z_SETTINGS_RAIL {
                let cat = self.settings_nav.cat as i32;
                self.set_focus(Z_SETTINGS_RAIL, cat);
            }
        }
        if self.edit_index >= 0 {
            return self.push_settings();
        }
        self.build_settings();
        if self.zone == Z_SETTINGS {
            let headers: Vec<bool> = self.settings_rows.iter().map(|r| r.kind == 0).collect();
            match settle(&headers, self.idx.max(0) as usize) {
                Some(i) if i as i32 != self.idx => self.set_focus(Z_SETTINGS, i as i32),
                Some(_) => {}
                None => self.set_focus(Z_SETTINGS_RAIL, self.settings_nav.cat as i32),
            }
        }
        self.push_settings();
    }

    fn sys_error(&mut self, title: &str, error: &str) {
        crate::log!("{title}: {error}");
        audio::play(Sound::Error);
        self.toast(title, error, 2);
    }

    /// Run `call`; a failure shows as a toast titled `fail`. `done` gets whether it worked.
    fn sys_run(&mut self, call: Call, fail: &'static str, done: impl FnOnce(&mut App, bool) + Send + 'static) {
        bg(move || system::call(&call), move |app, res| {
            if let Err(e) = &res {
                app.sys_error(fail, e);
            }
            done(app, res.is_ok());
        });
    }

    /// The rows of the System pages in `cats`.
    pub fn system_rows(&self, cats: &[Cat]) -> Vec<(Cat, SId, SettingData)> {
        let mut rows = Vec::new();
        if cats.contains(&Cat::Network) {
            self.network_rows(&mut rows);
        }
        if cats.contains(&Cat::Controllers) {
            self.controllers_rows(&mut rows);
        }
        if cats.contains(&Cat::Sound) {
            self.sound_rows(&mut rows);
        }
        if cats.contains(&Cat::Display) {
            self.display_rows(&mut rows);
        }
        if cats.contains(&Cat::Storage) {
            self.storage_rows(&mut rows);
        }
        if cats.contains(&Cat::OsUpdates) {
            self.os_rows(&mut rows);
        }
        if cats.contains(&Cat::Time) {
            self.time_rows(&mut rows);
        }
        rows
    }

    /// A row of a System page was chosen.
    pub fn sys_activate(&mut self, id: SId) {
        match id {
            SId::Network(i) => self.net_join(i),
            SId::WifiScan => {
                audio::play(Sound::Select);
                self.net_load(true);
            }
            SId::SavedNetwork(i) => self.net_forget(i),
            SId::Output(i) => self.sound_set_default(i),
            SId::Partition(d, p) => self.use_for_games(d, p),
            SId::OsStatus => self.os_status_chosen(),
            SId::OsDownload => self.os_update(),
            SId::OsRollback => self.os_rollback(),
            SId::NvInstall => self.nv_install(),
            SId::NvLater => self.nv_later(),
            SId::NvRetry => self.nv_queue_key(),
            SId::NvRestart => self.nv_restart(),
            SId::NvOpenSource => self.nv_switch(Image::Main),
            SId::TimeZone => self.tz_open(),
            SId::TzBack => self.tz_pick(Picker::Regions),
            SId::TzRegion(i) => self.tz_pick(Picker::Region(i)),
            SId::TzZone(j) => self.tz_set(j),
            SId::BtDevice(i) => self.bt_forget(i),
            SId::BtPair => self.pair_open(),
            SId::BtScan => self.pair_scan(),
            SId::BtFound(i) => self.pair_start(i),
            SId::BtPairClose => self.pair_close(),
            _ => {}
        }
    }

    /// Left/Right on a System row, or a switch flipped (`dir` > 0: on).
    pub fn sys_change(&mut self, id: SId, dir: i32) {
        match id {
            SId::WifiOn => self.net_radio(dir > 0),
            SId::Mute => self.sound_mute(dir > 0),
            SId::Volume => self.sound_step(dir),
            SId::OutResolution => self.out_step(dir, false),
            SId::OutRefresh => self.out_step(dir, true),
            SId::Ntp => self.time_ntp(dir > 0),
            SId::BtPower => self.bt_power(dir > 0),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ Network

    fn network_rows(&self, rows: &mut Vec<(Cat, SId, SettingData)>) {
        let net = &self.sys.net;
        let start = rows.len();
        if let Some(status) = network::wired(&net.devices) {
            let mut r = row(4, "Wired");
            r.value_kind = if status == "Connected" { 1 } else { 0 };
            r.value = status.into();
            let connection = net.devices.iter().find(|d| d.kind == "ethernet" && d.state == "connected").and_then(|d| d.connection.clone());
            r.hint = connection.unwrap_or_default().into();
            rows.push((Cat::Network, SId::Wired, r));
        }
        if network::has_wifi(&net.devices) {
            let mut r = row(2, "Wi-Fi");
            r.on = net.radio;
            rows.push((Cat::Network, SId::WifiOn, r));
        }
        if net.radio {
            rows.push((Cat::Network, SId::Header, row(0, "NETWORKS")));
            for (i, w) in net.wifi.iter().enumerate() {
                let mut r = row(4, &w.ssid);
                r.bars = network::bars(w.signal);
                r.lock = network::secured(w);
                if net.joining.as_deref() == Some(w.ssid.as_str()) {
                    r.value = "Connecting…".into();
                } else if w.in_use {
                    (r.value, r.value_kind) = ("Connected".into(), 1);
                }
                rows.push((Cat::Network, SId::Network(i), r));
                if net.asking.as_deref() == Some(w.ssid.as_str()) {
                    let mut r = row(1, &format!("Password for {}", w.ssid));
                    r.secret = true;
                    r.hint = "Enter or Done joins the network".into();
                    rows.push((Cat::Network, SId::WifiPassword, r));
                }
            }
            let mut r = row(4, "Scan for networks");
            if net.scanning {
                r.value = "Scanning…".into();
            } else if net.wifi.is_empty() {
                r.hint = "No networks found".into();
            }
            rows.push((Cat::Network, SId::WifiScan, r));
        }
        let saved: Vec<(usize, &network::Saved)> = net.saved.iter().enumerate().collect();
        if !saved.is_empty() {
            rows.push((Cat::Network, SId::Header, row(0, "SAVED NETWORKS")));
            for (i, s) in saved {
                let mut r = row(4, &s.name);
                (r.value, r.value_kind) = ("Forget".into(), 3);
                rows.push((Cat::Network, SId::SavedNetwork(i), r));
            }
        }
        if rows.len() == start {
            let mut r = row(4, "No network devices");
            r.hint = "NetworkManager finds no Ethernet port and no Wi-Fi".into();
            rows.push((Cat::Network, SId::Wired, r));
        }
    }

    /// Read the Network page again. An answer older than a change or a newer read is dropped.
    pub fn net_load(&mut self, rescan: bool) {
        let ticket = self.sys.gen.net.next();
        if rescan {
            self.sys.net.scanning = true;
            self.sys.net.scan = Some(ticket);
            self.sys_show();
        }
        bg(move || load_net(rescan), move |app, res| {
            let net = &mut app.sys.net;
            if net.scan == Some(ticket) {
                (net.scanning, net.scan) = (false, None);
            }
            if app.sys.gen.net.current(ticket) {
                match res {
                    Ok(net) => app.sys_set_net(net),
                    Err(e) => app.sys_error("Couldn't read the network", &e),
                }
            }
            app.sys_show();
        });
    }

    fn net_radio(&mut self, on: bool) {
        if self.sys.net.radio == on {
            return;
        }
        audio::play(Sound::Move);
        self.sys.gen.net.next();
        self.sys.net.radio = on;
        self.sys_show();
        self.sys_run(network::set_radio_call(on), "Couldn't switch Wi-Fi", |app, _| app.net_load(false));
    }

    fn net_join(&mut self, i: usize) {
        let Some(wifi) = self.sys.net.wifi.get(i).cloned() else { return };
        if self.sys.net.joining.is_some() {
            return;
        }
        audio::play(Sound::Select);
        match network::join(&wifi, &self.sys.net.saved) {
            Join::Connected => self.toast(&format!("Connected to {}", wifi.ssid), "", 1),
            Join::Saved(uuid) => self.net_connect(wifi.ssid, network::up_call(&uuid)),
            Join::Open => {
                let call = network::connect_call(&wifi.ssid, None);
                self.net_connect(wifi.ssid, call);
            }
            Join::AskPassword => {
                self.sys.net.asking = Some(wifi.ssid);
                self.build_settings();
                if let Some(at) = self.settings_ids.iter().position(|id| *id == SId::WifiPassword) {
                    self.set_focus(Z_SETTINGS, at as i32);
                    self.settings_activate(at);
                }
                self.push_settings();
            }
        }
    }

    /// The password row's text, when it is confirmed.
    pub fn net_password(&mut self, password: String) {
        let Some(ssid) = self.sys.net.asking.take() else { return };
        self.net_focus(&ssid);
        if password.is_empty() {
            return;
        }
        let call = network::connect_call(&ssid, Some(&password));
        self.net_connect(ssid, call);
    }

    /// The password row was left without confirming it.
    pub fn net_password_cancelled(&mut self) {
        if let Some(ssid) = self.sys.net.asking.take() {
            self.net_focus(&ssid);
        }
    }

    /// Focus goes back to the network's row once the password row is gone.
    fn net_focus(&mut self, ssid: &str) {
        let Some(i) = self.sys.net.wifi.iter().position(|w| w.ssid == ssid) else { return };
        self.build_settings();
        if let Some(at) = self.settings_ids.iter().position(|id| *id == SId::Network(i)) {
            self.set_focus(Z_SETTINGS, at as i32);
        }
    }

    fn net_connect(&mut self, ssid: String, call: Call) {
        self.sys.gen.net.next();
        self.sys.net.joining = Some(ssid.clone());
        self.sys_show();
        bg(move || system::call(&call), move |app, res| {
            app.sys.net.joining = None;
            match res {
                Ok(_) => app.toast(&format!("Connected to {ssid}"), "", 1),
                Err(e) => app.sys_error(&format!("Couldn't join {ssid}"), &e),
            }
            app.net_load(false);
        });
    }

    fn net_forget(&mut self, i: usize) {
        let Some(saved) = self.sys.net.saved.get(i).cloned() else { return };
        audio::play(Sound::Select);
        self.sys.gen.net.next();
        self.sys_run(network::forget_call(&saved.uuid), "Couldn't forget the network", move |app, ok| {
            if ok {
                app.toast(&format!("Forgot {}", saved.name), "Joining it again asks for its password.", 1);
            }
            app.net_load(false);
        });
    }

    // ------------------------------------------------------------------ Controllers

    /// The connected controllers: the last read, or, before the first one, the list without
    /// batteries.
    pub fn controllers(&self) -> Vec<Controller> {
        match &self.sys.ctl.pads {
            Some(pads) => pads.clone(),
            None => crate::gamepad::pads().into_iter().map(|pad| Controller { pad, battery: None }).collect(),
        }
    }

    /// Read the controllers and their batteries again, off the UI thread: reading a battery can
    /// ask the controller.
    pub fn pads_load(&mut self) {
        let ticket = self.sys.gen.pads.next();
        self.sys.ctl.read_at = Some(Instant::now());
        bg(load_pads, move |app, pads| {
            if !app.sys.gen.pads.current(ticket) {
                return;
            }
            app.sys.ctl.pads = Some(pads);
            app.sys_show();
            if app.quick_open() {
                app.push_quick();
            }
        });
    }

    /// Each clock tick: read the controllers again when one came or went, and the batteries now
    /// and then while the Quick Menu or the Controllers page shows them.
    pub fn pads_tick(&mut self, count_changed: bool) {
        let page = self.overlay == Overlay::Settings && self.settings_nav.cats.get(self.settings_nav.cat) == Some(&Cat::Controllers);
        let shown = self.overlay == Overlay::Quick || page;
        if count_changed || (shown && due(self.sys.ctl.read_at, Instant::now(), BATTERY_EVERY)) {
            self.pads_load();
        }
    }

    fn controllers_rows(&self, rows: &mut Vec<(Cat, SId, SettingData)>) {
        let ctl = &self.sys.ctl;
        if let Some(pairing) = &ctl.pairing {
            return Self::pairing_rows(pairing, rows);
        }
        header(rows, Cat::Controllers, "CONNECTED");
        let pads = self.controllers();
        if pads.is_empty() {
            let mut r = row(4, "No controller connected");
            r.hint = if self.sys.tools.bluetooth { "Connect one with a USB cable, or pair it over Bluetooth below" } else { "Connect one with a USB cable" }.into();
            rows.push((Cat::Controllers, SId::Pad(0), r));
        }
        for (i, c) in pads.iter().enumerate() {
            let mut r = row(4, &c.pad.name);
            if let Some(b) = &c.battery {
                r.value = battery::text(b).into();
                r.value_kind = if b.charging { 1 } else { 0 };
            }
            let bus = match c.pad.bus {
                Some(crate::gamepad::Bus::Bluetooth) => " · Bluetooth",
                Some(crate::gamepad::Bus::Usb) => " · USB",
                None => "",
            };
            r.hint = format!("Player {}{bus}", i + 1).into();
            rows.push((Cat::Controllers, SId::Pad(i), r));
        }
        let Some(adapter) = ctl.adapter.as_ref().filter(|_| self.sys.tools.bluetooth) else { return };
        header(rows, Cat::Controllers, "BLUETOOTH");
        let mut r = row(2, "Bluetooth");
        r.on = adapter.powered;
        r.hint = match ctl.busy {
            Some(busy) => busy.into(),
            None => format!("This PC shows up as {}", adapter.name).into(),
        };
        rows.push((Cat::Controllers, SId::BtPower, r));
        if adapter.powered {
            let mut r = row(4, "Pair a new controller");
            r.hint = "DualSense, DualShock 4, Xbox and Switch Pro controllers".into();
            rows.push((Cat::Controllers, SId::BtPair, r));
        }
        if !ctl.paired.is_empty() {
            header(rows, Cat::Controllers, "PAIRED DEVICES");
            for (i, d) in ctl.paired.iter().enumerate() {
                let mut r = row(4, &d.name);
                (r.value, r.value_kind) = if ctl.forgetting.as_deref() == Some(d.mac.as_str()) { ("Forgetting…".into(), 0) } else { ("Forget".into(), 3) };
                r.hint = match (d.connected, d.battery) {
                    (true, Some(level)) => format!("Connected · battery {level}%"),
                    (true, None) => "Connected".into(),
                    (false, _) => "Not connected".into(),
                }
                .into();
                rows.push((Cat::Controllers, SId::BtDevice(i), r));
            }
        }
    }

    fn pairing_rows(pairing: &Pairing, rows: &mut Vec<(Cat, SId, SettingData)>) {
        let push = |rows: &mut Vec<(Cat, SId, SettingData)>, id: SId, label: &str, hint: &str| {
            let mut r = row(4, label);
            r.hint = hint.into();
            rows.push((Cat::Controllers, id, r));
        };
        let cancel = |rows: &mut Vec<(Cat, SId, SettingData)>| push(rows, SId::BtPairClose, "Cancel", "");
        match &pairing.stage {
            Stage::Ready => {
                push(rows, SId::BtScan, "Start scanning", &format!("Hold the buttons first. The scan takes {} seconds.", bluetooth::SCAN_SECS));
                cancel(rows);
            }
            Stage::Scanning => {
                push(rows, SId::BtScan, "Scanning…", "Keep the light flashing");
                cancel(rows);
            }
            Stage::Done { .. } => push(rows, SId::BtPairClose, "Done", ""),
            Stage::Results | Stage::Running { .. } | Stage::Failed { .. } => {
                let running = match &pairing.stage {
                    Stage::Running { mac, step } => Some((mac.as_str(), *step)),
                    _ => None,
                };
                if !pairing.found.is_empty() {
                    header(rows, Cat::Controllers, "CONTROLLERS FOUND");
                    for (i, info) in pairing.found.iter().enumerate() {
                        let mut r = row(4, &info.name);
                        (r.value, r.value_kind) = match running {
                            Some((mac, step)) if mac == info.mac => (step.doing().into(), 0),
                            _ => ("Pair".into(), 3),
                        };
                        r.hint = info.mac.clone().into();
                        rows.push((Cat::Controllers, SId::BtFound(i), r));
                    }
                    header(rows, Cat::Controllers, "");
                }
                let hint = if pairing.found.is_empty() { "No controllers found. Hold the buttons until the light flashes, then scan again." } else { "" };
                push(rows, SId::BtScan, "Scan again", hint);
                cancel(rows);
            }
        }
    }

    /// The pairing card over the Controllers page's rows.
    pub fn pair_card(&self) -> crate::PairCard {
        let Some(pairing) = &self.sys.ctl.pairing else { return crate::PairCard::default() };
        let lines = |l: &[&str]| model(l.iter().map(|s| (*s).into()).collect());
        let card = |step: i32, kind: i32, title: &str, text: &[&str], pictures: bool| crate::PairCard { show: true, step, kind, title: title.into(), lines: lines(text), pictures };
        let name = |mac: &str| pairing.found.iter().find(|i| i.mac == mac).map(|i| i.name.clone()).unwrap_or_else(|| mac.to_string());
        match &pairing.stage {
            Stage::Ready => card(0, 0, "Put the controller in pairing mode", &["Hold its buttons until the light flashes, then choose Start scanning.", "Pair one controller at a time."], true),
            Stage::Scanning => card(1, 0, "Looking for controllers…", &[&format!("Keep the light flashing. The scan takes up to {} seconds.", bluetooth::SCAN_SECS)], true),
            Stage::Results if pairing.found.is_empty() => card(1, 0, "No controllers found", &["Hold the buttons until the light flashes, then choose Scan again."], true),
            Stage::Results => card(2, 0, "Choose your controller", &["It stays in pairing mode for a short while only. Choose it now."], false),
            Stage::Running { mac, step } => {
                let at = if *step == Step::Connect { 3 } else { 2 };
                card(at, 0, &format!("{} {}", step.doing().trim_end_matches('…'), name(mac)), &["Keep the controller close to the PC."], false)
            }
            Stage::Failed { step, what, reason, detail } => {
                let at = match step {
                    None => 1,
                    Some(Step::Connect) => 3,
                    Some(_) => 2,
                };
                // A failed scan shows the buttons again; after a failed step the found list matters more.
                let lines: Vec<&str> = [*reason, detail.as_str()].into_iter().filter(|l| !l.is_empty()).collect();
                card(at, 1, what, &lines, step.is_none())
            }
            Stage::Done { name } => card(4, 2, "Controller connected", &[&format!("{name} is ready to play."), "Next time, press its PS, Xbox or Home button and it connects by itself."], false),
        }
    }

    /// Read the adapter and the paired devices again. Without bluetoothctl or an adapter the
    /// page keeps only the connected controllers.
    fn bt_load(&mut self) {
        let ticket = self.sys.gen.bt.next();
        if !installed("bluetoothctl") {
            self.sys.tools.bluetooth = false;
            return;
        }
        bg(load_bt, move |app, res| {
            if !app.sys.gen.bt.current(ticket) {
                return;
            }
            match res {
                Ok((adapter, paired)) => {
                    (app.sys.ctl.adapter, app.sys.ctl.paired) = (Some(adapter), paired);
                    app.sys.tools.bluetooth = true;
                }
                Err(e) => {
                    crate::log!("Bluetooth rows hidden: {e}");
                    (app.sys.ctl.adapter, app.sys.ctl.paired) = (None, Vec::new());
                    app.sys.tools.bluetooth = false;
                    app.sys.ctl.pairing = None;
                }
            }
            app.sys_show();
        });
    }

    /// A bluetoothctl error as a toast: the plain sentence, with the tool's words in the log.
    fn bt_error(&mut self, title: &str, error: &str) {
        crate::log!("{title}: {error}");
        audio::play(Sound::Error);
        self.toast(title, bluetooth::explain(error), 2);
    }

    fn bt_power(&mut self, on: bool) {
        let Some(adapter) = self.sys.ctl.adapter.as_mut() else { return };
        if adapter.powered == on || self.sys.ctl.busy.is_some() {
            return;
        }
        adapter.powered = on;
        audio::play(Sound::Move);
        self.sys.gen.bt.next();
        self.sys.ctl.busy = Some(if on { "Turning on…" } else { "Turning off…" });
        if !on {
            self.sys.ctl.pairing = None;
        }
        self.sys_show();
        let call = bluetooth::power_call(on);
        bg(move || system::call(&call), |app, res| {
            app.sys.ctl.busy = None;
            if let Err(e) = res {
                app.bt_error("Couldn't switch Bluetooth", &e);
            }
            app.bt_load();
        });
    }

    fn bt_forget(&mut self, i: usize) {
        let Some(device) = self.sys.ctl.paired.get(i).cloned() else { return };
        if self.sys.ctl.forgetting.is_some() {
            return;
        }
        let call = match bluetooth::remove_call(&device.mac) {
            Ok(call) => call,
            Err(e) => return self.bt_error("Couldn't forget the device", &e),
        };
        audio::play(Sound::Select);
        self.sys.gen.bt.next();
        self.sys.ctl.forgetting = Some(device.mac.clone());
        self.sys_show();
        bg(move || system::call(&call), move |app, res| {
            app.sys.ctl.forgetting = None;
            match res {
                Ok(_) => app.toast(&format!("Forgot {}", device.name), "Pair it again to use it over Bluetooth.", 1),
                Err(e) => app.bt_error(&format!("Couldn't forget {}", device.name), &e),
            }
            app.bt_load();
            app.pads_load();
        });
    }

    /// "Pair a new controller": the buttons to hold, and Start scanning.
    fn pair_open(&mut self) {
        audio::play(Sound::Select);
        self.sys.ctl.pairing = Some(Pairing::default());
        self.ui().set_settings_y(0.0);
        self.sys_focus(SId::BtScan);
    }

    /// Scan for controllers. Nothing starts while a scan or a pairing runs.
    fn pair_scan(&mut self) {
        let Some(pairing) = self.sys.ctl.pairing.as_mut() else { return };
        if matches!(pairing.stage, Stage::Scanning | Stage::Running { .. }) {
            return;
        }
        audio::play(Sound::Select);
        pairing.stage = Stage::Scanning;
        let ticket = self.sys.gen.pair.next();
        self.sys_focus(SId::BtScan);
        bg(scan_bt, move |app, res| {
            if !app.sys.gen.pair.current(ticket) {
                return;
            }
            let Some(pairing) = app.sys.ctl.pairing.as_mut() else { return };
            match res {
                Ok(found) => {
                    pairing.stage = Stage::Results;
                    pairing.found = found;
                }
                Err(e) => {
                    crate::log!("Bluetooth scan: {e}");
                    audio::play(Sound::Error);
                    pairing.stage = Stage::Failed { step: None, what: "The scan failed", reason: bluetooth::explain(&e), detail: bluetooth::detail(&e) };
                }
            }
            let focus = if pairing.found.is_empty() { SId::BtScan } else { SId::BtFound(0) };
            app.pair_focus(focus);
        });
    }

    /// Pair, trust and connect the controller the scan found at `i`, a step at a time.
    fn pair_start(&mut self, i: usize) {
        let Some(pairing) = self.sys.ctl.pairing.as_mut() else { return };
        let Some(info) = pairing.found.get(i).cloned() else { return };
        if matches!(pairing.stage, Stage::Scanning | Stage::Running { .. }) {
            return;
        }
        audio::play(Sound::Select);
        pairing.stage = Stage::Running { mac: info.mac.clone(), step: Step::Pair };
        let ticket = self.sys.gen.pair.next();
        self.sys_show();
        let mac = info.mac.clone();
        bg(
            move || {
                let steps_mac = mac.clone();
                bluetooth::pair_flow(&system::call_status, &mac, &move |step| {
                    let mac = steps_mac.clone();
                    let _ = slint::invoke_from_event_loop(move || with_app(move |app| app.pair_step(ticket, mac, step)));
                })
            },
            move |app, end| app.pair_done(ticket, info, end),
        );
    }

    /// A step of pairing started.
    fn pair_step(&mut self, ticket: u64, mac: String, step: Step) {
        let Some(pairing) = self.sys.ctl.pairing.as_mut().filter(|_| self.sys.gen.pair.current(ticket)) else { return };
        pairing.stage = Stage::Running { mac, step };
        self.sys_show();
    }

    fn pair_done(&mut self, ticket: u64, info: bluetooth::Info, end: PairEnd) {
        // Paired or not, the lists changed.
        self.bt_load();
        self.pads_load();
        if !self.sys.gen.pair.current(ticket) || self.sys.ctl.pairing.is_none() {
            return;
        }
        let gone = end == PairEnd::Gone;
        let stage = match end {
            PairEnd::Connected => {
                audio::play(Sound::Select);
                self.toast("Controller connected", &info.name, 1);
                Stage::Done { name: info.name.clone() }
            }
            PairEnd::Gone => {
                audio::play(Sound::Error);
                Stage::Failed {
                    step: Some(Step::Pair),
                    what: "The controller left pairing mode",
                    reason: "Hold its buttons again until the light flashes, then choose Scan again.",
                    detail: String::new(),
                }
            }
            PairEnd::Failed { step, error } => {
                crate::log!("Pairing {}: {error}", info.mac);
                audio::play(Sound::Error);
                Stage::Failed { step: Some(step), what: "Pairing failed", reason: bluetooth::explain(&error), detail: bluetooth::detail(&error) }
            }
        };
        // After a failed step, focus stays on that controller: choosing it again retries.
        let failed = self.sys.ctl.pairing.as_ref().and_then(|p| p.found.iter().position(|f| f.mac == info.mac)).filter(|_| !gone);
        let focus = match (&stage, failed) {
            (Stage::Done { .. }, _) => SId::BtPairClose,
            (_, Some(i)) => SId::BtFound(i),
            _ => SId::BtScan,
        };
        if let Some(pairing) = self.sys.ctl.pairing.as_mut() {
            if gone {
                // bluetoothd forgot it: choosing it again cannot work.
                pairing.found.retain(|f| f.mac != info.mac);
            }
            pairing.stage = stage;
        }
        self.pair_focus(focus);
    }

    /// Focus on `id` when the Controllers page has focus; otherwise only show the change.
    fn pair_focus(&mut self, id: SId) {
        let page = self.overlay == Overlay::Settings && self.settings_nav.cats.get(self.settings_nav.cat) == Some(&Cat::Controllers);
        if page && self.zone == Z_SETTINGS {
            self.sys_focus(id);
        } else {
            self.sys_show();
        }
    }

    /// Close the pairing. A pairing that runs goes on; its answer is dropped.
    fn pair_close(&mut self) {
        if self.sys.ctl.pairing.take().is_none() {
            return;
        }
        audio::play(Sound::Back);
        self.sys.gen.pair.next();
        self.ui().set_settings_y(0.0);
        let to = if self.sys.ctl.adapter.as_ref().is_some_and(|a| a.powered) { SId::BtPair } else { SId::BtPower };
        self.sys_focus(to);
    }

    /// Back on a System page: close what it has open. True when Back was used.
    pub fn sys_back(&mut self) -> bool {
        let page = self.settings_nav.cats.get(self.settings_nav.cat) == Some(&Cat::Controllers);
        if page && self.sys.ctl.pairing.is_some() {
            self.pair_close();
            return true;
        }
        false
    }

    // ------------------------------------------------------------------ Sound

    fn sound_rows(&self, rows: &mut Vec<(Cat, SId, SettingData)>) {
        let outputs = &self.sys.outputs;
        if let Some(out) = sound::default_output(outputs) {
            let mut r = row(3, "Volume");
            r.value = crate::quick_ui::sound_status(out).into();
            r.hint = out.name.clone().into();
            rows.push((Cat::Sound, SId::Volume, r));
            let mut r = row(2, "Mute");
            r.on = out.muted;
            rows.push((Cat::Sound, SId::Mute, r));
        }
        header(rows, Cat::Sound, "OUTPUT");
        for (i, out) in outputs.iter().enumerate() {
            let mut r = row(4, &out.name);
            if out.default {
                (r.value, r.value_kind) = ("In use".into(), 1);
            }
            rows.push((Cat::Sound, SId::Output(i), r));
        }
    }

    /// Read the outputs again. `loud`: a failure shows as a toast. While sound writes run or
    /// wait, the read waits for them: it would show the state before them.
    pub fn sound_load(&mut self, loud: bool) {
        if !self.sys.writes.idle() {
            self.sys.sound_reload = true;
            return;
        }
        let ticket = self.sys.gen.sound.next();
        bg(load_sound, move |app, res| {
            if !app.sys.gen.sound.current(ticket) || !app.sys.writes.idle() {
                return;
            }
            match res {
                Ok(outputs) => app.sys.outputs = outputs,
                Err(e) if loud => app.sys_error("Couldn't read the sound outputs", &e),
                Err(e) => crate::log!("wpctl: {e}"),
            }
            app.sys_show();
            if app.quick_open() {
                app.push_quick();
            }
        });
    }

    /// Queue a sound write; it starts when no other runs. A read that runs now is dropped.
    fn sound_write(&mut self, write: sound::Write) {
        self.sys.gen.sound.next();
        if let Some(next) = self.sys.writes.push(write) {
            self.sound_run(next);
        }
    }

    /// Run `write`, then the next one that waits. Mute and output changes read the outputs
    /// again when the writes are done; a volume change only when it failed.
    fn sound_run(&mut self, write: sound::Write) {
        let fail = match write {
            sound::Write::Volume(..) => "Couldn't change the volume",
            sound::Write::Mute(..) => "Couldn't mute the sound",
            sound::Write::Default(..) => "Couldn't change the output",
        };
        let call = write.call();
        bg(move || system::call(&call), move |app, res| {
            if let Err(e) = &res {
                app.sys_error(fail, e);
            }
            app.sys.sound_reload |= res.is_err() || !matches!(write, sound::Write::Volume(..));
            match app.sys.writes.done() {
                Some(next) => app.sound_run(next),
                None if std::mem::take(&mut app.sys.sound_reload) => app.sound_load(false),
                None => {}
            }
        });
    }

    /// One volume step on the output in use, from the Sound page or the Quick Menu.
    pub fn sound_step(&mut self, dir: i32) {
        let Some(out) = self.sys.outputs.iter_mut().find(|o| o.default) else { return };
        let volume = sound::next_volume(out.volume, dir);
        if volume == out.volume {
            return;
        }
        out.volume = volume;
        let id = out.id;
        audio::play(Sound::Move);
        self.sys_show();
        if self.quick_open() {
            self.push_quick();
        }
        self.sound_write(sound::Write::Volume(id, volume));
    }

    fn sound_mute(&mut self, on: bool) {
        let Some(out) = self.sys.outputs.iter_mut().find(|o| o.default) else { return };
        if out.muted == on {
            return;
        }
        out.muted = on;
        let id = out.id;
        audio::play(Sound::Move);
        self.sys_show();
        self.sound_write(sound::Write::Mute(id, on));
    }

    fn sound_set_default(&mut self, i: usize) {
        let Some(out) = self.sys.outputs.get(i) else { return };
        if out.default {
            return;
        }
        let id = out.id;
        audio::play(Sound::Select);
        for (j, o) in self.sys.outputs.iter_mut().enumerate() {
            o.default = j == i;
        }
        self.sys_show();
        self.sound_write(sound::Write::Default(id));
    }

    // ------------------------------------------------------------------ Storage

    /// Whether `dir` is one of the game folders.
    fn is_game_dir(&self, dir: &Path) -> bool {
        self.cfg.lock().unwrap().game_dirs.iter().any(|d| util::expand_home(d) == dir)
    }

    fn storage_rows(&self, rows: &mut Vec<(Cat, SId, SettingData)>) {
        for (d, drive) in self.sys.drives.iter().enumerate() {
            header(rows, Cat::Storage, &storage::drive_title(drive));
            let listed: Vec<(usize, &storage::Partition)> = drive.partitions.iter().enumerate().filter(|(_, p)| storage::listed(p)).collect();
            if listed.is_empty() {
                let mut r = row(4, "No file system");
                r.hint = "The PC can't read this drive yet".into();
                rows.push((Cat::Storage, SId::Drive(d), r));
            }
            for (p, part) in listed {
                let name = part.label.clone().unwrap_or_else(|| part.path.trim_start_matches("/dev/").to_string());
                let fstype = storage::fs_name(part.fstype.as_deref().unwrap_or_default());
                let mut r = row(4, &format!("{name} · {fstype}"));
                let size = storage::gigabytes(part.bytes);
                r.hint = match (&part.mountpoint, self.sys.free.get(&part.path)) {
                    (Some(mp), Some(free)) => format!("{} free of {size} · {mp}", storage::gigabytes(*free)),
                    (Some(mp), None) => format!("{size} · {mp}"),
                    (None, _) => format!("{size} · not mounted"),
                }
                .into();
                if storage::offers_games(drive, part, &|dir| self.is_game_dir(dir)) {
                    (r.value, r.value_kind) = ("Use for games".into(), 3);
                } else if part.mountpoint.as_deref().is_some_and(|mp| self.is_game_dir(&storage::games_dir(mp))) {
                    (r.value, r.value_kind) = ("Game folder".into(), 1);
                }
                rows.push((Cat::Storage, SId::Partition(d, p), r));
            }
        }
    }

    fn storage_load(&mut self) {
        let ticket = self.sys.gen.storage.next();
        bg(load_storage, move |app, res| {
            if !app.sys.gen.storage.current(ticket) {
                return;
            }
            match res {
                Ok((drives, free)) => (app.sys.drives, app.sys.free) = (drives, free),
                Err(e) => app.sys_error("Couldn't read the drives", &e),
            }
            app.sys_show();
        });
    }

    /// "Use for games": a Games folder on the partition joins the game folders.
    fn use_for_games(&mut self, d: usize, p: usize) {
        let Some(drive) = self.sys.drives.get(d) else { return };
        let Some(part) = drive.partitions.get(p) else { return };
        if !storage::offers_games(drive, part, &|dir| self.is_game_dir(dir)) {
            return;
        }
        let Some(mp) = part.mountpoint.clone() else { return };
        let name = part.label.clone().unwrap_or_else(|| drive.name.clone());
        audio::play(Sound::Select);
        let dir = storage::games_dir(&mp);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return self.sys_error("Couldn't make a Games folder", &format!("{}: {e}", dir.display()));
        }
        let dir = dir.to_string_lossy().into_owned();
        let added = dir.clone();
        self.save_cfg(move |c| {
            if !c.game_dirs.contains(&added) {
                c.game_dirs.push(added);
            }
        });
        self.rescan_library();
        self.toast(&format!("{name} is a game folder"), &format!("Games in {dir} show in your library."), 1);
        self.sys_show();
    }

    // ------------------------------------------------------------------ Updates (OS)

    fn os_rows(&self, rows: &mut Vec<(Cat, SId, SettingData)>) {
        let os = &self.sys.os;
        let Some(status) = &os.status else { return };
        let found = os.found.is_some() || status.available.is_some();
        let staged = status.staged.is_some();
        let mut r = row(4, "PS5 Launcher OS");
        r.value = osupdate::status_text(status, os.found.as_deref(), os.checking).into();
        r.value_kind = if staged || found { 3 } else if os.checking { 0 } else { 1 };
        let version = status.booted.version.clone().unwrap_or_else(|| osupdate::short_digest(&status.booted.digest));
        r.hint = match status.staged.as_ref().and_then(|s| s.version.clone()) {
            Some(next) => format!("Version {version} · {next} installs when the PC restarts"),
            None if staged => format!("Version {version} · the update installs when the PC restarts"),
            None => format!("Version {version}"),
        }
        .into();
        rows.push((Cat::OsUpdates, SId::OsStatus, r));
        if found && !staged {
            let mut r = row(4, "Download update");
            r.value = os.busy.filter(|b| *b == "Downloading…").unwrap_or_default().into();
            r.hint = if os.key.is_some() {
                "Enroll the key first (the steps are above), then download the update again"
            } else {
                "You can keep playing while it downloads. It installs when you restart."
            }
            .into();
            rows.push((Cat::OsUpdates, SId::OsDownload, r));
        }
        if let Some(previous) = &status.rollback {
            header(rows, Cat::OsUpdates, "ADVANCED");
            let mut r = row(4, "Undo the last system update");
            r.value = os.busy.filter(|b| *b == "Undoing…").unwrap_or_default().into();
            let version = previous.version.clone().map(|v| format!(" ({v})")).unwrap_or_default();
            r.hint = format!("Starts the previous system{version} at the next restart").into();
            rows.push((Cat::OsUpdates, SId::OsRollback, r));
        }
    }

    /// Read `bootc status` again; with `check`, ask the helper for an update first. No check
    /// starts while the helper downloads or undoes an update.
    pub fn os_load(&mut self, check: bool) {
        let check = check && self.sys.os.busy.is_none();
        self.os_read(check, false);
    }

    /// `after_task`: the read after a helper task, which ends `busy` whatever the task's end. An
    /// answer older than a change or a newer read is dropped.
    fn os_read(&mut self, check: bool, after_task: bool) {
        let ticket = self.sys.gen.os.next();
        if check {
            self.sys.os.checking = true;
            self.sys.os.check = Some(ticket);
            self.sys_show();
        }
        bg(
            move || {
                let found = check.then(|| system::call(&osupdate::helper_call(Task::UpdateCheck)).map(|out| osupdate::parse_check(&out)));
                (found, load_os())
            },
            move |app, (found, status)| {
                let os = &mut app.sys.os;
                if os.check == Some(ticket) {
                    (os.checking, os.check) = (false, None);
                }
                if after_task {
                    os.busy = None;
                }
                if app.sys.gen.os.current(ticket) {
                    match found {
                        Some(Ok(found)) => app.sys.os.found = found,
                        Some(Err(e)) => app.sys_error("Couldn't check for a system update", &e),
                        None => {}
                    }
                    match status {
                        Ok(status) => app.sys.os.status = Some(status),
                        Err(e) => app.sys_error("Couldn't read the system's state", &e),
                    }
                }
                app.sys_show();
            },
        );
    }

    fn os_status_chosen(&mut self) {
        let staged = self.sys.os.status.as_ref().is_some_and(|s| s.staged.is_some());
        if staged {
            // Its Restart row says "Update and restart".
            self.open_power_menu();
        } else if !self.sys.os.checking {
            audio::play(Sound::Select);
            self.os_load(true);
        }
    }

    /// A helper task may start: none runs, no check runs, and the state was read after the last
    /// one.
    fn os_idle(&self) -> bool {
        self.sys.os.busy.is_none() && !self.sys.os.checking
    }

    fn os_update(&mut self) {
        if !self.os_idle() {
            return;
        }
        audio::play(Sound::Select);
        self.sys.gen.os.next();
        // No toast yet: on the NVIDIA image the helper may stop at the key before downloading.
        self.sys.os.busy = Some("Downloading…");
        self.sys_show();
        bg(|| osupdate::update_flow(&system::call_status), |app, end| {
            // `busy` stays until the state is read again.
            match end {
                UpdateEnd::Staged => {
                    app.sys.os.key = None;
                    app.toast("System update downloaded", "It installs when you restart: choose Update and restart in the Power menu.", 1);
                }
                UpdateEnd::Password(password) => {
                    app.sys.os.key = Some(password);
                    app.toast("Enroll the driver's key", "The steps and the password are on the Updates page.", 0);
                    app.ui().set_settings_y(0.0);
                }
                UpdateEnd::KeyPending => {
                    app.toast("Restart and enroll the key first", "The blue screen asks for the key's password when the PC starts. Then download the update again.", 0);
                }
                UpdateEnd::Failed(e) => app.sys_error("Couldn't download the system update", &e),
            }
            app.os_read(false, true);
        });
    }

    fn os_rollback(&mut self) {
        if !self.os_idle() {
            return;
        }
        audio::play(Sound::Select);
        self.sys.gen.os.next();
        self.sys.os.busy = Some("Undoing…");
        self.sys_show();
        self.sys_run(osupdate::helper_call(Task::Rollback), "Couldn't undo the system update", |app, ok| {
            // `busy` stays until the state is read again.
            if ok {
                app.toast("The previous system starts at the next restart", "Restart from the Power menu.", 1);
            }
            app.os_read(false, true);
        });
    }

    /// The digits of the key's password, when the Updates page shows its steps.
    pub fn os_key_digits(&self) -> Vec<slint::SharedString> {
        self.sys.os.key.as_deref().unwrap_or_default().chars().map(|c| c.to_string().into()).collect()
    }

    // ------------------------------------------------------------------ Display

    /// The NVIDIA driver's offer for the screen's card.
    fn nv_offer(&self) -> Offer {
        let Some(d) = &self.sys.display.state else { return Offer::Nothing };
        nvidia::offer(d.image, Some(&d.screen.card), &self.cfg.lock().unwrap().nvidia)
    }

    /// The Display page has a step of the NVIDIA driver waiting.
    pub fn display_dot(&self) -> bool {
        nvidia::dot(self.nv_offer())
    }

    /// The card's name: lspci's, or the vendor and IDs.
    fn gpu_name(d: &DisplayState) -> String {
        d.name.clone().unwrap_or_else(|| gpu::fallback_name(&d.screen.card))
    }

    /// `helper switch nvidia` runs.
    fn nv_downloading(&self) -> bool {
        self.sys.display.busy == Some("Downloading…")
    }

    /// The NVIDIA driver's card over the Display page's rows.
    pub fn nv_card(&self) -> crate::NvCard {
        let Some(d) = &self.sys.display.state else { return crate::NvCard::default() };
        let offer = self.nv_offer();
        let downloading = self.nv_downloading();
        let Some((title, lines)) = nvidia::card_text(offer, &Self::gpu_name(d), downloading) else { return crate::NvCard::default() };
        let kind = match offer {
            Offer::Install if !downloading => 0,
            Offer::OldCard | Offer::UnknownCard | Offer::KeyMissed => 1,
            Offer::KeyEnrolled | Offer::Restart(_) if !downloading => 2,
            _ => 3,
        };
        crate::NvCard {
            show: true,
            step: nvidia::stepper(offer, downloading).map_or(-1, |s| s as i32),
            kind,
            title: title.into(),
            lines: model(lines.into_iter().map(Into::into).collect()),
        }
    }

    /// The digits of the key's password while the Display page waits for the restart.
    pub fn nv_key_digits(&self) -> Vec<slint::SharedString> {
        if self.nv_offer() != Offer::KeyWaiting {
            return Vec::new();
        }
        self.sys.display.key.as_deref().unwrap_or_default().chars().map(|c| c.to_string().into()).collect()
    }

    fn display_rows(&self, rows: &mut Vec<(Cat, SId, SettingData)>) {
        let Some(d) = &self.sys.display.state else { return };
        let busy = self.sys.display.busy;
        let mut r = row(4, &Self::gpu_name(d));
        r.value = d.screen.connector.clone().into();
        r.hint = format!("Drives the screen · {}", gpu::driver_text(&d.screen.card, d.nvidia_version.as_deref())).into();
        rows.push((Cat::Display, SId::Gpu, r));
        let action = |id: SId, label: &str, hint: &str| {
            let mut r = row(4, label);
            r.hint = hint.into();
            if let Some(b) = busy {
                r.value = b.into();
            }
            (Cat::Display, id, r)
        };
        let download = format!("About {} MB", nvidia::DOWNLOAD_MB);
        match self.nv_offer() {
            Offer::Install => {
                let mut install = action(SId::NvInstall, "Install driver", "");
                if busy.is_none() {
                    (install.2.value, install.2.value_kind) = (download.into(), 3);
                }
                rows.push(install);
                if busy.is_none() {
                    rows.push(action(SId::NvLater, "Later", "The offer stays here, on System → Display"));
                }
            }
            Offer::Later => {
                let mut r = action(SId::NvInstall, "NVIDIA driver", "The card runs on the open-source driver now. Install the NVIDIA driver to play at full speed.");
                if busy.is_none() {
                    (r.2.value, r.2.value_kind) = ("Install".into(), 3);
                }
                rows.push(r);
            }
            Offer::KeyWaiting if self.sys.display.key.is_some() => {
                rows.push(action(SId::NvRestart, "Restart now", "Keep the USB keyboard plugged in: the blue screen comes before the launcher"));
            }
            Offer::KeyWaiting => {
                rows.push(action(SId::NvRetry, "Show a new password", "The password shows only once. A new one replaces the old one."));
                rows.push(action(SId::NvRestart, "Restart now", "Only if you have the password: the blue screen asks for it"));
            }
            Offer::KeyMissed => rows.push(action(SId::NvRetry, "Try again", "Queues the key with a new password")),
            Offer::KeyEnrolled => {
                let mut r = action(SId::NvInstall, "Download the driver", "You can keep playing while it downloads");
                if busy.is_none() {
                    (r.2.value, r.2.value_kind) = (download.into(), 3);
                }
                rows.push(r);
            }
            Offer::Restart(_) => rows.push(action(SId::NvRestart, "Restart now", "")),
            Offer::UseOpenSource => rows.push(action(SId::NvOpenSource, "Use the open-source driver", "Switches the system back at the next restart. Games run slower on it.")),
            Offer::SwitchBack => rows.push(action(SId::NvOpenSource, "Switch to the open-source driver", "Starts the main system at the next restart")),
            Offer::Nothing | Offer::OldCard | Offer::UnknownCard => {}
        }

        header(rows, Cat::Display, "SCREEN OUTPUT");
        let chosen = self.cfg.lock().unwrap().session_output;
        let apply = "Applies at the next session start: restart the PC. Restart launcher is not enough, because the screen stays on.";
        let mut r = row(3, "Resolution");
        r.value = chosen.map_or("Automatic".to_string(), |o| format!("{} × {}", o.width, o.height)).into();
        r.hint = apply.into();
        rows.push((Cat::Display, SId::OutResolution, r));
        if let Some(out) = chosen {
            let mut r = row(3, "Refresh rate");
            r.value = out.refresh.map_or("Automatic".to_string(), |hz| format!("{hz} Hz")).into();
            if screen::edid_rates(&d.screen.edid, out.width, out.height).is_empty() {
                r.hint = "The screen lists no rates for this size: Automatic lets the PC pick".into();
            }
            rows.push((Cat::Display, SId::OutRefresh, r));
        }
    }

    /// Read the Display page again. An answer older than a change or a newer read is dropped.
    fn display_load(&mut self) {
        let ticket = self.sys.gen.display.next();
        bg(load_display, move |app, res| {
            if !app.sys.gen.display.current(ticket) {
                return;
            }
            match res {
                Ok(state) => app.sys.display.state = Some(state),
                Err(e) => app.sys_error("Couldn't read the screen", &e),
            }
            app.sys_show();
        });
    }

    /// Left/Right on Resolution (`rate` false) or Refresh rate: the next choice, saved for the
    /// next session start.
    fn out_step(&mut self, dir: i32, rate: bool) {
        let Some(d) = &self.sys.display.state else { return };
        let current = self.cfg.lock().unwrap().session_output;
        let choices: Vec<Option<screen::Output>> = if rate {
            let Some(out) = current else { return };
            let rates = screen::edid_rates(&d.screen.edid, out.width, out.height);
            std::iter::once(None).chain(rates.into_iter().map(Some)).map(|refresh| Some(screen::Output { refresh, ..out })).collect()
        } else {
            let modes = screen::parse_modes(&d.screen.modes);
            std::iter::once(None).chain(modes.into_iter().map(|(width, height)| Some(screen::Output { width, height, refresh: None }))).collect()
        };
        let pos = choices.iter().position(|c| match (c, current) {
            (Some(c), Some(cur)) if !rate => (c.width, c.height) == (cur.width, cur.height),
            (c, cur) => *c == cur,
        });
        let next = choices[(pos.unwrap_or(0) as i32 + dir).rem_euclid(choices.len() as i32) as usize];
        if next == current {
            return;
        }
        if let Err(e) = screen::save(&screen::session_conf_path(), next.as_ref()) {
            return self.sys_error("Couldn't save the screen output", &e.to_string());
        }
        audio::play(Sound::Move);
        self.save_cfg(move |c| c.session_output = next);
        self.sys_show();
    }

    /// The PC's boot ID, for the flow's restarts.
    fn boot() -> String {
        gpu::boot_id(Path::new(gpu::BOOT_ID))
    }

    /// At each start in PS5 Launcher OS: resume the NVIDIA driver's flow after a restart, and
    /// compare the screen's card with the image.
    pub fn nvidia_start(&mut self) {
        let Some(image) = system::os_image() else { return };
        let before = self.cfg.lock().unwrap().nvidia.clone();
        let mut flow = before.clone();
        let resumed = nvidia::resume(&mut flow, image, &Self::boot());
        // Most starts change nothing: then the config is not written.
        if flow != before {
            self.save_cfg(|c| c.nvidia = flow);
        }
        match resumed {
            nvidia::Resume::Done(Image::Nvidia) => self.toast("NVIDIA driver installed", "Games run at full speed. To go back: Settings → Display.", 1),
            nvidia::Resume::Done(Image::Main) => self.toast("Open-source driver in use", "The PC runs the main system again.", 1),
            nvidia::Resume::NotSwitched(_) => {
                self.toast("The driver did not change", "The PC started the previous system. Settings → Display shows what you can do.", 2)
            }
            nvidia::Resume::CheckKey => return self.nv_check_key(),
            nvidia::Resume::Nothing => {}
        }
        // A changed graphics card: tell the player once at the start; the page has the action.
        let ticket = self.sys.gen.display.next();
        bg(load_display, move |app, res| {
            let Ok(state) = res else { return };
            if app.sys.gen.display.current(ticket) {
                app.sys.display.state = Some(state);
            }
            match app.nv_offer() {
                Offer::Install => app.toast("NVIDIA card found", "Install the NVIDIA driver in Settings → Display to play at full speed.", 0),
                Offer::SwitchBack => app.toast("No NVIDIA card drives the screen", "Switch to the open-source driver in Settings → Display.", 0),
                _ => {}
            }
        });
    }

    /// After the restart: is the key enrolled now? Read-only.
    fn nv_check_key(&mut self) {
        self.sys.display.busy = Some("Checking the key…");
        bg(
            || system::call(&nvidia::key_state_call()).and_then(|out| nvidia::parse_key_state(&out)),
            |app, res| {
                app.sys.display.busy = None;
                match res {
                    Ok(state) => {
                        let mut flow = app.cfg.lock().unwrap().nvidia.clone();
                        nvidia::key_checked(&mut flow, state);
                        app.save_cfg(|c| c.nvidia = flow);
                        match state {
                            nvidia::KeyState::Enrolled | nvidia::KeyState::SecureBootOff => {
                                app.toast("Key enrolled", "Download the NVIDIA driver in Settings → Display.", 1)
                            }
                            _ => app.toast("The key was not enrolled", "Nothing was changed. Try again in Settings → Display.", 2),
                        }
                    }
                    // The step stays: the next start asks again.
                    Err(e) => app.sys_error("Couldn't check the driver's key", &e),
                }
                app.display_load();
            },
        );
    }

    /// Install driver: with Secure Boot on and the key not enrolled, the key comes first (the
    /// password, then a restart); otherwise the download. The helper's switch checks the key
    /// again by itself.
    fn nv_install(&mut self) {
        if self.sys.display.busy.is_some() {
            return;
        }
        audio::play(Sound::Select);
        self.sys.gen.display.next();
        self.sys.display.busy = Some("Checking Secure Boot…");
        self.sys_show();
        bg(
            || system::call(&nvidia::key_state_call()).and_then(|out| nvidia::parse_key_state(&out)),
            |app, res| {
                app.sys.display.busy = None;
                match res {
                    Ok(nvidia::KeyState::Enrolled | nvidia::KeyState::SecureBootOff) => app.nv_switch(Image::Nvidia),
                    Ok(nvidia::KeyState::Pending | nvidia::KeyState::Missing) => app.nv_queue_key(),
                    Err(e) => {
                        app.sys_error("Couldn't check Secure Boot", &e);
                        app.sys_show();
                    }
                }
            },
        );
    }

    /// Install driver (to the NVIDIA image), or the way back (to main).
    fn nv_switch(&mut self, to: Image) {
        if self.sys.display.busy.is_some() {
            return;
        }
        audio::play(Sound::Select);
        self.sys.gen.display.next();
        self.sys.display.busy = Some(if to == Image::Nvidia { "Downloading…" } else { "Switching…" });
        self.sys_show();
        bg(move || nvidia::switch_flow(&system::call_status, to), move |app, end| {
            app.sys.display.busy = None;
            let mut flow = app.cfg.lock().unwrap().nvidia.clone();
            nvidia::switched(&mut flow, to, &end, &Self::boot());
            app.save_cfg(|c| c.nvidia = flow);
            match end {
                SwitchEnd::Staged => {
                    app.sys.display.key = None;
                    app.toast("Ready: restart to finish", "Choose Restart now on Settings → Display.", 1);
                    // The Power menu's Restart follows the staged system.
                    app.check_staged();
                }
                SwitchEnd::Password(password) => {
                    app.sys.display.key = Some(password);
                    app.toast("Enroll the driver's key", "The steps and the password are on Settings → Display.", 0);
                    app.ui().set_settings_y(0.0);
                }
                SwitchEnd::Failed(e) => {
                    let what = if to == Image::Nvidia { "Couldn't install the NVIDIA driver" } else { "Couldn't switch to the open-source driver" };
                    app.sys_error(what, &e);
                }
            }
            app.display_load();
        });
    }

    /// The offer folds to one row; the dot stays.
    fn nv_later(&mut self) {
        audio::play(Sound::Back);
        self.save_cfg(|c| c.nvidia.later = true);
        self.sys_show();
    }

    /// Queue the key again: a new password (the old one was lost, or not enrolled).
    fn nv_queue_key(&mut self) {
        if self.sys.display.busy.is_some() {
            return;
        }
        audio::play(Sound::Select);
        self.sys.gen.display.next();
        self.sys.display.busy = Some("Queuing the key…");
        self.sys_show();
        let call = osupdate::helper_call(Task::QueueKey);
        bg(
            move || system::call(&call).and_then(|out| osupdate::parse_key(&out)),
            |app, res| {
                app.sys.display.busy = None;
                let boot = Self::boot();
                match res {
                    Ok(osupdate::Key::Password(p)) => {
                        app.sys.display.key = Some(p);
                        app.save_cfg(|c| c.nvidia.step = nvidia::Step::KeyQueued { boot });
                        app.ui().set_settings_y(0.0);
                    }
                    Ok(osupdate::Key::Enrolled) => app.save_cfg(|c| c.nvidia.step = nvidia::Step::KeyEnrolled),
                    Err(e) => app.sys_error("Couldn't queue the driver's key", &e),
                }
                app.display_load();
            },
        );
    }

    /// Restart now: the Power menu's restart, with its countdown and what a restart would lose.
    fn nv_restart(&mut self) {
        let staged = self.sys.os.status.as_ref().is_some_and(|s| s.staged.is_some()) || matches!(self.cfg.lock().unwrap().nvidia.step, nvidia::Step::Staged { .. });
        self.guard_power(PowerAction::Restart { update: staged });
    }

    // ------------------------------------------------------------------ Time

    fn time_rows(&self, rows: &mut Vec<(Cat, SId, SettingData)>) {
        let t = &self.sys.time;
        let Some(clock) = &t.clock else { return };
        let mut r = row(4, "Time zone");
        r.value = if t.setting { "Setting…".into() } else { clock.zone.clone().into() };
        r.hint = if t.picker.is_some() { "Choose a region, then a city".into() } else { "Choose to change it".into() };
        rows.push((Cat::Time, SId::TimeZone, r));
        if clock.can_ntp {
            let mut r = row(2, "Set the time automatically");
            r.on = clock.ntp;
            r.hint = "Uses time servers on the internet".into();
            rows.push((Cat::Time, SId::Ntp, r));
        }
        match t.picker {
            Some(Picker::Regions) => {
                header(rows, Cat::Time, "CHOOSE A REGION");
                for (i, (region, zones)) in t.regions.iter().enumerate() {
                    let mut r = row(4, region);
                    r.value = format!("{} zone{}", zones.len(), if zones.len() == 1 { "" } else { "s" }).into();
                    if clock.zone.starts_with(&format!("{region}/")) || (region == "Other" && zones.contains(&clock.zone)) {
                        (r.value, r.value_kind) = ("In use".into(), 1);
                    }
                    rows.push((Cat::Time, SId::TzRegion(i), r));
                }
            }
            Some(Picker::Region(i)) => {
                let Some((region, zones)) = t.regions.get(i) else { return };
                header(rows, Cat::Time, &region.to_uppercase());
                rows.push((Cat::Time, SId::TzBack, row(4, "‹ All regions")));
                for (j, zone) in zones.iter().enumerate() {
                    let mut r = row(4, &timezone::city(zone));
                    if *zone == clock.zone {
                        (r.value, r.value_kind) = ("In use".into(), 1);
                    }
                    rows.push((Cat::Time, SId::TzZone(j), r));
                }
            }
            None => {}
        }
    }

    /// Read the Time page again. A read while the picker is open keeps it.
    fn time_load(&mut self) {
        let ticket = self.sys.gen.time.next();
        bg(load_time, move |app, res| {
            if !app.sys.gen.time.current(ticket) {
                return;
            }
            match res {
                Ok((clock, regions)) => (app.sys.time.clock, app.sys.time.regions) = (Some(clock), regions),
                Err(e) => app.sys_error("Couldn't read the time zone", &e),
            }
            app.sys_show();
        });
    }

    /// Focus on the row `id` of the open page, once the rows changed.
    fn sys_focus(&mut self, id: SId) {
        self.build_settings();
        if let Some(at) = self.settings_ids.iter().position(|x| *x == id) {
            self.set_focus(Z_SETTINGS, at as i32);
        }
        self.push_settings();
        self.scroll_settings();
    }

    /// Time zone: open the regions, or close the picker.
    fn tz_open(&mut self) {
        if self.sys.time.setting || self.sys.time.regions.is_empty() {
            return;
        }
        audio::play(Sound::Select);
        if self.sys.time.picker.take().is_some() {
            return self.sys_focus(SId::TimeZone);
        }
        let zone = self.sys.time.clock.as_ref().map(|c| c.zone.clone()).unwrap_or_default();
        let current = self.sys.time.regions.iter().position(|(_, zones)| zones.contains(&zone)).unwrap_or(0);
        self.sys.time.picker = Some(Picker::Regions);
        self.sys_focus(SId::TzRegion(current));
    }

    /// Open a region's zones, or go back to the regions.
    fn tz_pick(&mut self, picker: Picker) {
        audio::play(Sound::Select);
        let from = self.sys.time.picker;
        self.sys.time.picker = Some(picker);
        let zone = self.sys.time.clock.as_ref().map(|c| c.zone.clone()).unwrap_or_default();
        let focus = match (picker, from) {
            (Picker::Region(i), _) => {
                let zones = self.sys.time.regions.get(i).map(|(_, z)| z.as_slice()).unwrap_or_default();
                SId::TzZone(zones.iter().position(|z| *z == zone).unwrap_or(0))
            }
            (Picker::Regions, Some(Picker::Region(i))) => SId::TzRegion(i),
            (Picker::Regions, _) => SId::TzRegion(0),
        };
        self.sys_focus(focus);
    }

    /// A zone was chosen: set it, then read the clock again.
    fn tz_set(&mut self, j: usize) {
        let Some(Picker::Region(i)) = self.sys.time.picker else { return };
        let Some(zone) = self.sys.time.regions.get(i).and_then(|(_, z)| z.get(j)).cloned() else { return };
        if self.sys.time.setting || !timezone::valid_zone(&zone) {
            return;
        }
        audio::play(Sound::Select);
        self.sys.gen.time.next();
        self.sys.time.picker = None;
        self.sys.time.setting = true;
        self.sys_focus(SId::TimeZone);
        let call = timezone::set_zone_call(Mode::current() == Mode::Os, &zone);
        self.sys_run(call, "Couldn't set the time zone", move |app, ok| {
            app.sys.time.setting = false;
            if ok {
                app.toast(&format!("Time zone: {}", timezone::city(&zone)), &zone, 1);
            }
            app.time_load();
        });
    }

    fn time_ntp(&mut self, on: bool) {
        let Some(clock) = self.sys.time.clock.as_mut() else { return };
        if clock.ntp == on || self.sys.time.setting {
            return;
        }
        clock.ntp = on;
        audio::play(Sound::Move);
        self.sys.gen.time.next();
        self.sys.time.setting = true;
        self.sys_show();
        let call = timezone::set_ntp_call(Mode::current() == Mode::Os, on);
        self.sys_run(call, "Couldn't change automatic time", |app, _| {
            app.sys.time.setting = false;
            app.time_load();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_read_again_only_after_a_while() {
        let now = Instant::now();
        assert!(due(None, now, REREAD), "never read");
        assert!(!due(Some(now), now + Duration::from_secs(5), REREAD));
        assert!(due(Some(now), now + REREAD, REREAD));
        assert!(!due(Some(now + Duration::from_secs(1)), now, REREAD), "a clock going back is not due");
    }

    #[test]
    fn a_read_older_than_a_change_or_a_newer_read_is_dropped() {
        let mut gen = Gen::default();
        let first = gen.next();
        assert!(gen.current(first));
        let second = gen.next();
        assert!(!gen.current(first), "a newer read started");
        assert!(gen.current(second));
        gen.next(); // the player changed something
        assert!(!gen.current(second), "the read started before the change");
        let third = gen.next();
        assert!(gen.current(third), "a read after the change counts");
    }

    #[test]
    fn focus_stays_on_a_row_when_the_rows_change() {
        // header, row, row, header, row
        let rows = [true, false, false, true, false];
        assert_eq!(settle(&rows, 2), Some(2), "still a row");
        assert_eq!(settle(&rows, 3), Some(2), "a header now: the row above");
        assert_eq!(settle(&rows, 9), Some(4), "past the end: the last row");
        assert_eq!(settle(&rows, 0), Some(1), "nothing above: the first row");
        assert_eq!(settle(&[true], 0), None);
        assert_eq!(settle(&[], 0), None);
    }
}
