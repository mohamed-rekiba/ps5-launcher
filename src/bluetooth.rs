//! Bluetooth through BlueZ's `bluetoothctl`, for the Controllers page (Phase 6 of
//! docs/plans/ps5-launcher-os.md): the adapter's power, the paired devices, a scan for
//! controllers, and pair → trust → connect. The command lines and the parsing live here;
//! `system::call` runs them.
//!
//! The formats come from bluez 5.87 (Fedora 44's bluez-5.87+1.git8750129efca8), client/main.c and
//! client/print.c. In non-interactive mode `bt_shell_printf` is a plain `vprintf`
//! (src/shared/shell.c), so colour codes stay in the output:
//! - `list`: `print_adapter`, "Controller <mac> <alias> [default]".
//! - `show`: "Controller <mac> (<type>)", then `print_property` lines: a tab, the name, ": ",
//!   and the value (`print_iter`: "yes"/"no" for a boolean, "0x%02x (%d)" for a byte,
//!   "0x%04x (%d)" for a 16-bit and "0x%08x (%d)" for a 32-bit number).
//! - `devices`: `print_device`, "Device <mac> <alias>"; a device that does not advertise as
//!   discoverable is wrapped in COLOR_BOLDGRAY ("\x1B[1;30m") and COLOR_OFF ("\x1B[0m").
//! - `info <mac>`: "Device <mac> (<type>)", then Name, Alias, Class, Appearance, Icon, Paired,
//!   Bonded, Trusted, Blocked, Connected, …, and "Battery Percentage: 0x50 (80)"
//!   (`print_property_with_label` of org.bluez.Battery1.Percentage, a byte).
//! - An unknown device: "Device <mac> not available" (`find_device`), exit 1.
//!
//! Every call has a time limit (`Call`): without bluetoothd, bluetoothctl waits forever for it.
//! bluetoothctl's own `--timeout` keeps every command running until it ends, even after it is done
//! (`bt_shell_noninteractive_quit` returns early when it is set), so only the scan uses it.
//!
//! No agent: in non-interactive mode bluetoothctl does not register the `--agent` it is given
//! (main.c, `proxy_added`: "!bt_shell_get_env(\"NON_INTERACTIVE\")"). Without an agent, bluetoothd
//! pairs as NoInputNoOutput (src/device.c, `pair_device`), the "just works" pairing that
//! controllers use.

use crate::system::{Call, Ran};

const PROGRAM: &str = "bluetoothctl";

/// Seconds for a call that only reads, or sets a property.
const READ_SECS: u32 = 10;
/// Seconds for pairing: the controller has to answer, and Secure Simple Pairing takes a while.
const PAIR_SECS: u32 = 60;
const CONNECT_SECS: u32 = 30;
/// How long a scan looks for controllers.
pub const SCAN_SECS: u32 = 30;

/// A MAC address as bluetoothctl prints it: six pairs of upper-case hex digits split by colons.
pub fn valid_mac(mac: &str) -> bool {
    let b = mac.as_bytes();
    b.len() == 17 && b.iter().enumerate().all(|(i, c)| if i % 3 == 2 { *c == b':' } else { c.is_ascii_digit() || (b'A'..=b'F').contains(c) })
}

/// The adapter's MACs, from `bluetoothctl list`. An empty list: no adapter.
pub fn list_call() -> Call {
    Call::new(PROGRAM, &["list"], READ_SECS)
}

/// The default adapter, from `bluetoothctl show`.
pub fn show_call() -> Call {
    Call::new(PROGRAM, &["show"], READ_SECS)
}

/// Every device bluetoothd knows: paired ones, and the ones a scan found.
pub fn devices_call() -> Call {
    Call::new(PROGRAM, &["devices"], READ_SECS)
}

/// The paired devices (`devices Paired`, one of `device_arguments` in client/main.c).
pub fn paired_call() -> Call {
    Call::new(PROGRAM, &["devices", "Paired"], READ_SECS)
}

pub fn power_call(on: bool) -> Call {
    Call::new(PROGRAM, &["power", if on { "on" } else { "off" }], READ_SECS)
}

/// A scan of `SCAN_SECS`. The launcher's limit is a little longer than bluetoothctl's own.
pub fn scan_call() -> Call {
    Call::new(PROGRAM, &["--timeout", &SCAN_SECS.to_string(), "scan", "on"], SCAN_SECS + 10)
}

