//! One emulator addon's document (emulators/<id>/emulator.yaml), read strictly and checked on its
//! own.
//!
//! The gates run in this order: the strict YAML gates (yaml.rs), `schema_version` (it selects
//! the meaning of everything else), `min_launcher_version`, the typed parse, then the checks
//! below. The first two read the untyped document, so an addon written for a newer launcher says
//! so instead of "unknown field". Checks that need other addons or the folder (the id against
//! the folder name, duplicate ids, resource files) are in discovery.rs.

use super::manifest::*;
use super::yaml::{self, YamlError};
use std::collections::BTreeSet;
use std::fmt;

/// The most items any one list may hold (arguments, settings, hosts, …).
pub const MAX_ITEMS: usize = 64;

/// One problem, at a place in the document ("launch.arguments[3]"); the path is empty for the
/// whole document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub path: String,
    pub message: String,
}

impl Issue {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Issue {
        Issue { path: path.into(), message: message.into() }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.path.is_empty() { f.write_str(&self.message) } else { write!(f, "{}: {}", self.path, self.message) }
    }
}

impl From<YamlError> for Issue {
    fn from(e: YamlError) -> Self {
        Issue::new("", e.to_string())
    }
}

/// A launcher version, compared as a semantic version: major, minor, patch, and a pre-release
/// ("1.15.0-beta.1") below its release. Two pre-releases of one version compare equal; build
/// metadata ("+abc") is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    /// False for a pre-release, so it sorts below the release.
    release: bool,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.split_once('+').map_or(s, |(v, _)| v);
        let (core, pre) = match s.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, true),
            Some(_) => return None,
            None => (s, false),
        };
        let mut parts = core.split('.').map(|p| if p.is_empty() || (p.len() > 1 && p.starts_with('0')) { None } else { p.parse::<u64>().ok() });
        let (major, minor, patch) = (parts.next()??, parts.next()??, parts.next()??);
        if parts.next().is_some() {
            return None;
        }
        Some(Version { major, minor, patch, release: !pre })
    }

    /// This launcher's version (Cargo.toml).
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml's version is a semantic version")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}.{}{}", self.major, self.minor, self.patch, if self.release { "" } else { " (a pre-release)" })
    }
}

/// Read one document. `launcher` is the version `min_launcher_version` is compared with.
pub fn parse(text: &str, launcher: Version) -> Result<Emulator, Vec<Issue>> {
    let value = yaml::load(text).map_err(|e| vec![e.into()])?;
    check_version(&value).map_err(|e| vec![e])?;
    check_launcher(&value, launcher).map_err(|e| vec![e])?;
    let e: Emulator = yaml::typed(text).map_err(|e| vec![e.into()])?;
    let mut v = Validator::default();
    v.emulator(&e, "");
    if v.issues.is_empty() { Ok(e) } else { Err(v.issues) }
}

fn check_version(value: &serde_norway::Value) -> Result<(), Issue> {
    match value.get("schema_version").and_then(serde_norway::Value::as_u64) {
        Some(v) if v == u64::from(SCHEMA_VERSION) => Ok(()),
        Some(v) => Err(Issue::new("schema_version", format!("version {v} is not supported; this launcher reads version {SCHEMA_VERSION}"))),
        None => Err(Issue::new("schema_version", format!("missing or not a number; this launcher reads version {SCHEMA_VERSION}"))),
    }
}

fn check_launcher(value: &serde_norway::Value, launcher: Version) -> Result<(), Issue> {
    let Some(min) = value.get("min_launcher_version") else { return Ok(()) };
    let text = min.as_str().unwrap_or_default();
    let needed = LauncherVersion::try_from(text.to_string()).ok().and_then(|v| Version::parse(v.as_str()));
    match needed {
        None => Err(Issue::new("min_launcher_version", format!("{text:?} is not a version like 1.14.0"))),
        Some(needed) if needed > launcher => Err(Issue::new("min_launcher_version", format!("the addon needs launcher {text} or newer; this is {launcher}"))),
        Some(_) => Ok(()),
    }
}

