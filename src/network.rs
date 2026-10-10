//! Network state from NetworkManager's `nmcli`, for the Network page and the Quick Menu (Phase 6
//! of docs/plans/ps5-launcher-os.md). Parsing and the command lines live here; `system::call`
//! runs them.

use crate::system::Call;

/// One line of `nmcli -t -f DEVICE,TYPE,STATE,CONNECTION device`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Device {
    pub name: String,
    /// "ethernet", "wifi", "loopback", ...
    pub kind: String,
    /// "connected", "disconnected", "unavailable", "connected (externally)", ...
    pub state: String,
    /// The active connection's name, if any.
    pub connection: Option<String>,
}

/// One line of `nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Wifi {
    pub in_use: bool,
    pub ssid: String,
    /// 0 to 100.
    pub signal: u8,
    /// Empty for an open network, otherwise for example "WPA2" or "WPA1 WPA2".
    pub security: String,
}

/// Split one terse nmcli line into its fields. nmcli escapes `:` and `\` with `\`.
fn fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.last_mut().unwrap().extend(chars.next()),
            ':' => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out
}

pub fn parse_devices(output: &str) -> Vec<Device> {
    output
        .lines()
        .filter_map(|line| match <[String; 4]>::try_from(fields(line)) {
            Ok([name, kind, state, connection]) if !name.is_empty() => Some(Device {
                name,
                kind,
                state,
                connection: (!connection.is_empty()).then_some(connection),
            }),
            _ => None,
        })
        .collect()
}

/// Wi-Fi networks, in nmcli's order. Hidden networks (no SSID) are left out.
pub fn parse_wifi(output: &str) -> Vec<Wifi> {
    output
        .lines()
        .filter_map(|line| match <[String; 4]>::try_from(fields(line)) {
            Ok([in_use, ssid, signal, security]) if !ssid.is_empty() => Some(Wifi {
                in_use: in_use == "*",
                ssid,
                signal: signal.parse().ok()?,
                security,
            }),
            _ => None,
        })
        .collect()
}

/// A saved connection, from `nmcli -t -f NAME,UUID,TYPE connection show`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Saved {
    pub name: String,
    pub uuid: String,
}

/// The saved Wi-Fi connections. nmcli names the type `802-11-wireless` in terse output.
pub fn parse_saved(output: &str) -> Vec<Saved> {
    output
        .lines()
        .filter_map(|line| match <[String; 3]>::try_from(fields(line)) {
            Ok([name, uuid, kind]) if !uuid.is_empty() && matches!(kind.as_str(), "802-11-wireless" | "wifi") => {
                Some(Saved { name, uuid })
            }
            _ => None,
        })
        .collect()
}

/// `nmcli radio wifi`: "enabled" or "disabled".
pub fn parse_radio(output: &str) -> Result<bool, String> {
    match output.trim() {
        "enabled" => Ok(true),
        "disabled" => Ok(false),
        other => Err(format!("unexpected answer from nmcli radio wifi: {other:?}")),
    }
}

/// An open network has no security; nmcli prints nothing or "--" for it.
pub fn secured(wifi: &Wifi) -> bool {
    !matches!(wifi.security.trim(), "" | "--")
}

/// Signal bars, 1 to 4.
pub fn bars(signal: u8) -> i32 {
    match signal {
        0..=24 => 1,
        25..=49 => 2,
        50..=74 => 3,
        _ => 4,
    }
}

/// The wired status for the Network page, or None when the PC has no Ethernet port.
pub fn wired(devices: &[Device]) -> Option<String> {
    let ports: Vec<&Device> = devices.iter().filter(|d| d.kind == "ethernet").collect();
    let first = ports.first()?;
    if ports.iter().any(|d| d.state == "connected") {
        return Some("Connected".into());
    }
    Some(match first.state.as_str() {
        "unavailable" => "No cable".into(),
        s if s.starts_with("connecting") => "Connecting…".into(),
        _ => "Not connected".into(),
    })
}

/// The PC has a Wi-Fi device (it may be switched off).
pub fn has_wifi(devices: &[Device]) -> bool {
    devices.iter().any(|d| d.kind == "wifi")
}

/// What choosing a network on the Network page does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Join {
    /// It is the one in use.
    Connected,
    /// A saved connection (by UUID) has its password.
    Saved(String),
    /// Secured and not saved: ask for the password first.
    AskPassword,
    Open,
}

