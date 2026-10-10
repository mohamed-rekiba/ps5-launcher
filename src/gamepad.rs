//! Controller input straight from evdev (/dev/input/event*). No udev/SDL dependency.
//! Works for DualSense, DualShock 4, Xbox and most pads with the standard Linux mapping,
//! and keeps working while a game has focus (needed for the PS button toggle).
//!
//! On macOS, controllers come from the system through gilrs instead (see the macOS items below).
#![cfg_attr(not(target_os = "linux"), allow(dead_code))] // the evdev code is Linux-only

use std::collections::HashMap;
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pad {
    Up,
    Down,
    Left,
    Right,
    Confirm,  // ✕ / A
    Back,     // ○ / B
    Square,   // □ / X
    Triangle, // △ / Y
    L1,
    R1,
    L2,
    R2,
    Options,
    /// The PS / Guide button, pressed and let go within 2 seconds (sent on release).
    Ps,
    /// The PS / Guide button held for 2 seconds (sent once, while it is still held).
    PsHold,
    /// Confirm held for 500 ms while the on-screen keyboard is open (sent once, while held).
    /// While the keyboard is open, a short Confirm press is sent on release instead of on press.
    ConfirmHold,
}

const EV_SYN: u16 = 0;
const SYN_REPORT: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const BTN_SOUTH: u16 = 0x130;
const BTN_MODE: u16 = 0x13c;

fn map_button(code: u16) -> Option<Pad> {
    Some(match code {
        0x130 => Pad::Confirm,
        0x131 => Pad::Back,
        0x133 => Pad::Triangle,
        0x134 => Pad::Square,
        0x136 => Pad::L1,
        0x137 => Pad::R1,
        0x138 => Pad::L2,
        0x139 => Pad::R2,
        0x13b => Pad::Options,
        0x13c => Pad::Ps,
        0x220 => Pad::Up,
        0x221 => Pad::Down,
        0x222 => Pad::Left,
        0x223 => Pad::Right,
        _ => return None,
    })
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

fn abs_info(fd: RawFd, axis: u16) -> Option<AbsInfo> {
    // EVIOCGABS(axis) = _IOR('E', 0x40 + axis, struct input_absinfo)
    let req = (2u64 << 30) | ((std::mem::size_of::<AbsInfo>() as u64) << 16) | ((b'E' as u64) << 8) | (0x40 + axis as u64);
    let mut info = AbsInfo::default();
    let r = unsafe { libc::ioctl(fd, req as _, &mut info as *mut AbsInfo) };
    (r >= 0 && info.maximum > info.minimum).then_some(info)
}

struct Device {
    fd: RawFd,
    path: String,
    ranges: HashMap<u16, (i32, i32)>,
    axes: HashMap<u16, f32>,
    buttons: [bool; 4], // dpad buttons up/down/left/right
    ps: HoldButton,
    confirm: HoldButton,
    /// The axes that are L2 and R2 on this device, if any (see `trigger_axes`).
    trigger_codes: [Option<u16>; 2],
    triggers: [TriggerAxis; 2],
}

/// evdev nodes of gamepads (devices with a joystick handler and a south face button).
fn gamepad_paths() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string("/proc/bus/input/devices") else { return Vec::new() };
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let (mut handlers, mut keys) = ("", "");
        for line in block.lines() {
            if let Some(h) = line.strip_prefix("H: Handlers=") {
                handlers = h;
            } else if let Some(k) = line.strip_prefix("B: KEY=") {
                keys = k;
            }
        }
        if keys.is_empty() || !handlers.split_whitespace().any(|h| h.starts_with("js")) {
            continue;
        }
        // KEY bitmap: space-separated 64-bit words, most significant first.
        let words: Vec<u64> = keys.split_whitespace().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect();
        let bit = |n: u16| {
            let (w, b) = ((n / 64) as usize, n % 64);
            words.len() > w && words[words.len() - 1 - w] >> b & 1 == 1
        };
        if bit(BTN_SOUTH) || bit(BTN_MODE) {
            if let Some(ev) = handlers.split_whitespace().find(|h| h.starts_with("event")) {
                out.push(format!("/dev/input/{ev}"));
            }
        }
    }
    out
}

/// Names of the connected controllers, for showing whether games will get one. Reads the same
/// device list as `gamepad_paths`, so it works even without permission to open the devices.
#[cfg(target_os = "linux")]
pub fn connected() -> Vec<String> {
    let mut names = device_names();
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| seen.insert(n.clone()));
    names
}

/// How many controllers are connected. Two controllers of the same model count as two.
#[cfg(target_os = "linux")]
pub fn count() -> usize {
    device_names().len()
}

