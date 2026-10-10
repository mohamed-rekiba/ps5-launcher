# Plan: emulators from a configuration file

Status: **approved by the owner** (GO), with these choices, which replace the design's format
section (§3) where they differ:

1. **Format: YAML**, parsed strictly into typed Rust structs; unknown fields are rejected.
2. **Parser: `serde_norway`** (approved dependency).
3. **Schema: `schemars`** generates `assets/emulators.schema.json` from the Rust structs, so the
   schema never drifts from the code (a test fails when the committed file differs).

The embedded default is `assets/emulators.yaml`; the user override is
`~/.config/ps5-launcher/emulators.yaml`. Everything else below is the advisor's design as written.

---

**Use an embedded JSON manifest, a validated user override, and a small set of compiled Rust adapters.** Adding an emulator should require only data when its release, installation, launch, and compatibility conventions match an existing adapter. New conventions require a Rust adapter.


## 1. Architecture and the data/code split

Introduce a deep `emulators` module with this caller-facing interface:

```rust
registry.resolve(game, preferences) -> Result<EmulatorId>
manager.prepare_launch(id, game, preferences) -> Result<LaunchPlan>
manager.request_update(id, intent)
manager.request_rollback(id)
manager.snapshot(id) -> EmulatorSnapshot
```

`LaunchPlan` contains `executable: PathBuf`, `argv: Vec<OsString>`, `cwd: PathBuf`, environment changes, and an immutable build identity. Sessions execute it and retain its emulator id and build identity.

Internally:

| Becomes manifest data | Stays Rust code |
|---|---|
| Id, name, supported consoles, default selection | Resolution and validation |
| Release repository, channel, asset selectors | GitHub release fetching and response parsing |
| Executable paths, shared directories | Extraction, staging, symlinks, atomic activation |
| Ordered launch arguments, conditional flags, environment | Typed argument expansion and process spawning |
| Setting labels, types, defaults, choices | Editing, persistence, controller navigation |
| Compatibility URLs and parser selection | Source-specific parsing and report construction |
| Update intervals, startup policy, rollback policy | Scheduling, concurrency, recovery |
| Requirements and keyboard reference data | OS capability detection, focus, stop, process discovery |
| Game installation convention selection | Metadata parsing, package validation and safe publication |

Use enums with exhaustive dispatch, rather than dynamic plugins:

- `ReleaseSource`: `github_latest`, `manual`.
- `Installer`: `archive_flat_or_one_child`, `zip_appimage`.
- `VersionStrategy`: `kyty_help_git_date`, `shad_help_banner`.
- `CompatibilityParser`: `kyty_json_v1`, `shad_json_v1`, `none`.
- `ReportStrategy`: `kyty_issue_v1`, `shad_issue_v1`, `none`.
- `ContentLayout`: `plain_folder`, `shad_ps4`.
- `SettingsBackend`: `launcher_cli`, `external`.
- `DiscoveryStrategy`: platform-native process discovery with declared names and game flags.

Do **not** implement DMG installation until needed. It would be a compiled `dmg_app_bundle` adapter with mount/copy/unmount cleanup, not a manifest command sequence.

Keep console metadata separate from emulator definitions. PS5 JSON and PS4 SFO are game formats, not emulator identities. Today library metadata determines the console, while title prefixes provide another inference path: [library.rs:20](src/library.rs:20), [platform.rs:28](src/platform.rs:28).

Resolution should be:

1. Explicit per-game emulator selection.
2. User’s default emulator for the console.
3. Embedded default for the console.
4. Clear error if that selection is disabled or incompatible.

An explicit missing custom executable must fail visibly; it must not silently select another emulator.

## 2. Concrete manifest schema

The following is a proposed **valid JSON document**, with compiled adapter names defined above. `legacy_any` deliberately means “the existing selector ignores host architecture”; it does not claim that an asset runs on every architecture.