pub fn join(wifi: &Wifi, saved: &[Saved]) -> Join {
    if wifi.in_use {
        Join::Connected
    } else if let Some(s) = saved.iter().find(|s| s.name == wifi.ssid) {
        Join::Saved(s.uuid.clone())
    } else if secured(wifi) {
        Join::AskPassword
    } else {
        Join::Open
    }
}

pub fn devices_call() -> Call {
    Call::new("nmcli", &["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"], 10)
}

/// The networks. With `rescan`, nmcli scans first and waits for it (a few seconds).
pub fn wifi_call(rescan: bool) -> Call {
    let rescan = if rescan { "yes" } else { "no" };
    Call::new("nmcli", &["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list", "--rescan", rescan], 30)
}

pub fn saved_call() -> Call {
    Call::new("nmcli", &["-t", "-f", "NAME,UUID,TYPE", "connection", "show"], 10)
}

pub fn radio_call() -> Call {
    Call::new("nmcli", &["radio", "wifi"], 10)
}

pub fn set_radio_call(on: bool) -> Call {
    Call::new("nmcli", &["radio", "wifi", if on { "on" } else { "off" }], 10)
}

/// Join a network. The SSID and the password are arguments of their own: no shell sees them.
/// Joining waits for the address (DHCP), so it gets a minute.
pub fn connect_call(ssid: &str, password: Option<&str>) -> Call {
    let mut call = Call::new("nmcli", &["device", "wifi", "connect"], 60);
    call.args.push(ssid.to_string());
    if let Some(password) = password {
        call.args.extend(["password".to_string(), password.to_string()]);
    }
    call
}

/// Join a network that has a saved connection, with its saved password.
pub fn up_call(uuid: &str) -> Call {
    Call::new("nmcli", &["connection", "up", "uuid", uuid], 60)
}