/// `bluetoothctl <command> <mac>`, once the MAC is checked.
fn device_call(command: &'static str, mac: &str, secs: u32) -> Result<Call, String> {
    if !valid_mac(mac) {
        return Err(format!("{mac:?} is not a Bluetooth address"));
    }
    Ok(Call::new(PROGRAM, &[command, mac], secs))
}

pub fn info_call(mac: &str) -> Result<Call, String> {
    device_call("info", mac, READ_SECS)
}

/// Forget: the device's pairing and its keys go.
pub fn remove_call(mac: &str) -> Result<Call, String> {
    device_call("remove", mac, READ_SECS)
}

pub fn pair_call(mac: &str) -> Result<Call, String> {
    device_call("pair", mac, PAIR_SECS)
}

/// A trusted device may connect by itself: a controller reconnects when its PS button is pressed.
pub fn trust_call(mac: &str) -> Result<Call, String> {
    device_call("trust", mac, READ_SECS)
}

pub fn connect_call(mac: &str) -> Result<Call, String> {
    device_call("connect", mac, CONNECT_SECS)
}

/// `text` without its colour codes (ESC [ … m).
pub fn strip_colours(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("\x1B[") {
        out.push_str(&rest[..at]);
        rest = &rest[at + 2..];
        rest = match rest.find('m') {
            Some(end) => &rest[end + 1..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// "<word> <MAC> <rest>" → (MAC, rest), for a line that starts with `word`.
fn mac_line<'a>(line: &'a str, word: &str) -> Option<(&'a str, &'a str)> {
    let rest = line.strip_prefix(word)?.strip_prefix(' ')?;
    let (mac, rest) = rest.split_at_checked(17)?;
    if !valid_mac(mac) || !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    Some((mac, rest.strip_prefix(' ').unwrap_or(rest)))
}

/// The value of a `print_iter` line, "\t<name>: <value>".
fn property<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.strip_prefix('\t')?.strip_prefix(name)?.strip_prefix(": ")
}

/// A number as `print_iter` prints it: "0x50 (80)" → 80. A negative one (RSSI) is None.
fn number(value: &str) -> Option<u64> {
    let (_, decimal) = value.split_once(" (")?;
    decimal.strip_suffix(')')?.parse::<u64>().ok()
}

fn yes(value: Option<&str>) -> bool {
    value == Some("yes")
}

/// The adapters' MACs in `bluetoothctl list`.
pub fn parse_list(output: &str) -> Vec<String> {
    strip_colours(output).lines().filter_map(|line| mac_line(line, "Controller")).map(|(mac, _)| mac.to_string()).collect()
}

/// The default adapter.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Adapter {
    pub mac: String,
    /// The alias, which is what other devices see.
    pub name: String,
    pub powered: bool,
}

/// `bluetoothctl show`. An error when there is no adapter.
pub fn parse_show(output: &str) -> Result<Adapter, String> {
    let text = strip_colours(output);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let Some((mac, _)) = mac_line(first, "Controller") else {
        return Err(format!("no Bluetooth adapter: {}", first.trim()));
    };
    let (mut name, mut alias, mut powered) = (None, None, None);
    // Only the adapter's own lines, up to "Advertising Features:".
    for line in lines.take_while(|l| l.starts_with('\t')) {
        name = name.or(property(line, "Name"));
        alias = alias.or(property(line, "Alias"));
        powered = powered.or(property(line, "Powered"));
    }
    let powered = powered.ok_or("bluetoothctl show has no Powered line")?;
    Ok(Adapter { mac: mac.into(), name: alias.or(name).unwrap_or(mac).into(), powered: powered == "yes" })
}

/// A line of `bluetoothctl devices`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Device {
    pub mac: String,
    pub name: String,
}

pub fn parse_devices(output: &str) -> Vec<Device> {
    strip_colours(output)
        .lines()
        .filter_map(|line| mac_line(line, "Device"))
        .map(|(mac, name)| Device { mac: mac.into(), name: name.into() })
        .collect()
}

