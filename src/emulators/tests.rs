//! The emulator addons' tests: the default documents against today's code, one document's
//! checks, and resolution.

use super::bundle::{DefaultSource, Embedded, ShippedAddon};
use super::document::{self, Version, MAX_ITEMS};
use super::manifest::*;
use super::registry::{ChosenBy, Preferences, Registry, ResolveError, Unavailable};
use std::path::PathBuf;

/// The default addons' documents as they ship.
fn shipped() -> Vec<Emulator> {
    let read = |a: &ShippedAddon| document::parse(a.document().unwrap(), Version::current()).unwrap_or_else(|e| panic!("{}: {e:?}", a.id));
    Embedded.emulators().iter().map(read).collect()
}

fn shipped_one(id: &str) -> Emulator {
    shipped().into_iter().find(|e| e.id.as_str() == id).unwrap()
}

fn kyty() -> Emulator {
    shipped_one("kyty")
}

fn shad() -> Emulator {
    shipped_one("shadps4")
}

fn github(e: &Emulator) -> &GithubRelease {
    match &e.release {
        ReleaseSource::GithubLatest(r) => r,
        ReleaseSource::Manual {} => panic!("{} has a manual release", e.id),
    }
}

/// Where a data root is on this computer, as the launcher's code finds it.
fn path_of(root: &DataRoot) -> PathBuf {
    let launcher = crate::util::data_dir();
    let base = match root.base {
        DataBase::LauncherData => launcher,
        DataBase::XdgData => launcher.parent().unwrap().to_path_buf(),
    };
    base.join(root.relative.as_str())
}

// ------------------------------------------------------------------ the default addons

#[test]
fn the_default_documents_load_and_are_within_the_limits() {
    for addon in Embedded.emulators() {
        assert!(addon.document().unwrap().len() <= super::yaml::MAX_BYTES, "{}", addon.id);
    }
    let all = shipped();
    let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["kyty", "shadps4"]);
    assert!(all.iter().all(|e| e.enabled && e.min_launcher_version.is_none()));
}

#[test]
fn catalogs_are_optional_and_checked_with_the_addon() {
    assert!(document::parse(PS4_LAB, Version::current()).unwrap().catalogs.is_empty());
    let text = include_str!("../../assets/addons/emulators/kyty/emulator.yaml");
    for (from, to, issue) in [
        ("refresh_seconds: 21600", "refresh_seconds: 0", "catalogs[0].refresh_seconds"),
        ("- console: ps5", "- console: ps4", "catalogs[0].console"),
        ("bundled_snapshot: catalog.json", "bundled_snapshot: ../escape.json", "relative path"),
        ("override_env: PS5_LAUNCHER_CATALOG_PATH", "override_env: bad-name", "catalogs[0].override_env"),
        ("snapshot: ps5-topics.json", "snapshot: ../escape.json", "relative path"),
    ] {
        let errors = document::parse(&text.replace(from, to), Version::current()).unwrap_err();
        let messages = errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ");
        assert!(messages.contains(issue), "expected {issue}: {messages}");
    }
}

#[test]
fn the_bundle_holds_every_file_of_the_default_folders() {
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/addons/emulators");
    let mut folders: Vec<String> = std::fs::read_dir(&assets).unwrap().flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    folders.sort();
    let shipped = Embedded.emulators();
    assert_eq!(shipped.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), folders);
    for addon in shipped {
        let mut files: Vec<String> = std::fs::read_dir(assets.join(&addon.id)).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        files.sort();
        assert_eq!(addon.files.iter().filter(|(p, _)| p != "catalog.json").map(|(p, _)| p.clone()).collect::<Vec<_>>(), files, "{}", addon.id);
        for (path, bytes) in &addon.files {
            let source = if path == "catalog.json" {
                let emulator = document::parse(addon.document().unwrap(), Version::current()).unwrap();
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/rutracker").join(emulator.catalogs[0].snapshot.as_str())
            } else { assets.join(&addon.id).join(path) };
            assert_eq!(&std::fs::read(source).unwrap_or_default(), bytes, "{}/{path}", addon.id);
        }
    }
}

#[test]
fn the_console_defaults_name_default_addons_for_their_console() {
    let defaults = Embedded.console_defaults();
    for console in Console::ALL {
        let id = defaults.get(console).unwrap();
        assert!(shipped_one(id.as_str()).consoles.contains(&console), "{id} for {console:?}");
    }
    assert_eq!((defaults.ps5.unwrap().as_str(), defaults.ps4.unwrap().as_str()), ("kyty", "shadps4"));
}

