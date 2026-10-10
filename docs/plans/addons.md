# Plan: addons for emulators and themes

Status: **approved**. The owner wrote the plan below; the advisor (Codex) reviewed it and settled the
open points. The advisor's fixes and decisions follow the plan and win where they differ.

Progress: the revised Phase 1 (below) is **done** for emulators, slices 1 to 5, in
`src/emulators/`. Not done yet: themes, the Settings actions in the UI, the on/off and choice
preferences (Phase 2), the startup call (Phase 2), image size and SVG loading, and the "Show the
folder" route. Addon files have size limits (16 MiB a file, 64 MiB a folder).
Also not done: the lifecycle's changes (making folders, staging writes, renames, removals) go
by path after an lstat check of each parent, so a parent swapped for a link between the check
and the change is not caught. Reads already go through folder handles. The fix is mkdirat,
renameat and unlinkat through handles (TODOs in `lifecycle.rs`).

The catalog importer now consumes addon configuration at runtime. Its first use reconciles
and scans the addons and retains an immutable snapshot for the run. Other emulator consumers
still need the Phase 2 wiring. Each addon may declare a `catalogs` list: console, RuTracker
source URL, snapshot filename, cache filename, refresh interval, optionality, optional
environment override name, and optional `bundled_snapshot` path inside the addon folder. Snapshot paths are relative
to `<data>/rutracker/` (or `dist/rutracker/` in a checkout); cache paths are relative to the
launcher cache folder. The importer is offline: source URLs identify snapshots and are never
fetched. Parsing stays compiled into the launcher. The default bundle copies its snapshots
into each addon as `catalog.json`; the importer reads only the user copy, through folder
handles with size limits and link checks. User-added addons can provide their own fallback
file, including in a subfolder, without Rust changes.

Catalogs contribute to the shared Library even when the declaring emulator is disabled.
Identical declarations are imported once; different declarations using the same cache path
are all rejected and logged. Multiple catalogs per console merge with duplicate console/topic
IDs removed. An addon with no `catalogs` section contributes no releases; installed games and
emulator resolution are independent of this section. Changes take effect on the next start.

## Decisions
1. Emulators and themes are addons: one folder per addon, each with its own YAML file. The launcher finds them by scanning folders.
2. YAML, parsed strictly into typed Rust structs: unknown fields and duplicate keys rejected, YAML aliases and tags blocked. serde_norway.
3. JSON Schemas generated from the Rust structs with schemars (emulator.schema.json, theme.schema.json). A test fails on drift.
4. The launcher reads only the user's folders. The defaults are copied there on first start, so the user can change them.
5. Addons are data and media only. No scripts or code. Emulator behaviour comes from adapters compiled into the launcher; an addon picks one by name.

## Folders
~/.local/share/<app>/
  emulators/kyty/emulator.yaml (+ icon.svg, controls.yaml)
  emulators/shadps4/emulator.yaml
  emulators/<user-added>/emulator.yaml
  themes/default/theme.yaml (+ backgrounds/, fonts/, icons/, sounds/)
  themes/<user-added>/theme.yaml
  addons-state.yaml (which default each copy came from)
Master defaults ship with the launcher, never read at run time: inside the binary (every install type, AppImage, macOS), or under /usr/share/<app>/ (deb, rpm, OS image).

## First start
1. Copy each default emulator and the default theme into the user's folders.
2. Record each copy's default version and checksum in addons-state.yaml.

## Every start
1. Scan emulators/*/emulator.yaml and themes/*/theme.yaml.
2. Validate each addon on its own: schema_version, optional min_launcher_version, fields, adapter names, declared download hosts, placeholder types, every path inside the addon's own folder.
3. Skip a broken addon, show one clear message (name + what is wrong). Others still load.
4. Emulators: Settings lists each, on/off. One default per console.
5. Themes: Settings > Appearance lists them. Broken/missing chosen theme -> built-in default theme.

## After a launcher update
- User's copy unchanged (checksum matches record): replace with new default, update record.
- User changed it: keep user's, write new default next to it (emulator.yaml.new, or default.new/ for a theme), toast "A newer default exists for X".
- Copy deleted: don't bring back. Settings offers "Restore missing defaults".
- User-added addons never touched.