/// The name of every connected controller, one entry per device.
#[cfg(target_os = "linux")]
fn device_names() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string("/proc/bus/input/devices") else { return Vec::new() };
    let mut names: Vec<String> = Vec::new();
    for block in text.split("\n\n") {
        let (mut name, mut handlers, mut keys) = ("", "", "");
        for line in block.lines() {
            if let Some(n) = line.strip_prefix("N: Name=") {
                name = n.trim_matches('"');
            } else if let Some(h) = line.strip_prefix("H: Handlers=") {
                handlers = h;
            } else if let Some(k) = line.strip_prefix("B: KEY=") {
                keys = k;
            }
        }
        if keys.is_empty() || !handlers.split_whitespace().any(|h| h.starts_with("js")) {
            continue;
        }
        let words: Vec<u64> = keys.split_whitespace().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect();
        let bit = |n: u16| {
            let (w, b) = ((n / 64) as usize, n % 64);
            words.len() > w && words[words.len() - 1 - w] >> b & 1 == 1
        };
        if (bit(BTN_SOUTH) || bit(BTN_MODE)) && !name.is_empty() {
            names.push(name.to_string());
        }
    }
    names
}

/// Names of the controllers macOS reports, kept current by the `run` loop below.
#[cfg(target_os = "macos")]
static MAC_PADS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Names of the connected controllers, for showing whether games will get one.
#[cfg(target_os = "macos")]
pub fn connected() -> Vec<String> {
    MAC_PADS.lock().map(|n| n.clone()).unwrap_or_default()
}

/// How many controllers macOS reports, kept current by the `run` loop below. Two controllers of
/// the same model count as two.
#[cfg(target_os = "macos")]
static MAC_PAD_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many controllers are connected.
#[cfg(target_os = "macos")]
pub fn count() -> usize {
    MAC_PAD_COUNT.load(std::sync::atomic::Ordering::Relaxed)
}

/// "DualSense Wireless Controller connected" or "No controller connected".
pub fn status() -> String {
    match connected().first() {
        Some(name) => format!("{name} connected"),
        None => "No controller connected".into(),
    }
}

fn open_device(path: &str) -> Option<Device> {
    let c = std::ffi::CString::new(path).ok()?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let mut ranges = HashMap::new();
    for axis in [0u16, 1, 16, 17, ABS_Z, ABS_RX, ABS_RY, ABS_RZ, ABS_GAS, ABS_BRAKE] {
        if let Some(i) = abs_info(fd, axis) {
            ranges.insert(axis, (i.minimum, i.maximum));
        }
    }
    let trigger_codes = trigger_axes(|c| ranges.contains_key(&c));
    crate::log!("controller connected: {path}");
    Some(Device {
        fd,
        path: path.to_string(),
        ranges,
        axes: HashMap::new(),
        buttons: [false; 4],
        ps: HoldButton::ps(),
        confirm: HoldButton::confirm(),
        trigger_codes,
        triggers: [TriggerAxis::new(Pad::L2), TriggerAxis::new(Pad::R2)],
    })
}

/// Spawns the input thread. `emit` receives presses (directions auto-repeat while held).
pub fn spawn(emit: impl Fn(Pad) + Send + 'static) {
    std::thread::Builder::new()
        .name("gamepad".into())
        .spawn(move || run(emit))
        .ok();
}

/// Turns "which directions are held" into presses with auto-repeat, like a keyboard: one press
/// at once, a pause, then a steady repeat. Shared by the Linux and macOS input backends.
#[derive(Default)]
struct Repeater {
    /// Direction -> when its next repeat is due.
    next: HashMap<Pad, Instant>,
}

const REPEAT_AFTER: Duration = Duration::from_millis(380);
const REPEAT_EVERY: Duration = Duration::from_millis(85);

impl Repeater {
    /// `held` is [up, down, left, right]. Calls `emit` for every press that is due at `now`.
    fn update(&mut self, held: [bool; 4], now: Instant, mut emit: impl FnMut(Pad)) {
        for (i, p) in [Pad::Up, Pad::Down, Pad::Left, Pad::Right].into_iter().enumerate() {
            if !held[i] {
                self.next.remove(&p);
            } else if let Some(next) = self.next.get_mut(&p) {
                if now >= *next {
                    *next = now + REPEAT_EVERY;
                    emit(p);
                }
            } else {
                self.next.insert(p, now + REPEAT_AFTER);
                emit(p);
            }
        }
    }

    /// How long until the next repeat is due (zero if overdue), or None if nothing is held.
    fn next_due(&self, now: Instant) -> Option<Duration> {
        self.next.values().map(|t| t.saturating_duration_since(now)).min()
    }
}

/// A button with a short press and a hold, for one controller. A short press is `tap`, sent when
/// the button is let go; holding it for `after` is `long`, sent once while it is held, and then
/// letting go sends nothing.
///
/// The PS button always works this way (2 seconds: `Pad::Ps` or `Pad::PsHold`). The Confirm button
/// works this way only while the on-screen keyboard is open (500 ms: `Pad::Confirm` or
/// `Pad::ConfirmHold`, for accents); otherwise it presses at once, as every other button does.
struct HoldButton {
    held_since: Option<Instant>,
    /// `long` was sent for the current press.
    fired: bool,
    after: Duration,
    tap: Pad,
    long: Pad,
}