/// What `bluetoothctl info <mac>` says about a device.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Info {
    pub mac: String,
    /// The alias; the name when there is none.
    pub name: String,
    pub icon: Option<String>,
    /// The Class of Device, for a Bluetooth Classic device.
    pub class: Option<u32>,
    /// The GAP appearance, for a Bluetooth Low Energy device.
    pub appearance: Option<u16>,
    pub paired: bool,
    pub trusted: bool,
    pub connected: bool,
    /// Battery1.Percentage, while it is connected.
    pub battery: Option<u8>,
}

/// `bluetoothctl info <mac>`; None for "Device … not available".
pub fn parse_info(output: &str) -> Option<Info> {
    let text = strip_colours(output);
    let mut lines = text.lines();
    let (mac, rest) = mac_line(lines.next()?, "Device")?;
    if rest == "not available" {
        return None;
    }
    let mut info = Info { mac: mac.into(), ..Info::default() };
    let (mut name, mut alias) = (None, None);
    for line in lines {
        let get = |n| property(line, n);
        name = name.or(get("Name"));
        alias = alias.or(get("Alias"));
        if let Some(v) = get("Icon") {
            info.icon = Some(v.into());
        }
        if let Some(v) = get("Class").and_then(number) {
            info.class = u32::try_from(v).ok();
        }
        if let Some(v) = get("Appearance").and_then(number) {
            info.appearance = u16::try_from(v).ok();
        }
        info.paired |= yes(get("Paired"));
        info.trusted |= yes(get("Trusted"));
        info.connected |= yes(get("Connected"));
        if let Some(v) = get("Battery Percentage").and_then(number) {
            info.battery = u8::try_from(v).ok().filter(|p| *p <= 100);
        }
    }
    info.name = alias.or(name).unwrap_or(mac).into();
    Some(info)
}

/// A game controller: bluetoothd's icon is input-gaming. Its rules (src/dbus-common.c,
/// `class_to_icon` and `gap_appearance_to_icon`) are applied here too, for an answer without an
/// Icon line: a Class with major class 0x05 (peripheral), no keyboard or mouse bits, and minor
/// class joystick or gamepad; or the appearance 0x03c3 (joystick) or 0x03c4 (gamepad).
pub fn is_gamepad(info: &Info) -> bool {
    let by_class = info.class.is_some_and(|c| (c & 0x1f00) >> 8 == 0x05 && (c & 0xc0) >> 6 == 0 && matches!((c & 0x1e) >> 2, 1 | 2));
    let by_appearance = matches!(info.appearance, Some(0x03c3 | 0x03c4));
    info.icon.as_deref() == Some("input-gaming") || by_class || by_appearance
}

/// The controllers a scan found: game controllers that are not paired yet, in bluetoothd's order.
pub fn found(infos: Vec<Info>) -> Vec<Info> {
    infos.into_iter().filter(|i| !i.paired && is_gamepad(i)).collect()
}

/// What went wrong in a scan's output, if it did. With `--timeout` bluetoothctl exits 0 even when
/// discovery did not start, so the output tells.
pub fn scan_error(output: &str) -> Option<String> {
    strip_colours(output)
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("Failed to start discovery") || l.starts_with("SetDiscoveryFilter failed") || *l == "No default controller available")
        .map(Into::into)
}

/// A step of pairing a controller.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Pair,
    Trust,
    Connect,
}

impl Step {
    pub fn doing(self) -> &'static str {
        match self {
            Step::Pair => "Pairing…",
            Step::Trust => "Trusting…",
            Step::Connect => "Connecting…",
        }
    }
}

/// How pairing ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PairEnd {
    Connected,
    /// bluetoothd forgot the device: the controller left pairing mode, or the scan's results
    /// expired (TemporaryTimeout, 30 s by default in /etc/bluetooth/main.conf). Scan again.
    Gone,
    Failed { step: Step, error: String },
}

