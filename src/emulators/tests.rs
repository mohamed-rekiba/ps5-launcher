//! The registry's tests: the embedded manifest against today's code, the user's override, and
//! resolution.

use super::manifest::*;
use super::registry::{Origin, Registry, ResolveError, EMBEDDED, MAX_ITEMS};
use std::path::{Path, PathBuf};

fn embedded() -> Registry {
    Registry::embedded().unwrap_or_else(|e| panic!("{e}"))
}

fn kyty() -> Emulator {
    embedded().get("kyty").unwrap().clone()
}

fn shad() -> Emulator {
    embedded().get("shadps4").unwrap().clone()
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

// ------------------------------------------------------------------ the embedded manifest

#[test]
fn the_embedded_manifest_loads_and_is_within_the_limits() {
    let r = embedded();
    assert!(EMBEDDED.len() <= super::yaml::MAX_BYTES);
    let ids: Vec<&str> = r.emulators().iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["kyty", "shadps4"]);
    assert!(r.emulators().iter().all(|e| e.enabled));
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

// ------------------------------------------------------------------ the user's override

/// The plan's third-emulator fixture (§8), complete.
const PS4_LAB: &str = "\
id: ps4-lab
display_name: PS4 Lab
consoles: [ps4]
enabled: true
release: {kind: manual}
custom_build: {allowed: true, empty_path: error, discovery: none, version: custom_build_label}
launch:
  cwd: executable_parent
  inherit_environment: true
  env: {}
  env_remove: []
  arguments:
    - {kind: literal, text: --game}
    - {kind: value, from: game.path, type: path}
settings: {backend: external, definitions: [], composite_rows: [], rows: [custom_executable]}
compatibility: {parser: none}
content: {metadata: ps4_param_sfo, layout: {kind: plain_folder}}
session:
  process_names: [ps4-lab]
  include_custom_basename: true
  game_flags: {linux: [--game], macos: [--game]}
  error_parser: none
  controls_profile: none
";

/// A definition as an operation's `definition`, indented to fit.
fn op(kind: &str, definition: &str) -> String {
    let body: String = definition.lines().map(|l| format!("      {l}\n")).collect();
    format!("  - op: {kind}\n    definition:\n{body}")
}

/// kyty's definition from the embedded manifest, with `edit` applied to its text.
fn kyty_text(edit: impl Fn(String) -> String) -> String {
    let start = EMBEDDED.find("  - id: kyty\n").unwrap();
    let end = EMBEDDED.find("  - id: shadps4\n").unwrap();
    let text: String = EMBEDDED[start..end].lines().map(|l| l.get(4..).unwrap_or("").to_string() + "\n").collect();
    edit(format!("id: kyty\n{}", text.split_once('\n').unwrap().1))
}

fn file(operations: &str) -> String {
    format!("schema_version: 1\noperations:\n{operations}")
}

fn apply(text: &str) -> Result<Registry, String> {
    embedded().with_override(text, Origin::User("emulators.yaml".into())).map_err(|e| e.to_string())
}

fn refused(text: &str, expected: &str) {
    match apply(text) {
        Ok(_) => panic!("accepted:\n{text}"),
        Err(e) => assert!(e.contains(expected), "expected {expected:?} in:\n{e}"),
    }
}

#[test]
fn an_override_adds_an_emulator() {
    let r = apply(&file(&op("add", PS4_LAB))).unwrap();
    assert_eq!(r.emulators().len(), 3);
    let lab = r.get("ps4-lab").unwrap();
    assert_eq!(lab.release, ReleaseSource::Manual {});
    assert_eq!(lab.compatibility, CompatibilityParser::None {});
    assert_eq!(r.resolve(Console::Ps4, Some("ps4-lab")).unwrap().id.as_str(), "ps4-lab");
    assert_eq!(r.resolve(Console::Ps4, None).unwrap().id.as_str(), "shadps4", "other PS4 games keep the default");
}

#[test]
fn an_override_replaces_a_definition() {
    let text = kyty_text(|t| t.replace("display_name: KytyPS5", "display_name: KytyPS5 nightly"));
    let r = apply(&file(&op("replace", &text))).unwrap();
    assert_eq!(r.get("kyty").unwrap().display_name, "KytyPS5 nightly");
    assert_eq!(r.emulators().len(), 2);
    // Unchanged, the definition is the embedded one.
    assert_eq!(apply(&file(&op("replace", &kyty_text(|t| t)))).unwrap(), embedded());
}

#[test]
fn an_override_disables_an_emulator_and_moves_the_default() {
    let text = format!("schema_version: 1\ndefaults: {{ps4: ps4-lab}}\noperations:\n{}  - {{op: disable, id: shadps4}}\n", op("add", PS4_LAB));
    let r = apply(&text).unwrap();
    assert!(!r.get("shadps4").unwrap().enabled, "kept, with its settings, but off");
    assert_eq!(r.resolve(Console::Ps4, None).unwrap().id.as_str(), "ps4-lab");
    assert_eq!(r.resolve(Console::Ps4, Some("shadps4")), Err(ResolveError::Disabled(EmulatorId::try_from("shadps4".to_string()).unwrap())));
    assert_eq!(r.resolve(Console::Ps5, None).unwrap().id.as_str(), "kyty", "the other console keeps its default");
}

#[test]
fn defaults_alone_change_the_default() {
    let text = format!("schema_version: 1\ndefaults:\n  ps4: ps4-lab\noperations:\n{}", op("add", PS4_LAB));
    assert_eq!(apply(&text).unwrap().resolve(Console::Ps4, None).unwrap().id.as_str(), "ps4-lab");
    assert_eq!(apply("schema_version: 1\n").unwrap(), embedded(), "an empty override changes nothing");
}

#[test]
fn disabling_the_default_without_a_new_one_is_refused() {
    refused(&file("  - {op: disable, id: kyty}\n"), "defaults.ps5: \"kyty\" is disabled; set another default for PS5 games");
}

#[test]
fn a_default_for_the_wrong_console_is_refused() {
    refused("schema_version: 1\ndefaults: {ps5: shadps4}\n", "defaults.ps5: \"shadps4\" does not run PS5 games");
    refused("schema_version: 1\ndefaults: {ps5: nothing}\n", "there is no emulator \"nothing\"");
}

#[test]
fn an_unknown_field_is_refused() {
    refused("schema_version: 1\noperation: []\n", "unknown field `operation`");
    refused(&file(&op("add", &PS4_LAB.replace("enabled: true", "enabled: true\nenabld: true"))), "unknown field `enabld`");
    refused(&file(&op("add", &PS4_LAB.replace("{kind: manual}", "{kind: manual, repo: a/b}"))), "unknown field `repo`");
}

#[test]
fn a_duplicate_key_is_refused() {
    let lab = PS4_LAB.replace("  env: {}\n", "  env:\n    A: one\n    A: two\n");
    refused(&file(&op("add", &lab)), "duplicate entry with key \"A\"");
}

#[test]
fn a_duplicate_id_is_refused() {
    refused(&file(&format!("{}{}", op("add", PS4_LAB), op("add", PS4_LAB))), "operations[1]: a second operation for \"ps4-lab\"");
    let twice = EMBEDDED.replace("  - id: shadps4\n", "  - id: kyty\n");
    let e = super::registry::parse_manifest(&twice, Origin::Embedded).unwrap_err().to_string();
    assert!(e.contains("emulators[1].id: \"kyty\" is used twice"), "{e}");
}

#[test]
fn replacing_a_missing_emulator_is_refused() {
    refused(&file(&op("replace", PS4_LAB)), "operations[0]: there is no emulator \"ps4-lab\"");
    refused(&file("  - {op: disable, id: nothing}\n"), "operations[0]: there is no emulator \"nothing\"");
}

#[test]
fn adding_an_existing_emulator_is_refused() {
    refused(&file(&op("add", &kyty_text(|t| t))), "cannot add \"kyty\": it exists (use replace)");
}

#[test]
fn a_bad_host_is_refused() {
    refused(&file(&op("replace", &kyty_text(|t| t.replace("- objects.githubusercontent.com", "- \"*.githubusercontent.com\"")))), "is not a host name");
    refused(&file(&op("replace", &kyty_text(|t| t.replace("[kytyps5.github.io, github.com]", "[github.com]")))), "compatibility.url: the host kytyps5.github.io is not in allowed_hosts");
    refused(&file(&op("replace", &kyty_text(|t| t.replace("https://kytyps5.github.io/data/", "http://kytyps5.github.io/data/")))), "is not an https URL");
    refused(&file(&op("replace", &kyty_text(|t| t.replace("https://kytyps5.github.io/data/", "https://user@kytyps5.github.io:8443/data/")))), "no user name, password or port");
}

#[test]
fn a_placeholder_of_the_wrong_type_is_refused() {
    let text = kyty_text(|t| t.replace("{kind: value, from: settings.video_out, type: enum}", "{kind: value, from: settings.video_out, type: bool_string}"));
    refused(&file(&op("replace", &text)), "operations[0].definition.launch.arguments[9]: settings.video_out is an enum; bool_string is incompatible");
    let text = kyty_text(|t| t.replace("condition: settings.amd_cpu", "condition: settings.width"));
    refused(&file(&op("replace", &text)), "settings.width is a u32; a condition must be a bool");
    let text = kyty_text(|t| t.replace("from: settings.extra_args", "from: settings.nothing"));
    refused(&file(&op("replace", &text)), "there is no setting \"nothing\"");
    refused(&file(&op("add", &PS4_LAB.replace("from: game.path", "from: game.name"))), "unknown value \"game.name\"");
}

#[test]
fn a_default_outside_its_choices_or_range_is_refused() {
    refused(&file(&op("replace", &kyty_text(|t| t.replace("default: Title", "default: Hd")))), "\"Hd\" is not one of the choices");
    refused(&file(&op("replace", &kyty_text(|t| t.replace("default: 1920, min: 0", "default: 1920, min: 2000")))), "1920 is outside 2000..=4294967295");
    refused(&file(&op("replace", &kyty_text(|t| t.replace("[3840, 2160]", "[3840, 2160], [7680, 4320]").replace("max: 4294967295}\n    - {type: u32, key: height", "max: 4000}\n    - {type: u32, key: height")))), "7680 × 4320 is outside the settings' ranges");
}

#[test]
fn a_row_must_name_something() {
    refused(&file(&op("replace", &kyty_text(|t| t.replace("present_mode, amd_cpu", "present_mode, amd")))), "there is no setting or composite row \"amd\"");
    refused(&file(&op("add", &PS4_LAB.replace("rows: [custom_executable]", "rows: [custom_executable, rollback]"))), "\"rollback\" needs a managed build");
}

#[test]
fn another_schema_version_is_refused_before_anything_else() {
    refused("schema_version: 2\nsomething_new: true\n", "schema_version: version 2 is not supported; this launcher reads version 1");
    refused("operations: []\n", "schema_version: missing");
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
    refused("schema_version: 1\noperations: !!python/object:os.system []\n", "tags (!tag) are not allowed");
    refused(&file(&op("add", &PS4_LAB.replace("display_name: PS4 Lab", "display_name: !str PS4 Lab"))), "tags (!tag) are not allowed");
}

#[test]
fn a_file_that_is_too_large_is_refused() {
    let big = format!("schema_version: 1\n# {}\n", "x".repeat(300 * 1024));
    refused(&big, "the limit is 256 KiB");
}

#[test]
fn too_many_items_are_refused() {
    let many: String = (0..=MAX_ITEMS).map(|i| format!("    - {{kind: literal, text: --a{i}}}\n")).collect();
    let text = kyty_text(|t| t.replace("  arguments:\n", &format!("  arguments:\n{many}")));
    refused(&file(&op("replace", &text)), "the limit is 64");
}

#[test]
fn the_norway_problem_cannot_change_a_value() {
    // `no` stays text in a text field; a switch must be true or false.
    let lab = PS4_LAB.replace("display_name: PS4 Lab", "display_name: no").replace("  env: {}\n", "  env: {NO: no, YES: yes}\n");
    let r = apply(&file(&op("add", &lab))).unwrap();
    let e = r.get("ps4-lab").unwrap();
    assert_eq!(e.display_name, "no");
    assert_eq!(e.launch.env.values().collect::<Vec<_>>(), ["no", "yes"]);
    refused(&file(&op("add", &PS4_LAB.replace("enabled: true", "enabled: yes"))), "enabled");
    refused(&file(&op("add", &PS4_LAB.replace("include_custom_basename: true", "include_custom_basename: on"))), "include_custom_basename");
}

#[test]
fn a_nul_character_is_refused() {
    refused(&file(&op("add", &PS4_LAB.replace("text: --game", "text: \"--ga\\0me\""))), "contains a NUL character");
}

// ------------------------------------------------------------------ loading the user's file

#[test]
fn without_a_user_file_the_registry_is_the_embedded_one() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = Registry::load_with(&dir.path().join("emulators.yaml")).unwrap();
    assert_eq!((loaded.registry, loaded.user_error), (embedded(), None));
}

