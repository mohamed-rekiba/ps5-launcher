//! What the launcher ships: the default addons, built into the binary, and the bundle's
//! metadata (the console defaults). The launcher never runs the defaults from here: they are
//! the source of the copies in the user's folder (see docs/plans/addons.md).

use super::manifest::{Defaults, EmulatorId};

/// One default addon: its id (the folder name) and its files, by path inside the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShippedAddon {
    pub id: String,
    pub files: Vec<(String, Vec<u8>)>,
}

impl ShippedAddon {
    /// The text of its emulator.yaml.
    pub fn document(&self) -> Option<&str> {
        self.files.iter().find(|(p, _)| p == "emulator.yaml").and_then(|(_, b)| std::str::from_utf8(b).ok())
    }
}

/// Where the default addons and the console defaults come from: the binary, or a test's own.
pub trait DefaultSource {
    /// The default emulator addons, in a fixed order.
    fn emulators(&self) -> Vec<ShippedAddon>;
    /// The emulator for each console when neither the game nor the user chose one.
    fn console_defaults(&self) -> Defaults;
}

/// The defaults built into this binary.
pub struct Embedded;

/// Every file of every default emulator addon (assets/addons/emulators/<id>/). A test checks
/// that this list and the folders hold the same files.
const EMULATORS: [(&str, &[(&str, &[u8])]); 2] = [
    ("kyty", &[("emulator.yaml", include_bytes!("../../assets/addons/emulators/kyty/emulator.yaml"))]),
    ("shadps4", &[("emulator.yaml", include_bytes!("../../assets/addons/emulators/shadps4/emulator.yaml"))]),
];

impl DefaultSource for Embedded {
    fn emulators(&self) -> Vec<ShippedAddon> {
        EMULATORS
            .iter()
            .map(|(id, files)| ShippedAddon { id: id.to_string(), files: files.iter().map(|(p, b)| (p.to_string(), b.to_vec())).collect() })
            .collect()
    }

    fn console_defaults(&self) -> Defaults {
        let id = |s: &str| Some(EmulatorId::try_from(s.to_string()).expect("a valid id"));
        Defaults { ps5: id("kyty"), ps4: id("shadps4") }
    }
}
