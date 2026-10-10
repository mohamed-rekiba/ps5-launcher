//! The types of an emulator addon's document (emulators/<id>/emulator.yaml).
//!
//! Every struct refuses unknown fields, and every closed set of words is an enum, so a typo is
//! an error, not a silent default. The names of the compiled adapters (`ReleaseSource`,
//! `Installer`, `VersionStrategy`, …) are the ones in docs/plans/data-driven-emulators.md.
//! `schemars` turns these types into assets/addons/emulators/emulator.schema.json (see
//! `schema.rs`); the doc comments become the schema's descriptions, which editors show.

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

/// The only schema version this launcher reads.
pub const SCHEMA_VERSION: u32 = 1;

// ------------------------------------------------------------------ checked text

/// A string type checked when it is read, with a JSON Schema pattern that says the same.
macro_rules! checked_text {
    ($(#[$doc:meta])* $name:ident, $pattern:expr, $check:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
        #[serde(try_from = "String")]
        pub struct $name(String);

        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(s: String) -> Result<Self, String> {
                let check: fn(&str) -> Result<(), String> = $check;
                check(&s).map(|()| $name(s))
            }
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({ "type": "string", "pattern": $pattern })
            }
        }
    };
}

fn word(s: &str, what: &str, first: fn(char) -> bool, rest: fn(char) -> bool, max: usize) -> Result<(), String> {
    let mut chars = s.chars();
    let ok = chars.next().is_some_and(first) && chars.all(rest) && s.len() <= max;
    if ok { Ok(()) } else { Err(format!("{s:?} is not a valid {what}")) }
}