/// Pair, trust, then connect the controller at `mac`. `run` runs a call (`system::call_status`);
/// `on_step` hears each step as it starts. A device that is paired already goes on to trust.
pub fn pair_flow(run: &dyn Fn(&Call) -> Result<Ran, String>, mac: &str, on_step: &dyn Fn(Step)) -> PairEnd {
    for step in [Step::Pair, Step::Trust, Step::Connect] {
        let call = match step {
            Step::Pair => pair_call(mac),
            Step::Trust => trust_call(mac),
            Step::Connect => connect_call(mac),
        };
        let call = match call {
            Ok(call) => call,
            Err(error) => return PairEnd::Failed { step, error },
        };
        on_step(step);
        let ran = match run(&call) {
            Ok(ran) => ran,
            Err(error) => return PairEnd::Failed { step, error },
        };
        let said = strip_colours(&ran.stdout);
        if ran.code == Some(0) || (step == Step::Pair && said.contains("org.bluez.Error.AlreadyExists")) {
            continue;
        }
        if said.lines().any(|l| l.trim() == format!("Device {mac} not available")) {
            return PairEnd::Gone;
        }
        return PairEnd::Failed { step, error: strip_colours(&ran.error(&call)) };
    }
    PairEnd::Connected
}

/// The short form of an error, for under its plain sentence: bluetoothctl's last line, which
/// names BlueZ's error ("Failed to pair: org.bluez.Error.AuthenticationFailed").
pub fn detail(error: &str) -> String {
    error.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default().to_string()
}