#[test]
fn kytyps5_matches_kyty_rs() {
    let e = kyty();
    assert_eq!(e.display_name, "KytyPS5");
    assert_eq!(e.consoles, [Console::Ps5]);
    assert_eq!(github(&e).repo.as_str(), crate::kyty::REPO);
    let Some(Installer::ArchiveFlatOrOneChild(i)) = &e.install else { panic!("kyty unpacks an archive") };
    assert_eq!(path_of(&i.root), crate::kyty::root());
    assert_eq!(path_of(&i.root).join(i.current.as_str()).join(i.executable.linux.as_str()), crate::kyty::managed_emulator());
    assert_eq!(i.executable.macos, i.executable.linux);
    assert_eq!((i.versions.as_str(), i.state.as_str()), ("versions", "state.json"));
    let SharedData::CwdSymlinks { directory, entries, .. } = &i.shared_data else { panic!("kyty links its data folders") };
    assert_eq!(directory.as_str(), "data");
    assert_eq!(entries.iter().map(RelPath::as_str).collect::<Vec<_>>(), crate::kyty::DATA_DIRS);
    assert_eq!((i.probe.kind, i.probe.args.as_slice(), i.probe.timeout_seconds, i.probe.cwd), (VersionStrategy::KytyHelpGitDate, &["--help".to_string()][..], 3, ProbeCwd::ExecutableParent));
    let u = e.update.as_ref().unwrap();
    assert_eq!(f64::from(u.check_seconds), crate::kyty::CHECK_INTERVAL);
    assert_eq!((u.poll_seconds, u.keep_versions, u.compare, u.install_priority), (30 * 60, 2, VersionCompare::TagCommit, 0));
    assert_eq!(e.custom_build.empty_path, EmptyPath::LegacyKytyDetection);
}

#[test]
fn shadps4_matches_shad_rs() {
    let e = shad();
    assert_eq!(e.display_name, "shadPS4");
    assert_eq!(e.consoles, [Console::Ps4]);
    assert_eq!(github(&e).repo.as_str(), crate::shad::REPO);
    let Some(Installer::ZipAppimage(i)) = &e.install else { panic!("shadPS4 ships an AppImage in a zip") };
    assert_eq!(path_of(&i.root), crate::shad::root());
    assert_eq!(path_of(&i.root).join(i.current.as_str()).join(i.executable.linux.as_str()), crate::shad::emulator());
    assert_eq!((i.zip_member_suffix.as_str(), i.extracted_name.as_str()), (".AppImage", "shadps4.AppImage"));
    assert_eq!((i.probe.timeout_seconds, i.probe.cwd, i.probe.stdout_contains.as_deref()), (10, ProbeCwd::Inherit, Some("shadPS4")));
    let u = e.update.as_ref().unwrap();
    assert_eq!(f64::from(u.check_seconds), crate::shad::CHECK_INTERVAL);
    assert_eq!((u.poll_seconds, u.compare, u.install_priority), (30 * 60, VersionCompare::TagEquality, 1));
    assert_eq!(e.custom_build.empty_path, EmptyPath::Managed);
    let ContentLayout::ShadPs4 { patch_suffix, addons_root } = &e.content.layout else { panic!("shadPS4's layout") };
    assert_eq!(patch_suffix, "-patch");
    assert_eq!(path_of(addons_root), crate::pkgx::addons_dir());
    assert_eq!(e.settings.backend, SettingsBackend::External);
}

