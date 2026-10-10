//! The System pages of Settings in Session and OS mode: Network, Sound, Storage and Updates
//! (docs/plans/ps5-launcher-os.md, Phase 6), and the Quick Menu's volume. The backends (network,
//! sound, storage, osupdate) build the command lines and read the answers; this file runs them
//! off the UI thread and shows the result. A page shows only when its tool answered.

use crate::app::*;
use crate::audio::{self, Sound};
use crate::network::{self, Join};
use crate::osupdate::{self, Task, UpdateEnd};
use crate::settings::{categories, row, Cat, SId, Tools};
use crate::system::{self, Call, Mode};
use crate::{sound, storage, util, SettingData};
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
        let tickets = (g.net.next(), g.sound.next(), g.storage.next(), g.os.next());
        bg(
            move || (load_net(false), load_sound(), load_storage(), if os { load_os() } else { Err("not the OS".into()) }),
            move |app, (net, sound, storage, os)| {
                let (net_t, sound_t, storage_t, os_t) = tickets;
                for (page, e) in [("network", net.as_ref().err()), ("sound", sound.as_ref().err()), ("storage", storage.as_ref().err())] {
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
                app.sys_show();
            },
        );
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
            Cat::Sound => self.sound_load(true),
            Cat::Storage => self.storage_load(),
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
        if cats.contains(&Cat::Sound) {
            self.sound_rows(&mut rows);
        }
        if cats.contains(&Cat::Storage) {
            self.storage_rows(&mut rows);
        }
        if cats.contains(&Cat::OsUpdates) {
            self.os_rows(&mut rows);
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
            _ => {}
        }
    }

    /// Left/Right on a System row, or a switch flipped (`dir` > 0: on).
    pub fn sys_change(&mut self, id: SId, dir: i32) {
        match id {
            SId::WifiOn => self.net_radio(dir > 0),
            SId::Mute => self.sound_mute(dir > 0),
            SId::Volume => self.sound_step(dir),
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