checked_text!(
    /// An emulator's id: lower-case letters, digits, `-` and `_`, at most 32 characters.
    EmulatorId,
    "^[a-z0-9][a-z0-9_-]{0,31}$",
    |s| word(s, "emulator id (a-z, 0-9, - and _)", |c| c.is_ascii_lowercase() || c.is_ascii_digit(), |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_', 32)
);

checked_text!(
    /// A setting's key: lower-case letters, digits and `_`, starting with a letter.
    SettingKey,
    "^[a-z][a-z0-9_]{0,31}$",
    is_setting_key
);

fn is_setting_key(s: &str) -> Result<(), String> {
    word(s, "setting key (a-z, 0-9 and _)", |c| c.is_ascii_lowercase(), |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_', 32)
}

checked_text!(
    /// An environment variable's name.
    EnvName,
    "^[A-Za-z_][A-Za-z0-9_]{0,127}$",
    |s| word(s, "environment variable name", |c| c.is_ascii_alphabetic() || c == '_', |c| c.is_ascii_alphanumeric() || c == '_', 128)
);

checked_text!(
    /// A GitHub repository, "owner/name".
    GithubRepo,
    "^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$",
    |s| {
        let part = |p: &str| !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
        match s.split_once('/') {
            Some((owner, name)) if part(owner) && part(name) && s.len() <= 200 => Ok(()),
            _ => Err(format!("{s:?} is not a GitHub repository (owner/name)")),
        }
    }
);

checked_text!(
    /// A path relative to a folder the launcher owns: no leading `/`, no `..`, no empty parts.
    RelPath,
    "^[^/\\u0000]+(/[^/\\u0000]+)*$",
    |s| {
        let ok = !s.is_empty() && s.len() <= 255 && s.split('/').all(|p| !p.is_empty() && p != ".." && p != "." && !p.contains('\0'));
        if ok { Ok(()) } else { Err(format!("{s:?} is not a relative path (no leading /, no . or .. parts)")) }
    }
);

checked_text!(
    /// A host name, exactly as it must appear in a URL: lower case, no scheme, port or wildcard.
    Host,
    "^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$",
    |s| {
        let label = |l: &str| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if s.len() <= 253 && s.contains('.') && s.split('.').all(label) { Ok(()) } else { Err(format!("{s:?} is not a host name (lower case, like api.github.com)")) }
    }
);

checked_text!(
    /// An https URL without user name, password or port. Its host must also be listed in the
    /// section's `allowed_hosts`.
    HttpsUrl,
    "^https://[^\\s@]+$",
    |s| {
        let url = url::Url::parse(s).map_err(|e| format!("{s:?} is not a URL: {e}"))?;
        if url.scheme() != "https" {
            return Err(format!("{s:?} is not an https URL"));
        }
        if !url.username().is_empty() || url.password().is_some() || url.port().is_some() || url.host_str().is_none() {
            return Err(format!("{s:?}: a URL here has no user name, password or port"));
        }
        Ok(())
    }
);

impl HttpsUrl {
    pub fn host(&self) -> String {
        url::Url::parse(&self.0).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default()
    }
}

checked_text!(
    /// A launcher version, "major.minor.patch" (like 1.14.0), compared as a semantic version.
    LauncherVersion,
    "^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$",
    |s| {
        let part = |p: &str| !p.is_empty() && p.len() <= 9 && p.chars().all(|c| c.is_ascii_digit()) && (p == "0" || !p.starts_with('0'));
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() == 3 && parts.iter().all(|p| part(p)) { Ok(()) } else { Err(format!("{s:?} is not a version like 1.14.0")) }
    }
);

// ------------------------------------------------------------------ console defaults

/// The emulator for each console's games when neither the game nor the user chose one. The
/// launcher's bundle holds them (bundle.rs), not the addon documents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Defaults {
    pub ps5: Option<EmulatorId>,
    pub ps4: Option<EmulatorId>,
}

impl Defaults {
    pub fn get(&self, console: Console) -> Option<&EmulatorId> {
        match console {
            Console::Ps5 => self.ps5.as_ref(),
            Console::Ps4 => self.ps4.as_ref(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Console {
    Ps5,
    Ps4,
}

impl Console {
    pub const ALL: [Console; 2] = [Console::Ps5, Console::Ps4];

    pub fn key(self) -> &'static str {
        match self {
            Console::Ps5 => "ps5",
            Console::Ps4 => "ps4",
        }
    }
}

impl From<crate::platform::Platform> for Console {
    fn from(p: crate::platform::Platform) -> Self {
        match p {
            crate::platform::Platform::Ps5 => Console::Ps5,
            crate::platform::Platform::Ps4 => Console::Ps4,
        }
    }
}

/// One value for each host OS.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PerOs<T> {
    pub linux: T,
    pub macos: T,
}

impl<T> PerOs<T> {
    pub fn get(&self, os: Os) -> &T {
        match os {
            Os::Linux => &self.linux,
            Os::Macos => &self.macos,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Linux,
    Macos,
}

/// One emulator addon: the whole of emulators/<id>/emulator.yaml.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Emulator {
    /// The format's version; it selects the meaning of the rest. This launcher reads version 1.
    pub schema_version: u32,
    /// The oldest launcher that can use this addon. An older one leaves it untouched and
    /// unavailable.
    pub min_launcher_version: Option<LauncherVersion>,
    /// The same as the addon's folder name.
    pub id: EmulatorId,
    /// The name the launcher shows.
    pub display_name: String,
    /// The consoles whose games it runs.
    pub consoles: Vec<Console>,
    /// A disabled emulator keeps its settings but runs no games.
    pub enabled: bool,
    /// An image file in the addon's folder.
    pub icon: Option<RelPath>,
    /// A file in the addon's folder with the keyboard and controller reference.
    pub controls: Option<RelPath>,
    pub release: ReleaseSource,
    /// How a managed build is installed. Required with a `github_latest` release.
    pub install: Option<Installer>,
    pub custom_build: CustomBuild,
    pub launch: Launch,
    pub settings: Settings,
    pub compatibility: CompatibilityParser,
    pub content: Content,
    pub session: Session,
    /// Automatic updates. Required with a `github_latest` release.
    pub update: Option<Update>,
    pub macos: Option<Macos>,
}

// ------------------------------------------------------------------ release

/// Where managed builds come from.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseSource {
    /// The latest release of a GitHub repository.
    GithubLatest(GithubRelease),
    /// No managed builds: the user sets the executable.
    Manual {},
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GithubRelease {
    pub repo: GithubRepo,
    pub asset_selection: AssetSelection,
    /// Which release asset each host takes.
    pub targets: Vec<AssetTarget>,
    /// The hosts downloads may use, redirects included.
    pub allowed_hosts: Vec<Host>,
    pub integrity: Integrity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssetSelection {
    /// The first asset, in the release's order, that a target for this host matches.
    FirstMatch,
}

/// Which release assets suit one host. A name matches when it contains every `name_all`
/// word, at least one `name_any` word (when there are any), starts with `name_prefix` (when
/// set) and ends with one of `name_suffixes`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssetTarget {
    pub os: Os,
    pub host_arch: HostArch,
    #[serde(default)]
    pub name_all: Vec<String>,
    #[serde(default)]
    pub name_any: Vec<String>,
    pub name_prefix: Option<String>,
    pub name_suffixes: Vec<String>,
    pub case_sensitive: bool,
    #[serde(default)]
    pub support: TargetSupport,
}

/// The host architectures a target is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostArch {
    /// Any architecture: the selector ignores it, as today's code does. It does not claim the
    /// asset runs everywhere.
    LegacyAny,
    X86_64,
    Aarch64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetSupport {
    #[default]
    Supported,
    /// Today's code tries this asset although it is not built for the host.
    LegacyAttemptOnly,
}

impl AssetTarget {
    /// Whether a release asset's file name matches this target.
    pub fn matches(&self, name: &str) -> bool {
        let fold = |s: &str| if self.case_sensitive { s.to_string() } else { s.to_ascii_lowercase() };
        let name = fold(name);
        self.name_all.iter().all(|w| name.contains(&fold(w)))
            && (self.name_any.is_empty() || self.name_any.iter().any(|w| name.contains(&fold(w))))
            && self.name_prefix.as_ref().is_none_or(|p| name.starts_with(&fold(p)))
            && self.name_suffixes.iter().any(|s| name.ends_with(&fold(s)))
    }

    fn suits(&self, os: Os, arch: &str) -> bool {
        self.os == os
            && match self.host_arch {
                HostArch::LegacyAny => true,
                HostArch::X86_64 => arch == "x86_64",
                HostArch::Aarch64 => arch == "aarch64",
            }
    }
}

impl GithubRelease {
    /// The asset this host downloads, from the release's asset names in their order.
    /// `arch` is `std::env::consts::ARCH`.
    pub fn select<'a>(&self, os: Os, arch: &str, names: &[&'a str]) -> Option<&'a str> {
        let targets: Vec<&AssetTarget> = self.targets.iter().filter(|t| t.suits(os, arch)).collect();
        match self.asset_selection {
            AssetSelection::FirstMatch => names.iter().copied().find(|n| targets.iter().any(|t| t.matches(n))),
        }
    }
}

/// How a download is checked.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Integrity {
    /// The SHA-256 digest GitHub lists for the asset. `required: false`: an asset without a
    /// digest still installs; a wrong digest never does.
    GithubAssetSha256 { required: bool },
}