/// A plain sentence for a bluetoothctl error.
pub fn explain(error: &str) -> &'static str {
    let has = |words: &[&str]| words.iter().any(|w| error.contains(w));
    if has(&["AuthenticationFailed", "AuthenticationRejected", "AuthenticationCanceled", "AuthenticationTimeout"]) {
        "The controller did not accept the pairing."
    } else if has(&["page-timeout", "Page Timeout", "ConnectionAttemptFailed", "Host is down"]) {
        "The controller did not answer. Is it still in pairing mode?"
    } else if has(&["InProgress", "org.bluez.Error.Busy"]) {
        "Bluetooth is busy with another device. Try again in a moment."
    } else if has(&["org.bluez.Error.Blocked"]) {
        "Bluetooth is blocked: check the PC's airplane-mode switch or key."
    } else if has(&["NotReady"]) {
        "Bluetooth is off."
    } else if has(&["did not answer within"]) {
        "Bluetooth did not answer in time."
    } else {
        "Bluetooth reported an error."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const MAC: &str = "A0:AB:51:5F:23:1A";

    #[test]
    fn a_mac_is_six_upper_case_hex_pairs() {
        assert!(valid_mac(MAC));
        assert!(valid_mac("00:1A:7D:DA:71:13"));
        for bad in ["", "a0:ab:51:5f:23:1a", "A0:AB:51:5F:23", "A0:AB:51:5F:23:1A:00", "A0-AB-51-5F-23-1A", "A0:AB:51:5F:23:1G", "A0:AB:51:5F:23:1A ", " A0:AB:51:5F:23:1A", "A0:AB:51:5F:231:A", "--help", "A0:AB:51:5F:23:1A;", "Ａ0:AB:51:5F:23:1A"] {
            assert!(!valid_mac(bad), "{bad:?}");
        }
    }

    #[test]
    fn every_call_has_a_time_limit_and_no_shell() {
        for call in [list_call(), show_call(), devices_call(), paired_call(), power_call(true), scan_call(), info_call(MAC).unwrap(), pair_call(MAC).unwrap()] {
            assert_eq!(call.program, "bluetoothctl", "{call:?}");
            assert!(call.secs > 0 && call.secs <= 60, "{call:?}");
            assert_eq!(call.command().get_program(), "timeout", "{call:?}");
        }
    }

    #[test]
    fn the_command_lines() {
        let args = |c: Call| c.args;
        assert_eq!(args(list_call()), ["list"]);
        assert_eq!(args(show_call()), ["show"]);
        assert_eq!(args(devices_call()), ["devices"]);
        assert_eq!(args(paired_call()), ["devices", "Paired"]);
        assert_eq!(args(power_call(true)), ["power", "on"]);
        assert_eq!(args(power_call(false)), ["power", "off"]);
        assert_eq!(args(info_call(MAC).unwrap()), ["info", MAC]);
        assert_eq!(args(remove_call(MAC).unwrap()), ["remove", MAC]);
        assert_eq!(args(pair_call(MAC).unwrap()), ["pair", MAC]);
        assert_eq!(args(trust_call(MAC).unwrap()), ["trust", MAC]);
        assert_eq!(args(connect_call(MAC).unwrap()), ["connect", MAC]);
    }

    #[test]
    fn only_the_scan_uses_bluetoothctls_own_timeout() {
        let scan = scan_call();
        assert_eq!(scan.args, ["--timeout", "30", "scan", "on"]);
        assert!(scan.secs > SCAN_SECS, "the launcher's limit must not end the scan first: {}", scan.secs);
        assert!(!list_call().args.contains(&"--timeout".to_string()));
        assert!(!pair_call(MAC).unwrap().args.contains(&"--timeout".to_string()));
    }

    #[test]
    fn a_bad_mac_never_reaches_a_command() {
        for bad in ["*", "--timeout", "a0:ab:51:5f:23:1a", "A0:AB:51:5F:23:1A\nshow"] {
            assert!(remove_call(bad).is_err(), "{bad:?}");
            assert!(pair_call(bad).is_err(), "{bad:?}");
            assert!(info_call(bad).is_err(), "{bad:?}");
            assert!(connect_call(bad).is_err(), "{bad:?}");
            assert!(trust_call(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn colours_come_out() {
        assert_eq!(strip_colours("[\x1B[0;92mNEW\x1B[0m] Device A0:AB:51:5F:23:1A Pad"), "[NEW] Device A0:AB:51:5F:23:1A Pad");
        assert_eq!(strip_colours("\x1B[1;30mDevice 4C:87:5D:00:11:22 TV\x1B[0m"), "Device 4C:87:5D:00:11:22 TV");
        assert_eq!(strip_colours("plain"), "plain");
        assert_eq!(strip_colours("cut \x1B[0;9"), "cut ", "an unfinished code at the end");
    }

    #[test]
    fn the_adapter_list() {
        // print_adapter: "[default]" on the default one, an empty string (so a trailing space)
        // on the others.
        let out = "Controller 00:1A:7D:DA:71:13 PS5 Launcher [default]\nController 5C:F3:70:00:00:01 usb dongle \n";
        assert_eq!(parse_list(out), ["00:1A:7D:DA:71:13", "5C:F3:70:00:00:01"]);
        assert_eq!(parse_list(""), Vec::<String>::new(), "no adapter: list prints nothing and exits 0");
        assert_eq!(parse_list("Waiting to connect to bluetoothd...\n"), Vec::<String>::new());
    }

    const SHOW: &str = "Controller 00:1A:7D:DA:71:13 (public)
\tManufacturer: 0x000a (10)
\tVersion: 0x09 (9)
\tName: fedora
\tAlias: PS5 Launcher
\tClass: 0x006c010c (7078156)
\tPowered: yes
\tPowerState: on
\tDiscoverable: no
\tDiscoverableTimeout: 0x000000b4 (180)
\tPairable: yes
\tUUID: A/V Remote Control        (0000110e-0000-1000-8000-00805f9b34fb)
\tModalias: usb:v1D6Bp0246d0557
\tDiscovering: no
\tRoles: central
\tRoles: peripheral
Advertising Features:
\tActiveInstances: 0x00 (0)
\tSupportedInstances: 0x0c (12)
\tSupportedIncludes: tx-power
";

    #[test]
    fn show_gives_the_adapter_and_its_power() {
        assert_eq!(parse_show(SHOW), Ok(Adapter { mac: "00:1A:7D:DA:71:13".into(), name: "PS5 Launcher".into(), powered: true }));
        let off = SHOW.replace("\tPowered: yes", "\tPowered: no").replace("PowerState: on", "PowerState: off-blocked");
        assert_eq!(parse_show(&off).map(|a| a.powered), Ok(false));
        // Older bluez prints no address type.
        assert_eq!(parse_show("Controller 00:1A:7D:DA:71:13\n\tName: pc\n\tPowered: no\n").map(|a| a.name), Ok("pc".into()));
    }

    #[test]
    fn show_without_an_adapter_is_an_error() {
        assert!(parse_show("No default controller available\n").is_err());
        assert!(parse_show("").is_err());
        assert!(parse_show("Controller 00:1A:7D:DA:71:13 (public)\n\tName: pc\n").is_err(), "no Powered line");
    }

    #[test]
    fn the_device_list_with_names_that_have_spaces_and_colours() {
        let out = "Device A0:AB:51:5F:23:1A DualSense Wireless Controller\n\
                   \x1B[1;30mDevice 4C:87:5D:00:11:22 [TV] Samsung 7 Series (55)\x1B[0m\n\
                   Device 7A:21:9C:0E:44:01 7A-21-9C-0E-44-01\n\
                   Device bogus line\n\
                   No default controller available\n";
        assert_eq!(
            parse_devices(out),
            [
                Device { mac: MAC.into(), name: "DualSense Wireless Controller".into() },
                Device { mac: "4C:87:5D:00:11:22".into(), name: "[TV] Samsung 7 Series (55)".into() },
                Device { mac: "7A:21:9C:0E:44:01".into(), name: "7A-21-9C-0E-44-01".into() },
            ]
        );
        assert_eq!(parse_devices("Device 00:11:22:33:44:55\n"), [Device { mac: "00:11:22:33:44:55".into(), name: String::new() }]);
    }

    const DUALSENSE: &str = "Device A0:AB:51:5F:23:1A (public)
\tName: DualSense Wireless Controller
\tAlias: DualSense Wireless Controller
\tClass: 0x00002508 (9480)
\tIcon: input-gaming
\tPaired: yes
\tBonded: yes
\tTrusted: yes
\tBlocked: no
\tConnected: yes
\tWakeAllowed: yes
\tLegacyPairing: no
\tCablePairing: no
\tUUID: Human Interface Device... (00001124-0000-1000-8000-00805f9b34fb)
\tUUID: PnP Information           (00001200-0000-1000-8000-00805f9b34fb)
\tModalias: usb:v054Cp0CE6d0100
\tBattery Percentage: 0x50 (80)
";

    #[test]
    fn info_on_a_paired_dualsense() {
        assert_eq!(
            parse_info(DUALSENSE),
            Some(Info {
                mac: MAC.into(),
                name: "DualSense Wireless Controller".into(),
                icon: Some("input-gaming".into()),
                class: Some(0x2508),
                appearance: None,
                paired: true,
                trusted: true,
                connected: true,
                battery: Some(80),
            })
        );
    }

    #[test]
    fn info_on_a_controller_in_pairing_mode_and_on_an_le_xbox_controller() {
        let new = "Device 1C:A0:B8:4E:91:02 (public)\n\tName: Wireless Controller\n\tAlias: Wireless Controller\n\tClass: 0x00002508 (9480)\n\tIcon: input-gaming\n\tPaired: no\n\tBonded: no\n\tTrusted: no\n\tBlocked: no\n\tConnected: no\n\tLegacyPairing: no\n\tRSSI: 0xffffffc4 (-60)\n";
        let info = parse_info(new).unwrap();
        assert_eq!((info.paired, info.connected, info.battery, info.name.as_str()), (false, false, None, "Wireless Controller"));
        let xbox = "Device F4:6A:D7:00:00:01 (public)\n\tName: Xbox Wireless Controller\n\tAlias: Xbox Wireless Controller\n\tAppearance: 0x03c4 (964)\n\tIcon: input-gaming\n\tPaired: no\n\tConnected: no\n";
        assert_eq!(parse_info(xbox).unwrap().appearance, Some(0x03c4));
        // The alias wins over the name; a device with neither shows its MAC (bluetoothd's alias).
        let renamed = "Device A0:AB:51:5F:23:1A (public)\n\tName: DualSense Wireless Controller\n\tAlias: Sam's pad\n\tPaired: yes\n";
        assert_eq!(parse_info(renamed).unwrap().name, "Sam's pad");
    }

    #[test]
    fn info_on_an_unknown_device_is_none() {
        assert_eq!(parse_info("Device A0:AB:51:5F:23:1A not available\n"), None);
        assert_eq!(parse_info(""), None);
        assert_eq!(parse_info("No default controller available\n"), None);
    }

    fn info(icon: Option<&str>, class: Option<u32>, appearance: Option<u16>) -> Info {
        Info { mac: MAC.into(), name: "x".into(), icon: icon.map(Into::into), class, appearance, ..Info::default() }
    }

    #[test]
    fn game_controllers_by_icon_class_or_appearance() {
        assert!(is_gamepad(&info(Some("input-gaming"), None, None)));
        assert!(is_gamepad(&info(None, Some(0x2508), None)), "DualSense, DualShock 4, Switch Pro: peripheral, gamepad");
        assert!(is_gamepad(&info(None, Some(0x0504), None)), "a joystick");
        assert!(is_gamepad(&info(None, None, Some(0x03c4))), "an Xbox controller over LE");
        assert!(is_gamepad(&info(None, None, Some(0x03c3))));
        assert!(!is_gamepad(&info(Some("input-keyboard"), Some(0x2540), None)), "a keyboard");
        assert!(!is_gamepad(&info(None, Some(0x2580), None)), "a mouse");
        assert!(!is_gamepad(&info(Some("audio-headset"), Some(0x240404), None)));
        assert!(!is_gamepad(&info(None, None, Some(0x03c1))), "an LE keyboard");
        assert!(!is_gamepad(&info(None, None, None)));
    }

    #[test]
    fn a_scan_offers_controllers_that_are_not_paired() {
        let pad = |mac: &str, paired: bool| Info { mac: mac.into(), name: mac.into(), icon: Some("input-gaming".into()), paired, ..Info::default() };
        let headset = Info { mac: "00:00:00:00:00:09".into(), icon: Some("audio-headset".into()), ..Info::default() };
        let offered = found(vec![pad("00:00:00:00:00:01", true), headset, pad("00:00:00:00:00:02", false)]);
        assert_eq!(offered.iter().map(|i| i.mac.as_str()).collect::<Vec<_>>(), ["00:00:00:00:00:02"]);
    }

    #[test]
    fn a_scan_that_could_not_start_says_why() {
        let ok = "SetDiscoveryFilter success\nDiscovery started\n[\x1B[0;93mCHG\x1B[0m] Controller 00:1A:7D:DA:71:13 Discovering: yes\n[\x1B[0;92mNEW\x1B[0m] Device A0:AB:51:5F:23:1A DualSense Wireless Controller\n";
        assert_eq!(scan_error(ok), None);
        assert_eq!(scan_error("SetDiscoveryFilter success\nFailed to start discovery: org.bluez.Error.NotReady\n").as_deref(), Some("Failed to start discovery: org.bluez.Error.NotReady"));
        assert_eq!(scan_error("No default controller available\n").as_deref(), Some("No default controller available"));
        assert_eq!(scan_error("SetDiscoveryFilter failed: org.bluez.Error.NotReady\n").as_deref(), Some("SetDiscoveryFilter failed: org.bluez.Error.NotReady"));
    }

    fn ran(code: i32, stdout: &str) -> Ran {
        Ran { code: Some(code), stdout: stdout.into(), stderr: String::new() }
    }

    /// Run the pairing flow with these answers, one per call, and keep the calls and the steps.
    fn pair_with(answers: Vec<Result<Ran, String>>) -> (PairEnd, Vec<Vec<String>>, Vec<Step>) {
        let answers = RefCell::new(answers.into_iter());
        let calls = RefCell::new(Vec::new());
        let steps = RefCell::new(Vec::new());
        let end = pair_flow(
            &|call: &Call| {
                calls.borrow_mut().push(call.args.clone());
                answers.borrow_mut().next().expect("an answer for each call")
            },
            MAC,
            &|step| steps.borrow_mut().push(step),
        );
        (end, calls.into_inner(), steps.into_inner())
    }

    #[test]
    fn pairing_pairs_trusts_and_connects() {
        let (end, calls, steps) = pair_with(vec![
            Ok(ran(0, "Attempting to pair with A0:AB:51:5F:23:1A\nPairing successful\n")),
            Ok(ran(0, "Changing A0:AB:51:5F:23:1A trust succeeded\n")),
            Ok(ran(0, "Attempting to connect to A0:AB:51:5F:23:1A\nConnection successful\n")),
        ]);
        assert_eq!(end, PairEnd::Connected);
        assert_eq!(calls, [vec!["pair", MAC], vec!["trust", MAC], vec!["connect", MAC]]);
        assert_eq!(steps, [Step::Pair, Step::Trust, Step::Connect]);
    }

    #[test]
    fn a_device_paired_already_goes_on_to_trust() {
        let (end, calls, _) = pair_with(vec![
            Ok(ran(1, "Attempting to pair with A0:AB:51:5F:23:1A\nFailed to pair: org.bluez.Error.AlreadyExists\n")),
            Ok(ran(0, "Changing A0:AB:51:5F:23:1A trust succeeded\n")),
            Ok(ran(0, "Connection successful\n")),
        ]);
        assert_eq!((end, calls.len()), (PairEnd::Connected, 3));
    }

    #[test]
    fn a_failed_step_stops_the_flow_and_names_the_step() {
        let (end, calls, steps) = pair_with(vec![Ok(ran(1, "Attempting to pair with A0:AB:51:5F:23:1A\nFailed to pair: org.bluez.Error.AuthenticationFailed\n"))]);
        let PairEnd::Failed { step, error } = end else { panic!("{end:?}") };
        assert_eq!(step, Step::Pair);
        assert!(error.contains("org.bluez.Error.AuthenticationFailed"), "{error}");
        assert_eq!((calls.len(), steps), (1, vec![Step::Pair]));

        let (end, _, _) = pair_with(vec![
            Ok(ran(0, "Pairing successful\n")),
            Ok(ran(0, "Changing A0:AB:51:5F:23:1A trust succeeded\n")),
            Ok(ran(1, "Attempting to connect to A0:AB:51:5F:23:1A\nFailed to connect: org.bluez.Error.Failed br-connection-page-timeout\n")),
        ]);
        assert!(matches!(end, PairEnd::Failed { step: Step::Connect, .. }), "{end:?}");

        let (end, _, _) = pair_with(vec![Err("bluetoothctl pair A0:AB:51:5F:23:1A did not answer within 60 s".into())]);
        assert!(matches!(end, PairEnd::Failed { step: Step::Pair, ref error } if error.contains("did not answer")), "{end:?}");
    }

    #[test]
    fn a_device_bluetoothd_forgot_means_scan_again() {
        let (end, calls, _) = pair_with(vec![Ok(ran(1, "Device A0:AB:51:5F:23:1A not available\n"))]);
        assert_eq!((end, calls.len()), (PairEnd::Gone, 1));
    }

    #[test]
    fn a_bad_mac_pairs_nothing() {
        let end = pair_flow(&|_: &Call| panic!("no call for a bad MAC"), "a0:ab:51:5f:23:1a", &|_| {});
        assert!(matches!(end, PairEnd::Failed { step: Step::Pair, .. }), "{end:?}");
    }

    #[test]
    fn the_detail_is_bluetoothctls_last_line() {
        let failed = "bluetoothctl pair A0:AB:51:5F:23:1A failed (exit 1): Attempting to pair with A0:AB:51:5F:23:1A\nFailed to pair: org.bluez.Error.AuthenticationFailed";
        assert_eq!(detail(failed), "Failed to pair: org.bluez.Error.AuthenticationFailed");
        let slow = "bluetoothctl pair A0:AB:51:5F:23:1A did not answer within 60 s";
        assert_eq!(detail(slow), slow);
        assert_eq!(detail("one line\n\n"), "one line");
        assert_eq!(detail(""), "");
    }

    #[test]
    fn errors_in_plain_words() {
        assert_eq!(explain("Failed to pair: org.bluez.Error.AuthenticationFailed"), "The controller did not accept the pairing.");
        assert_eq!(explain("Failed to pair: org.bluez.Error.AuthenticationTimeout"), "The controller did not accept the pairing.");
        assert_eq!(explain("Failed to connect: org.bluez.Error.Failed br-connection-page-timeout"), "The controller did not answer. Is it still in pairing mode?");
        assert_eq!(explain("Failed to pair: org.bluez.Error.ConnectionAttemptFailed"), "The controller did not answer. Is it still in pairing mode?");
        assert_eq!(explain("Failed to pair: org.bluez.Error.InProgress"), "Bluetooth is busy with another device. Try again in a moment.");
        assert_eq!(explain("Failed to set power on: org.bluez.Error.Blocked"), "Bluetooth is blocked: check the PC's airplane-mode switch or key.");
        assert_eq!(explain("Failed to start discovery: org.bluez.Error.NotReady"), "Bluetooth is off.");
        assert_eq!(explain("bluetoothctl pair A0:AB:51:5F:23:1A did not answer within 60 s"), "Bluetooth did not answer in time.");
        assert_eq!(explain("something new"), "Bluetooth reported an error.");
    }
}
