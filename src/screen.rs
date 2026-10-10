//! The screen's output for the next session start (System → Display): the resolution and the
//! refresh rate gamescope uses, or "Automatic". The launcher writes the choice to
//! `~/.config/ps5-launcher/session.conf`, and packaging/linux/ps5-launcher-session passes it to
//! gamescope as `-W -H -r` when the session starts. gamescope keeps running while the launcher
//! restarts, so a change applies only at the next session start.
//!
//! The resolutions come from the connector's `modes` file. That file has no refresh rates (the
//! kernel prints only each mode's name, once for each rate), so the rates come from the
//! detailed timings in the connector's EDID. A rate the EDID lists only as a CTA video code (most
//! TV modes) does not show; "Automatic" lets gamescope pick.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The output the session starts with. `refresh` None: gamescope picks the rate.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Output {
    pub width: u32,
    pub height: u32,
    pub refresh: Option<u32>,
}

/// The sizes and rates the session wrapper accepts; anything else is ignored there too.
const SIZE: std::ops::RangeInclusive<u32> = 320..=16384;
const RATE: std::ops::RangeInclusive<u32> = 1..=1000;

impl Output {
    pub fn valid(&self) -> bool {
        SIZE.contains(&self.width) && SIZE.contains(&self.height) && self.refresh.is_none_or(|r| RATE.contains(&r))
    }

    /// "2560 × 1440" or "2560 × 1440 · 144 Hz".
    pub fn label(&self) -> String {
        match self.refresh {
            Some(r) => format!("{} × {} · {r} Hz", self.width, self.height),
            None => format!("{} × {}", self.width, self.height),
        }
    }
}

