//! Offline RuTracker release catalogs (PS4 and PS5 forums), merged into one Library.
//! Browser collection is a separate tool.

use crate::platform::Platform;
use crate::util::{atomic_write, cache_dir, now_secs};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;

pub const CATALOG_SCHEMA: u32 = 4;
/// One addon's contribution to the shared Library. The importer stays compiled; its
/// source, paths, refresh policy and optionality come from the validated document.
#[derive(Debug, PartialEq)]
struct Source {
    platform: Platform,
    url: String,
    file: String,
    cache: String,
    env: Option<String>,
    bundled: Vec<u8>,
    refresh_seconds: u64,
    optional: bool,
}

fn sources(emulators: &[crate::emulators::manifest::Emulator], root: &std::path::Path) -> Vec<Source> {
    use crate::emulators::safefs::{Dir, MAX_FILE};
    use std::ffi::OsStr;
    let mut sources = Vec::new();
    for emulator in emulators {
        for c in &emulator.catalogs {
            let bundled = match &c.bundled_snapshot {
                None => Vec::new(),
                Some(path) => match Dir::open(root)
                    .and_then(|dir| dir.dir(OsStr::new("emulators")))
                    .and_then(|dir| dir.dir(OsStr::new(emulator.id.as_str())))
                    .and_then(|dir| dir.read_relative(path.as_str(), MAX_FILE)) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        crate::log!("Addon {} catalog fallback {} {error}", emulator.id, path.as_str());
                        continue;
                    }
                },
            };
            let source = Source {
                platform: c.console.into(),
                url: c.source.as_str().to_string(),
                file: c.snapshot.as_str().to_string(),
                cache: c.cache.as_str().to_string(),
                env: c.override_env.clone(),
                bundled,
                refresh_seconds: c.refresh_seconds,
                optional: c.optional,
            };
            // Identical contributions from several emulators are imported once.
            if !sources.contains(&source) { sources.push(source); }
        }
    }
    // Conflicting cache owners are all rejected: folder scan order must not decide
    // which source overwrites the other. Other catalogs remain available.
    let mut conflicts = HashSet::new();
    for (i, a) in sources.iter().enumerate() {
        for b in &sources[i + 1..] {
            if a.cache == b.cache { conflicts.insert(a.cache.clone()); }
        }
    }
    sources.retain(|src| {
        if conflicts.contains(&src.cache) {
            crate::log!("RuTracker catalog {} rejected: cache {} is used by conflicting addon catalogs", src.url, src.cache);
            false
        } else { true }
    });
    sources
}

/// Immutable for this run, just like the addon registry. Catalogs are shared Library
/// data, so disabling an emulator does not remove its catalog contribution.
static SOURCES: LazyLock<Vec<Source>> = LazyLock::new(|| {
    let loaded = crate::emulators::startup::load_for_app();
    for problem in loaded.problems { crate::log!("Addon: {problem}"); }
    sources(loaded.registry.emulators(), &crate::emulators::startup::root())
});

#[cfg(test)]
fn source_of(platform: Platform) -> &'static Source {
    // Tests use shipped definitions, without reconciling the real user's addon folders.
    static SHIPPED: LazyLock<Vec<Source>> = LazyLock::new(|| {
        let root = tempfile::tempdir().unwrap();
        let loaded = crate::emulators::startup::load(
            root.path(), &crate::emulators::bundle::Embedded,
            &crate::emulators::lifecycle::RealFiles::default(),
            crate::emulators::document::Version::current(),
        );
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        sources(loaded.registry.emulators(), root.path())
    });
    SHIPPED.iter().find(|src| src.platform == platform).expect("every shipped console has a source")
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Game {
    pub id: i64,
    pub name: String,
    pub title: String,
    pub date: String,
    pub link: String,
    pub cover: String,
    pub trailer: String,
    pub genres: Vec<String>,
    pub mode: String,
    pub release: String,
    pub size: String,
    pub size_gb: Option<f64>,
    pub version: String,
    pub title_id: String,
    pub update: String,
    pub excerpt: String,
    pub description: Vec<String>,
    pub title_ids: Vec<String>,
    pub region: String,
    pub magnet: String,
    pub seeders: Option<u64>,
    pub leechers: Option<u64>,
    pub peers_observed: String,
    pub game_info: Value,
    /// The console this release is for (from the forum it was listed in).
    pub platform: Platform,
}