#[test]
fn the_settings_match_config_rs() {
    let cfg = crate::config::Config::default();
    let e = kyty();
    let get = |key: &str| e.settings.definitions.iter().find(|d| d.key().as_str() == key).unwrap().clone();
    let choices = |c: &[Choice]| c.iter().map(|c| (c.value.clone(), c.label.clone())).collect::<Vec<_>>();
    let pairs = |c: &[(&str, &str)]| c.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect::<Vec<_>>();
    assert!(matches!(get("width"), Setting::U32 { default, min: 0, max: u32::MAX, .. } if default == cfg.width));
    assert!(matches!(get("height"), Setting::U32 { default, min: 0, max: u32::MAX, .. } if default == cfg.height));
    let Setting::String { default, choices: c, allow_existing_other: true, .. } = get("present_mode") else { panic!() };
    assert_eq!((default, choices(&c)), (cfg.present_mode.clone(), pairs(&crate::config::PRESENT_MODES)));
    let Setting::Enum { default, choices: c, .. } = get("video_out") else { panic!() };
    assert_eq!((default, choices(&c)), (cfg.video_out.clone(), pairs(&crate::config::VIDEO_OUT_MODES)));
    assert!(matches!(get("amd_cpu"), Setting::Bool { default, .. } if default == cfg.amd_cpu));
    assert!(matches!(get("extra_args"), Setting::Argv { default, .. } if default == crate::sessions::shell_split(&cfg.extra_args)));
    let CompositeRow::ResolutionPair { choices, .. } = &e.settings.composite_rows[0];
    assert_eq!(choices.as_slice(), crate::config::RESOLUTIONS);
}

#[test]
fn compatibility_matches_compat_rs() {
    let check = |e: &Emulator, url: &str, list: &str, report: &str| {
        let (CompatibilityParser::KytyJsonV1(s) | CompatibilityParser::ShadJsonV1(s)) = &e.compatibility else { panic!("{} has a list", e.id) };
        let (ReportStrategy::KytyIssueV1 { url: form } | ReportStrategy::ShadIssueV1 { url: form }) = &s.report else { panic!() };
        assert_eq!((s.url.as_str(), s.list_page.as_str(), form.as_str()), (url, list, report));
        assert_eq!(f64::from(s.refresh_seconds), crate::compat::REFRESH);
    };
    check(&kyty(), crate::compat::URL, crate::compat::LIST_PAGE, crate::compat::REPORT_FORM);
    check(&shad(), crate::compat::SHAD_URL, crate::compat::SHAD_LIST_PAGE, crate::compat::SHAD_REPORT_FORM);
    assert!(matches!(kyty().compatibility, CompatibilityParser::KytyJsonV1(_)));
    assert!(matches!(shad().compatibility, CompatibilityParser::ShadJsonV1(_)));
}

#[test]
fn sessions_match_sessions_rs() {
    for e in [kyty(), shad()] {
        assert_eq!(e.session.process_names, crate::sessions::EMULATOR_NAMES, "one list for every emulator today");
    }
    assert!(kyty().session.include_custom_basename && !shad().session.include_custom_basename);
}

/// Expand the launch arguments the way phase 3 will, for comparison with sessions.rs.
fn argv(e: &Emulator, cfg: &crate::config::Config, game: &str) -> Vec<String> {
    let setting = |key: &SettingKey| -> Vec<String> {
        match key.as_str() {
            "width" => vec![cfg.width.to_string()],
            "height" => vec![cfg.height.to_string()],
            "present_mode" => vec![cfg.present_mode.clone()],
            "video_out" => {
                let Some(Setting::Enum { choices, default, .. }) = e.settings.definitions.iter().find(|d| d.key() == key) else { panic!() };
                vec![if choices.iter().any(|c| c.value == cfg.video_out) { cfg.video_out.clone() } else { default.clone() }]
            }
            "amd_cpu" => vec![cfg.amd_cpu.to_string()],
            "extra_args" => crate::sessions::shell_split(&cfg.extra_args),
            other => panic!("no such setting {other}"),
        }
    };
    let value = |p: &Placeholder| match p {
        Placeholder::GamePath => vec![game.to_string()],
        Placeholder::PlayingFullscreen => vec![cfg.fullscreen.to_string()],
        Placeholder::Setting(k) => setting(k),
    };
    let mut out = Vec::new();
    for a in &e.launch.arguments {
        match a {
            LaunchArg::Literal { text } => out.push(text.clone()),
            LaunchArg::Value { from, .. } | LaunchArg::Spread { from } => out.extend(value(from)),
            LaunchArg::When { condition, emit } => {
                if value(condition) == ["true"] {
                    out.extend(emit.iter().cloned());
                }
            }
        }
    }
    out
}