const PS_HOLD: Duration = Duration::from_secs(2);
const CONFIRM_HOLD: Duration = Duration::from_millis(500);

impl HoldButton {
    fn ps() -> Self {
        HoldButton { held_since: None, fired: false, after: PS_HOLD, tap: Pad::Ps, long: Pad::PsHold }
    }

    fn confirm() -> Self {
        HoldButton { held_since: None, fired: false, after: CONFIRM_HOLD, tap: Pad::Confirm, long: Pad::ConfirmHold }
    }

    fn press(&mut self, now: Instant) {
        // A press while already held keeps the first press's deadline.
        if self.held_since.is_none() {
            self.held_since = Some(now);
            self.fired = false;
        }
    }

    fn release(&mut self, now: Instant, mut emit: impl FnMut(Pad)) {
        let Some(since) = self.held_since.take() else { return };
        if self.fired {
            return;
        }
        // The release can arrive before an update has seen the deadline pass.
        emit(if now >= since + self.after { self.long } else { self.tap });
    }

    /// Sends `long` when the button has been held long enough at `now`.
    fn update(&mut self, now: Instant, mut emit: impl FnMut(Pad)) {
        if let Some(since) = self.held_since {
            if !self.fired && now >= since + self.after {
                self.fired = true;
                emit(self.long);
            }
        }
    }

    /// Forget the current press: neither the hold nor the release sends anything.
    fn cancel(&mut self) {
        self.held_since = None;
    }

    /// How long until `long` is due (zero if overdue), or None if nothing is pending.
    fn next_due(&self, now: Instant) -> Option<Duration> {
        match self.held_since {
            Some(since) if !self.fired => Some((since + self.after).saturating_duration_since(now)),
            _ => None,
        }
    }
}

/// An analog trigger read as a button: L2 or R2 for a pad that reports its triggers only as an
/// axis (Xbox pads under `xpad`, for example). Pulling past 3/4 is one press; the trigger must come
/// back to 0.65 before the next pull counts. These are the thresholds gilrs uses on macOS.
///
/// Pads that report the trigger as a button too (DualSense, DualShock 4) must not press twice:
/// once the device sends the button, its axis is ignored for good (`button`).
struct TriggerAxis {
    pad: Pad,
    /// The latest normalised value, 0.0 (let go) to 1.0 (fully pulled).
    level: f32,
    pressed: bool,
    /// The device sent the digital button for this trigger: the button presses, not the axis.
    digital: bool,
}

const TRIGGER_PRESS: f32 = 0.75;
const TRIGGER_RELEASE: f32 = 0.65;

impl TriggerAxis {
    fn new(pad: Pad) -> Self {
        TriggerAxis { pad, level: 0.0, pressed: false, digital: false }
    }

    /// The axis moved to `level` (0.0 to 1.0). Takes effect at the next `sync`.
    fn axis(&mut self, level: f32) {
        self.level = level;
    }

    /// The device sent the digital button for this trigger (pressed or released).
    fn button(&mut self) {
        self.digital = true;
        self.pressed = false;
    }

    /// End of one report from the device: sends the press if this pull crossed the threshold.
    /// Called once per report so that a button sent in the same report as the axis counts first.
    fn sync(&mut self, mut emit: impl FnMut(Pad)) {
        if self.digital {
            return;
        }
        if !self.pressed && self.level >= TRIGGER_PRESS {
            self.pressed = true;
            emit(self.pad);
        } else if self.pressed && self.level <= TRIGGER_RELEASE {
            self.pressed = false;
        }
    }
}

const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
const ABS_GAS: u16 = 0x09;
const ABS_BRAKE: u16 = 0x0a;
const BTN_TL2: u16 = 0x138;
const BTN_TR2: u16 = 0x139;

/// Which axes are the L2 and R2 triggers on a device, given the axes it has (`has`).
///
/// ABS_BRAKE and ABS_GAS are triggers wherever they appear: the kernel gives them only to the
/// HID "Brake" and "Accelerator" controls, never to a stick. ABS_Z and ABS_RZ are triggers only
/// when the device also has ABS_RX and ABS_RY: then the right stick is there (xpad,
/// hid-playstation). A generic HID pad without RX/RY puts its right stick on Z/RZ, and a stick
/// pushed to one side must not press L2 or R2. The range alone cannot tell them apart: both a
/// DualSense trigger and a generic stick can run from 0 to 255.
fn trigger_axes(has: impl Fn(u16) -> bool) -> [Option<u16>; 2] {
    let right_stick = has(ABS_RX) && has(ABS_RY);
    let pick = |pedal: u16, axis: u16| {
        if has(pedal) {
            Some(pedal)
        } else if right_stick && has(axis) {
            Some(axis)
        } else {
            None
        }
    };
    [pick(ABS_BRAKE, ABS_Z), pick(ABS_GAS, ABS_RZ)]
}