impl Game {
    pub fn detail(&self, key: &str) -> String { text(&self.game_info, key) }

    /// A PS5 release shipped as a PKG package, which the launcher can't install (PS5 games
    /// install from folders, archives and exFAT images). PS4 PKGs are extracted on install.
    pub fn is_pkg(&self) -> bool {
        static PKG: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)\bf?pkg\b").unwrap());
        self.platform == Platform::Ps5 && (PKG.is_match(&self.detail("format")) || PKG.is_match(&self.title))
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CatalogFile {
    pub updated: f64,
    pub schema: u32,
    pub source: String,
    pub fingerprint: String,
    pub snapshot_at: String,
    pub games: Vec<Game>,
}

impl CatalogFile {
    /// All catalog contributions from the emulator addons, merged into one Library.
    pub fn load() -> Self {
        merge(SOURCES.iter().map(|src| (src.platform, load_one(src))).collect())
    }

    pub fn stale(&self) -> bool {
        self.games.is_empty() || SOURCES.iter().any(|src| {
            let Ok(bytes) = source_bytes(src) else { return false };
            if bytes.is_empty() { return false; }
            let cache = read_cache(src);
            cache.schema != CATALOG_SCHEMA || cache.source != src.url || cache.games.is_empty()
                || now_secs() - cache.updated > src.refresh_seconds as f64 || fingerprint(&bytes) != cache.fingerprint
        })
    }
}

fn read_cache(src: &Source) -> CatalogFile {
    let cache: CatalogFile = std::fs::read(cache_dir().join(&src.cache)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    // Changing a declaration must not reuse games tagged for a different console.
    if cache.source == src.url && cache.games.iter().all(|game| game.platform == src.platform) { cache } else { CatalogFile::default() }
}

/// One console's catalog: the cache when it matches the source, else a fresh import.
fn load_one(src: &Source) -> CatalogFile {
    let cache = read_cache(src);
    let valid = cache.schema == CATALOG_SCHEMA && cache.source == src.url && !cache.games.is_empty();
    let bundled = || if src.bundled.is_empty() { CatalogFile::default() } else { import_for(src, &src.bundled).unwrap_or_default() };
    match source_bytes(src) {
        Ok(bytes) if bytes.is_empty() => CatalogFile::default(), // no snapshot collected
        Ok(bytes) if valid && cache.fingerprint == fingerprint(&bytes) => cache,
        Ok(bytes) => match import_for(src, &bytes).and_then(|file| save(src, file)) {
            Ok(file) => file,
            Err(error) => {
                crate::log!("RuTracker {} catalog import failed: {error}", src.platform.label());
                if valid { cache } else { bundled() }
            }
        },
        Err(error) => {
            crate::log!("RuTracker {} catalog source unavailable: {error}", src.platform.label());
            if valid { cache } else { bundled() }
        }
    }
}

/// One Library from every source, in the registry's deterministic order.
fn merge(files: Vec<(Platform, CatalogFile)>) -> CatalogFile {
    let mut files = files.into_iter();
    let mut merged = files.next().map(|(_, file)| file).unwrap_or_default();
    for (_, file) in files {
        merged.games.extend(file.games);
    }
    merged.games.sort_by(|a, b| b.id.cmp(&a.id));
    let mut seen = HashSet::new();
    merged.games.retain(|game| seen.insert((game.platform, game.id)));
    merged
}

/// Explicit override, then user data, then a source checkout's generated JSON.
/// The addon can select a bundled snapshot if none of those exist.
fn source_path(src: &Source) -> Option<PathBuf> {
    if let Some(path) = src.env.as_ref().and_then(std::env::var_os).filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let data = crate::util::data_dir().join("rutracker").join(&src.file);
    if data.is_file() { return Some(data); }
    let generated = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dist/rutracker").join(&src.file);
    generated.is_file().then_some(generated)
}

fn source_bytes(src: &Source) -> Result<Vec<u8>, String> {
    match source_path(src) {
        Some(path) => std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display())),
        None => Ok(src.bundled.clone()),
    }
}