## Settings actions
Emulator: on/off, "Reset to default", "Show the folder". Theme: select, "Reset to default theme". "Restore missing defaults" for both.

## Emulator addons: what stays in code
Adapters: release source, installer, version check, compatibility parser, report, content layout, settings backend, process discovery. Typed launch templates never run through a shell. Downloads only from declared hosts; checksums checked when given.

## Themes
Colours, fonts, backgrounds, icons, sounds, and existing layout options. Screens are compiled Slint; no runtime .slint loading.

## Shared code
One addon loader for both kinds: copying defaults, version+checksum records, scan, per-addon validation, path containment, update rules.

## Changes to the existing emulator plan
Phase 1 changes from "embedded manifest plus override file" to this addon loader. Types, strict parsing, schema, adapters stay. Phases 2-7 stay. Acceptance test: drop in a folder for a second PS4 emulator, no Rust/UI change, it appears and launches. Themes are a separate task after the emulator phases, reusing the loader.

## Open points
- The app's new name (folder paths and package names depend on it). Couchbox was suggested; check it is free.
- Master defaults from the binary everywhere, or from /usr/share on Linux packages.
- How the user reaches the folder in OS mode: file sharing, or "Show the folder" in the fallback desktop.

---

## The advisor's fixes

1. **Folder checksum.** SHA-256 over the sorted relative paths and the length-delimited contents
   of every file in the addon folder, media included. Added and deleted files change it.
   Symlinks and special files are rejected. A checksum detects edits; it does not prove trust.
2. **Updates replace the whole folder.** A newer default for a changed addon goes to a separate
   folder that the scan never reads, keyed by addon ID and default revision, so `default.new/`
   never loads as another theme. The same offer is not repeated at every start.
3. **Copies are transactions.** Stage and validate the full folder on the same filesystem,
   publish it with a rename, then write the state file atomically. A journal lets the next start
   finish or undo an interrupted copy. It never overwrites an edit, and it never counts a
   partial copy as a user addon. One lock serializes changes.
4. **Deletion needs a durable record.** The state records each shipped ID, its default revision,
   its digest, and whether the user deleted it. A new shipped default is copied once; a recorded
   deletion is never undone by itself. Missing or broken state: keep every folder as it is, and
   offer "Restore missing defaults".
5. **"One default per console".** The shipped console defaults live in the bundle metadata, not
   in each addon. The choice is: the game's choice, then the user's console choice, then the
   shipped console default. Scan order never decides. A disabled, deleted or broken addon keeps
   the user's choices and settings, and the launcher says the choice is unavailable. A console
   can have no usable default for a while.
6. **On/off lives in the preferences**, with the console and game choices, so "Reset to
   default" on the files does not reset it. Phase 2 adds this; the existing `emulator` and
   `shad_emulator` fields hold executable paths, not console defaults.
7. **Path containment covers addon resources** (icons, controls). Install folders, game paths
   and custom executables keep their typed path rules from the emulator plan. The addon ID must
   match its folder name. Two addons with the same ID are both rejected. Staging folders are
   skipped.
8. **Data and media are untrusted.** Limits on YAML size, depth and list lengths; duplicate keys,
   aliases and tags are rejected before expansion. Limits on media file size and image size. SVG
   files load with no external references and no scripts. Declared download hosts limit
   requests; they do not prove who published a file.
9. **Versions.** `schema_version` selects the meaning of the document. `min_launcher_version` is
   an extra gate, compared as semantic versions. An addon that fails either is left untouched
   and unavailable. Neither replaces the shipped default revision.
10. **A compiled emergency theme** is the one exception to "user folders only": a missing or
    broken chosen theme uses it, without recreating deleted files.

## Decisions on the open points

- **The name: not decided yet; the owner decides.** Before that, check the trademark registers,
  existing software and package names, domains, repositories and app IDs. The addon work starts
  with `ps5-launcher` behind `util::APP_NAME`. A rename later needs a migration for data, config
  and cache that can run twice safely: keep the old folder until the copy succeeds, never
  overwrite an existing target, and keep the emulator IDs and installs. Package and OS names
  change in a separate, coordinated rename.
