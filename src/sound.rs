//! Sound outputs from PipeWire's `wpctl status`, for the Sound page and the Quick Menu (Phase 6 of
//! docs/plans/ps5-launcher-os.md). Only parsing lives here. The launcher's own interface sounds
//! are in audio.rs.
#![allow(dead_code)] // nothing calls it until the Sound page (Phase 6)

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

#[cfg(test)]
mod tests {
    use super::*;

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