fn fingerprint(bytes: &[u8]) -> String { sha1_smol::Sha1::from(bytes).digest().to_string() }
fn text(value: &Value, key: &str) -> String { value[key].as_str().unwrap_or("").trim().to_string() }

static SIZE_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)([\d.,]+)\s*(TB|GB|MB)").unwrap());
static TITLE_ID_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
    r"(?i)(?:^|[^A-Z])(?:P\s*)+S\s*A\s*(\d{5})|(?:^|[^A-Z])C\s*U\s*S\s*A\s*(\d{5})"
).unwrap());
static CLEAN_TITLE_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^(?:\[[^\]]*\]\s*)+").unwrap());
static MAGNET_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
    r"(?i)[?&]xt=urn:btih:(?:[a-f0-9]{40}|[a-z2-7]{32})(?:&|$)"
).unwrap());

pub fn find_title_id(texts: &[&str]) -> String {
    texts.iter().flat_map(|value| title_ids(value)).next().unwrap_or_default()
}

fn title_ids(value: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    TITLE_ID_RE.captures_iter(value).filter_map(|captures| {
        // Do not consume the separator: adjacent IDs like PPSA12345/PPSA54321
        // must both survive. Reject a prefix of an invalid six-digit ID.
        if value.as_bytes().get(captures.get(0)?.end()).is_some_and(u8::is_ascii_digit) { return None; }
        let id = captures.get(1).map(|id| format!("PPSA{}", id.as_str()))
            .or_else(|| captures.get(2).map(|id| format!("CUSA{}", id.as_str())))?;
        seen.insert(id.clone()).then_some(id)
    }).collect()
}

fn parse_size_gb(value: &str) -> Option<f64> {
    let captures = SIZE_RE.captures(value)?;
    let size: f64 = captures[1].replace(',', ".").parse().ok()?;
    Some(size * match captures[2].to_uppercase().as_str() { "TB" => 1024.0, "MB" => 1.0 / 1024.0, _ => 1.0 })
}

fn clean_name(value: &str) -> String {
    let name = CLEAN_TITLE_RE.replace(value, "");
    let name = name.split(" [").next().unwrap_or("").trim();
    name.strip_prefix("PSVR2 only ").or_else(|| name.strip_prefix("PSVR only ")).unwrap_or(name).to_string()
}

/// Import a PS5 forum snapshot.
#[cfg(test)]
pub fn import(bytes: &[u8]) -> Result<CatalogFile, String> {
    import_for(source_of(Platform::Ps5), bytes)
}

/// The PS4 forum also carries apps (YouTube, Media Player), system software and forum
/// threads (rules, chat); the Library lists games only.
fn is_game(title: &str, genre: &str, title_id: &str, magnet: &str) -> bool {
    static NOT_GAME: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(
        r"(?i)\b(firmware|exploits?|ps4hen|theme pack|psnow|ps now|shadps4)\b").unwrap());
    let app = ["app", "apps", "application", "applications", "program", "programs", "software", "media", "utility", "utilities"]
        .iter().any(|g| genre.eq_ignore_ascii_case(g));
    let thread = magnet.is_empty() && title_id.is_empty() && genre.is_empty();
    !app && !thread && !NOT_GAME.is_match(title)
}