// ------------------------------------------------------------------ install

/// How a managed build is unpacked and laid out.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Installer {
    /// An archive (zip or tar) whose executable is at its root or in one top-level folder.
    ArchiveFlatOrOneChild(ArchiveInstall),
    /// A zip that holds an AppImage, unpacked with `--appimage-extract` so no FUSE is needed.
    ZipAppimage(AppImageInstall),
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveInstall {
    pub root: DataRoot,
    /// One folder per release under the root.
    pub versions: RelPath,
    /// The link to the active version.
    pub current: RelPath,
    pub state: RelPath,
    pub format_detection: FormatDetection,
    /// The executable, inside a version folder.
    pub executable: PerOs<RelPath>,
    pub shared_data: SharedData,
    pub probe: Probe,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppImageInstall {
    pub root: DataRoot,
    pub versions: RelPath,
    pub current: RelPath,
    pub state: RelPath,
    /// The zip member to take: the first whose name ends with this.
    pub zip_member_suffix: String,
    pub member_selection: MemberSelection,
    /// The AppImage's name in the version folder while it unpacks.
    pub extracted_name: RelPath,
    pub executable: PerOs<RelPath>,
    pub shared_data: SharedData,
    pub probe: Probe,
}

impl Installer {
    pub fn root(&self) -> &DataRoot {
        match self {
            Installer::ArchiveFlatOrOneChild(i) => &i.root,
            Installer::ZipAppimage(i) => &i.root,
        }
    }

    pub fn executable(&self) -> &PerOs<RelPath> {
        match self {
            Installer::ArchiveFlatOrOneChild(i) => &i.executable,
            Installer::ZipAppimage(i) => &i.executable,
        }
    }

    pub fn probe(&self) -> &Probe {
        match self {
            Installer::ArchiveFlatOrOneChild(i) => &i.probe,
            Installer::ZipAppimage(i) => &i.probe,
        }
    }

    pub fn shared_data(&self) -> &SharedData {
        match self {
            Installer::ArchiveFlatOrOneChild(i) => &i.shared_data,
            Installer::ZipAppimage(i) => &i.shared_data,
        }
    }

    pub fn current(&self) -> &RelPath {
        match self {
            Installer::ArchiveFlatOrOneChild(i) => &i.current,
            Installer::ZipAppimage(i) => &i.current,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FormatDetection {
    /// A zip by its first bytes, else a tar archive.
    ZipMagicElseTar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemberSelection {
    FirstMatch,
}

/// A folder under one of the user's data folders.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DataRoot {
    pub base: DataBase,
    pub relative: RelPath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataBase {
    /// The launcher's data folder: $XDG_DATA_HOME/ps5-launcher, or ~/.local/share/ps5-launcher.
    LauncherData,
    /// $XDG_DATA_HOME, or ~/.local/share.
    XdgData,
}

/// Where an emulator keeps saves and caches, which every version shares.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SharedData {
    /// The emulator writes these folders in its working folder; each version folder gets links
    /// to one shared copy under the install root.
    CwdSymlinks {
        directory: RelPath,
        entries: Vec<RelPath>,
        archive_merge: ArchiveMerge,
        custom_import: CustomImport,
    },
    /// The emulator keeps its data in a folder of its own, outside the install.
    External { root: DataRoot },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveMerge {
    /// A folder the archive ships is copied into the shared one without overwriting.
    CopyMissing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CustomImport {
    /// On the switch from a custom build: its data is copied into shared folders still empty.
    CopyIntoEmpty,
}

/// The check that a freshly unpacked build runs on this host.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub kind: VersionStrategy,
    pub args: Vec<String>,
    pub timeout_seconds: u32,
    pub cwd: ProbeCwd,
    /// The probe passes only when the output contains this text.
    pub stdout_contains: Option<String>,
}

/// How a build's version is found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VersionStrategy {
    /// The first line of `--help`: "git = 6799ecb, date = 2026.09.29".
    KytyHelpGitDate,
    /// `--help` prints a banner with the emulator's name; it gives no version.
    ShadHelpBanner,
    /// No probe: a custom build shows as "Own build".
    CustomBuildLabel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProbeCwd {
    ExecutableParent,
    /// The launcher's own working folder.
    Inherit,
}

// ------------------------------------------------------------------ custom build

/// The user's own build of the emulator.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomBuild {
    pub allowed: bool,
    /// What an empty executable path means.
    pub empty_path: EmptyPath,
    pub discovery: Discovery,
    pub version: VersionStrategy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EmptyPath {
    /// The managed build when it is installed, else a build found on this computer.
    LegacyKytyDetection,
    /// The managed build.
    Managed,
    /// No emulator: the game does not start.
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Discovery {
    /// PATH, then KytyPS5's usual build folders.
    LegacyKytyLocationsV1,
    None,
}

// ------------------------------------------------------------------ launch

/// How a game starts. The launcher runs the executable directly, never through a shell.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub cwd: LaunchCwd,
    pub inherit_environment: bool,
    #[serde(default)]
    pub env: BTreeMap<EnvName, String>,
    #[serde(default)]
    pub env_remove: Vec<EnvName>,
    /// The arguments, in order.
    pub arguments: Vec<LaunchArg>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchCwd {
    ExecutableParent,
}

/// One part of the command line.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchArg {
    /// This text, as one argument.
    Literal { text: String },
    /// A value as one argument. `type` must suit the value: a path for game.path, bool_string
    /// for a switch ("true" or "false"), or the setting's own type.
    Value {
        from: Placeholder,
        #[serde(rename = "type")]
        ty: ArgType,
    },
    /// These arguments when a switch is on.
    When { condition: Placeholder, emit: Vec<String> },
    /// Every item of a list setting, each as one argument.
    Spread { from: Placeholder },
}

/// How a value becomes an argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArgType {
    Path,
    U32,
    String,
    Enum,
    /// A switch as "true" or "false".
    BoolString,
}

/// A value a launch argument can use.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum Placeholder {
    /// The game's folder.
    GamePath,
    /// Settings → Playing → Full screen, shared by every emulator.
    PlayingFullscreen,
    /// One of this emulator's settings.
    Setting(SettingKey),
}

impl TryFrom<String> for Placeholder {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        match s.as_str() {
            "game.path" => Ok(Placeholder::GamePath),
            "playing.fullscreen" => Ok(Placeholder::PlayingFullscreen),
            _ => match s.strip_prefix("settings.") {
                Some(key) => Ok(Placeholder::Setting(SettingKey::try_from(key.to_string())?)),
                None => Err(format!("unknown value {s:?} (expected game.path, playing.fullscreen or settings.<key>)")),
            },
        }
    }
}

impl fmt::Display for Placeholder {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Placeholder::GamePath => f.write_str("game.path"),
            Placeholder::PlayingFullscreen => f.write_str("playing.fullscreen"),
            Placeholder::Setting(k) => write!(f, "settings.{k}"),
        }
    }
}