#[test]
fn the_launch_arguments_match_sessions_rs() {
    let mut cfg = crate::config::Config::default();
    let game = "/games/My Game/PPSA01234";
    for change in [
        (|_: &mut crate::config::Config| {}) as fn(&mut crate::config::Config),
        |c| c.fullscreen = false,
        |c| c.amd_cpu = true,
        |c| c.video_out = "Uhd".into(),
        |c| c.video_out = "Bogus".into(),
        |c| c.present_mode = "Something else".into(),
        |c| (c.width, c.height) = (1234, 567),
        |c| c.extra_args = r#"--tessellation --name "a b""#.into(),
    ] {
        change(&mut cfg);
        assert_eq!(argv(&kyty(), &cfg, game), crate::sessions::kyty_args(&cfg, game));
        // shadPS4 gets only the game and the full screen switch (sessions.rs, Sessions::launch).
        assert_eq!(argv(&shad(), &cfg, game), ["-g", game, "-f", &cfg.fullscreen.to_string()]);
    }
}

// ------------------------------------------------------------------ release assets

/// Asset names from kyty.rs's tests, and shadPS4's naming ("shadps4-<os>-<ui>-<version>.zip").
const ASSETS: [&str; 13] = [
    "KytyPS5-2026-10-01-4479808-Linux-x86_64.tar.gz",
    "KytyPS5-2026-10-01-4479808-Linux-aarch64.tar.gz",
    "KytyPS5-2026-10-01-4479808-Windows-x86_64.zip",
    "KytyPS5-2026-10-01-4479808-macOS-x86_64.tar.gz",
    "KytyPS5-macos.zip",
    "kyty-darwin-x86_64.tgz",
    "KytyPS5-macOS.dmg",
    "KYTYPS5-OSX.ZIP",
    "shadps4-linux-sdl-0.12.0.zip",
    "shadps4-linux-qt-0.12.0.zip",
    "shadps4-macos-sdl-0.12.0.zip",
    "shadps4-win64-sdl-0.12.0.zip",
    "Shadps4-linux-sdl-0.12.0.zip",
];

fn legacy(id: &str, name: &str, os: Os) -> bool {
    match id {
        "kyty" => crate::kyty::asset_matches(name, if os == Os::Macos { "macos" } else { "linux" }),
        _ => crate::shad::asset_matches(name),
    }
}

#[test]
fn the_asset_selectors_match_the_code_on_every_os_and_architecture() {
    for e in [kyty(), shad()] {
        let r = github(&e);
        for os in [Os::Linux, Os::Macos] {
            for arch in ["x86_64", "aarch64"] {
                for name in ASSETS {
                    assert_eq!(r.select(os, arch, &[name]).is_some(), legacy(e.id.as_str(), name, os), "{} {os:?} {arch} {name}", e.id);
                }
                // The first match in the release's order, as the code's `find`.
                let first = ASSETS.iter().copied().find(|n| legacy(e.id.as_str(), n, os));
                assert_eq!(r.select(os, arch, &ASSETS), first, "{} {os:?} {arch}", e.id);
            }
        }
    }
}

#[test]
fn the_asset_selectors_pick_these() {
    let (k, s) = (kyty(), shad());
    assert_eq!(github(&k).select(Os::Linux, "x86_64", &ASSETS), Some("KytyPS5-2026-10-01-4479808-Linux-x86_64.tar.gz"));
    assert_eq!(github(&k).select(Os::Macos, "aarch64", &ASSETS), Some("KytyPS5-2026-10-01-4479808-macOS-x86_64.tar.gz"));
    assert_eq!(github(&k).select(Os::Macos, "x86_64", &["KytyPS5-macOS.dmg", "KYTYPS5-OSX.ZIP"]), Some("KYTYPS5-OSX.ZIP"));
    assert_eq!(github(&s).select(Os::Linux, "x86_64", &ASSETS), Some("shadps4-linux-sdl-0.12.0.zip"));
    assert_eq!(github(&s).select(Os::Macos, "aarch64", &ASSETS), Some("shadps4-linux-sdl-0.12.0.zip"), "macOS takes the Linux build today");
    assert_eq!(github(&s).select(Os::Linux, "x86_64", &["Shadps4-linux-sdl-0.12.0.zip"]), None, "case-sensitive");
}

// ------------------------------------------------------------------ one document

/// The plan's third-emulator fixture (§8), complete, as an addon document.
const PS4_LAB: &str = include_str!("../../testdata/addons/emulators/ps4-lab/emulator.yaml");

/// kyty's document as it ships, with `edit` applied to its text.
fn kyty_text(edit: impl Fn(String) -> String) -> String {
    edit(Embedded.emulators()[0].document().unwrap().to_string())
}