#[test]
fn a_bad_user_file_falls_back_to_the_embedded_one_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("emulators.yaml");
    std::fs::write(&path, file(&format!("{}  - {{op: disable, id: nothing}}\n", op("add", PS4_LAB)))).unwrap();
    let loaded = Registry::load_with(&path).unwrap();
    assert_eq!(loaded.registry, embedded(), "nothing of the file applies, not even the valid add");
    let e = loaded.user_error.unwrap();
    assert_eq!(e.origin, Origin::User(path.clone()));
    assert_eq!(e.issues.len(), 1);
    assert_eq!(e.to_string(), format!("{}:\noperations[1]: there is no emulator \"nothing\"", path.display()));
    assert!(std::fs::read_to_string(&path).unwrap().contains("nothing"), "the file stays as it is");
}

#[test]
fn a_good_user_file_applies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("emulators.yaml");
    std::fs::write(&path, file(&op("add", PS4_LAB))).unwrap();
    let loaded = Registry::load_with(&path).unwrap();
    assert!(loaded.user_error.is_none());
    assert!(loaded.registry.get("ps4-lab").is_some());
}

#[test]
fn an_unreadable_or_oversized_user_file_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("emulators.yaml");
    std::fs::write(&path, vec![b'#'; super::yaml::MAX_BYTES + 1]).unwrap();
    let loaded = Registry::load_with(&path).unwrap();
    assert!(loaded.user_error.unwrap().to_string().contains("the limit is 256 KiB"));
    std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
    assert!(Registry::load_with(&path).unwrap().user_error.unwrap().to_string().contains("not UTF-8"));
    let folder = dir.path().join("folder.yaml");
    std::fs::create_dir(&folder).unwrap();
    assert!(Registry::load_with(Path::new(&folder)).unwrap().user_error.is_some());
}

// ------------------------------------------------------------------ resolution

#[test]
fn each_console_resolves_to_its_default() {
    let r = embedded();
    assert_eq!(r.resolve(Console::Ps5, None).unwrap().id.as_str(), "kyty");
    assert_eq!(r.resolve(Console::Ps4, None).unwrap().id.as_str(), "shadps4");
    assert_eq!(r.resolve(crate::platform::Platform::Ps4.into(), None).unwrap().id.as_str(), "shadps4");
}

#[test]
fn a_games_own_choice_wins_and_never_falls_back() {
    let r = embedded();
    assert_eq!(r.resolve(Console::Ps5, Some("kyty")).unwrap().id.as_str(), "kyty");
    assert_eq!(r.resolve(Console::Ps5, Some("gone")), Err(ResolveError::Unknown("gone".into())));
    let shad = EmulatorId::try_from("shadps4".to_string()).unwrap();
    assert_eq!(r.resolve(Console::Ps5, Some("shadps4")), Err(ResolveError::WrongConsole(shad, Console::Ps5)));
    assert_eq!(ResolveError::NoDefault(Console::Ps4).to_string(), "no emulator is set for PS4 games");
}