```json
{
  "schema_version": 1,
  "defaults": {
    "ps5": "kyty",
    "ps4": "shadps4"
  },
  "emulators": [
    {
      "id": "kyty",
      "display_name": "KytyPS5",
      "consoles": ["ps5"],
      "enabled": true,
      "release": {
        "kind": "github_latest",
        "repo": "KytyPS5/KytyPS5",
        "asset_selection": "first_match",
        "targets": [
          {
            "os": "linux",
            "host_arch": "legacy_any",
            "name_all": ["Linux", "x86_64"],
            "name_suffixes": [".tar.gz"],
            "case_sensitive": true
          },
          {
            "os": "macos",
            "host_arch": "legacy_any",
            "name_any": ["macos", "darwin", "osx"],
            "name_suffixes": [".tar.gz", ".tgz", ".zip"],
            "case_sensitive": false
          }
        ],
        "allowed_hosts": [
          "api.github.com",
          "github.com",
          "release-assets.githubusercontent.com",
          "objects.githubusercontent.com"
        ],
        "integrity": {
          "kind": "github_asset_sha256",
          "required": false
        }
      },
      "install": {
        "kind": "archive_flat_or_one_child",
        "root": {"base": "launcher_data", "relative": "kyty"},
        "versions": "versions",
        "current": "current",
        "state": "state.json",
        "format_detection": "zip_magic_else_tar",
        "executable": {
          "linux": "kyty_emulator",
          "macos": "kyty_emulator"
        },
        "shared_data": {
          "kind": "cwd_symlinks",
          "directory": "data",
          "entries": [
            "_SaveData", "_PipelineCache", "_DownloadData",
            "_TempData", "_Textures", "_Patches"
          ],
          "archive_merge": "copy_missing",
          "custom_import": "copy_into_empty"
        },
        "probe": {
          "kind": "kyty_help_git_date",
          "args": ["--help"],
          "timeout_seconds": 3,
          "cwd": "executable_parent"
        }
      },
      "custom_build": {
        "allowed": true,
        "empty_path": "legacy_kyty_detection",
        "discovery": "legacy_kyty_locations_v1",
        "version": "kyty_help_git_date"
      },
      "launch": {
        "cwd": "executable_parent",
        "inherit_environment": true,
        "env": {},
        "env_remove": [],
        "arguments": [
          {"literal": "--game"},
          {"value": "game.path", "type": "path"},
          {"literal": "--screen-width"},
          {"value": "settings.width", "type": "u32"},
          {"literal": "--screen-height"},
          {"value": "settings.height", "type": "u32"},
          {"literal": "--present-mode"},
          {"value": "settings.present_mode", "type": "string"},
          {"literal": "--video-out-resolution"},
          {"value": "settings.video_out", "type": "enum"},
          {"when": "playing.fullscreen", "emit": ["--fullscreen"]},
          {"when": "settings.amd_cpu", "emit": ["--amd-cpu"]},
          {"spread": "settings.extra_args", "type": "argv"}
        ]
      },
      "settings": {
        "backend": "launcher_cli",
        "definitions": [
          {
            "key": "width", "type": "u32", "default": 1920,
            "min": 0, "max": 4294967295, "row": false
          },
          {
            "key": "height", "type": "u32", "default": 1080,
            "min": 0, "max": 4294967295, "row": false
          },
          {
            "key": "present_mode", "type": "string",
            "default": "Mailbox",
            "choices": [
              ["Mailbox", "Mailbox (low latency)"],
              ["Fifo", "Fifo (V-Sync)"],
              ["Immediate", "Immediate (uncapped)"]
            ],
            "allow_existing_other": true,
            "label": "Present mode",
            "hint": "Try V-Sync if the picture tears"
          },
          {
            "key": "video_out", "type": "enum", "default": "Title",
            "choices": [
              ["Title", "Game default"],
              ["FullHd", "1080p (Full HD)"],
              ["Uhd", "4K (Ultra HD)"]
            ],
            "invalid_effective_value": "default",
            "label": "Game output resolution",
            "hint": "The screen resolution the game is told it runs on. The game renders at this size."
          },
          {
            "key": "amd_cpu", "type": "bool", "default": false,
            "label": "AMD CPU patches",
            "hint": "Only for AMD processors"
          },
          {
            "key": "extra_args", "type": "argv", "default": [],
            "editor": "legacy_quoted_arguments",
            "label": "Extra KytyPS5 arguments",
            "hint": "For example --vblank-frequency 60 --tessellation"
          }
        ],
        "composite_rows": [
          {
            "kind": "resolution_pair",
            "keys": ["width", "height"],
            "label": "Resolution",
            "hint": "The size of the window. What the game is told is Game output resolution",
            "choices": [
              [1280, 720], [1600, 900], [1920, 1080],
              [2560, 1440], [3840, 2160]
            ]
          }
        ],
        "rows": [
          "update", "custom_executable", "resolution_pair",
          "video_out", "present_mode", "amd_cpu",
          "extra_args", "rollback"
        ]
      },
      "compatibility": {
        "parser": "kyty_json_v1",
        "url": "https://kytyps5.github.io/data/compatibility.json",
        "list_page": "https://kytyps5.github.io/",
        "cache": "compatibility.json",
        "refresh_seconds": 21600,
        "selection": "legacy_linux_else_best",
        "report": {
          "kind": "kyty_issue_v1",
          "url": "https://github.com/KytyPS5/KytyPS5/issues/new?template=kytyps5-game-emulation.yaml"
        },
        "allowed_hosts": ["kytyps5.github.io", "github.com"]
      },
      "content": {
        "metadata": "ps5_param_json",
        "layout": "plain_folder"
      },
      "session": {
        "process_names": ["kyty_emulator", "kyty_emulator.exe"],
        "include_custom_basename": true,
        "game_flags": {"linux": ["--game"], "macos": ["--game"]},
        "error_parser": "kyty_error_block",
        "controls_profile": "kyty_keyboard_v1"
      },
      "update": {
        "policy": "legacy_kyty_v1",
        "check_seconds": 21600,
        "poll_seconds": 1800,
        "automatic_default": true,
        "compare": "tag_commit",
        "display_version": "kyty_date_commit",
        "install_priority": 0,
        "defer": "any_live_game",
        "keep_versions": 2,
        "rollback": "swap_current_previous_and_skip_departed"
      },
      "macos": {
        "rosetta": "required_for_x86_64_on_arm64",
        "moltenvk": "bundled",
        "modify_quarantine": false,
        "environment_changes": {}
      }
    },
    {
      "id": "shadps4",
      "display_name": "shadPS4",
      "consoles": ["ps4"],
      "enabled": true,
      "release": {
        "kind": "github_latest",
        "repo": "shadps4-emu/shadPS4",
        "asset_selection": "first_match",
        "targets": [
          {
            "os": "linux",
            "host_arch": "legacy_any",
            "name_prefix": "shadps4-linux",
            "name_suffixes": [".zip"],
            "case_sensitive": true
          },
          {
            "os": "macos",
            "host_arch": "legacy_any",
            "name_prefix": "shadps4-linux",
            "name_suffixes": [".zip"],
            "case_sensitive": true,
            "support": "legacy_attempt_only"
          }
        ],
        "allowed_hosts": [
          "api.github.com",
          "github.com",
          "release-assets.githubusercontent.com",
          "objects.githubusercontent.com"
        ],
        "integrity": {
          "kind": "github_asset_sha256",
          "required": false
        }
      },
      "install": {
        "kind": "zip_appimage",
        "root": {"base": "launcher_data", "relative": "shadps4"},
        "versions": "versions",
        "current": "current",
        "state": "state.json",
        "zip_member_suffix": ".AppImage",
        "member_selection": "first_match",
        "extracted_name": "shadps4.AppImage",
        "executable": {
          "linux": "squashfs-root/AppRun",
          "macos": "squashfs-root/AppRun"
        },
        "shared_data": {
          "kind": "external",
          "root": {"base": "xdg_data", "relative": "shadPS4"}
        },
        "probe": {
          "kind": "shad_help_banner",
          "args": ["--help"],
          "timeout_seconds": 10,
          "stdout_contains": "shadPS4"
        }
      },
      "custom_build": {
        "allowed": true,
        "empty_path": "managed",
        "discovery": "none",
        "version": "custom_build_label"
      },
      "launch": {
        "cwd": "executable_parent",
        "inherit_environment": true,
        "env": {},
        "env_remove": [],
        "arguments": [
          {"literal": "-g"},
          {"value": "game.path", "type": "path"},
          {"literal": "-f"},
          {"value": "playing.fullscreen", "type": "bool_string"}
        ]
      },
      "settings": {
        "backend": "external",
        "definitions": [],
        "composite_rows": [],
        "rows": ["update", "custom_executable", "rollback"]
      },
      "compatibility": {
        "parser": "shad_json_v1",
        "url": "https://github.com/shadps4-compatibility/shadps4-game-compatibility/releases/latest/download/compatibility_data.json",
        "list_page": "https://github.com/shadps4-compatibility/shadps4-game-compatibility/issues",
        "cache": "compatibility-shadps4.json",
        "refresh_seconds": 21600,
        "selection": "legacy_linux_else_best",
        "report": {
          "kind": "shad_issue_v1",
          "url": "https://github.com/shadps4-compatibility/shadps4-game-compatibility/issues/new?template=game_compatibility.yml"
        },
        "allowed_hosts": [
          "github.com",
          "release-assets.githubusercontent.com",
          "objects.githubusercontent.com"
        ]
      },
      "content": {
        "metadata": "ps4_param_sfo",
        "layout": "shad_ps4",
        "patch_suffix": "-patch",
        "addons_root": {
          "base": "xdg_data",
          "relative": "shadPS4/addcont"
        }
      },
      "session": {
        "process_names": ["shadps4"],
        "include_custom_basename": false,
        "game_flags": {"linux": ["-g"], "macos": []},
        "error_parser": "kyty_error_block",
        "controls_profile": "shad_keyboard_v1"
      },
      "update": {
        "policy": "legacy_shad_v1",
        "check_seconds": 21600,
        "poll_seconds": 1800,
        "automatic_default": true,
        "compare": "tag_equality",
        "display_version": "strip_v_then_dot",
        "install_priority": 1,
        "defer": "any_live_game_except_install_then_launch",
        "keep_versions": 2,
        "rollback": "swap_current_previous_and_skip_departed"
      },
      "macos": {
        "managed_support": "legacy_linux_asset_attempt",
        "custom_support": "executable_check",
        "environment_changes": {}
      }
    }
  ]
}
```