fn parse_at(text: &str, launcher: &str) -> Result<Emulator, String> {
    let launcher = Version::parse(launcher).unwrap();
    document::parse(text, launcher).map_err(|issues| issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))
}

fn apply(text: &str) -> Result<Emulator, String> {
    parse_at(text, env!("CARGO_PKG_VERSION"))
}

fn refused(text: &str, expected: &str) {
    match apply(text) {
        Ok(_) => panic!("accepted:\n{text}"),
        Err(e) => assert!(e.contains(expected), "expected {expected:?} in:\n{e}"),
    }
}

#[test]
fn the_test_emulator_reads() {
    let lab = apply(PS4_LAB).unwrap();
    assert_eq!((lab.id.as_str(), lab.consoles.as_slice()), ("ps4-lab", &[Console::Ps4][..]));
    assert_eq!(lab.release, ReleaseSource::Manual {});
    assert_eq!(lab.compatibility, CompatibilityParser::None {});
    assert_eq!(apply(&kyty_text(|t| t)).unwrap(), kyty(), "unchanged, it reads as the default");
}

#[test]
fn an_unknown_field_is_refused() {
    refused(&kyty_text(|t| t.replace("enabled: true", "enabled: true\nenabld: true")), "unknown field `enabld`");
    refused(&PS4_LAB.replace("{kind: manual}", "{kind: manual, repo: a/b}"), "unknown field `repo`");
    refused(&kyty_text(|t| t.replace("id: kyty\n", "id: kyty\ndefaults: {ps5: kyty}\n")), "unknown field `defaults`");
}

#[test]
fn a_duplicate_key_is_refused() {
    refused(&PS4_LAB.replace("  env: {}\n", "  env:\n    A: one\n    A: two\n"), "duplicate entry with key \"A\"");
}

#[test]
fn a_bad_host_is_refused() {
    refused(&kyty_text(|t| t.replace("- objects.githubusercontent.com", "- \"*.githubusercontent.com\"")), "is not a host name");
    refused(&kyty_text(|t| t.replace("[kytyps5.github.io, github.com]", "[github.com]")), "compatibility.url: the host kytyps5.github.io is not in allowed_hosts");
    refused(&kyty_text(|t| t.replace("https://kytyps5.github.io/data/", "http://kytyps5.github.io/data/")), "is not an https URL");
    refused(&kyty_text(|t| t.replace("https://kytyps5.github.io/data/", "https://user@kytyps5.github.io:8443/data/")), "no user name, password or port");
}

#[test]
fn a_placeholder_of_the_wrong_type_is_refused() {
    let text = kyty_text(|t| t.replace("{kind: value, from: settings.video_out, type: enum}", "{kind: value, from: settings.video_out, type: bool_string}"));
    refused(&text, "launch.arguments[9]: settings.video_out is an enum; bool_string is incompatible");
    refused(&kyty_text(|t| t.replace("condition: settings.amd_cpu", "condition: settings.width")), "settings.width is a u32; a condition must be a bool");
    refused(&kyty_text(|t| t.replace("from: settings.extra_args", "from: settings.nothing")), "there is no setting \"nothing\"");
    refused(&PS4_LAB.replace("from: game.path", "from: game.name"), "unknown value \"game.name\"");
}

#[test]
fn a_default_outside_its_choices_or_range_is_refused() {
    refused(&kyty_text(|t| t.replace("default: Title", "default: Hd")), "\"Hd\" is not one of the choices");
    refused(&kyty_text(|t| t.replace("default: 1920, min: 0", "default: 1920, min: 2000")), "1920 is outside 2000..=4294967295");
    let text = kyty_text(|t| t.replace("[3840, 2160]", "[3840, 2160], [7680, 4320]").replace("max: 4294967295}\n    - {type: u32, key: height", "max: 4000}\n    - {type: u32, key: height"));
    refused(&text, "7680 × 4320 is outside the settings' ranges");
}

#[test]
fn a_row_must_name_something() {
    refused(&kyty_text(|t| t.replace("present_mode, amd_cpu", "present_mode, amd")), "there is no setting or composite row \"amd\"");
    refused(&PS4_LAB.replace("rows: [custom_executable]", "rows: [custom_executable, rollback]"), "\"rollback\" needs a managed build");
}