/// Forget a saved network: delete its connection.
pub fn forget_call(uuid: &str) -> Call {
    Call::new("nmcli", &["connection", "delete", "uuid", uuid], 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured on Fedora 44 (PS5 Launcher OS spike, VM boot test).
    const DEVICES: &str = "ens2:ethernet:connected:Wired connection 1\nlo:loopback:connected (externally):lo\n";

    #[test]
    fn devices_from_real_output() {
        assert_eq!(
            parse_devices(DEVICES),
            vec![
                Device {
                    name: "ens2".into(),
                    kind: "ethernet".into(),
                    state: "connected".into(),
                    connection: Some("Wired connection 1".into()),
                },
                Device {
                    name: "lo".into(),
                    kind: "loopback".into(),
                    state: "connected (externally)".into(),
                    connection: Some("lo".into()),
                },
            ]
        );
    }

    #[test]
    fn a_device_without_a_connection() {
        let out = "wlp3s0:wifi:disconnected:\n";
        assert_eq!(parse_devices(out)[0].connection, None);
        assert_eq!(parse_devices(out)[0].state, "disconnected");
    }

    #[test]
    fn escaped_colons_and_backslashes_stay_in_the_name() {
        assert_eq!(fields(r"*:Cafe\: Lumiere:72:WPA2"), ["*", "Cafe: Lumiere", "72", "WPA2"]);
        assert_eq!(fields(r" :back\\slash:40:"), [" ", r"back\slash", "40", ""]);
    }

    #[test]
    fn wifi_networks() {
        let out = "*:Home-5G:88:WPA2\n :Cafe\\: Lumiere:72:\n :Office:40:WPA1 WPA2\n";
        assert_eq!(
            parse_wifi(out),
            vec![
                Wifi { in_use: true, ssid: "Home-5G".into(), signal: 88, security: "WPA2".into() },
                Wifi { in_use: false, ssid: "Cafe: Lumiere".into(), signal: 72, security: "".into() },
                Wifi { in_use: false, ssid: "Office".into(), signal: 40, security: "WPA1 WPA2".into() },
            ]
        );
    }

    #[test]
    fn broken_lines_are_skipped() {
        // The VM has no Wi-Fi: nmcli prints nothing. A hidden network has an empty SSID.
        assert!(parse_wifi("").is_empty());
        assert!(parse_wifi(" ::30:WPA2\nnot nmcli output\n").is_empty());
        assert!(parse_devices("only:two\n").is_empty());
    }

    #[test]
    fn saved_wifi_connections() {
        let out = "Home-5G:0b3c9a52-1111-4a6e-9f60-6d0d8f1c2a01:802-11-wireless\nWired connection 1:7f2e:802-3-ethernet\nCafe\\: Lumiere:9d1a:wifi\nlo:2c7b:loopback\n";
        assert_eq!(
            parse_saved(out),
            vec![
                Saved { name: "Home-5G".into(), uuid: "0b3c9a52-1111-4a6e-9f60-6d0d8f1c2a01".into() },
                Saved { name: "Cafe: Lumiere".into(), uuid: "9d1a".into() },
            ]
        );
        assert!(parse_saved("").is_empty());
    }

    #[test]
    fn the_wifi_radio() {
        assert_eq!(parse_radio("enabled\n"), Ok(true));
        assert_eq!(parse_radio("disabled\n"), Ok(false));
        assert!(parse_radio("Error: NetworkManager is not running.").is_err());
    }

    #[test]
    fn open_and_secured_networks() {
        let wifi = |security: &str| Wifi { in_use: false, ssid: "x".into(), signal: 50, security: security.into() };
        assert!(!secured(&wifi("")));
        assert!(!secured(&wifi("--")));
        assert!(secured(&wifi("WPA2")));
        assert!(secured(&wifi("WPA1 WPA2")));
    }

    #[test]
    fn choosing_a_network() {
        let wifi = |in_use, ssid: &str, security: &str| Wifi { in_use, ssid: ssid.into(), signal: 60, security: security.into() };
        let saved = [Saved { name: "Home-5G".into(), uuid: "0b3c".into() }];
        assert_eq!(join(&wifi(true, "Home-5G", "WPA2"), &saved), Join::Connected);
        assert_eq!(join(&wifi(false, "Home-5G", "WPA2"), &saved), Join::Saved("0b3c".into()));
        assert_eq!(join(&wifi(false, "Office", "WPA2"), &saved), Join::AskPassword);
        assert_eq!(join(&wifi(false, "Cafe", ""), &saved), Join::Open);
    }

    #[test]
    fn signal_bars() {
        assert_eq!([0, 24, 25, 49, 50, 74, 75, 100].map(bars), [1, 1, 2, 2, 3, 3, 4, 4]);
    }

    #[test]
    fn the_wired_status_from_real_output() {
        assert_eq!(wired(&parse_devices(DEVICES)), Some("Connected".into()));
        assert!(!has_wifi(&parse_devices(DEVICES)));
    }

    #[test]
    fn the_wired_status() {
        let dev = |kind: &str, state: &str| Device { name: "d".into(), kind: kind.into(), state: state.into(), connection: None };
        assert_eq!(wired(&[dev("wifi", "connected")]), None, "no Ethernet port");
        assert_eq!(wired(&[dev("ethernet", "unavailable")]), Some("No cable".into()));
        assert_eq!(wired(&[dev("ethernet", "disconnected")]), Some("Not connected".into()));
        assert_eq!(wired(&[dev("ethernet", "connecting (getting IP configuration)")]), Some("Connecting…".into()));
        assert_eq!(wired(&[dev("ethernet", "unavailable"), dev("ethernet", "connected")]), Some("Connected".into()));
        assert!(has_wifi(&[dev("wifi", "unavailable")]));
    }

    #[test]
    fn the_ssid_and_the_password_are_arguments_of_their_own() {
        let call = connect_call("Cafe; rm -rf ~", Some("p@ss word'\""));
        assert_eq!(call.program, "nmcli");
        assert_eq!(call.args, ["device", "wifi", "connect", "Cafe; rm -rf ~", "password", "p@ss word'\""]);
        assert_eq!(connect_call("Open Cafe", None).args, ["device", "wifi", "connect", "Open Cafe"]);
        assert!(connect_call("x", None).secs >= 30, "joining waits for DHCP");
    }

    #[test]
    fn command_lines() {
        assert_eq!(set_radio_call(true).args, ["radio", "wifi", "on"]);
        assert_eq!(set_radio_call(false).args, ["radio", "wifi", "off"]);
        assert_eq!(forget_call("9d1a").args, ["connection", "delete", "uuid", "9d1a"]);
        assert_eq!(up_call("9d1a").args, ["connection", "up", "uuid", "9d1a"]);
        assert_eq!(wifi_call(true).args[5..], ["list", "--rescan", "yes"]);
        assert_eq!(wifi_call(false).args[5..], ["list", "--rescan", "no"]);
        assert_eq!(devices_call().args, ["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"]);
    }
}