fn import_for(src: &Source, bytes: &[u8]) -> Result<CatalogFile, String> {
    let report: Value = serde_json::from_slice(bytes).map_err(|error| format!("Invalid RuTracker JSON: {error}"))?;
    if report["complete"].as_bool() != Some(true) || report["source"].as_str() != Some(src.url.as_str()) {
        return Err(format!("Expected a complete {} forum listing ({}); partial results are not imported", src.platform.label(), src.url));
    }
    if report["translation"]["language"].as_str() != Some("en") {
        return Err("Translate the RuTracker snapshot to English before importing it".into());
    }
    let topics = report["topics"].as_array().ok_or("RuTracker topics array is missing")?;
    if topics.is_empty() || report["topic_count"].as_u64() != Some(topics.len() as u64) {
        return Err("RuTracker topic count is empty or does not match the listing".into());
    }
    let snapshot_at = text(&report, "collected_at");
    let mut seen = HashSet::new();
    let mut games = Vec::with_capacity(topics.len());
    for topic in topics {
        let id = topic["id"].as_str().and_then(|id| id.parse::<i64>().ok())
            .filter(|id| *id > 0).ok_or("Invalid RuTracker topic ID")?;
        if !seen.insert(id) { return Err(format!("Duplicate RuTracker topic ID: {id}")); }
        let info = topic["game_info"].clone();
        let title = text(topic, "title");
        let named = text(&info, "name");
        let name = clean_name(if named.is_empty() { &title } else { &named });
        if name.is_empty() { return Err(format!("Topic {id} has no game name")); }
        let raw_id = text(&info, "title_id");
        let ids = title_ids(&raw_id);
        let primary_id = ids.first().cloned().unwrap_or_else(|| find_title_id(&[&title]));
        let genre = text(&info, "genre");
        let size = text(topic, "size");
        let summary = text(&info, "release_summary");
        let images: Vec<String> = info["image_urls"].as_array().into_iter().flatten()
            .filter_map(Value::as_str).filter(|url| url.starts_with("https://") || url.starts_with("http://"))
            .map(str::to_string).collect();
        let candidate = text(topic, "magnet");
        if !is_game(&title, &genre, &raw_id, &candidate) { continue; }
        let magnet = if candidate.starts_with("magnet:?") && MAGNET_RE.is_match(&candidate) { candidate } else { String::new() };
        let multiplayer = text(&info, "multiplayer");
        let mode = match multiplayer.as_str() { "No" | "None" => "Single player".to_string(), "Yes" => "Multiplayer".to_string(), _ => multiplayer };
        // Release dates are not posting dates: do not invent "added" timestamps.
        let release_date = text(&info, "release_date");
        games.push(Game {
            id, name, title, date: text(topic, "published_at"), link: format!("https://rutracker.net/forum/viewtopic.php?t={id}"),
            cover: images.first().cloned().unwrap_or_default(), trailer: String::new(),
            genres: genre.split([',', '/', '|']).map(str::trim).filter(|genre| !genre.is_empty()).map(str::to_string).collect(),
            mode, release: if release_date.is_empty() { text(&info, "release_year") } else { release_date },
            size_gb: parse_size_gb(&size), size, version: text(&info, "version"), title_id: primary_id,
            update: text(&info, "release_update"), excerpt: summary.clone(),
            description: (!summary.is_empty()).then_some(summary).into_iter().collect(),
            title_ids: ids, region: text(&info, "region"), magnet,
            seeders: topic["seeders"].as_u64(), leechers: topic["leechers"].as_u64(),
            peers_observed: snapshot_at.clone(), game_info: info, platform: src.platform,
        });
    }
    // Topics, not title IDs, are the identity: retain regional/version variants.
    games.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(CatalogFile { updated: now_secs(), schema: CATALOG_SCHEMA, source: src.url.clone(),
        fingerprint: fingerprint(bytes), snapshot_at, games })
}

fn save(src: &Source, file: CatalogFile) -> Result<CatalogFile, String> {
    let bytes = serde_json::to_vec(&file).map_err(|error| error.to_string())?;
    atomic_write(&cache_dir().join(&src.cache), &bytes).map_err(|error| format!("Could not cache RuTracker catalog: {error}"))?;
    Ok(file)
}

