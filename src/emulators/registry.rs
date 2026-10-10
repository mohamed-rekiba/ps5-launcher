//! The registry: the embedded manifest with the user's override applied, validated as a whole.
//!
//! A user file that cannot be read, parsed or validated changes nothing: the registry is the
//! embedded one, and the error says what is wrong and where, for the app to show. An embedded
//! manifest that does not validate is a bug, caught by the tests.

use super::manifest::*;
use super::yaml::{self, YamlError};
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

pub const EMBEDDED: &str = include_str!("../../assets/emulators.yaml");

/// The most items any one list may hold (emulators, operations, arguments, settings, …).
pub const MAX_ITEMS: usize = 64;

/// The emulators the launcher knows.
#[derive(Clone, Debug, PartialEq)]
pub struct Registry {
    defaults: Defaults,
    emulators: Vec<Emulator>,
}

/// Which file a problem is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Embedded,
    User(PathBuf),
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Origin::Embedded => f.write_str("assets/emulators.yaml (built in)"),
            Origin::User(p) => f.write_str(&crate::util::display_path(&p.to_string_lossy())),
        }
    }
}

/// One problem, at a place in the file ("emulators[1].launch.arguments[3]").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub path: String,
    pub message: String,
}

/// Why a manifest was not used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError {
    pub origin: Origin,
    pub issues: Vec<Issue>,
}

impl LoadError {
    fn one(origin: Origin, path: impl Into<String>, message: impl Into<String>) -> LoadError {
        LoadError { origin, issues: vec![Issue { path: path.into(), message: message.into() }] }
    }

    fn yaml(origin: Origin, e: YamlError) -> LoadError {
        LoadError::one(origin, "", e.to_string())
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:", self.origin)?;
        for i in &self.issues {
            if i.path.is_empty() {
                write!(f, "\n{}", i.message)?;
            } else {
                write!(f, "\n{}: {}", i.path, i.message)?;
            }
        }
        Ok(())
    }
}

/// The registry, and the user file's error when that file was not used.
#[derive(Debug)]
pub struct Loaded {
    pub registry: Registry,
    pub user_error: Option<LoadError>,
}

/// Why a game has no emulator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    Unknown(String),
    Disabled(EmulatorId),
    WrongConsole(EmulatorId, Console),
    NoDefault(Console),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ResolveError::Unknown(id) => write!(f, "there is no emulator \"{id}\""),
            ResolveError::Disabled(id) => write!(f, "the emulator \"{id}\" is disabled"),
            ResolveError::WrongConsole(id, c) => write!(f, "the emulator \"{id}\" does not run {} games", c.key().to_uppercase()),
            ResolveError::NoDefault(c) => write!(f, "no emulator is set for {} games", c.key().to_uppercase()),
        }
    }
}

impl Registry {
    /// The built-in manifest alone.
    pub fn embedded() -> Result<Registry, LoadError> {
        parse_manifest(EMBEDDED, Origin::Embedded)
    }

    /// The built-in manifest with ~/.config/ps5-launcher/emulators.yaml applied.
    pub fn load() -> Result<Loaded, LoadError> {
        Self::load_with(&crate::util::config_dir().join("emulators.yaml"))
    }