Several choices here intentionally preserve less obvious behavior:

- **Asset matching:** Kyty Linux matching is case-sensitive and demands `Linux`, `x86_64`, and `.tar.gz`; macOS matching is case-insensitive and ignores architecture. shad matches the first `shadps4-linux*.zip`, without architecture filtering or OS switching. Token predicates reproduce this more clearly than regex; an optional Rust-regex selector can support future definitions. [kyty.rs:93](src/kyty.rs:93), [shad.rs:75](src/shad.rs:75).
- **Installation:** Kyty accepts an executable at archive root or one immediate child, probes it, links six shared directories, and imports custom-build data only into empty destinations. shad extracts one AppImage and runs its extraction mode, avoiding FUSE. Both retain current and previous versions. [kyty.rs:235](src/kyty.rs:235), [kyty.rs:286](src/kyty.rs:286), [shad.rs:161](src/shad.rs:161).
- **Launch:** Kyty’s extra arguments come last and can override preceding arguments. shad receives only `-g PATH -f true|false`; it ignores Kyty’s resolution, present mode, AMD patches, video-out, and extra arguments. Both inherit environment and run from the executable’s parent. [sessions.rs:150](src/sessions.rs:150), [sessions.rs:336](src/sessions.rs:336).
- **Settings:** fullscreen remains a shared Playing preference, defaulting to `true`. Width/height currently have no restrictions beyond `u32`; the five resolutions are UI choices, not validation limits. Preserve existing arbitrary present-mode strings; invalid video-out values effectively become `Title`. [config.rs:50](src/config.rs:50), [settings.rs:388](src/settings.rs:388), [settings.rs:450](src/settings.rs:450).
- **Compatibility:** both parsers prefer Linux reports even on macOS. Keep this during parity migration, then separately fix selection and OS labels. Source parsing, status mappings, report fields, system information and log excerpts belong in their adapters. [compat.rs:181](src/compat.rs:181), [compat.rs:225](src/compat.rs:225), [compat.rs:318](src/compat.rs:318).
- **Session discovery:** Linux recognizes both game flags; macOS currently parses only `--game`. Custom basename discovery currently covers Kyty only. The schema records those limitations rather than pretending shad discovery already works everywhere. [sessions.rs:174](src/sessions.rs:174), [sessions.rs:433](src/sessions.rs:433).
- **macOS:** Kyty ships x86-64 plus MoltenVK and needs Rosetta on Apple Silicon. There are no emulator-specific environment modifications today. Do not invent `DYLD_*` or Vulkan overrides. [README.md:77](README.md:77), [kyty.rs:322](src/kyty.rs:322).