/// Re-import local metadata only: no website requests, torrent client or browser.
/// Each addon declares whether its catalog is required or optional.
pub fn sync(progress: &dyn Fn(String)) -> Result<CatalogFile, String> {
    progress("Loading RuTracker snapshots".into());
    let mut files = Vec::new();
    for src in SOURCES.iter() {
        let optional = src.optional;
        let imported = source_bytes(src).and_then(|bytes| {
            if bytes.is_empty() && optional { return Ok(CatalogFile::default()); }
            import_for(src, &bytes).and_then(|file| save(src, file))
        });
        match imported {
            Ok(file) => files.push((src.platform, file)),
            Err(error) if optional => {
                crate::log!("RuTracker {} catalog import failed: {error}", src.platform.label());
                files.push((src.platform, read_cache(src)));
            }
            Err(error) => return Err(error),
        }
    }
    let file = merge(files);
    progress(format!("Loaded {} RuTracker releases", file.games.len()));
    crate::log!("RuTracker catalog loaded: {} release topics", file.games.len());
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addon() -> crate::emulators::manifest::Emulator {
        crate::emulators::document::parse(
            include_str!("../assets/addons/emulators/kyty/emulator.yaml"),
            crate::emulators::document::Version::current(),
        ).map(|mut emulator| {
            emulator.catalogs[0].bundled_snapshot = None;
            emulator
        }).unwrap()
    }

    fn sources(emulators: &[crate::emulators::manifest::Emulator]) -> Vec<Source> {
        super::sources(emulators, std::path::Path::new("/unused"))
    }

    #[test]
    fn a_user_addon_supplies_its_own_fallback_snapshot() {
        use crate::emulators::{discovery, document::Version};
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("emulators/custom-emulator");
        std::fs::create_dir_all(folder.join("releases")).unwrap();
        let text = include_str!("../assets/addons/emulators/kyty/emulator.yaml")
            .replace("id: kyty\n", "id: custom-emulator\n")
            .replace("https://rutracker.net/forum/viewforum.php?f=546", "https://example.com/releases")
            .replace("bundled_snapshot: catalog.json", "bundled_snapshot: releases/offline.json");
        std::fs::write(folder.join("emulator.yaml"), text).unwrap();
        let snapshot = serde_json::to_vec(&serde_json::json!({
            "source": "https://example.com/releases", "complete": true,
            "translation": { "language": "en" }, "topic_count": 1,
            "topics": [{ "id": "42", "title": "Custom game", "game_info": { "title_id": "PPSA00042" } }]
        })).unwrap();
        std::fs::write(folder.join("releases/offline.json"), &snapshot).unwrap();
        let scan = discovery::scan(root.path(), Version::current());
        assert!(scan.rejected.is_empty(), "{:?}", scan.rejected);
        let emulators = scan.addons.into_iter().map(|addon| addon.emulator).collect::<Vec<_>>();
        let configured = super::sources(&emulators, root.path());
        assert_eq!(configured.len(), 1);
        assert_eq!(configured[0].bundled, snapshot);
        assert_eq!(import_for(&configured[0], &configured[0].bundled).unwrap().games[0].name, "Custom game");

        // A resource cannot redirect the importer out of the addon folder.
        std::fs::remove_file(folder.join("releases/offline.json")).unwrap();
        let outside = root.path().join("outside.json");
        std::fs::write(&outside, snapshot).unwrap();
        std::os::unix::fs::symlink(outside, folder.join("releases/offline.json")).unwrap();
        assert_eq!(discovery::scan(root.path(), Version::current()).rejected.len(), 1);
        assert!(super::sources(&emulators, root.path()).is_empty());
    }

    #[test]
    fn addon_catalog_configuration_drives_the_importer() {
        let mut emulator = addon();
        let catalog = &mut emulator.catalogs[0];
        catalog.source = "https://example.com/forum".to_string().try_into().unwrap();
        catalog.snapshot = "custom.json".to_string().try_into().unwrap();
        catalog.cache = "custom-cache.json".to_string().try_into().unwrap();
        catalog.refresh_seconds = 120;
        catalog.optional = true;
        catalog.override_env = None;
        catalog.bundled_snapshot = None;
        let configured = sources(&[emulator]);
        let src = &configured[0];
        assert_eq!((&*src.file, &*src.cache, src.refresh_seconds, src.optional), ("custom.json", "custom-cache.json", 120, true));
        assert!(src.env.is_none() && src.bundled.is_empty());
        let bytes = fixture(serde_json::json!([{ "id": "100", "title": "[PS5] Example", "game_info": { "title_id": "PPSA12345" } }]));
        assert!(import_for(src, &bytes).is_err(), "the configured source is required, not the old hardcoded forum");
        let mut report: Value = serde_json::from_slice(&bytes).unwrap();
        report["source"] = serde_json::json!(src.url);
        assert_eq!(import_for(src, &serde_json::to_vec(&report).unwrap()).unwrap().games.len(), 1);
    }

    #[test]
    fn shared_sources_are_imported_once_and_cache_conflicts_are_rejected() {
        let a = addon();
        assert_eq!(sources(&[a.clone(), a.clone()]).len(), 1);
        let mut b = a.clone();
        b.catalogs[0].source = "https://example.com/another-forum".to_string().try_into().unwrap();
        assert!(sources(&[a.clone(), b.clone()]).is_empty());
        b.catalogs[0].cache = "another.json".to_string().try_into().unwrap();
        assert_eq!(sources(&[a, b]).len(), 2);
    }

    #[test]
    fn multiple_catalogs_for_one_console_merge_without_duplicate_topics() {
        let file = |ids: &[i64]| CatalogFile {
            games: ids.iter().map(|id| Game { id: *id, platform: Platform::Ps5, ..Game::default() }).collect(),
            ..CatalogFile::default()
        };
        let merged = merge(vec![(Platform::Ps5, file(&[1, 2])), (Platform::Ps5, file(&[2, 3]))]);
        assert_eq!(merged.games.iter().map(|g| g.id).collect::<Vec<_>>(), [3, 2, 1]);
    }
    fn fixture(topics: Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({ "source": source_of(Platform::Ps5).url, "complete": true,
            "translation": { "language": "en" }, "collected_at": "2026-09-30T00:56:18Z",
            "topic_count": topics.as_array().unwrap().len(), "topics": topics })).unwrap()
    }
    #[test]
    fn ps4_snapshot_imports_as_ps4_and_merges() {
        let ps4 = serde_json::to_vec(&serde_json::json!({ "source": source_of(Platform::Ps4).url, "complete": true,
            "translation": { "language": "en" }, "collected_at": "2026-10-02T00:00:00Z", "topic_count": 1,
            "topics": [{ "id": "900", "title": "[PS4] Firewatch [CUSA04118] [EUR]", "game_info": { "title_id": "CUSA04118" } }] })).unwrap();
        let file = import_for(source_of(Platform::Ps4), &ps4).unwrap();
        assert_eq!(file.games[0].platform, Platform::Ps4);
        assert_eq!(file.games[0].name, "Firewatch");
        assert!(import(&ps4).is_err(), "a PS4 snapshot is not accepted as the PS5 catalog");
        let ps5 = fixture(serde_json::json!([{ "id": "100", "title": "[PS5] Example [EUR]", "game_info": { "title_id": "PPSA12345" } }]));
        let merged = merge(vec![(Platform::Ps4, file), (Platform::Ps5, import(&ps5).unwrap())]);
        assert_eq!(merged.games.iter().map(|g| (g.id, g.platform)).collect::<Vec<_>>(), vec![(900, Platform::Ps4), (100, Platform::Ps5)]);
    }

    #[test]
    fn apps_system_software_and_forum_threads_are_not_games() {
        let bytes = fixture(serde_json::json!([
            { "id": "1", "title": "[PS5] YouTube [EUR] [2.23]", "magnet": "magnet:?xt=urn:btih:FFEA223A07AB3759DD3C037B0EB4B3992700553A", "game_info": { "genre": "App", "title_id": "PPSA01116" } },
            { "id": "2", "title": "[PS5] SHAREfactory [EUR]", "game_info": { "genre": "Media", "title_id": "PPSA00572" } },
            { "id": "3", "title": "[PS5] System firmware version 1.52 [RUS]", "game_info": { "genre": "Software" } },
            { "id": "4", "title": "[PS5] 4.05 + PS4HEN", "magnet": "magnet:?xt=urn:btih:FFEA223A07AB3759DD3C037B0EB4B3992700553B" },
            { "id": "5", "title": "[PS5] Fludilka" },
            { "id": "6", "title": "[PS5] Far Cry 5 [EUR/RUS] (v1.00)", "magnet": "magnet:?xt=urn:btih:FFEA223A07AB3759DD3C037B0EB4B3992700553C" },
            { "id": "7", "title": "[PS5] Patapon 3 [USA]", "game_info": { "genre": "Action Adventure, Rhythm, Music", "title_id": "PPSA12345" } }
        ]));
        let file = import(&bytes).unwrap();
        assert_eq!(file.games.iter().map(|g| g.id).collect::<Vec<_>>(), vec![7, 6]);
    }

    #[test]
    fn title_ids_and_sizes() {
        assert_eq!(find_title_id(&["P PSA16106 – EUR"]), "PPSA16106");
        assert_eq!(find_title_id(&["PPPSA30140 – EUR"]), "PPSA30140");
        assert_eq!(find_title_id(&["CUSA12345"]), "CUSA12345");
        assert_eq!(find_title_id(&["PS5 Digital Edition"]), "");
        assert_eq!(title_ids("PPSA02572 / PPSA02571* / PPSA02572"), vec!["PPSA02572", "PPSA02571"]);
        assert_eq!(title_ids("PPSA02572/PPSA02571"), vec!["PPSA02572", "PPSA02571"]);
        assert_eq!(find_title_id(&["PPSA123456"]), "");
        assert_eq!(parse_size_gb("57 GB"), Some(57.0));
        assert_eq!(parse_size_gb("100 MB"), Some(100.0 / 1024.0));
        assert_eq!(parse_size_gb("1.5 TB"), Some(1536.0));
    }
    #[test]
    fn missing_and_zero_peers_are_distinct_and_variants_survive() {
        let bytes = fixture(serde_json::json!([
            { "id": "100", "title": "[PS5] Example [USA]", "seeders": 0, "leechers": null,
              "game_info": { "title_id": "PPSA12345", "region": "USA", "version": "1.00" } },
            { "id": "101", "title": "[PS5] Example [EUR]", "game_info": { "title_id": "PPSA12345" } }
        ]));
        let file = import(&bytes).unwrap();
        assert_eq!(file.games.len(), 2);
        let game = file.games.iter().find(|game| game.id == 100).unwrap();
        assert_eq!(game.seeders, Some(0));
        assert_eq!(game.leechers, None);
        assert_eq!(game.title_id, "PPSA12345");
        assert_eq!(game.region, "USA");
        assert_eq!(game.name, "Example");
        assert!(game.date.is_empty());
    }
    #[test]
    fn rejects_partial_and_wrong_sources() {
        let mut report: Value = serde_json::from_slice(&fixture(serde_json::json!([
            { "id": "100", "title": "[PS5] Example" }
        ]))).unwrap();
        report["complete"] = Value::Bool(false);
        assert!(import(&serde_json::to_vec(&report).unwrap()).is_err());
        report["complete"] = Value::Bool(true);
        report["source"] = Value::String("https://example.com".into());
        assert!(import(&serde_json::to_vec(&report).unwrap()).is_err());
    }
    #[test]
    fn rejects_count_mismatch_duplicates_and_untranslated_sources() {
        let bytes = fixture(serde_json::json!([
            { "id": "100", "title": "[PS5] Example" },
            { "id": "100", "title": "[PS5] Duplicate" }
        ]));
        assert!(import(&bytes).is_err());
        let mut report: Value = serde_json::from_slice(&bytes).unwrap();
        report["topics"][1]["id"] = Value::String("101".into());
        report["topic_count"] = Value::from(3);
        assert!(import(&serde_json::to_vec(&report).unwrap()).is_err());
        report["topic_count"] = Value::from(2);
        report["translation"]["language"] = Value::String("ru".into());
        assert!(import(&serde_json::to_vec(&report).unwrap()).is_err());
    }
    #[test]
    fn bundled_snapshot_is_complete_and_keeps_magnets() {
        let src = source_of(Platform::Ps5);
        let file = import_for(src, &src.bundled).unwrap();
        assert_eq!(file.games.len(), 614);
        assert!(file.games.iter().all(|game| !game.magnet.is_empty()));
        assert!(file.games.iter().all(|game| game.seeders.is_some() && game.leechers.is_some()));
        assert!(file.games.iter().all(|game| !game.name.contains("[PS5]")));
        assert!(file.games.iter().all(|game| game.date.is_empty()));
    }
}