- **The master defaults are in the binary everywhere.** That ties each default to the adapters
  compiled with it, and a bootc upgrade or rollback cannot mix them. `/usr/share` is not a
  second source. An older launcher skips an addon it cannot read, without rewriting it.
- **OS mode reaches the folder through "Show the folder" in the fallback desktop**, with a
  route a controller can use. No network sharing in Phase 1.

## The revised Phase 1 (each slice test-first)

The finished embedded-manifest work keeps its typed definitions, adapter enums, strict parsing,
validators, generated schema, parity fixtures and pure resolution logic. The single manifest
splits into one document per addon. The add/replace/disable override file and the whole-registry
fallback go away. The embedded data stays only as the source of the default copies.

1. **The document contract:** strict parsing, the schema drift test, adapter and placeholder
   checks; KytyPS5 and shadPS4 match today's behaviour. **Done** (`document.rs`, `bundle.rs`,
   `assets/addons/emulators/`).
2. **Discovery:** an injected addon root; a scan in a fixed order; ID conflicts, containment, and
   one broken addon rejected on its own. **Done** (`discovery.rs`).
3. **The default lifecycle:** an injected default source and file operations; folder digests,
   the first copy, edits, deletions, collisions and interrupted copies. **Done**
   (`lifecycle.rs`). A default's revision is the digest of its shipped files. Proposals go to
   `proposals/emulators/<id>/<revision>/`, outside the scanned folder.
4. **The registry:** injected preferences; the choice order, an unavailable default, and a
   second PS4 emulator from a test folder. **Done** (`registry.rs`, `testdata/addons/`).
5. **Startup:** reconcile, scan, an immutable snapshot and one combined list of problems. The
   launcher still launches through the existing code. **Done** (`startup.rs`). Catalog loading
   now calls it lazily; Phase 2 shares its snapshot with the other consumers in `main`.

Showing the second emulator in the UI and launching it stay with the later phases. Themes come
after the emulator phases.

## Emulator media (advisor's decision)

An emulator addon carries its **identity**; the theme owns the **look**.

| Owner | Media |
|---|---|
| Emulator addon | a square icon, an optional transparent logo, an optional background, the controls data, and the media notices |
| Theme | colours, fonts, gradients, panel backgrounds, sounds, controller button pictures, the loading animation, the layout choice; it may also override an emulator's icon, logo or background by emulator id |
| Console (PS4, PS5) | the id, name, abilities and controller meaning, stored once. A theme may add console badges and backgrounds by console id |
| Game | the covers, logos, screenshots and hero art the launcher already fetches. Showcase stays about games; the emulator's background only fills in when a game has no art |

New optional fields in `emulator.yaml`, next to `icon` and `controls`: `logo`, `background` and
`media_notices` (all `RelPath`). No emulator colours, fonts, sounds or layout fields.

```text
emulators/<id>/emulator.yaml, controls.yaml, media/icon.svg, media/logo.png,
               media/background.webp, MEDIA-NOTICES.md, licenses/<licence>.txt
themes/<id>/theme.yaml, backgrounds/, icons/, emulators/<emulator-id>/, consoles/<console-id>/
```

- **Choice order for each picture:** the theme's override, then the addon's file, then a generic
  icon (or the name as text) or the theme's background, then the compiled emergency picture. The
  emulator's name always shows. A missing or broken picture gives one problem message and never
  stops a launch.
- **Formats:** SVG only for icons and logos, static only: no scripts, no external references,
  no embedded active content, no animation. PNG and WebP for every picture. Sizes: icons
  512×512, logos within 1024×256, backgrounds 1920×1080. Decoding stops above 4096 pixels on a
  side or 8 megapixels, on top of the 16 MiB file and 64 MiB folder limits.
- **Licences:** shadPS4's logo files are GPL-2.0-or-later (their `REUSE.toml`); credit Xphalnos
  and keep the notices. KytyPS5 is GPL-2.0-only, but its logo's licence is not confirmed: ship a
  generic icon until it is.
- **When:** after Phase 2, before Phase 5's generated Settings rows: the safe picture loading
  and the fallbacks, then the cleared pictures. Theme overrides come with the theme loader.
