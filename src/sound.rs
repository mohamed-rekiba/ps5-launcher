//! Sound outputs from PipeWire's `wpctl status`, for the Sound page and the Quick Menu (Phase 6 of
//! docs/plans/ps5-launcher-os.md). Parsing and the command lines live here; `system::call` runs
//! them. The launcher's own interface sounds are in audio.rs.

use crate::system::Call;

/// An audio output ("sink"): speakers, HDMI, headphones.
#[derive(Clone, PartialEq, Debug)]
pub struct Output {
    /// PipeWire's id, for `wpctl set-default` and `wpctl set-volume`.
    pub id: u32,
    pub name: String,
    /// 1.0 is 100%.
    pub volume: f32,
    pub muted: bool,
    /// The output sound goes to now.
    pub default: bool,
}

/// The outputs listed under Audio → Sinks.
pub fn parse_outputs(status: &str) -> Vec<Output> {
    let (mut in_audio, mut in_sinks) = (false, false);
    let mut outputs = Vec::new();
    for line in status.lines() {
        if !line.starts_with(' ') {
            // A top-level section: "Audio", "Video", "Settings", ...
            in_audio = line.trim() == "Audio";
            in_sinks = false;
        } else if line.contains("├─") || line.contains("└─") {
            in_sinks = in_audio && line.contains("Sinks:");
        } else if in_sinks {
            outputs.extend(parse_sink(line));
        }
    }
    outputs
}

/// " │  *   58. LG TV (HDMI)        [vol: 0.65]" (the * marks the default output).
fn parse_sink(line: &str) -> Option<Output> {
    let rest = line.trim_start_matches([' ', '│']);
    let (default, rest) = match rest.strip_prefix('*') {
        Some(after) => (true, after.trim_start()),
        None => (false, rest),
    };
    let (id, rest) = rest.split_once(". ")?;
    let (name, props) = rest.rsplit_once('[')?;
    let props = props.strip_suffix(']')?.strip_prefix("vol: ")?;
    let mut words = props.split_whitespace();
    Some(Output {
        id: id.trim().parse().ok()?,
        name: name.trim().to_string(),
        volume: words.next()?.parse().ok()?,
        muted: words.any(|w| w == "MUTED"),
        default,
    })
}

/// Volume steps from 0 to 100%: 5% each.
const STEPS: i32 = 20;

/// The volume after one step up (`dir` > 0) or down: on the 5% grid, from 0 to 100%. A volume
/// above 100% (set elsewhere) steps down from 100%.
pub fn next_volume(volume: f32, dir: i32) -> f32 {
    let step = (volume.clamp(0.0, 1.0) * STEPS as f32).round() as i32 + dir.signum();
    step.clamp(0, STEPS) as f32 / STEPS as f32
}

/// "65%".
pub fn percent(volume: f32) -> String {
    format!("{}%", (volume * 100.0).round() as i32)
}

/// The output sound goes to now.
pub fn default_output(outputs: &[Output]) -> Option<&Output> {
    outputs.iter().find(|o| o.default)
}

pub fn status_call() -> Call {
    Call::new("wpctl", &["status"], 10)
}

pub fn set_default_call(id: u32) -> Call {
    Call::new("wpctl", &["set-default", &id.to_string()], 10)
}

/// Set the volume to `volume` (0.0 to 1.0), as `next_volume` computes it.
pub fn set_volume_call(id: u32, volume: f32) -> Call {
    Call::new("wpctl", &["set-volume", &id.to_string(), &format!("{:.2}", volume.clamp(0.0, 1.0))], 10)
}

/// Mute or unmute: the state the player chose, never a toggle, so a second press cannot undo
/// the first.
pub fn set_mute_call(id: u32, on: bool) -> Call {
    Call::new("wpctl", &["set-mute", &id.to_string(), if on { "1" } else { "0" }], 10)
}

/// A change to the sound settings, by output id.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Write {
    Volume(u32, f32),
    Mute(u32, bool),
    Default(u32),
}

impl Write {
    pub fn call(self) -> Call {
        match self {
            Write::Volume(id, volume) => set_volume_call(id, volume),
            Write::Mute(id, on) => set_mute_call(id, on),
            Write::Default(id) => set_default_call(id),
        }
    }
}

/// The sound writes, one at a time and in order: two `wpctl` calls that run at once can finish
/// in either order, and the older volume would win. A volume change that waits is replaced by a
/// newer one for the same output, so only the latest value is sent.
#[derive(Default)]
pub struct Writes {
    running: bool,
    waiting: std::collections::VecDeque<Write>,
}

impl Writes {
    /// Add `write`. Returns the write to start now: `write` itself when none runs.
    pub fn push(&mut self, write: Write) -> Option<Write> {
        let waiting_volume = self.waiting.iter_mut().find_map(|w| match (w, write) {
            (Write::Volume(id, volume), Write::Volume(new_id, new_volume)) if *id == new_id => Some((volume, new_volume)),
            _ => None,
        });
        match waiting_volume {
            Some((volume, new_volume)) => *volume = new_volume,
            None => self.waiting.push_back(write),
        }
        if self.running { None } else { self.done() }
    }