// ------------------------------------------------------------------ validation

#[derive(Default)]
struct Validator {
    issues: Vec<Issue>,
}

impl Validator {
    /// Paths are built as "{at}.field", with an empty `at` at the document's top level; the
    /// leading dot that leaves is dropped here.
    fn err(&mut self, path: impl Into<String>, message: impl Into<String>) {
        let path: String = path.into();
        let path = path.strip_prefix('.').map(str::to_string).unwrap_or(path);
        self.issues.push(Issue { path, message: message.into() });
    }

    fn count<T>(&mut self, at: &str, items: &[T]) {
        if items.len() > MAX_ITEMS {
            self.err(at, format!("{} items; the limit is {MAX_ITEMS}", items.len()));
        }
    }

    fn text(&mut self, at: &str, s: &str) {
        if s.contains('\0') {
            self.err(at, "contains a NUL character");
        }
    }


    fn emulator(&mut self, e: &Emulator, at: &str) {
        let name = e.display_name.trim();
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
            self.err(format!("{at}.display_name"), "1 to 64 characters, no control characters");
        }
        let consoles: BTreeSet<_> = e.consoles.iter().collect();
        if e.consoles.is_empty() || consoles.len() != e.consoles.len() {
            self.err(format!("{at}.consoles"), "at least one console, each once");
        }
        if !e.consoles.contains(&e.content.metadata.console()) {
            self.err(format!("{at}.content.metadata"), format!("reads {} games, which are not in consoles", e.content.metadata.console().key()));
        }
        match (&e.release, &e.install, &e.update) {
            (ReleaseSource::GithubLatest(r), install, update) => {
                self.github(r, &format!("{at}.release"));
                if install.is_none() {
                    self.err(format!("{at}.install"), "a github_latest release needs an install section");
                }
                if update.is_none() {
                    self.err(format!("{at}.update"), "a github_latest release needs an update section");
                }
            }
            (ReleaseSource::Manual {}, install, update) => {
                if install.is_some() || update.is_some() {
                    self.err(format!("{at}.release"), "a manual release has no install or update section");
                }
                if !e.custom_build.allowed {
                    self.err(format!("{at}.custom_build.allowed"), "a manual release needs a custom build: nothing else can run");
                }
            }
        }
        if let Some(i) = &e.install {
            self.install(i, &format!("{at}.install"));
        }
        if e.custom_build.version == VersionStrategy::ShadHelpBanner {
            self.err(format!("{at}.custom_build.version"), "shad_help_banner gives no version; use custom_build_label");
        }
        if matches!(e.release, ReleaseSource::Manual {}) && e.custom_build.empty_path == EmptyPath::Managed {
            self.err(format!("{at}.custom_build.empty_path"), "there is no managed build with a manual release");
        }
        self.launch(e, &format!("{at}.launch"));
        self.settings(e, &format!("{at}.settings"));
        self.compatibility(&e.compatibility, &format!("{at}.compatibility"));
        if let ContentLayout::ShadPs4 { patch_suffix, .. } = &e.content.layout {
            if patch_suffix.is_empty() || patch_suffix.contains(['/', '\0']) {
                self.err(format!("{at}.content.layout.patch_suffix"), "a non-empty name part without /");
            }
        }
        self.session(&e.session, &format!("{at}.session"));
        if let Some(u) = &e.update {
            let at = format!("{at}.update");
            if u.check_seconds < 60 || u.poll_seconds < 60 {
                self.err(&at, "check_seconds and poll_seconds are at least 60");
            }
            if !(1..=10).contains(&u.keep_versions) {
                self.err(format!("{at}.keep_versions"), "1 to 10");
            }
        }
        if let Some(m) = &e.macos {
            self.count(&format!("{at}.macos.environment_changes"), &m.environment_changes.keys().collect::<Vec<_>>());
            for (k, v) in &m.environment_changes {
                self.text(&format!("{at}.macos.environment_changes.{k}"), v);
            }
        }
    }

    fn github(&mut self, r: &GithubRelease, at: &str) {
        self.count(&format!("{at}.targets"), &r.targets);
        self.count(&format!("{at}.allowed_hosts"), &r.allowed_hosts);
        if r.targets.is_empty() {
            self.err(format!("{at}.targets"), "at least one target");
        }
        for (i, t) in r.targets.iter().enumerate() {
            let at = format!("{at}.targets[{i}]");
            if t.name_suffixes.is_empty() {
                self.err(format!("{at}.name_suffixes"), "at least one suffix, so not every file matches");
            }
            let words = t.name_all.iter().chain(&t.name_any).chain(&t.name_suffixes).chain(&t.name_prefix);
            if words.clone().any(|w| w.is_empty()) {
                self.err(&at, "a name word cannot be empty");
            }
            for w in words {
                self.text(&at, w);
            }
        }
        for required in ["api.github.com", "github.com"] {
            if !r.allowed_hosts.iter().any(|h| h.as_str() == required) {
                self.err(format!("{at}.allowed_hosts"), format!("a GitHub release needs {required}"));
            }
        }
        self.unique(&format!("{at}.allowed_hosts"), r.allowed_hosts.iter().map(Host::as_str));
    }

    fn unique<'a>(&mut self, at: &str, items: impl Iterator<Item = &'a str>) {
        let mut seen = BTreeSet::new();
        for s in items {
            if !seen.insert(s) {
                self.err(at, format!("\"{s}\" is listed twice"));
            }
        }
    }

    fn install(&mut self, i: &Installer, at: &str) {
        let p = i.probe();
        if p.kind == VersionStrategy::CustomBuildLabel {
            self.err(format!("{at}.probe.kind"), "custom_build_label is not a probe");
        }
        if !(1..=60).contains(&p.timeout_seconds) {
            self.err(format!("{at}.probe.timeout_seconds"), "1 to 60 seconds");
        }
        self.count(&format!("{at}.probe.args"), &p.args);
        for a in &p.args {
            self.text(&format!("{at}.probe.args"), a);
        }
        if let SharedData::CwdSymlinks { entries, .. } = i.shared_data() {
            self.count(&format!("{at}.shared_data.entries"), entries);
            self.unique(&format!("{at}.shared_data.entries"), entries.iter().map(RelPath::as_str));
        }
        if let Installer::ZipAppimage(z) = i {
            if z.zip_member_suffix.is_empty() {
                self.err(format!("{at}.zip_member_suffix"), "cannot be empty");
            }
        }
    }

    fn launch(&mut self, e: &Emulator, at: &str) {
        let l = &e.launch;
        self.count(&format!("{at}.arguments"), &l.arguments);
        self.count(&format!("{at}.env_remove"), &l.env_remove);
        self.count(&format!("{at}.env"), &l.env.keys().collect::<Vec<_>>());
        for (k, v) in &l.env {
            self.text(&format!("{at}.env.{k}"), v);
        }
        for (i, a) in l.arguments.iter().enumerate() {
            let at = format!("{at}.arguments[{i}]");
            match a {
                LaunchArg::Literal { text } => self.text(&at, text),
                LaunchArg::Value { from, ty } => {
                    if let Some(source) = self.placeholder(e, from, &at) {
                        let fits = matches!(
                            (source, ty),
                            (ValueType::Path, ArgType::Path)
                                | (ValueType::U32, ArgType::U32)
                                | (ValueType::String, ArgType::String)
                                | (ValueType::Enum, ArgType::Enum)
                                | (ValueType::Bool, ArgType::BoolString)
                        );
                        if !fits {
                            self.err(&at, format!("{from} is {}; {} is incompatible", source.article_name(), arg_type_name(*ty)));
                        }
                    }
                }
                LaunchArg::When { condition, emit } => {
                    if let Some(source) = self.placeholder(e, condition, &at) {
                        if source != ValueType::Bool {
                            self.err(&at, format!("{condition} is {}; a condition must be a bool", source.article_name()));
                        }
                    }
                    if emit.is_empty() {
                        self.err(&at, "emit at least one argument");
                    }
                    for s in emit {
                        self.text(&at, s);
                    }
                }
                LaunchArg::Spread { from } => {
                    if let Some(source) = self.placeholder(e, from, &at) {
                        if source != ValueType::Argv {
                            self.err(&at, format!("{from} is {}; spread needs an argv list", source.article_name()));
                        }
                    }
                }
            }
        }
    }

    /// The type a placeholder stands for, or None (with an issue) when it names no setting.
    fn placeholder(&mut self, e: &Emulator, p: &Placeholder, at: &str) -> Option<ValueType> {
        match p {
            Placeholder::GamePath => Some(ValueType::Path),
            Placeholder::PlayingFullscreen => Some(ValueType::Bool),
            Placeholder::Setting(key) => match e.settings.definitions.iter().find(|s| s.key() == key) {
                Some(s) => Some(s.value_type()),
                None => {
                    self.err(at, format!("settings.{key}: there is no setting \"{key}\""));
                    None
                }
            },
        }
    }

    fn settings(&mut self, e: &Emulator, at: &str) {
        let s = &e.settings;
        self.count(&format!("{at}.definitions"), &s.definitions);
        self.count(&format!("{at}.composite_rows"), &s.composite_rows);
        self.count(&format!("{at}.rows"), &s.rows);
        let mut keys = BTreeSet::new();
        let names = s.definitions.iter().map(Setting::key).chain(s.composite_rows.iter().map(CompositeRow::id));
        for (i, key) in names.enumerate() {
            if RowRef::RESERVED.contains(&key.as_str()) {
                self.err(format!("{at}: {key}"), format!("\"{key}\" is a reserved row name"));
            }
            if !keys.insert(key.clone()) {
                self.err(format!("{at}: {key}"), format!("\"{key}\" is used twice (item {i})"));
            }
        }
        for (i, d) in s.definitions.iter().enumerate() {
            self.setting(d, &format!("{at}.definitions[{i}]"));
        }
        for (i, c) in s.composite_rows.iter().enumerate() {
            let at = format!("{at}.composite_rows[{i}]");
            match c {
                CompositeRow::ResolutionPair { width, height, choices, .. } => {
                    let range = |key: &SettingKey| match s.definitions.iter().find(|d| d.key() == key) {
                        Some(Setting::U32 { min, max, .. }) => Some((*min, *max)),
                        _ => None,
                    };
                    let (Some(w), Some(h)) = (range(width), range(height)) else {
                        self.err(&at, "width and height must name u32 settings");
                        continue;
                    };
                    if choices.is_empty() {
                        self.err(format!("{at}.choices"), "at least one choice");
                    }
                    self.count(&format!("{at}.choices"), choices);
                    for (x, y) in choices {
                        if !(w.0..=w.1).contains(x) || !(h.0..=h.1).contains(y) {
                            self.err(format!("{at}.choices"), format!("{x} × {y} is outside the settings' ranges"));
                        }
                    }
                }
            }
        }
        let mut shown = BTreeSet::new();
        for (i, r) in s.rows.iter().enumerate() {
            let at = format!("{at}.rows[{i}]");
            if !shown.insert(r.to_string()) {
                self.err(&at, format!("\"{r}\" is listed twice"));
            }
            match r {
                RowRef::Update | RowRef::Rollback if e.install.is_none() => self.err(&at, format!("\"{r}\" needs a managed build (an install section)")),
                RowRef::CustomExecutable if !e.custom_build.allowed => self.err(&at, "custom builds are not allowed"),
                RowRef::Item(key) => match s.definitions.iter().find(|d| d.key() == key) {
                    Some(d) if d.label().is_none() => self.err(&at, format!("the setting \"{key}\" has a row, so it needs a label")),
                    Some(_) => {}
                    None if s.composite_rows.iter().any(|c| c.id() == key) => {}
                    None => self.err(&at, format!("there is no setting or composite row \"{key}\"")),
                },
                _ => {}
            }
        }
    }

    fn setting(&mut self, d: &Setting, at: &str) {
        match d {
            Setting::U32 { default, min, max, .. } => {
                if min > max {
                    self.err(at, format!("min {min} is above max {max}"));
                } else if !(min..=max).contains(&default) {
                    self.err(format!("{at}.default"), format!("{default} is outside {min}..={max}"));
                }
            }
            Setting::String { default, choices, allow_existing_other, .. } => {
                self.choices(choices, at);
                if !choices.is_empty() && !allow_existing_other && !choices.iter().any(|c| c.value == *default) {
                    self.err(format!("{at}.default"), format!("\"{default}\" is not one of the choices"));
                }
                self.text(&format!("{at}.default"), default);
            }
            Setting::Enum { default, choices, .. } => {
                if choices.is_empty() {
                    self.err(format!("{at}.choices"), "an enum needs choices");
                }
                self.choices(choices, at);
                if !choices.iter().any(|c| c.value == *default) {
                    self.err(format!("{at}.default"), format!("\"{default}\" is not one of the choices"));
                }
            }
            Setting::Bool { .. } => {}
            Setting::Argv { default, .. } => {
                self.count(&format!("{at}.default"), default);
                for a in default {
                    self.text(&format!("{at}.default"), a);
                }
            }
        }
    }

    fn choices(&mut self, choices: &[Choice], at: &str) {
        self.count(&format!("{at}.choices"), choices);
        self.unique(&format!("{at}.choices"), choices.iter().map(|c| c.value.as_str()));
        for c in choices {
            self.text(&format!("{at}.choices"), &c.value);
        }
    }

    fn compatibility(&mut self, c: &CompatibilityParser, at: &str) {
        let (CompatibilityParser::KytyJsonV1(s) | CompatibilityParser::ShadJsonV1(s)) = c else { return };
        self.count(&format!("{at}.allowed_hosts"), &s.allowed_hosts);
        self.unique(&format!("{at}.allowed_hosts"), s.allowed_hosts.iter().map(Host::as_str));
        let report = match &s.report {
            ReportStrategy::KytyIssueV1 { url } | ReportStrategy::ShadIssueV1 { url } => Some(url),
            ReportStrategy::None {} => None,
        };
        for (field, url) in [("url", Some(&s.url)), ("list_page", Some(&s.list_page)), ("report.url", report)] {
            let Some(url) = url else { continue };
            let host = url.host();
            if !s.allowed_hosts.iter().any(|h| h.as_str() == host) {
                self.err(format!("{at}.{field}"), format!("the host {host} is not in allowed_hosts"));
            }
        }
        if s.refresh_seconds < 60 {
            self.err(format!("{at}.refresh_seconds"), "at least 60");
        }
        if s.cache.as_str().contains('/') {
            self.err(format!("{at}.cache"), "a file name, without /");
        }
    }

    fn session(&mut self, s: &Session, at: &str) {
        self.count(&format!("{at}.process_names"), &s.process_names);
        for n in &s.process_names {
            if n.is_empty() || n.contains(['/', '\0']) {
                self.err(format!("{at}.process_names"), format!("\"{n}\" is not a file name"));
            }
        }
        for (os, flags) in [("linux", &s.game_flags.linux), ("macos", &s.game_flags.macos)] {
            self.count(&format!("{at}.game_flags.{os}"), flags);
            for f in flags {
                if !f.starts_with('-') || f.contains([' ', '\0']) {
                    self.err(format!("{at}.game_flags.{os}"), format!("\"{f}\" is not an option (like --game)"));
                }
            }
        }
    }
}

fn arg_type_name(t: ArgType) -> &'static str {
    match t {
        ArgType::Path => "path",
        ArgType::U32 => "u32",
        ArgType::String => "string",
        ArgType::Enum => "enum",
        ArgType::BoolString => "bool_string",
    }
}