    /// The built-in manifest with the user file at `path` applied, when that file exists. Err
    /// only when the built-in manifest is broken.
    pub fn load_with(path: &Path) -> Result<Loaded, LoadError> {
        let embedded = Self::embedded()?;
        let origin = Origin::User(path.to_path_buf());
        let text = match std::fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Loaded { registry: embedded, user_error: None }),
            Ok(m) if m.len() > yaml::MAX_BYTES as u64 => {
                let e = LoadError::one(origin, "", format!("the file is {} KiB; the limit is {} KiB", m.len().div_ceil(1024), yaml::MAX_BYTES / 1024));
                return Ok(Loaded { registry: embedded, user_error: Some(e) });
            }
            _ => std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| String::from_utf8(b).map_err(|_| "the file is not UTF-8 text".to_string())),
        };
        let result = text.map_err(|e| LoadError::one(origin.clone(), "", e)).and_then(|t| embedded.with_override(&t, origin));
        Ok(match result {
            Ok(registry) => Loaded { registry, user_error: None },
            Err(e) => Loaded { registry: embedded, user_error: Some(e) },
        })
    }

    /// This registry with an override's text applied, or why it cannot be.
    pub fn with_override(&self, text: &str, origin: Origin) -> Result<Registry, LoadError> {
        let value = yaml::load(text).map_err(|e| LoadError::yaml(origin.clone(), e))?;
        check_version(&value, &origin)?;
        let o: Override = yaml::typed(text).map_err(|e| LoadError::yaml(origin.clone(), e))?;
        let mut v = Validator::default();
        if o.operations.len() > MAX_ITEMS {
            v.err("operations", format!("{} operations; the limit is {MAX_ITEMS}", o.operations.len()));
        }
        let mut emulators = self.emulators.clone();
        // Where each definition came from, for the messages.
        let mut paths: Vec<String> = (0..emulators.len()).map(|i| format!("(built in) emulators[{i}]")).collect();
        let mut seen = BTreeSet::new();
        for (j, op) in o.operations.iter().enumerate() {
            let at = format!("operations[{j}]");
            let id = match op.target() {
                Ok(id) => id,
                Err(e) => {
                    v.err(&at, e);
                    continue;
                }
            };
            if !seen.insert(id.clone()) {
                v.err(&at, format!("a second operation for \"{id}\": one per emulator"));
                continue;
            }
            let found = emulators.iter().position(|e| e.id == *id);
            match (op.op, &op.definition, found) {
                (OperationKind::Add, Some(definition), None) => {
                    emulators.push((**definition).clone());
                    paths.push(format!("{at}.definition"));
                }
                (OperationKind::Add, _, Some(_)) => v.err(&at, format!("cannot add \"{id}\": it exists (use replace)")),
                (OperationKind::Replace, Some(definition), Some(i)) => {
                    emulators[i] = (**definition).clone();
                    paths[i] = format!("{at}.definition");
                }
                (OperationKind::Disable, _, Some(i)) => emulators[i].enabled = false,
                _ => v.err(&at, format!("there is no emulator \"{id}\"")),
            }
        }
        let mut defaults = self.defaults.clone();
        let mut default_paths = ["(built in) defaults.ps5".to_string(), "(built in) defaults.ps4".to_string()];
        if let Some(d) = o.defaults {
            if d.ps5.is_some() {
                defaults.ps5 = d.ps5;
                default_paths[0] = "defaults.ps5".into();
            }
            if d.ps4.is_some() {
                defaults.ps4 = d.ps4;
                default_paths[1] = "defaults.ps4".into();
            }
        }
        v.registry(&defaults, &default_paths, &emulators, &paths);
        v.finish(origin)?;
        Ok(Registry { defaults, emulators })
    }

    /// Every emulator, the disabled ones too.
    pub fn emulators(&self) -> &[Emulator] {
        &self.emulators
    }

    pub fn get(&self, id: &str) -> Option<&Emulator> {
        self.emulators.iter().find(|e| e.id.as_str() == id)
    }

    pub fn default_for(&self, console: Console) -> Option<&EmulatorId> {
        self.defaults.get(console)
    }

    /// The emulator that runs a game: the game's own choice when it has one, else the
    /// console's default. A choice that is unknown, disabled or for another console is an
    /// error, never a quiet switch to another emulator.
    pub fn resolve(&self, console: Console, game_choice: Option<&str>) -> Result<&Emulator, ResolveError> {
        let id = match game_choice {
            Some(id) => id,
            None => self.default_for(console).ok_or(ResolveError::NoDefault(console))?.as_str(),
        };
        let e = self.get(id).ok_or_else(|| ResolveError::Unknown(id.to_string()))?;
        if !e.enabled {
            return Err(ResolveError::Disabled(e.id.clone()));
        }
        if !e.consoles.contains(&console) {
            return Err(ResolveError::WrongConsole(e.id.clone(), console));
        }
        Ok(e)
    }
}

pub(super) fn parse_manifest(text: &str, origin: Origin) -> Result<Registry, LoadError> {
    let value = yaml::load(text).map_err(|e| LoadError::yaml(origin.clone(), e))?;
    check_version(&value, &origin)?;
    let m: Manifest = yaml::typed(text).map_err(|e| LoadError::yaml(origin.clone(), e))?;
    let mut v = Validator::default();
    if m.emulators.len() > MAX_ITEMS {
        v.err("emulators", format!("{} emulators; the limit is {MAX_ITEMS}", m.emulators.len()));
    }
    let paths: Vec<String> = (0..m.emulators.len()).map(|i| format!("emulators[{i}]")).collect();
    v.registry(&m.defaults, &["defaults.ps5".into(), "defaults.ps4".into()], &m.emulators, &paths);
    v.finish(origin)?;
    Ok(Registry { defaults: m.defaults, emulators: m.emulators })
}

/// Read the version before the typed parse, so a newer file says so instead of "unknown field".
fn check_version(value: &serde_norway::Value, origin: &Origin) -> Result<(), LoadError> {
    let version = value.get("schema_version");
    match version.and_then(serde_norway::Value::as_u64) {
        Some(v) if v == u64::from(SCHEMA_VERSION) => Ok(()),
        Some(v) => Err(LoadError::one(origin.clone(), "schema_version", format!("version {v} is not supported; this launcher reads version {SCHEMA_VERSION}"))),
        None => Err(LoadError::one(origin.clone(), "schema_version", format!("missing or not a number; this launcher reads version {SCHEMA_VERSION}"))),
    }
}

// ------------------------------------------------------------------ validation

#[derive(Default)]
struct Validator {
    issues: Vec<Issue>,
}

impl Validator {
    fn err(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.issues.push(Issue { path: path.into(), message: message.into() });
    }

    fn finish(self, origin: Origin) -> Result<(), LoadError> {
        if self.issues.is_empty() { Ok(()) } else { Err(LoadError { origin, issues: self.issues }) }
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

    fn registry(&mut self, defaults: &Defaults, default_paths: &[String; 2], emulators: &[Emulator], paths: &[String]) {
        let mut ids = BTreeSet::new();
        for (e, at) in emulators.iter().zip(paths) {
            if !ids.insert(e.id.clone()) {
                self.err(format!("{at}.id"), format!("\"{}\" is used twice", e.id));
            }
            self.emulator(e, at);
        }
        for (console, at) in Console::ALL.iter().zip(default_paths) {
            let Some(id) = defaults.get(*console) else { continue };
            match emulators.iter().find(|e| e.id == *id) {
                None => self.err(at, format!("there is no emulator \"{id}\"")),
                Some(e) if !e.enabled => self.err(at, format!("\"{id}\" is disabled; set another default for {} games", console.key().to_uppercase())),
                Some(e) if !e.consoles.contains(console) => self.err(at, format!("\"{id}\" does not run {} games", console.key().to_uppercase())),
                Some(_) => {}
            }
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