    /// The running write ended. Returns the next write to start, if one waits.
    pub fn done(&mut self) -> Option<Write> {
        let next = self.waiting.pop_front();
        self.running = next.is_some();
        next
    }

    /// No write runs or waits.
    pub fn idle(&self) -> bool {
        !self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_steps_by_five_percent() {
        assert_eq!(percent(next_volume(0.65, 1)), "70%");
        assert_eq!(percent(next_volume(0.65, -1)), "60%");
        // An odd volume set elsewhere lands on the grid.
        assert_eq!(percent(next_volume(0.42, 1)), "45%");
        assert_eq!(percent(next_volume(0.43, -1)), "40%");
    }

    #[test]
    fn volume_stays_between_zero_and_a_hundred_percent() {
        assert_eq!(next_volume(1.0, 1), 1.0);
        assert_eq!(next_volume(0.98, 1), 1.0);
        assert_eq!(next_volume(0.0, -1), 0.0);
        assert_eq!(percent(next_volume(1.5, -1)), "95%");
        assert_eq!(next_volume(1.5, 1), 1.0);
    }

    #[test]
    fn volume_command_lines() {
        assert_eq!(set_volume_call(58, 0.7).args, ["set-volume", "58", "0.70"]);
        assert_eq!(set_volume_call(58, 1.4).args, ["set-volume", "58", "1.00"]);
        assert_eq!(set_default_call(51).args, ["set-default", "51"]);
        assert_eq!(set_mute_call(51, true).args, ["set-mute", "51", "1"]);
        assert_eq!(set_mute_call(51, false).args, ["set-mute", "51", "0"]);
        assert_eq!(status_call().program, "wpctl");
    }

    #[test]
    fn writes_run_one_at_a_time_in_order() {
        let mut w = Writes::default();
        assert_eq!(w.push(Write::Default(51)), Some(Write::Default(51)), "nothing runs: start it");
        assert_eq!(w.push(Write::Mute(51, true)), None, "one runs: it waits");
        assert_eq!(w.push(Write::Volume(51, 0.5)), None);
        assert!(!w.idle());
        assert_eq!(w.done(), Some(Write::Mute(51, true)));
        assert_eq!(w.done(), Some(Write::Volume(51, 0.5)));
        assert_eq!(w.done(), None);
        assert!(w.idle());
        assert_eq!(w.push(Write::Mute(51, false)), Some(Write::Mute(51, false)), "idle again: it starts");
    }

    #[test]
    fn waiting_volume_changes_keep_only_the_latest() {
        let mut w = Writes::default();
        assert_eq!(w.push(Write::Volume(58, 0.65)), Some(Write::Volume(58, 0.65)));
        assert_eq!(w.push(Write::Volume(58, 0.70)), None);
        assert_eq!(w.push(Write::Mute(58, true)), None);
        assert_eq!(w.push(Write::Volume(58, 0.75)), None);
        assert_eq!(w.push(Write::Volume(58, 0.80)), None);
        // Another output's volume is a write of its own.
        assert_eq!(w.push(Write::Volume(51, 0.20)), None);
        assert_eq!(w.done(), Some(Write::Volume(58, 0.80)));
        assert_eq!(w.done(), Some(Write::Mute(58, true)));
        assert_eq!(w.done(), Some(Write::Volume(51, 0.20)));
        assert_eq!(w.done(), None);
    }

    #[test]
    fn each_write_has_its_command_line() {
        assert_eq!(Write::Volume(58, 0.7).call(), set_volume_call(58, 0.7));
        assert_eq!(Write::Mute(58, true).call(), set_mute_call(58, true));
        assert_eq!(Write::Default(51).call(), set_default_call(51));
    }

    #[test]
    fn the_default_output() {
        let outputs = parse_outputs(include_str!("testdata/fedora44-vm-wpctl-status.txt"));
        assert_eq!(default_output(&outputs).map(|o| o.id), Some(50));
        assert_eq!(default_output(&[]), None);
    }

    #[test]
    fn the_output_from_real_output() {
        // A VM has only PipeWire's dummy output. Video also has a Sinks list; it is not sound.
        assert_eq!(
            parse_outputs(include_str!("testdata/fedora44-vm-wpctl-status.txt")),
            vec![Output { id: 50, name: "Dummy Output".into(), volume: 1.0, muted: false, default: true }]
        );
    }

    #[test]
    fn several_outputs_one_muted() {
        // The same layout as the captured output, with two outputs.
        let status = "Audio\n ├─ Devices:\n │      44. Built-in Audio                      [alsa]\n │  \n ├─ Sinks:\n │      51. Built-in Audio Analog Stereo        [vol: 0.40 MUTED]\n │  *   58. LG TV (HDMI)                        [vol: 0.65]\n │  \n ├─ Sources:\n │  \n";
        assert_eq!(
            parse_outputs(status),
            vec![
                Output { id: 51, name: "Built-in Audio Analog Stereo".into(), volume: 0.4, muted: true, default: false },
                Output { id: 58, name: "LG TV (HDMI)".into(), volume: 0.65, muted: false, default: true },
            ]
        );
    }

    #[test]
    fn no_pipewire_means_no_outputs() {
        assert!(parse_outputs("").is_empty());
        assert!(parse_outputs("Could not connect to PipeWire").is_empty());
    }
}