The named legacy update policies are finite Rust state machines:

- `legacy_kyty_v1`: startup checks when due or missing, including checks when automatic installation is off; commit-based comparison; first-run boot installation under the current boot condition; custom builds can switch to managed with data import.
- `legacy_shad_v1`: automatic checks stop for custom builds and wait for boot/Kyty installation; missing managed builds install in background or on first play; first-play installation resumes the selected game.
- Both honor rollback skip tags for automatic updates and allow manual requests to override them.

These differences are documented in [kyty_ui.rs:30](src/kyty_ui.rs:30), [kyty_ui.rs:119](src/kyty_ui.rs:119), [boot.rs:57](src/boot.rs:57), and [shad_ui.rs:34](src/shad_ui.rs:34).

`controls_profile` selects embedded reference data transcribed from today’s keyboard tables, including their footnotes. Ultimately those tables should also be manifest data; Slint renders a model. Today they are separate hard-coded layouts: [app.slint:1769](ui/app.slint:1769).

## 3. Format and crate decision

**Choose JSON using the existing `serde_json` dependency.** This adds no dependency requiring owner approval: [Cargo.toml:10](Cargo.toml:10).

| Option | Assessment |
|---|---|
| `serde_yaml` | Deprecated; upstream archived the repository and announced no further planned releases. [Upstream release notice](https://github.com/dtolnay/serde-yaml/releases). |
| `serde_yml` | Reject. RustSec identifies unsoundness and an archived project, with no patched versions. [RUSTSEC-2025-0068](https://rustsec.org/advisories/RUSTSEC-2025-0068). |
| `serde_norway` | Viable YAML alternative: RustSec lists it as maintained. It still adds a parser stack including `unsafe-libyaml-norway`; maintained does not establish that it is vulnerability-free. [RustSec](https://rustsec.org/advisories/RUSTSEC-2025-0068), [crate manifest](https://raw.githubusercontent.com/cafkafk/serde-norway/main/Cargo.toml). |
| `toml` | Viable, maintained Rust project, with comments and readable configuration. Its nested argument objects would be more verbose here, and it adds a dependency. [Current upstream](https://github.com/toml-rs/toml). |
| `serde_json` | Already available; matches existing configuration, state and compatibility formats. Best initial choice. |

JSON’s lack of comments is acceptable with a checked-in schema and documented example. If comments later become a demonstrated requirement, TOML would justify an approval request. There is no reason to request that approval now.

## 4. Embedding, overrides and validation

Embed the complete default:

```rust
const EMBEDDED: &str =
    include_str!("../assets/emulators.json");
```

Load once before config migration and update startup; retain an immutable registry snapshot.

**Override location:** use `util::config_dir().join("emulators.json")` on both Linux and macOS:

- `$XDG_CONFIG_HOME/ps5-launcher/emulators.json`, when set.
- Otherwise `~/.config/ps5-launcher/emulators.json`.

This preserves the project’s existing macOS path convention. Do not introduce a second `~/Library/Application Support` location in this migration. [util.rs:39](src/util.rs:39).

Use explicit operations, not recursive field merging:

```json
{
  "schema_version": 1,
  "operations": [
    {"op": "disable", "id": "kyty"},
    {"op": "add", "definition": {"id": "another-emulator"}},
    {"op": "replace", "definition": {"id": "shadps4"}}
  ]
}
```

The abbreviated definitions above must be complete in a real file.

Rules:

- `add`: id must not exist.
- `replace`: id must exist; replaces the whole definition.
- `disable`: retains definition and persisted preferences but removes availability.
- One operation per id; reject duplicates.
- Optional `defaults` replaces named console defaults.
- Replacement never deletes installations or settings.
- Apply and validate the entire override transactionally.

Validate schema version, unknown fields, duplicate ids, adapter availability, paths, hostnames, selectors, setting defaults, placeholder types, row references and enabled defaults. Reject duplicate JSON object keys explicitly; ordinary deserialization into a map can conceal them.

Example error:

```text
~/.config/ps5-launcher/emulators.json:
emulators[1].launch.arguments[3]:
settings.video_out is an enum; bool_string is incompatible
```

Malformed, unreadable or unsupported user files cause **complete fallback to embedded definitions**, one toast, and a detailed log. Keep the file intact. Invalid embedded data is a build/test failure; if encountered at runtime, stop emulator initialization with a clear error.

Schema version `1` is independent of app version. Reject unsupported versions; implement explicit migrations when semantics change. Ship a JSON Schema for editors, while Rust validation remains authoritative.

## 5. Safety and process handling

- Expand arguments directly into `OsString`; never invoke a shell. Support only literals, typed values, boolean conditions and argv expansion—no expressions, command substitutions or hooks.
- Keep `PathBuf` values as paths, without lossy string round-trips. Reject NULs, unresolved references, invalid enum values and unreasonable manifest sizes.
- Preserve existing extra-argument tokenization during migration. It is quote splitting, not shell execution. New stored values are argv arrays. [sessions.rs:588](src/sessions.rs:588).
- Restrict downloads to HTTPS and exact declared hosts. Validate the initial URL **and every redirect before requesting it**. Reject URL credentials and unexpected ports; do not use suffix checks such as “ends with github.com.”
- Validate release tags as single safe path components before constructing version directories.
- Reject archive traversal, escaping symlinks, special files, excessive expansion and ambiguous payloads. Treat these as explicit hardening changes rather than preserving unsafe extraction behavior.
- Integrity options: GitHub asset SHA-256, pinned SHA-256, or a declared checksum asset. Missing checksums remain permitted for the two parity definitions; present malformed or mismatched checksums fail installation. Today checksum verification is conditional. [kyty.rs:206](src/kyty.rs:206).
- Signature verification requires a compiled verifier and a pinned public key. Do not offer a “verified” signature option until implemented; a checksum fetched from the same release is not an independent signature.
- Extracted AppImages and version probes execute downloaded code. Verify integrity first; bound execution time and output.
- Retain process groups, logging, single-game enforcement and stop escalation in session code. Linux focus/close uses `xdotool`; macOS uses fixed AppleScript with numeric PIDs and Accessibility-dependent fallback. These are OS adapters, not user scripts. [sessions.rs:16](src/sessions.rs:16), [sessions.rs:56](src/sessions.rs:56), [sessions.rs:391](src/sessions.rs:391).

Installation and launch must share a mutation lease so activation cannot race with starting a game. Block rollback during a live game too; that is a deliberate safety improvement over the current direct rollback action.

## 6. Configuration and identity migration

Use typed preferences, separate from the manifest:

```rust
struct EmulatorPreferences {
    source: BuildSource, // Managed | Custom { executable: PathBuf }
    settings: BTreeMap<String, SettingValue>,
}

enum SettingValue {
    Bool(bool),
    U32(u32),
    String(String),
    Argv(Vec<String>),
}
```

Persist `config_version`, `emulators`, console defaults and optional game selections. Keep fullscreen and the common automatic-update switch at app scope.

| Existing field | Destination |
|---|---|
| `emulator` | `emulators.kyty.source` |
| `shad_emulator` | `emulators.shadps4.source` |
| `width`, `height` | `emulators.kyty.settings.width/height` |
| `present_mode`, `video_out` | Corresponding Kyty setting |
| `amd_cpu`, `extra_args` | Corresponding Kyty setting |
| `fullscreen` | Shared `playing.fullscreen` |
| Three update switches | Common `updates.automatic`, preserving existing normalization |

Most emulator settings are currently **unprefixed**, not `kyty_*`: [config.rs:9](src/config.rs:9).

Migration requirements:

1. Deserialize a dedicated legacy DTO before existing `Config::load()` can rewrite paths.
2. Preserve current managed-path classification and discovery ordering. Kyty searches PATH, known build folders and bounded deeper locations; shad’s blank path means managed. [config.rs:100](src/config.rs:100), [config.rs:239](src/config.rs:239).
3. Preserve a missing explicit path as stored data; retain any legacy effective fallback separately until the user saves a new selection.
4. Preserve extra-argument source text alongside its migrated argv for recovery.
5. Preserve unknown setting values and settings for disabled/missing emulators; validate effective values without discarding stored data.
6. For mixed old/new files, explicit new keys win; legacy keys fill only missing keys.
7. Preserve the existing automatic-update rule: any old false switch makes the common switch false. [config.rs:117](src/config.rs:117).
8. Make migration idempotent; atomically save on normal persistence, retaining a legacy backup.

Keep existing `kyty` and `shadps4` installation roots and `state.json` files. No directory migration is necessary.

Also migrate ratings and skipped prompts to `(emulator_id, console, title_id)`. Rename `MyResult.kyty` to `emulator_version`, preserving its value. Infer legacy emulator ids from title prefixes, retaining unresolved records. Today ratings are keyed only by title id and can collide once multiple emulators support one console. [results.rs:12](src/results.rs:12).

Capture the build identity **at launch**, then pass it through `Session` and `Ended`. Today rating code probes the current configured build afterward, which can misattribute a session: [app.rs:1272](src/app.rs:1272), [app.rs:2918](src/app.rs:2918).

## 7. Phased migration and tests

| Phase | Change; app remains usable | Required validation |
|---|---|---|
| **1. Registry, read-only** | Define types, embed manifest, validate it and overrides. Existing modules still execute everything. | Default parity fixtures; OS/arch asset matrix; malformed overrides; unknown adapters/placeholders; default resolution. |
| **2. Preferences and identity** | Add legacy migration and generic accessors. Introduce emulator id/build identity in sessions and ratings. Keep old launch path. | Old/minimal/mixed configs; missing paths; opt-outs; unusual resolutions; arbitrary present modes; extra-argument round-trip; idempotence; old results. |
| **3. Manifest launch** | Produce `LaunchPlan`; remove console branching from command construction. Resolve emulator in app launch flow. | Exact argv/cwd/env comparisons against old behavior; fullscreen false; flag ordering; spaces/non-UTF-8 paths; inert shell metacharacters; missing custom builds; fake-process lifecycle. |
| **4. Releases/install/update** | Move shared downloader/archive utilities out of Kyty/shad. Implement adapters and generic manager, initially with legacy policy strategies. | Release fixtures; archive wrapper cases; AppImage fixtures; checksum/redirect failures; save imports; activation failure recovery; rollback/skip; pending update cancellation; install-then-launch; concurrency leases. |
| **5. Generated Settings and UI** | Replace emulator-specific `SId`s with stable `(emulator_id, setting_key/action)` identities. Generate rows, search entries, status text and controls models. | Row order/defaults; enum cycling; composite resolution edits; OSK identity surviving row rebuilds; search/focus; busy/error states; disable/add emulator. |
| **6. Compatibility/content** | Key compatibility by emulator; dispatch reports by recorded emulator id. Route patch/DLC destinations through content-layout adapter. | Both existing parsers/report URLs; cache fallback; same title on two emulators; result precedence; patch/DLC publication; existing installer regressions. |
| **7. Remove hard-coded modules** | Delete wrappers and module registrations; retain source-specific adapters inside `emulators`. | Linux/macOS builds; end-to-end launch/update/rollback; third-emulator fixture; remaining emulator-name audit. |

Important scope details:

- `update.rs` keeps launcher self-update behavior; replace its Kyty downloader dependency and emulator-specific refresh calls. [update.rs:194](src/update.rs:194).
- `pkgx.rs` remains a package-extractor module with its pinned artifact and write guard. Share download/AppImage mechanics, but do not treat this tool as a playable emulator. [pkgx.rs:12](src/pkgx.rs:12), [pkgx.rs:223](src/pkgx.rs:223).
- PS4 patches belong beside games and DLC currently goes into shad’s external data root. This must become an explicit selected content layout, not another global constant. [installer.rs:971](src/installer.rs:971), [pkgx.rs:243](src/pkgx.rs:243).
- `ui/osk.slint`, `ui/power.slint` and `ui/widgets.slint` do not themselves encode emulator selection; “shadow” matches are unrelated. Avoid unnecessary edits there.
- Keep behavioral fixes separate from parity: macOS shad availability, host-OS compatibility preference, custom shad discovery, strict asset ambiguity and rollback blocking should each be explicit changes.

The largest risks are losing custom-build settings/data, racing activation with launch, misidentifying wrapper processes, and associating reports with the wrong emulator. The phases isolate those risks.

## 8. Third-emulator test

Use **a second PS4 emulator with a custom executable** as the first acceptance test. This proves extensibility without needing a new console parser.

A fixture definition would use:

```json
{
  "id": "ps4-lab",
  "display_name": "PS4 Lab",
  "consoles": ["ps4"],
  "release": {"kind": "manual"},
  "custom_build": {
    "allowed": true,
    "empty_path": "error",
    "discovery": "none",
    "version": "custom_build_label"
  },
  "launch": {
    "cwd": "executable_parent",
    "inherit_environment": true,
    "env": {},
    "env_remove": [],
    "arguments": [
      {"literal": "--game"},
      {"value": "game.path", "type": "path"}
    ]
  },
  "settings": {
    "backend": "external",
    "definitions": [],
    "composite_rows": [],
    "rows": ["custom_executable"]
  },
  "compatibility": {"parser": "none"},
  "content": {
    "metadata": "ps4_param_sfo",
    "layout": "plain_folder"
  }
}
```

This is an abbreviated **test fixture**, not a claim about a real emulator’s CLI. Complete its session fields and validate it with the same schema.

Acceptance criteria:

- It appears in Settings and the PS4 emulator selector without app or Slint branches.
- One game can select it while other PS4 games retain shadPS4.
- Its launch plan and rating identity remain separate.
- It does not inherit shad’s patch/DLC convention.
- Disabling it preserves preferences and produces a clear error for explicitly selected games.

RPCS3 would exercise a different limit: `Platform` currently supports only PS4/PS5, and the library/install code only understands those game formats. A PS3 emulator therefore needs PS3 metadata/content adapters and platform integration before a manifest can make it usable. A manifest cannot supply unsupported game-format semantics. [platform.rs:5](src/platform.rs:5), [library.rs:20](src/library.rs:20).