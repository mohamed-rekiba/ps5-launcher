//! Network state from NetworkManager's `nmcli`, for the Network page and the Quick Menu (Phase 6
//! of docs/plans/ps5-launcher-os.md). Only parsing lives here; `system::run_output` runs nmcli.
#![allow(dead_code)] // nothing calls it until the Network page (Phase 6)

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
}