#[test]
fn another_schema_version_is_refused_before_anything_else() {
    refused("schema_version: 2\nsomething_new: true\n", "schema_version: version 2 is not supported; this launcher reads version 1");
    refused("id: kyty\n", "schema_version: missing");
}

#[test]
fn an_addon_for_a_newer_launcher_is_refused_before_its_fields_are_read() {
    let newer = PS4_LAB.replace("schema_version: 1\n", "schema_version: 1\nmin_launcher_version: 1.14.10\nsomething_new: true\n");
    assert_eq!(parse_at(&newer, "1.14.9").unwrap_err(), "min_launcher_version: the addon needs launcher 1.14.10 or newer; this is 1.14.9");
    assert!(parse_at(&newer, "1.14.10").unwrap_err().contains("unknown field `something_new`"), "the version passes, then the field fails");
}

#[test]
fn min_launcher_version_compares_as_a_semantic_version() {
    let with = |min: &str| PS4_LAB.replace("schema_version: 1\n", &format!("schema_version: 1\nmin_launcher_version: {min}\n"));
    for (min, launcher) in [("1.14.3", "1.14.3"), ("1.9.0", "1.14.3"), ("0.0.0", "1.14.3"), ("1.14.0", "1.15.0-beta.1"), ("1.14.3", "1.14.3+build.7")] {
        assert!(parse_at(&with(min), launcher).is_ok(), "{min} on {launcher}");
    }
    assert_eq!(parse_at(&with("1.15.0"), "1.15.0-beta.1").unwrap_err(), "min_launcher_version: the addon needs launcher 1.15.0 or newer; this is 1.15.0 (a pre-release)");
    assert!(parse_at(&with("2.0.0"), "1.99.99").is_err());
    for bad in ["1.14", "v1.14.0", "1.14.0-beta", "01.14.0", "1.14.0.1", "\"\""] {
        assert!(parse_at(&with(bad), "1.14.3").unwrap_err().contains("is not a version like 1.14.0"), "{bad}");
    }
    assert_eq!(parse_at(&with("1.14"), "1.14.3").unwrap_err(), "min_launcher_version: 1.14 is not a version like 1.14.0", "a number is shown as written");
}

#[test]
fn resource_paths_are_relative_paths() {
    refused(&PS4_LAB.replace("enabled: true\n", "enabled: true\nicon: ../icon.svg\n"), "is not a relative path");
    refused(&PS4_LAB.replace("enabled: true\n", "enabled: true\ncontrols: /etc/passwd\n"), "is not a relative path");
    let lab = apply(&PS4_LAB.replace("enabled: true\n", "enabled: true\nicon: media/icon.svg\ncontrols: controls.yaml\n")).unwrap();
    assert_eq!((lab.icon.unwrap().as_str(), lab.controls.unwrap().as_str()), ("media/icon.svg", "controls.yaml"));
}