impl JsonSchema for Placeholder {
    fn schema_name() -> Cow<'static, str> {
        "Placeholder".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "game.path, playing.fullscreen or settings.<key>",
            "type": "string",
            "pattern": "^(game\\.path|playing\\.fullscreen|settings\\.[a-z][a-z0-9_]{0,31})$"
        })
    }
}

// ------------------------------------------------------------------ settings

/// The emulator's settings and their rows in Settings.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub backend: SettingsBackend,
    #[serde(default)]
    pub definitions: Vec<Setting>,
    #[serde(default)]
    pub composite_rows: Vec<CompositeRow>,
    /// The rows, in order: update, custom_executable, rollback, a setting's key or a composite
    /// row's id.
    pub rows: Vec<RowRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SettingsBackend {
    /// The launcher stores the settings and passes them on the command line.
    LauncherCli,
    /// The emulator keeps its own settings.
    External,
}

/// One setting. Width and height have no limits beyond u32 today: the resolutions in a
/// composite row are choices, not limits.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Setting {
    U32 {
        key: SettingKey,
        default: u32,
        min: u32,
        max: u32,
        label: Option<String>,
        hint: Option<String>,
    },
    String {
        key: SettingKey,
        default: String,
        #[serde(default)]
        choices: Vec<Choice>,
        /// A stored value that is not one of the choices is kept and used.
        allow_existing_other: bool,
        label: Option<String>,
        hint: Option<String>,
    },
    Enum {
        key: SettingKey,
        default: String,
        choices: Vec<Choice>,
        /// What a stored value that is not one of the choices becomes.
        invalid_effective_value: InvalidValue,
        label: Option<String>,
        hint: Option<String>,
    },
    Bool {
        key: SettingKey,
        default: bool,
        label: Option<String>,
        hint: Option<String>,
    },
    /// A list of arguments.
    Argv {
        key: SettingKey,
        default: Vec<String>,
        editor: ArgvEditor,
        label: Option<String>,
        hint: Option<String>,
    },
}

