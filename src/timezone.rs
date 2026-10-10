//! System → Time: the time zone and "Set the time automatically" (NTP), through timedatectl
//! (Phase 6 of docs/plans/ps5-launcher-os.md). Parsing and the command lines live here;
//! `system::call` runs them.
//!
//! Fedora 44's systemd (259) gives timedated's set-timezone and set-ntp `auth_admin_keep` for an
//! active local user (/usr/share/polkit-1/actions/org.freedesktop.timedate1.policy), and
//! the OS user is not an admin. So in PS5 Launcher OS both go through the root helper. In a
//! session on another Linux PC there is no helper: timedatectl runs as the user, without a
//! password prompt, and fails when that PC's polkit wants one.
//!
//! The time zone is not detected from the network: that calls an outside service, and the plan
//! says to ask first (open question 6), which is not decided yet.

use crate::osupdate::HELPER;
use crate::system::Call;

/// What timedatectl says about the clock.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Clock {
    pub zone: String,
    /// The time is set automatically.
    pub ntp: bool,
    /// An NTP service is installed, so the switch can work.
    pub can_ntp: bool,
}

/// `timedatectl show` with the three properties the page needs.
pub fn show_call() -> Call {
    Call::new("timedatectl", &["show", "-p", "Timezone", "-p", "NTP", "-p", "CanNTP"], 10)
}

pub fn parse_show(output: &str) -> Result<Clock, String> {
    let mut clock = Clock::default();
    for line in output.lines() {
        match line.trim().split_once('=') {
            Some(("Timezone", zone)) => clock.zone = zone.to_string(),
            Some(("NTP", v)) => clock.ntp = v == "yes",
            Some(("CanNTP", v)) => clock.can_ntp = v == "yes",
            _ => {}
        }
    }
    if clock.zone.is_empty() {
        return Err(format!("timedatectl gave no time zone: {:?}", output.trim()));
    }
    Ok(clock)
}

pub fn list_call() -> Call {
    Call::new("timedatectl", &["list-timezones"], 10)
}

/// The zones timedatectl lists, without anything that is not a valid zone name.
pub fn parse_list(output: &str) -> Vec<String> {
    output.lines().map(str::trim).filter(|z| valid_zone(z)).map(String::from).collect()
}

/// A zone name as the root helper accepts it: one to three parts of letters, digits, `_`, `+`
/// and `-`, split by `/`. No dot, so no `..`; no leading `/`.
pub fn valid_zone(zone: &str) -> bool {
    let parts: Vec<&str> = zone.split('/').collect();
    let part_ok = |p: &&str| !p.is_empty() && !p.starts_with('-') && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"_+-".contains(&b));
    (1..=3).contains(&parts.len()) && parts.iter().all(part_ok)
}

/// The zones grouped by region ("Europe", "America", …) in alphabetical order. A zone without a
/// region ("UTC") goes to "Other", last.
pub fn regions(zones: &[String]) -> Vec<(String, Vec<String>)> {
    let mut groups: std::collections::BTreeMap<(bool, String), Vec<String>> = std::collections::BTreeMap::new();
    for zone in zones {
        let key = match zone.split_once('/') {
            Some((region, _)) => (false, region.to_string()),
            None => (true, "Other".to_string()),
        };
        groups.entry(key).or_default().push(zone.clone());
    }
    groups
        .into_iter()
        .map(|((_, region), mut zones)| {
            zones.sort();
            (region, zones)
        })
        .collect()
}

/// The name a zone shows under its region: "America/Argentina/Buenos_Aires" → "Argentina /
/// Buenos Aires".
pub fn city(zone: &str) -> String {
    let place = zone.split_once('/').map_or(zone, |(_, rest)| rest);
    place.replace('_', " ").replace('/', " / ")
}

/// Set the zone: through the root helper in the OS (`os`), timedatectl itself elsewhere.
pub fn set_zone_call(os: bool, zone: &str) -> Call {
    if os {
        Call::new("pkexec", &[HELPER, "set-timezone", zone], HELPER_SECS)
    } else {
        Call::new("timedatectl", &["--no-ask-password", "set-timezone", zone], 15)
    }
}

/// The helper's own deadline for set-timezone and set-ntp ($short), plus the launcher's margin.
const HELPER_SECS: u32 = 30 + 30;

