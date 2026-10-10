//! Startup: bring the default addons up to date, scan, and hand over one immutable registry
//! with one list of problems.

use super::bundle::{DefaultSource, Embedded};
use super::discovery;
use super::document::Version;
use super::lifecycle::{self, FileOps, RealFiles};
use super::registry::Registry;
use super::Problem;
use std::path::{Path, PathBuf};

/// What one start found.
#[derive(Debug)]
pub struct Loaded {
    /// The snapshot every later question goes to; it never changes.
    pub registry: Registry,
    /// The lifecycle's problems and offers, then each rejected addon, in scan order.
    pub problems: Vec<Problem>,
}

/// Reconcile the defaults under `root`, then scan it.
pub fn load(root: &Path, source: &dyn DefaultSource, files: &dyn FileOps, launcher: Version) -> Loaded {
    let mut problems = lifecycle::reconcile(root, source, files);
    let scan = discovery::scan(root, launcher);
    for r in &scan.rejected {
        let subject = if r.folder.is_empty() { "emulators".to_string() } else { format!("emulators/{}", r.folder) };
        let message = r.issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ");
        problems.push(Problem::new(subject, message));
    }
    Loaded { registry: Registry::new(scan, source.console_defaults()), problems }
}

/// The launcher's addon root: its data folder, so the addons are in `<data>/emulators`.
pub fn root() -> PathBuf {
    crate::util::data_dir()
}

/// `load` for the app: the data folder, the defaults in this binary, the real file system.
pub fn load_for_app() -> Loaded {
    load(&root(), &Embedded, &RealFiles::default(), Version::current())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("addons-").tempdir().unwrap()
    }

    fn start(root: &Path) -> Loaded {
        load(root, &Embedded, &RealFiles::default(), Version::parse("1.14.3").unwrap())
    }

    fn ids(l: &Loaded) -> Vec<&str> {
        l.registry.emulators().iter().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn the_first_start_copies_the_defaults_and_finds_them() {
        let r = temp();
        let loaded = start(r.path());
        assert_eq!(loaded.problems, []);
        assert_eq!(ids(&loaded), ["kyty", "shadps4"]);
        assert!(r.path().join("emulators/kyty/emulator.yaml").is_file());
        let again = start(r.path());
        assert_eq!((again.problems, again.registry), (vec![], loaded.registry), "a second start finds the same");
    }

    #[test]
    fn the_problems_of_every_step_come_in_one_list() {
        let r = temp();
        start(r.path());
        fs::remove_file(r.path().join("addons-state.yaml")).unwrap();
        fs::create_dir(r.path().join("emulators/broken")).unwrap();
        fs::write(r.path().join("emulators/broken/emulator.yaml"), "schema_version: 2\n").unwrap();
        fs::create_dir(r.path().join("emulators/empty")).unwrap();
        let loaded = start(r.path());
        assert_eq!(
            loaded.problems,
            [
                Problem::new("addons-state.yaml", "the file is missing, so the launcher left the addon folders as they are. Restore missing defaults to record them again"),
                Problem::new("emulators/broken", "schema_version: version 2 is not supported; this launcher reads version 1"),
                Problem::new("emulators/empty", "there is no emulator.yaml"),
            ]
        );
        assert_eq!(ids(&loaded), ["kyty", "shadps4"], "the others still load");
    }

    #[test]
    fn several_issues_in_one_addon_are_one_problem() {
        let r = temp();
        start(r.path());
        fs::create_dir(r.path().join("emulators/lab")).unwrap();
        let text = include_str!("../../testdata/addons/emulators/ps4-lab/emulator.yaml").replace("enabled: true\n", "enabled: true\nicon: icon.svg\n");
        fs::write(r.path().join("emulators/lab/emulator.yaml"), text).unwrap();
        let loaded = start(r.path());
        assert_eq!(loaded.problems, [Problem::new("emulators/lab", "id: \"ps4-lab\" is not the folder's name, \"lab\"; icon: icon.svg is not in the addon's folder")]);
    }

    #[test]
    fn the_app_keeps_its_addons_in_its_data_folder() {
        assert_eq!(root(), crate::util::data_dir());
        assert!(root().ends_with(crate::util::APP_NAME));
    }
}