impl Setting {
    pub fn key(&self) -> &SettingKey {
        match self {
            Setting::U32 { key, .. } | Setting::String { key, .. } | Setting::Enum { key, .. } | Setting::Bool { key, .. } | Setting::Argv { key, .. } => key,
        }
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            Setting::U32 { label, .. } | Setting::String { label, .. } | Setting::Enum { label, .. } | Setting::Bool { label, .. } | Setting::Argv { label, .. } => label.as_deref(),
        }
    }

    pub fn value_type(&self) -> ValueType {
        match self {
            Setting::U32 { .. } => ValueType::U32,
            Setting::String { .. } => ValueType::String,
            Setting::Enum { .. } => ValueType::Enum,
            Setting::Bool { .. } => ValueType::Bool,
            Setting::Argv { .. } => ValueType::Argv,
        }
    }
}

/// The type of a value a placeholder stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    Path,
    U32,
    String,
    Enum,
    Bool,
    Argv,
}

impl ValueType {
    pub fn article_name(self) -> &'static str {
        match self {
            ValueType::Path => "a path",
            ValueType::U32 => "a u32",
            ValueType::String => "a string",
            ValueType::Enum => "an enum",
            ValueType::Bool => "a bool",
            ValueType::Argv => "an argv list",
        }
    }
}