#[test]
fn an_alias_bomb_is_refused() {
    let mut bomb = String::from("schema_version: 1\nx0: &a [\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\"]\n");
    for i in 1..9 {
        bomb.push_str(&format!("x{i}: &a{i} [*a,*a,*a,*a,*a,*a,*a,*a]\n"));
    }
    let start = std::time::Instant::now();
    refused(&bomb, "anchors (&name) are not allowed");
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn a_tag_is_refused() {
    refused("schema_version: 1\nid: !!python/object:os.system x\n", "tags (!tag) are not allowed");
    refused(&PS4_LAB.replace("display_name: PS4 Lab", "display_name: !str PS4 Lab"), "tags (!tag) are not allowed");
}

#[test]
fn a_file_that_is_too_large_is_refused() {
    let big = format!("schema_version: 1\n# {}\n", "x".repeat(300 * 1024));
    refused(&big, "the limit is 256 KiB");
}

#[test]
fn too_many_items_are_refused() {
    let many: String = (0..=MAX_ITEMS).map(|i| format!("    - {{kind: literal, text: --a{i}}}\n")).collect();
    refused(&kyty_text(|t| t.replace("  arguments:\n", &format!("  arguments:\n{many}"))), "the limit is 64");
}

#[test]
fn the_norway_problem_cannot_change_a_value() {
    // `no` stays text in a text field; a switch must be true or false.
    let lab = PS4_LAB.replace("display_name: PS4 Lab", "display_name: no").replace("  env: {}\n", "  env: {NO: no, YES: yes}\n");
    let e = apply(&lab).unwrap();
    assert_eq!(e.display_name, "no");
    assert_eq!(e.launch.env.values().collect::<Vec<_>>(), ["no", "yes"]);
    refused(&PS4_LAB.replace("enabled: true", "enabled: yes"), "enabled");
    refused(&PS4_LAB.replace("include_custom_basename: true", "include_custom_basename: on"), "include_custom_basename");
}

#[test]
fn a_nul_character_is_refused() {
    refused(&PS4_LAB.replace("text: --game", "text: \"--ga\\0me\""), "contains a NUL character");
}

#[test]
fn every_problem_in_a_document_is_listed() {
    let text = kyty_text(|t| t.replace("default: Title", "default: Hd").replace("present_mode, amd_cpu", "present_mode, amd"));
    let e = apply(&text).unwrap_err();
    assert_eq!(e.lines().count(), 2, "{e}");
}


// ------------------------------------------------------------------ resolution

/// The user's choices, as Phase 2's preferences will hold them.
#[derive(Default)]
struct Prefs {
    games: Vec<(&'static str, &'static str)>,
    consoles: Vec<(Console, &'static str)>,
    off: Vec<&'static str>,
}

impl Preferences for Prefs {
    fn game_choice(&self, title_id: &str) -> Option<String> {
        self.games.iter().find(|(t, _)| *t == title_id).map(|(_, id)| id.to_string())
    }
    fn console_choice(&self, console: Console) -> Option<String> {
        self.consoles.iter().find(|(c, _)| *c == console).map(|(_, id)| id.to_string())
    }
    fn enabled(&self, id: &str) -> bool {
        !self.off.contains(&id)
    }
}

/// A data folder after the first start, with the test folders `extra` dropped in.
fn installed(extra: &[&str]) -> (tempfile::TempDir, Registry) {
    let root = tempfile::Builder::new().prefix("addons-").tempdir().unwrap();
    assert_eq!(super::lifecycle::reconcile(root.path(), &Embedded, &super::lifecycle::RealFiles::default()), []);
    for name in extra {
        let from = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/addons/emulators").join(name);
        let to = root.path().join("emulators").join(name);
        std::fs::create_dir_all(&to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
    let registry = registry_of(root.path(), Embedded.console_defaults());
    (root, registry)
}

fn registry_of(root: &std::path::Path, defaults: Defaults) -> Registry {
    Registry::new(super::discovery::scan(root, Version::current()), defaults)
}

fn resolved(r: &Registry, console: Console, title: &str, prefs: &Prefs) -> Result<String, ResolveError> {
    r.resolve(console, title, prefs).map(|e| e.id.to_string())
}

fn unavailable(id: &str, console: Console, chosen_by: ChosenBy, reason: Unavailable) -> Result<String, ResolveError> {
    Err(ResolveError::Unavailable { id: id.into(), console, chosen_by, reason })
}

#[test]
fn each_console_resolves_to_the_shipped_default() {
    let (_root, r) = installed(&[]);
    let none = Prefs::default();
    assert_eq!(resolved(&r, Console::Ps5, "PPSA01234", &none), Ok("kyty".into()));
    assert_eq!(resolved(&r, Console::Ps4, "CUSA01234", &none), Ok("shadps4".into()));
    assert_eq!(resolved(&r, crate::platform::Platform::Ps4.into(), "CUSA01234", &none), Ok("shadps4".into()));
}

#[test]
fn a_second_ps4_emulator_from_a_folder_resolves_when_chosen() {
    let (_root, r) = installed(&["ps4-lab"]);
    assert_eq!(r.emulators().iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["kyty", "ps4-lab", "shadps4"]);
    let game = Prefs { games: vec![("CUSA00001", "ps4-lab")], ..Prefs::default() };
    assert_eq!(resolved(&r, Console::Ps4, "CUSA00001", &game), Ok("ps4-lab".into()));
    assert_eq!(resolved(&r, Console::Ps4, "CUSA00002", &game), Ok("shadps4".into()), "other PS4 games keep the default");
    let console = Prefs { consoles: vec![(Console::Ps4, "ps4-lab")], ..Prefs::default() };
    assert_eq!(resolved(&r, Console::Ps4, "CUSA00002", &console), Ok("ps4-lab".into()));
    assert_eq!(resolved(&r, Console::Ps5, "PPSA00001", &console), Ok("kyty".into()), "the other console keeps its default");
}

#[test]
fn the_games_choice_comes_first_then_the_users_then_the_shipped_default() {
    let (_root, r) = installed(&["ps4-lab"]);
    let both = Prefs { games: vec![("CUSA00001", "shadps4")], consoles: vec![(Console::Ps4, "ps4-lab")], ..Prefs::default() };
    assert_eq!(resolved(&r, Console::Ps4, "CUSA00001", &both), Ok("shadps4".into()));
    assert_eq!(resolved(&r, Console::Ps4, "CUSA00002", &both), Ok("ps4-lab".into()));
}

#[test]
fn an_unavailable_choice_is_said_and_never_replaced() {
    let (root, r) = installed(&["ps4-lab"]);
    let game = |id: &'static str| Prefs { games: vec![("CUSA00001", id)], ..Prefs::default() };
    assert_eq!(resolved(&r, Console::Ps4, "CUSA00001", &game("gone")), unavailable("gone", Console::Ps4, ChosenBy::Game, Unavailable::Missing));
    assert_eq!(resolved(&r, Console::Ps5, "CUSA00001", &game("shadps4")), unavailable("shadps4", Console::Ps5, ChosenBy::Game, Unavailable::WrongConsole));
    let off = Prefs { off: vec!["ps4-lab"], ..game("ps4-lab") };
    assert_eq!(resolved(&r, Console::Ps4, "CUSA00001", &off), unavailable("ps4-lab", Console::Ps4, ChosenBy::Game, Unavailable::Off));
    let mine = Prefs { consoles: vec![(Console::Ps4, "gone")], ..Prefs::default() };
    assert_eq!(resolved(&r, Console::Ps4, "CUSA00001", &mine), unavailable("gone", Console::Ps4, ChosenBy::User, Unavailable::Missing), "not the shipped default");

    // A broken addon keeps the user's choice, and says why it cannot run.
    std::fs::write(root.path().join("emulators/ps4-lab/emulator.yaml"), "schema_version: 1\nid: ps4-lab\n").unwrap();
    let r = registry_of(root.path(), Embedded.console_defaults());
    assert_eq!(resolved(&r, Console::Ps4, "CUSA00001", &game("ps4-lab")), unavailable("ps4-lab", Console::Ps4, ChosenBy::Game, Unavailable::Broken));
    let text = r.resolve(Console::Ps4, "CUSA00001", &game("ps4-lab")).unwrap_err().to_string();
    assert_eq!(text, "this game's emulator, \"ps4-lab\", cannot run it: its emulator.yaml has a problem");
}

#[test]
fn a_console_can_have_no_usable_default() {
    let (root, _) = installed(&["ps4-lab"]);
    std::fs::remove_dir_all(root.path().join("emulators/shadps4")).unwrap();
    let r = registry_of(root.path(), Embedded.console_defaults());
    let none = Prefs::default();
    let e = r.resolve(Console::Ps4, "CUSA00001", &none).unwrap_err();
    assert_eq!(e, ResolveError::Unavailable { id: "shadps4".into(), console: Console::Ps4, chosen_by: ChosenBy::Launcher, reason: Unavailable::Missing });
    assert_eq!(e.to_string(), "the default PS4 emulator, \"shadps4\", cannot run it: it is not installed");
    // Scan order never decides: with no default at all, ps4-lab is not picked.
    let r = registry_of(root.path(), Defaults { ps5: None, ps4: None });
    assert_eq!(resolved(&r, Console::Ps4, "CUSA00001", &none), Err(ResolveError::NoDefault(Console::Ps4)));
    assert_eq!(ResolveError::NoDefault(Console::Ps4).to_string(), "no emulator is set for PS4 games");
    let r = registry_of(root.path(), Embedded.console_defaults());
    let off = Prefs { off: vec!["kyty"], ..Prefs::default() };
    assert_eq!(r.resolve(Console::Ps5, "PPSA00001", &off).unwrap_err().to_string(), "the default PS5 emulator, \"kyty\", cannot run it: it is turned off");
    let mine = Prefs { consoles: vec![(Console::Ps5, "ps4-lab")], ..Prefs::default() };
    assert_eq!(r.resolve(Console::Ps5, "PPSA00001", &mine).unwrap_err().to_string(), "your PS5 emulator, \"ps4-lab\", cannot run it: it does not run PS5 games");
}