/// A raw trigger value as 0.0 (let go) to 1.0 (fully pulled), for the axis range `lo..=hi`.
fn trigger_level(value: i32, lo: i32, hi: i32) -> f32 {
    ((value - lo) as f32 / (hi - lo) as f32).clamp(0.0, 1.0)
}

/// The on-screen keyboard is open: Confirm tells a press from a hold (see `HoldButton`).
static CONFIRM_HOLDS: AtomicBool = AtomicBool::new(false);

/// Called by the on-screen keyboard when it opens and closes.
pub fn set_confirm_hold(on: bool) {
    CONFIRM_HOLDS.store(on, Ordering::Relaxed);
}

fn confirm_holds() -> bool {
    CONFIRM_HOLDS.load(Ordering::Relaxed)
}

#[cfg(target_os = "linux")]
fn run(emit: impl Fn(Pad)) {
    let mut devices: Vec<Device> = Vec::new();
    let mut last_scan = Instant::now() - Duration::from_secs(60);
    let mut repeater = Repeater::default();
    let mut buf = [0u8; 24 * 64];
    loop {
        if last_scan.elapsed() > Duration::from_secs(3) {
            last_scan = Instant::now();
            for p in gamepad_paths() {
                if !devices.iter().any(|d| d.path == p) {
                    if let Some(d) = open_device(&p) {
                        devices.push(d);
                    }
                }
            }
        }
        if devices.is_empty() {
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        // Wait for input, or until the next key-repeat, PS hold or Confirm hold is due.
        let now = Instant::now();
        let timeout = devices
            .iter()
            .flat_map(|d| [d.ps.next_due(now), d.confirm.next_due(now)])
            .flatten()
            .chain(repeater.next_due(now))
            .min()
            .unwrap_or(Duration::from_millis(3000));
        let mut fds: Vec<libc::pollfd> = devices.iter().map(|d| libc::pollfd { fd: d.fd, events: libc::POLLIN, revents: 0 }).collect();
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout.as_millis().min(3000) as i32) };

        let mut gone = Vec::new();
        for (i, pfd) in fds.iter().enumerate() {
            if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                gone.push(i);
                continue;
            }
            if pfd.revents & libc::POLLIN == 0 {
                continue;
            }
            let dev = &mut devices[i];
            let n = unsafe { libc::read(dev.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n <= 0 {
                if n < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EAGAIN) {
                    gone.push(i);
                }
                continue;
            }
            for ev in buf[..n as usize].chunks_exact(24) {
                let typ = u16::from_ne_bytes([ev[16], ev[17]]);
                let code = u16::from_ne_bytes([ev[18], ev[19]]);
                let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                match typ {
                    // 1 = pressed, 0 = released; 2 is the kernel's key repeat.
                    EV_KEY if code == BTN_MODE => match value {
                        1 => dev.ps.press(Instant::now()),
                        0 => dev.ps.release(Instant::now(), |p| emit(p)),
                        _ => {}
                    },
                    // Confirm presses at once, unless the on-screen keyboard wants its holds.
                    EV_KEY if code == BTN_SOUTH => match value {
                        1 if confirm_holds() => dev.confirm.press(Instant::now()),
                        1 => emit(Pad::Confirm),
                        0 if confirm_holds() => dev.confirm.release(Instant::now(), |p| emit(p)),
                        0 => dev.confirm.cancel(),
                        _ => {}
                    },
                    // L2/R2 as buttons: from now on this device's trigger axes are ignored.
                    EV_KEY if code == BTN_TL2 || code == BTN_TR2 => {
                        let t = &mut dev.triggers[(code - BTN_TL2) as usize];
                        t.button();
                        if value == 1 {
                            emit(t.pad);
                        }
                    }
                    EV_KEY => match map_button(code) {
                        Some(p @ (Pad::Up | Pad::Down | Pad::Left | Pad::Right)) => {
                            dev.buttons[p as usize] = value != 0;
                        }
                        Some(p) if value == 1 => emit(p),
                        _ => {}
                    },
                    EV_ABS if [0, 1, 16, 17].contains(&code) => {
                        let v = match dev.ranges.get(&code) {
                            Some(&(lo, hi)) if code < 16 => (value - lo) as f32 / (hi - lo) as f32 * 2.0 - 1.0,
                            _ => value.signum() as f32,
                        };
                        dev.axes.insert(code, v);
                    }
                    EV_ABS => {
                        let side = dev.trigger_codes.iter().position(|&c| c == Some(code));
                        if let (Some(i), Some(&(lo, hi))) = (side, dev.ranges.get(&code)) {
                            dev.triggers[i].axis(trigger_level(value, lo, hi));
                        }
                    }
                    // The end of one report: the triggers press now, after any L2/R2 button in it.
                    EV_SYN if code == SYN_REPORT => {
                        for t in &mut dev.triggers {
                            t.sync(|p| emit(p));
                        }
                    }
                    _ => {}
                }
            }
        }
        for i in gone.into_iter().rev() {
            crate::log!("controller disconnected: {}", devices[i].path);
            unsafe { libc::close(devices[i].fd) };
            devices.remove(i);
        }

        // Combine d-pad (hat or buttons) and left stick into directions with auto-repeat.
        let mut dirs = [false; 4];
        for d in &devices {
            let ax = |c: u16| d.axes.get(&c).copied().unwrap_or(0.0);
            dirs[0] |= d.buttons[0] || ax(17) < -0.5 || ax(1) < -0.55;
            dirs[1] |= d.buttons[1] || ax(17) > 0.5 || ax(1) > 0.55;
            dirs[2] |= d.buttons[2] || ax(16) < -0.5 || ax(0) < -0.55;
            dirs[3] |= d.buttons[3] || ax(16) > 0.5 || ax(0) > 0.55;
        }
        let now = Instant::now();
        repeater.update(dirs, now, |p| emit(p));
        let holds = confirm_holds();
        for d in &mut devices {
            d.ps.update(now, |p| emit(p));
            // The keyboard closed while Confirm was held: that press is dropped.
            if !holds {
                d.confirm.cancel();
            }
            d.confirm.update(now, |p| emit(p));
        }
    }
}