/// A value and the name Settings shows for it.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvalidValue {
    /// The setting's default.
    Default,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArgvEditor {
    /// One line of text, split like a shell does with quotes (no shell runs).
    LegacyQuotedArguments,
}

/// A row that edits more than one setting.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompositeRow {
    /// Width and height, chosen from a list.
    ResolutionPair {
        id: SettingKey,
        width: SettingKey,
        height: SettingKey,
        label: String,
        hint: Option<String>,
        choices: Vec<(u32, u32)>,
    },
}

impl CompositeRow {
    pub fn id(&self) -> &SettingKey {
        match self {
            CompositeRow::ResolutionPair { id, .. } => id,
        }
    }
}

/// A row in Settings.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum RowRef {
    /// The emulator's install and update row.
    Update,
    /// The path of the user's own build.
    CustomExecutable,
    /// Go back to the previous managed build.
    Rollback,
    /// A setting's key or a composite row's id.
    Item(SettingKey),
}

impl RowRef {
    pub const RESERVED: [&str; 3] = ["update", "custom_executable", "rollback"];
}

impl TryFrom<String> for RowRef {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Ok(match s.as_str() {
            "update" => RowRef::Update,
            "custom_executable" => RowRef::CustomExecutable,
            "rollback" => RowRef::Rollback,
            _ => RowRef::Item(SettingKey::try_from(s)?),
        })
    }
}

impl fmt::Display for RowRef {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            RowRef::Update => f.write_str("update"),
            RowRef::CustomExecutable => f.write_str("custom_executable"),
            RowRef::Rollback => f.write_str("rollback"),
            RowRef::Item(k) => f.write_str(k.as_str()),
        }
    }
}

impl JsonSchema for RowRef {
    fn schema_name() -> Cow<'static, str> {
        "RowRef".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "update, custom_executable, rollback, a setting's key or a composite row's id",
            "type": "string",
            "pattern": "^[a-z][a-z0-9_]{0,31}$"
        })
    }
}

// ------------------------------------------------------------------ compatibility

/// Where the community's compatibility list comes from, and how it is read.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "parser", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompatibilityParser {
    /// kytyps5.github.io's list: { "PPSA…": { "status", "platforms": { "linux": … } } }.
    KytyJsonV1(CompatibilitySource),
    /// shadPS4's list: { "CUSA…": { "os-linux": { "status": "status-ingame" } } }.
    ShadJsonV1(CompatibilitySource),
    /// No list.
    None {},
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompatibilitySource {
    pub url: HttpsUrl,
    /// The page the launcher opens to show the whole list.
    pub list_page: HttpsUrl,
    /// The cached copy's file name in the launcher's cache folder.
    pub cache: RelPath,
    pub refresh_seconds: u32,
    pub selection: CompatibilitySelection,
    pub report: ReportStrategy,
    /// The hosts `url`, `list_page` and the report form may use.
    pub allowed_hosts: Vec<Host>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilitySelection {
    /// The Linux result when there is one (on macOS too), else the best result elsewhere.
    LegacyLinuxElseBest,
}

/// The form a player's result goes to, pre-filled.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReportStrategy {
    KytyIssueV1 { url: HttpsUrl },
    ShadIssueV1 { url: HttpsUrl },
    None {},
}