/// Turn automatic time on or off, the same way.
pub fn set_ntp_call(os: bool, on: bool) -> Call {
    if os {
        Call::new("pkexec", &[HELPER, "set-ntp", if on { "on" } else { "off" }], HELPER_SECS)
    } else {
        Call::new("timedatectl", &["--no-ask-password", "set-ntp", if on { "true" } else { "false" }], 15)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_from_timedatectl() {
        let out = "Timezone=Europe/Berlin\nNTP=yes\nCanNTP=yes\n";
        assert_eq!(parse_show(out), Ok(Clock { zone: "Europe/Berlin".into(), ntp: true, can_ntp: true }));
        let off = "Timezone=UTC\nNTP=no\nCanNTP=no\n";
        assert_eq!(parse_show(off), Ok(Clock { zone: "UTC".into(), ntp: false, can_ntp: false }));
        assert!(parse_show("").is_err(), "no time zone");
        assert!(parse_show("NTP=yes\n").is_err());
    }

    #[test]
    fn the_zone_list_keeps_only_valid_names() {
        let out = "Africa/Abidjan\nAmerica/Argentina/Buenos_Aires\nUTC\n\n../etc/passwd\nEtc/GMT+5\n";
        assert_eq!(parse_list(out), ["Africa/Abidjan", "America/Argentina/Buenos_Aires", "UTC", "Etc/GMT+5"]);
    }

    #[test]
    fn zone_names_without_path_tricks() {
        for ok in ["Europe/Berlin", "America/Argentina/Buenos_Aires", "UTC", "Etc/GMT-14", "America/Port-au-Prince"] {
            assert!(valid_zone(ok), "{ok}");
        }
        for bad in ["", "/etc/passwd", "../../etc/shadow", "Europe/../../x", "Europe/Berlin/", "Europe//Berlin", "a/b/c/d", "Europe/Berlin\n", "Europe Berlin", "-rf", ".hidden", "Europe/Ber.lin"] {
            assert!(!valid_zone(bad), "{bad:?}");
        }
    }

    #[test]
    fn zones_grouped_by_region() {
        let zones: Vec<String> = ["Europe/Paris", "America/New_York", "UTC", "Europe/Berlin", "America/Argentina/Buenos_Aires"].map(String::from).to_vec();
        let groups = regions(&zones);
        let names: Vec<&str> = groups.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(names, ["America", "Europe", "Other"]);
        assert_eq!(groups[0].1, ["America/Argentina/Buenos_Aires", "America/New_York"]);
        assert_eq!(groups[1].1, ["Europe/Berlin", "Europe/Paris"]);
        assert_eq!(groups[2].1, ["UTC"]);
        assert_eq!(city("America/Argentina/Buenos_Aires"), "Argentina / Buenos Aires");
        assert_eq!(city("Europe/Berlin"), "Berlin");
        assert_eq!(city("UTC"), "UTC");
    }

    #[test]
    fn the_command_lines() {
        assert_eq!(show_call().program, "timedatectl");
        assert_eq!(show_call().args, ["show", "-p", "Timezone", "-p", "NTP", "-p", "CanNTP"]);
        assert_eq!(list_call().args, ["list-timezones"]);
        let os = set_zone_call(true, "Europe/Berlin");
        assert_eq!((os.program, os.args.as_slice()), ("pkexec", &[HELPER.to_string(), "set-timezone".into(), "Europe/Berlin".into()][..]));
        assert!(os.secs > 30, "longer than the helper's own deadline");
        let user = set_zone_call(false, "Europe/Berlin");
        assert_eq!((user.program, user.args.as_slice()), ("timedatectl", &["--no-ask-password".to_string(), "set-timezone".into(), "Europe/Berlin".into()][..]));
        assert_eq!(set_ntp_call(true, true).args, [HELPER, "set-ntp", "on"]);
        assert_eq!(set_ntp_call(true, false).args, [HELPER, "set-ntp", "off"]);
        assert_eq!(set_ntp_call(false, true).args, ["--no-ask-password", "set-ntp", "true"]);
        assert_eq!(set_ntp_call(false, false).args, ["--no-ask-password", "set-ntp", "false"]);
    }
}