/// The resolutions of a connector's `modes` file, without repeats and interlaced modes, largest
/// first.
pub fn parse_modes(text: &str) -> Vec<(u32, u32)> {
    let mut modes: Vec<(u32, u32)> = text
        .lines()
        .filter_map(|line| {
            let (w, h) = line.trim().split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .collect();
    modes.sort_by_key(|&(w, h)| std::cmp::Reverse((u64::from(w) * u64::from(h), w)));
    modes.dedup();
    modes
}

/// The refresh rates (rounded to whole hertz) the EDID's detailed timings give for `width` ×
/// `height`, highest first. The base block and CTA-861 extension blocks are read; a block with a
/// wrong checksum is skipped.
pub fn edid_rates(edid: &[u8], width: u32, height: u32) -> Vec<u32> {
    let mut rates = Vec::new();
    for (i, block) in edid.chunks(128).enumerate() {
        if block.len() < 128 || block.iter().fold(0u8, |a, b| a.wrapping_add(*b)) != 0 {
            continue;
        }
        // The base block's four descriptors start at byte 54; a CTA-861 block's timings start
        // at the offset in its byte 2 and end before the checksum.
        let (start, end) = match (i, block[0]) {
            (0, _) => (54, 126),
            (_, 0x02) if (4..=127).contains(&block[2]) => (usize::from(block[2]), 127),
            _ => continue,
        };
        for d in block[start..end].chunks_exact(18) {
            if let Some((w, h, hz)) = timing(d) {
                if (w, h) == (width, height) && !rates.contains(&hz) {
                    rates.push(hz);
                }
            }
        }
    }
    rates.sort_unstable_by(|a, b| b.cmp(a));
    rates
}

/// An 18-byte detailed timing descriptor: width, height and refresh rate. None for a display
/// descriptor (pixel clock 0) and for an interlaced timing.
fn timing(d: &[u8]) -> Option<(u32, u32, u32)> {
    let clock = u64::from(u16::from_le_bytes([d[0], d[1]])) * 10_000;
    if clock == 0 || d[17] & 0x80 != 0 {
        return None;
    }
    let hactive = u32::from(d[2]) | (u32::from(d[4] >> 4) << 8);
    let hblank = u32::from(d[3]) | (u32::from(d[4] & 0x0f) << 8);
    let vactive = u32::from(d[5]) | (u32::from(d[7] >> 4) << 8);
    let vblank = u32::from(d[6]) | (u32::from(d[7] & 0x0f) << 8);
    let total = u64::from(hactive + hblank) * u64::from(vactive + vblank);
    if total == 0 {
        return None;
    }
    let hz = (clock + total / 2) / total;
    Some((hactive, vactive, u32::try_from(hz).ok()?))
}

/// The session.conf text for `output`.
pub fn session_conf(output: &Output) -> String {
    let refresh = output.refresh.map(|r| r.to_string()).unwrap_or_default();
    format!(
        "# Written by PS5 Launcher (Settings → Display). Read at the next session start.\nWIDTH={}\nHEIGHT={}\nREFRESH={refresh}\n",
        output.width, output.height
    )
}

/// `~/.config/ps5-launcher/session.conf`.
pub fn session_conf_path() -> PathBuf {
    crate::util::config_dir().join("session.conf")
}

/// Write `output` for the next session start, or remove the file for "Automatic".
pub fn save(path: &Path, output: Option<&Output>) -> std::io::Result<()> {
    match output {
        Some(out) if !out.valid() => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("not a valid output: {}", out.label()))),
        Some(out) => crate::util::atomic_write(path, session_conf(out).as_bytes()),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_without_repeats_or_interlaced_largest_first() {
        let modes = "1920x1080\n1920x1080\n3840x2160\n1920x1080i\n1280x720\n2560x1440\n1280x720\n\nnonsense\n640x480\n";
        assert_eq!(parse_modes(modes), [(3840, 2160), (2560, 1440), (1920, 1080), (1280, 720), (640, 480)]);
        assert_eq!(parse_modes(""), []);
    }

    /// An 18-byte detailed timing: the pixel clock in 10 kHz, the active and blanking sizes.
    fn dtd(clock_10khz: u16, ha: u16, hb: u16, va: u16, vb: u16, interlaced: bool) -> [u8; 18] {
        let mut d = [0u8; 18];
        d[0..2].copy_from_slice(&clock_10khz.to_le_bytes());
        d[2] = ha as u8;
        d[3] = hb as u8;
        d[4] = (((ha >> 8) as u8) << 4) | ((hb >> 8) as u8);
        d[5] = va as u8;
        d[6] = vb as u8;
        d[7] = (((va >> 8) as u8) << 4) | ((vb >> 8) as u8);
        if interlaced {
            d[17] = 0x80;
        }
        d
    }

    fn with_checksum(mut block: [u8; 128]) -> [u8; 128] {
        let sum: u8 = block[..127].iter().fold(0u8, |a, b| a.wrapping_add(*b));
        block[127] = 0u8.wrapping_sub(sum);
        block
    }

    /// A base block with two timings, and a CTA extension with one more.
    fn edid() -> Vec<u8> {
        let mut base = [0u8; 128];
        base[0..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
        base[126] = 1; // one extension
        // 2560x1440 at 144 Hz (CVT-RB: 2720 x 1481 total, 580.0 MHz) and at 60 Hz (241.5 MHz).
        base[54..72].copy_from_slice(&dtd(58000, 2560, 160, 1440, 41, false));
        base[72..90].copy_from_slice(&dtd(24150, 2560, 160, 1440, 41, false));
        // A display descriptor (pixel clock 0): not a timing.
        base[90..108].copy_from_slice(&[0, 0, 0, 0xfc, 0, b'S', b'C', b'R', b'E', b'E', b'N', b'\n', b' ', b' ', b' ', b' ', b' ', b' ']);
        let mut cta = [0u8; 128];
        cta[0] = 0x02;
        cta[1] = 3;
        cta[2] = 4; // timings start at byte 4
        // 1920x1080 at 60 Hz (148.5 MHz, 2200 x 1125 total), and an interlaced 1920x1080.
        cta[4..22].copy_from_slice(&dtd(14850, 1920, 280, 1080, 45, false));
        cta[22..40].copy_from_slice(&dtd(7425, 1920, 280, 540, 22, true));
        [with_checksum(base).to_vec(), with_checksum(cta).to_vec()].concat()
    }

    #[test]
    fn refresh_rates_from_the_edid_timings() {
        let edid = edid();
        assert_eq!(edid_rates(&edid, 2560, 1440), [144, 60]);
        assert_eq!(edid_rates(&edid, 1920, 1080), [60], "from the CTA block; not the interlaced one");
        assert_eq!(edid_rates(&edid, 1280, 720), Vec::<u32>::new());
        assert_eq!(edid_rates(&[], 1920, 1080), Vec::<u32>::new(), "no EDID");
        assert_eq!(edid_rates(&edid[..100], 2560, 1440), Vec::<u32>::new(), "cut short");
        let mut broken = edid.clone();
        broken[130] ^= 0xff;
        assert_eq!(edid_rates(&broken, 1920, 1080), Vec::<u32>::new(), "a block with a wrong checksum");
        assert_eq!(edid_rates(&broken, 2560, 1440), [144, 60], "the base block still counts");
    }

    #[test]
    fn an_output_is_valid_only_within_limits() {
        assert!(Output { width: 1920, height: 1080, refresh: Some(60) }.valid());
        assert!(Output { width: 1920, height: 1080, refresh: None }.valid());
        assert!(!Output { width: 0, height: 1080, refresh: None }.valid());
        assert!(!Output { width: 1920, height: 99999, refresh: None }.valid());
        assert!(!Output { width: 1920, height: 1080, refresh: Some(0) }.valid());
        assert_eq!(Output { width: 2560, height: 1440, refresh: Some(144) }.label(), "2560 × 1440 · 144 Hz");
        assert_eq!(Output { width: 2560, height: 1440, refresh: None }.label(), "2560 × 1440");
    }

    #[test]
    fn the_session_file() {
        let out = Output { width: 2560, height: 1440, refresh: Some(144) };
        assert_eq!(session_conf(&out), "# Written by PS5 Launcher (Settings → Display). Read at the next session start.\nWIDTH=2560\nHEIGHT=1440\nREFRESH=144\n");
        let auto_rate = Output { refresh: None, ..out };
        assert!(session_conf(&auto_rate).ends_with("HEIGHT=1440\nREFRESH=\n"));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ps5-launcher/session.conf");
        save(&path, Some(&out)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), session_conf(&out));
        save(&path, None).unwrap();
        assert!(!path.exists(), "Automatic removes it");
        save(&path, None).unwrap();
        assert!(save(&path, Some(&Output { width: 1, height: 1, refresh: None })).is_err(), "never writes a bad size");
    }
}