// ------------------------------------------------------------------ content

/// How games for this emulator are recognized and where their updates and add-ons go.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub metadata: Metadata,
    pub layout: ContentLayout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Metadata {
    /// sce_sys/param.json
    Ps5ParamJson,
    /// sce_sys/param.sfo
    Ps4ParamSfo,
}

impl Metadata {
    pub fn console(self) -> Console {
        match self {
            Metadata::Ps5ParamJson => Console::Ps5,
            Metadata::Ps4ParamSfo => Console::Ps4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentLayout {
    /// The game's folder only.
    PlainFolder {},
    /// A game update beside the game as "<folder><patch_suffix>"; add-ons under `addons_root`.
    ShadPs4 { patch_suffix: String, addons_root: DataRoot },
}

// ------------------------------------------------------------------ session

/// How a running game is recognized and read.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// Executable names whose processes count as this emulator.
    pub process_names: Vec<String>,
    /// The custom build's file name counts too.
    pub include_custom_basename: bool,
    /// The options a game's path follows on a running emulator's command line.
    pub game_flags: PerOs<Vec<String>>,
    pub error_parser: ErrorParser,
    pub controls_profile: ControlsProfile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorParser {
    /// The text under "--- Error ---" in the log.
    KytyErrorBlock,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlsProfile {
    KytyKeyboardV1,
    ShadKeyboardV1,
    None,
}

// ------------------------------------------------------------------ update

/// Automatic updates of the managed build.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub policy: UpdatePolicy,
    /// How old the last check may be before the next one.
    pub check_seconds: u32,
    /// How often the launcher looks whether a check is due.
    pub poll_seconds: u32,
    pub automatic_default: bool,
    pub compare: VersionCompare,
    pub display_version: DisplayVersion,
    /// Lower installs first; a higher one waits for it.
    pub install_priority: u8,
    pub defer: Defer,
    /// The installed and the previous version.
    pub keep_versions: u8,
    pub rollback: Rollback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePolicy {
    LegacyKytyV1,
    LegacyShadV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VersionCompare {
    /// The commit at the end of the tag ("…-59a1760") against the running build's commit.
    TagCommit,
    /// The tag against the installed tag.
    TagEquality,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisplayVersion {
    /// "KytyPS5-2026-09-29-59a1760" shows as "2026-09-29 · 59a1760".
    KytyDateCommit,
    /// "v.0.18.0" shows as "0.18.0".
    StripVThenDot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Defer {
    /// An update found during a game installs when the game ends.
    AnyLiveGame,
    /// The same, except the install that starts a game the player chose.
    AnyLiveGameExceptInstallThenLaunch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rollback {
    /// Swap the current and previous builds; automatic updates skip the build left behind.
    SwapCurrentPreviousAndSkipDeparted,
}

// ------------------------------------------------------------------ macOS

/// What is different on macOS.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Macos {
    pub managed_support: MacManagedSupport,
    pub custom_support: MacCustomSupport,
    pub rosetta: Option<Rosetta>,
    pub moltenvk: Option<MoltenVk>,
    /// Whether the launcher removes the quarantine flag from downloads. It does not today.
    pub modify_quarantine: bool,
    #[serde(default)]
    pub environment_changes: BTreeMap<EnvName, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MacManagedSupport {
    /// The release has a macOS asset.
    MacosAsset,
    /// Today's code downloads the Linux asset on macOS too; it does not run there.
    LegacyLinuxAssetAttempt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MacCustomSupport {
    /// A custom build is used when the file is executable.
    ExecutableCheck,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rosetta {
    /// An x86-64 build: Apple Silicon needs Rosetta 2.
    RequiredForX86_64OnArm64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MoltenVk {
    /// The release ships MoltenVK.
    Bundled,
}