/// macOS: controllers through `gilrs` (IOKit underneath). It maps pads to one standard layout, so
/// buttons arrive by position (South = ✕ on a PlayStation pad, A on an Xbox pad).
#[cfg(target_os = "macos")]
fn map_gilrs_button(button: gilrs::Button) -> Option<Pad> {
    use gilrs::Button::*;
    Some(match button {
        South => Pad::Confirm,
        East => Pad::Back,
        North => Pad::Triangle,
        West => Pad::Square,
        LeftTrigger => Pad::L1,
        RightTrigger => Pad::R1,
        LeftTrigger2 => Pad::L2,
        RightTrigger2 => Pad::R2,
        Start => Pad::Options,
        Mode => Pad::Ps,
        DPadUp => Pad::Up,
        DPadDown => Pad::Down,
        DPadLeft => Pad::Left,
        DPadRight => Pad::Right,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
fn run(emit: impl Fn(Pad)) {
    use gilrs::{Axis, Event, EventType, GamepadId, Gilrs};

    /// What one controller is holding: d-pad buttons [up, down, left, right] and the left stick.
    struct State {
        dpad: [bool; 4],
        stick: [f32; 2], // x, y (gilrs reports up as positive y)
        ps: HoldButton,
        confirm: HoldButton,
    }
    impl Default for State {
        fn default() -> Self {
            State { dpad: [false; 4], stick: [0.0; 2], ps: HoldButton::ps(), confirm: HoldButton::confirm() }
        }
    }

    let mut gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            crate::log!("controllers unavailable: {e}");
            return;
        }
    };
    let mut states: HashMap<GamepadId, State> = HashMap::new();
    let mut repeater = Repeater::default();
    loop {
        // Sleep until input arrives or a direction repeat, PS hold or Confirm hold is due (never
        // long: pads can appear).
        let now = Instant::now();
        let wait = states
            .values()
            .flat_map(|st| [st.ps.next_due(now), st.confirm.next_due(now)])
            .flatten()
            .chain(repeater.next_due(now))
            .min()
            .unwrap_or(Duration::from_millis(500))
            .min(Duration::from_millis(500));
        let mut first = gilrs.next_event_blocking(Some(wait));
        while let Some(Event { id, event, .. }) = first.take().or_else(|| gilrs.next_event()) {
            let st = states.entry(id).or_default();
            match event {
                // Directions are tracked as held state; the Repeater turns them into presses.
                EventType::ButtonPressed(b, _) => match map_gilrs_button(b) {
                    Some(p) if (p as usize) < 4 => st.dpad[p as usize] = true,
                    Some(Pad::Ps) => st.ps.press(Instant::now()),
                    // Confirm presses at once, unless the on-screen keyboard wants its holds.
                    Some(Pad::Confirm) if confirm_holds() => st.confirm.press(Instant::now()),
                    Some(p) => emit(p),
                    None => {}
                },
                EventType::ButtonReleased(b, _) => match map_gilrs_button(b) {
                    Some(p) if (p as usize) < 4 => st.dpad[p as usize] = false,
                    Some(Pad::Ps) => st.ps.release(Instant::now(), |p| emit(p)),
                    Some(Pad::Confirm) if confirm_holds() => st.confirm.release(Instant::now(), |p| emit(p)),
                    Some(Pad::Confirm) => st.confirm.cancel(),
                    _ => {}
                },
                EventType::AxisChanged(Axis::LeftStickX, v, _) => st.stick[0] = v,
                EventType::AxisChanged(Axis::LeftStickY, v, _) => st.stick[1] = v,
                EventType::Connected => crate::log!("controller connected: {}", gilrs.gamepad(id).name()),
                EventType::Disconnected => {
                    crate::log!("controller disconnected");
                    states.remove(&id);
                }
                _ => {}
            }
        }
        // Keep the list of connected pads current for the Settings and launch screens.
        let mut names: Vec<String> = Vec::new();
        let mut count = 0;
        for (_, pad) in gilrs.gamepads() {
            if !pad.is_connected() {
                continue;
            }
            count += 1;
            if !names.iter().any(|n| n == pad.name()) {
                names.push(pad.name().to_string());
            }
        }
        MAC_PAD_COUNT.store(count, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut shared) = MAC_PADS.lock() {
            if *shared != names {
                *shared = names;
            }
        }
        let mut dirs = [false; 4];
        for st in states.values() {
            dirs[0] |= st.dpad[0] || st.stick[1] > 0.55;
            dirs[1] |= st.dpad[1] || st.stick[1] < -0.55;
            dirs[2] |= st.dpad[2] || st.stick[0] < -0.55;
            dirs[3] |= st.dpad[3] || st.stick[0] > 0.55;
        }
        let now = Instant::now();
        repeater.update(dirs, now, |p| emit(p));
        let holds = confirm_holds();
        for st in states.values_mut() {
            st.ps.update(now, |p| emit(p));
            // The keyboard closed while Confirm was held: that press is dropped.
            if !holds {
                st.confirm.cancel();
            }
            st.confirm.update(now, |p| emit(p));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the repeater at `at` ms after `t0` and collect what it presses.
    fn step(r: &mut Repeater, held: [bool; 4], t0: Instant, at: u64) -> Vec<Pad> {
        let mut out = Vec::new();
        r.update(held, t0 + Duration::from_millis(at), |p| out.push(p));
        out
    }
    const NONE: [bool; 4] = [false; 4];
    const DOWN: [bool; 4] = [false, true, false, false];

    #[test]
    fn a_held_direction_presses_at_once_then_pauses_then_repeats() {
        let (mut r, t0) = (Repeater::default(), Instant::now());
        assert_eq!(step(&mut r, DOWN, t0, 0), [Pad::Down], "immediate press");
        assert!(step(&mut r, DOWN, t0, 100).is_empty(), "still in the initial pause");
        assert!(step(&mut r, DOWN, t0, 379).is_empty());
        assert_eq!(step(&mut r, DOWN, t0, 380), [Pad::Down], "first repeat");
        assert!(step(&mut r, DOWN, t0, 400).is_empty());
        assert_eq!(step(&mut r, DOWN, t0, 465), [Pad::Down], "then every 85 ms");
        assert_eq!(step(&mut r, DOWN, t0, 550), [Pad::Down]);
    }

    #[test]
    fn releasing_stops_the_repeat_and_a_new_press_starts_over() {
        let (mut r, t0) = (Repeater::default(), Instant::now());
        step(&mut r, DOWN, t0, 0);
        assert!(step(&mut r, NONE, t0, 100).is_empty());
        assert_eq!(r.next_due(t0 + Duration::from_millis(100)), None);
        assert!(step(&mut r, NONE, t0, 500).is_empty(), "nothing repeats after release");
        assert_eq!(step(&mut r, DOWN, t0, 600), [Pad::Down], "pressed again: immediate");
        assert!(step(&mut r, DOWN, t0, 700).is_empty(), "with the initial pause again");
    }

    #[test]
    fn directions_repeat_independently() {
        let (mut r, t0) = (Repeater::default(), Instant::now());
        assert_eq!(step(&mut r, DOWN, t0, 0), [Pad::Down]);
        // Right joins while Down is mid-pause: Right presses at once, Down keeps its own clock.
        assert_eq!(step(&mut r, [false, true, false, true], t0, 200), [Pad::Right]);
        assert_eq!(step(&mut r, [false, true, false, true], t0, 380), [Pad::Down]);
        // Right's initial pause ends at 580, and Down (repeating every 85 ms since 380) is due too.
        assert_eq!(step(&mut r, [false, true, false, true], t0, 580), [Pad::Down, Pad::Right]);
    }

    fn ms(t0: Instant, at: u64) -> Instant {
        t0 + Duration::from_millis(at)
    }

    /// Let go of the PS button at `at` ms and collect what it sends.
    fn release(b: &mut HoldButton, t0: Instant, at: u64) -> Vec<Pad> {
        let mut out = Vec::new();
        b.release(ms(t0, at), |p| out.push(p));
        out
    }

    /// Update the PS button at `at` ms and collect what it sends.
    fn tick(b: &mut HoldButton, t0: Instant, at: u64) -> Vec<Pad> {
        let mut out = Vec::new();
        b.update(ms(t0, at), |p| out.push(p));
        out
    }

    #[test]
    fn a_short_ps_press_is_sent_when_let_go() {
        let (mut b, t0) = (HoldButton::ps(), Instant::now());
        b.press(t0);
        assert!(tick(&mut b, t0, 500).is_empty(), "nothing while held");
        assert_eq!(release(&mut b, t0, 1999), [Pad::Ps]);
        assert!(tick(&mut b, t0, 3000).is_empty(), "no hold after letting go");
    }

    #[test]
    fn holding_ps_for_2_seconds_sends_the_hold_once() {
        let (mut b, t0) = (HoldButton::ps(), Instant::now());
        b.press(t0);
        assert!(tick(&mut b, t0, 1999).is_empty());
        assert_eq!(tick(&mut b, t0, 2000), [Pad::PsHold], "exactly at 2 s");
        assert!(tick(&mut b, t0, 2500).is_empty(), "only once");
        assert!(release(&mut b, t0, 3000).is_empty(), "letting go sends no short press");
    }

    #[test]
    fn a_late_release_still_counts_as_a_hold() {
        // The release arrives before an update ran past the deadline.
        let (mut b, t0) = (HoldButton::ps(), Instant::now());
        b.press(t0);
        assert_eq!(release(&mut b, t0, 2100), [Pad::PsHold]);
    }

    #[test]
    fn repeated_presses_and_stray_releases_are_harmless() {
        let (mut b, t0) = (HoldButton::ps(), Instant::now());
        assert!(release(&mut b, t0, 0).is_empty(), "release without a press");
        b.press(t0);
        b.press(ms(t0, 1500));
        assert_eq!(tick(&mut b, t0, 2000), [Pad::PsHold], "a second press keeps the first deadline");
        release(&mut b, t0, 2100);
        b.press(ms(t0, 3000));
        assert_eq!(release(&mut b, t0, 3100), [Pad::Ps], "a new press starts over");
    }

    #[test]
    fn next_due_says_when_the_hold_is_due() {
        let (mut b, t0) = (HoldButton::ps(), Instant::now());
        assert_eq!(b.next_due(t0), None);
        b.press(t0);
        assert_eq!(b.next_due(ms(t0, 500)), Some(Duration::from_millis(1500)));
        assert_eq!(b.next_due(ms(t0, 2500)), Some(Duration::ZERO), "overdue means now");
        tick(&mut b, t0, 2500);
        assert_eq!(b.next_due(ms(t0, 2600)), None, "nothing pending after the hold fired");
    }

    #[test]
    fn each_controller_has_its_own_ps_button() {
        let (mut a, mut b, t0) = (HoldButton::ps(), HoldButton::ps(), Instant::now());
        a.press(t0);
        b.press(ms(t0, 1000));
        assert_eq!(release(&mut b, t0, 1500), [Pad::Ps]);
        assert_eq!(tick(&mut a, t0, 2000), [Pad::PsHold]);
    }

    #[test]
    fn a_short_confirm_press_is_sent_when_let_go() {
        let (mut b, t0) = (HoldButton::confirm(), Instant::now());
        b.press(t0);
        assert!(tick(&mut b, t0, 300).is_empty(), "nothing while held");
        assert_eq!(release(&mut b, t0, 499), [Pad::Confirm]);
    }

    #[test]
    fn holding_confirm_for_half_a_second_sends_the_hold_once() {
        let (mut b, t0) = (HoldButton::confirm(), Instant::now());
        b.press(t0);
        assert!(tick(&mut b, t0, 499).is_empty());
        assert_eq!(tick(&mut b, t0, 500), [Pad::ConfirmHold], "exactly at 500 ms");
        assert!(tick(&mut b, t0, 900).is_empty(), "only once");
        assert!(release(&mut b, t0, 1000).is_empty(), "letting go sends no short press");
        assert_eq!(b.next_due(ms(t0, 1000)), None);
    }

    #[test]
    fn cancel_drops_a_pending_confirm() {
        // The keyboard closed while the button was held: letting go must not press anything.
        let (mut b, t0) = (HoldButton::confirm(), Instant::now());
        b.press(t0);
        b.cancel();
        assert_eq!(b.next_due(ms(t0, 100)), None);
        assert!(tick(&mut b, t0, 600).is_empty());
        assert!(release(&mut b, t0, 700).is_empty());
        b.press(ms(t0, 800));
        assert_eq!(release(&mut b, t0, 900), [Pad::Confirm], "the next press works again");
    }

    /// Move the trigger to `level`, end the report, and collect what it sends.
    fn pull(t: &mut TriggerAxis, level: f32) -> Vec<Pad> {
        let mut out = Vec::new();
        t.axis(level);
        t.sync(|p| out.push(p));
        out
    }

    #[test]
    fn a_trigger_presses_once_per_pull() {
        let mut t = TriggerAxis::new(Pad::L2);
        assert!(pull(&mut t, 0.74).is_empty(), "not far enough");
        assert_eq!(pull(&mut t, 0.75), [Pad::L2], "exactly at 3/4");
        assert!(pull(&mut t, 1.0).is_empty(), "only once while held");
        assert!(pull(&mut t, 0.70).is_empty(), "between the thresholds: still held");
        assert!(pull(&mut t, 0.80).is_empty(), "so this is the same pull");
        assert!(pull(&mut t, 0.65).is_empty(), "let go at 0.65");
        assert_eq!(pull(&mut t, 0.80), [Pad::L2], "the next pull presses again");
    }

    #[test]
    fn a_trigger_must_come_back_to_0_65() {
        let mut t = TriggerAxis::new(Pad::R2);
        assert_eq!(pull(&mut t, 0.9), [Pad::R2]);
        assert!(pull(&mut t, 0.66).is_empty(), "not let go yet");
        assert!(pull(&mut t, 0.9).is_empty(), "so no second press");
        assert!(pull(&mut t, 0.0).is_empty());
        assert_eq!(pull(&mut t, 0.9), [Pad::R2]);
    }

    #[test]
    fn the_axis_counts_only_at_the_end_of_a_report() {
        let mut t = TriggerAxis::new(Pad::L2);
        t.axis(1.0);
        t.axis(0.0);
        let mut out = Vec::new();
        t.sync(|p| out.push(p));
        assert!(out.is_empty(), "the value at the end of the report is what counts");
    }

    #[test]
    fn a_pad_that_sends_the_button_does_not_press_twice() {
        // DualSense and DualShock 4 send BTN_TL2 and ABS_Z in the same report, the axis first.
        let mut t = TriggerAxis::new(Pad::L2);
        t.axis(1.0);
        t.button();
        let mut out = Vec::new();
        t.sync(|p| out.push(p));
        assert!(out.is_empty(), "the button press is the only press");
        assert!(pull(&mut t, 0.0).is_empty());
        assert!(pull(&mut t, 1.0).is_empty(), "and the axis stays ignored after that");
    }

    #[test]
    fn a_button_seen_after_an_axis_press_ends_the_axis_press() {
        let mut t = TriggerAxis::new(Pad::R2);
        assert_eq!(pull(&mut t, 1.0), [Pad::R2]);
        t.button();
        assert!(pull(&mut t, 0.0).is_empty());
        assert!(pull(&mut t, 1.0).is_empty());
    }

    fn axes_of(list: &'static [u16]) -> [Option<u16>; 2] {
        trigger_axes(|c| list.contains(&c))
    }

    #[test]
    fn xpad_and_playstation_triggers_are_z_and_rz() {
        // xpad: sticks on X/Y and RX/RY, triggers on Z/RZ. hid-playstation: the same.
        assert_eq!(axes_of(&[0, 1, ABS_Z, ABS_RX, ABS_RY, ABS_RZ, 16, 17]), [Some(ABS_Z), Some(ABS_RZ)]);
    }

    #[test]
    fn z_and_rz_are_the_right_stick_without_rx_and_ry() {
        // A generic HID pad: right stick on Z/RZ, no RX/RY.
        assert_eq!(axes_of(&[0, 1, ABS_Z, ABS_RZ, 16, 17]), [None, None]);
        assert_eq!(axes_of(&[0, 1, ABS_Z, ABS_RX, ABS_RZ]), [None, None], "both RX and RY are needed");
    }

    #[test]
    fn brake_and_gas_are_always_triggers() {
        // An Xbox pad through hid-generic: right stick on Z/RZ, triggers on BRAKE/GAS.
        assert_eq!(axes_of(&[0, 1, ABS_Z, ABS_RZ, ABS_GAS, ABS_BRAKE]), [Some(ABS_BRAKE), Some(ABS_GAS)]);
        assert_eq!(
            axes_of(&[0, 1, ABS_Z, ABS_RX, ABS_RY, ABS_RZ, ABS_GAS, ABS_BRAKE]),
            [Some(ABS_BRAKE), Some(ABS_GAS)],
            "BRAKE/GAS win over Z/RZ"
        );
    }

    #[test]
    fn a_pad_without_trigger_axes_has_none() {
        // xpad with MAP_TRIGGERS_TO_BUTTONS (fight sticks): BTN_TL2/BTN_TR2 only.
        assert_eq!(axes_of(&[0, 1, ABS_RX, ABS_RY, 16, 17]), [None, None]);
    }

    #[test]
    fn trigger_levels_use_the_axis_range() {
        assert_eq!(trigger_level(0, 0, 255), 0.0);
        assert_eq!(trigger_level(255, 0, 255), 1.0);
        assert!(trigger_level(191, 0, 255) < TRIGGER_PRESS && trigger_level(192, 0, 255) >= TRIGGER_PRESS);
        assert_eq!(trigger_level(1023, 0, 1023), 1.0, "Xbox One pads under xpad");
        assert_eq!(trigger_level(-10, 0, 255), 0.0, "clamped below");
        assert_eq!(trigger_level(300, 0, 255), 1.0, "clamped above");
    }

    #[test]
    fn next_due_says_when_to_wake_for_a_repeat() {
        let (mut r, t0) = (Repeater::default(), Instant::now());
        assert_eq!(r.next_due(t0), None);
        step(&mut r, DOWN, t0, 0);
        assert_eq!(r.next_due(t0 + Duration::from_millis(100)), Some(Duration::from_millis(280)));
        assert_eq!(r.next_due(t0 + Duration::from_millis(900)), Some(Duration::ZERO), "overdue means now");
    }
}
