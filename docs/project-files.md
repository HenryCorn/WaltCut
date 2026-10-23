# Project files

FilmCraft projects are `.fcproj` files: UTF-8 JSON with an explicit schema version. Reading and
writing them is `crates/format` (`waltcut-format`); the engine's `file.*` commands use it.

## Format

```json
{
  "format": "filmcraft.project",
  "schema_version": 16,
  "generator": "FilmCraft 0.1.0",
  "project": { "name": "…", "settings": { … }, "root": { … }, "items": { … }, "next_id": 48 },
  "view": { "open_sequences": [21, 50], "active_sequence": 50, "sequences": { "21": { "pps": 40.0, "scroll": 0.0, … } } }
}
```

- `schema_version` is the version of the whole document. This build writes
  `waltcut_format::SCHEMA_VERSION` and reads only that version.
- `project` is the serialized `waltcut_project::Project` (bins, media clips, sequences, tracks,
  clips, effects, keyframes, markers). Media is referenced by path, never embedded.
- Files are written compact (no indentation). A 2,000-clip project is about 1.4 MB.
- `view` (optional) is what was open when the project was saved: the Timeline's sequence tabs in
  order, the active one, each sequence's zoom, scroll and track heights, and each sequence's
  playhead (`waltcut_project::ProjectView`). It is beside the project, not in it: it is not part of the
  edit, never an undo step, and changing it does not mark the project as changed. Opening a
  project restores it when the Timeline preference "Restore open sequences when opening projects"
  is on. It is outside the schema version: older builds ignore it, ids that are not sequences and
  numbers out of range are dropped or clamped (a playhead also snaps to a frame of its sequence), and a `view` that cannot be read is ignored (the
  project opens on its first sequence, as it does without one or with no open sequence in it).
- A coordinate of an effect's point parameter may be `null`. Point parameters use NaN for "auto"
  (the frame centre, or the source centre for `anchor`), and JSON writes NaN as `null`; it reads back
  as "auto". Placing a clip resolves these points at once. The `anchor` of a clip whose media file was not
  found stays "auto", and the renderer centres it, until Link Media (`media.relink`, `media.autoRelink`) reads the file
  and with it the real picture size; a file that merely comes back at its old path does not
  change it. `null` anywhere else is a damaged file.

### One schema, no migrations

The animation-only reduction (epic #14) is a **breaking change**. `SCHEMA_VERSION` is 16, the
baseline of the reduced app, and there is no migration code: a project written before it does not
open. Earlier versions (1–15) are the FilmCraft and first WaltCut builds, up to the
[`v0.0.1-legacy`](https://github.com/HenryCorn/WaltCut/releases/tag/v0.0.1-legacy) release.

Opening an older file returns one error and does nothing else:

> this project was created by an earlier FilmCraft or WaltCut build (project schema v15); this
> build only opens schema v16. Open it with the v0.0.1-legacy release
> (https://github.com/HenryCorn/WaltCut/releases/tag/v0.0.1-legacy). The file has not been changed.

- **Never modified.** Reading a project never writes: the file, its `Auto-Save` siblings and its
  backups stay byte for byte as they were. There is no "upgrade on open" and no backup on first save.
- **Auto-saves.** `autosave::list_auto_saves` (and with it rotation) only see files of the current
  schema, and recovery (`scan_candidates`) offers only snapshots of the current schema. Older ones
  are not listed, not offered and **not deleted**.
- **No silent loss.** A field the model does not know is an error (`FormatError::UnknownField`
  with its path, for example `project.items.21.removed_feature`), not data that disappears on the
  next save. The check re-serializes the loaded project and compares keys; it covers every nested
  type, including enum variants and `flatten`. `view` is outside the schema and is not checked.
- Truncated, malformed, not-a-project, older-schema and newer-schema files all return `Err`
  (`crates/format`, `tests::hostile_documents_return_errors` and the fixtures tests).

To change the schema from here: bump `SCHEMA_VERSION`, add a fixture of the new shape under
`crates/format/tests/fixtures/`, and keep the golden round-trip test (`v16-edit.fcproj`) passing.
A change that older builds would load but then silently drop on save needs the bump too. Whether
to write a migration for a later schema is decided when that happens; none exists today.

### Newer files

A file with a `schema_version` newer than the build supports is **refused**, not half-read:

> this project was saved by a newer version of FilmCraft (project schema v17); this build reads up to
> v16. Update FilmCraft to open it.

Reading it partially and saving it back would silently drop whatever the newer version added.

## Saving is atomic

Every write of a project file, auto-save, recovery snapshot or preferences file goes through
`waltcut_format::atomic_write`:

1. write the bytes to a hidden temp file in the **same directory** (`.<name>.<pid>-<n>.tmp`);
2. `fsync` the temp file;
3. rename it over the target (an atomic replace on every supported OS);
4. `fsync` the directory so the rename itself is durable.

A crash, `kill -9` or power cut at any point leaves either the complete old file or the complete new
one. If a step fails, the temp file is removed and the old file is untouched.

## Commands

| Id | Menu | |
|---|---|---|
| `file.save {path?}` | File ▸ Save (⌘S) | write to the project's path (or `path`) |
| `file.saveAs {path}` | File ▸ Save As… (⇧⌘S) | write and adopt the new path |
| `file.saveCopy {path}` | File ▸ Save a Copy… (⌥⌘S) | write a copy; the project keeps its path and unsaved state |
| `file.revert` | File ▸ Revert | reload the last saved version, discarding changes |
| `file.open {path}` | File ▸ Open Project… (⌘O) | returns `{schemaVersion, path, missingMedia}`; an earlier or newer schema is an error |

In the UI, Save / Save As / Save a Copy without a `path` show a file dialog.

## Auto Save

Preferences ▸ Auto Save (Edit ▸ Preferences ▸ Auto Save…, ⌘,; FilmCraft ▸ Settings… on macOS) works
as follows:

| Setting | Key | Default |
|---|---|---|
| Automatically save projects | `autoSave.enabled` | on |
| Automatically Save Every N minute(s) | `autoSave.intervalMinutes` | 5 (1–1440) |
| Maximum Project Versions | `autoSave.maxVersions` | 20 (1–1000) |
| Auto Save also saves the current project(s) | `autoSave.saveCurrentProject` | off |
| Keep a recovery copy of unsaved changes | `autoSave.recoveryJournal` | on |
| Update it at least every N second(s) | `autoSave.recoveryIntervalSeconds` | 5 (1–600) |

At each interval, if the project changed since the last auto-save, a copy is written to the
`Auto-Save` folder next to the project as `<name>-YYYY-MM-DD_HH-MM-SS.fcproj` (local time). An
unsaved project uses `<data dir>/Auto-Save/`. After each write the oldest copies beyond the limit are
deleted. Only files named exactly `<name>-<timestamp>.fcproj` are ever deleted. An auto-save is an
ordinary project file: open it with File ▸ Open Project. `file.listAutoSaves` lists them, newest
first.

Project Settings ▸ Scratch Disks can send auto-saves (Project Auto Save), render previews (Video
Previews), captured and generated media (Captured Audio and Video)
and audio previews to another folder. A folder named in a project file is used only when it is
inside the project's own folder or was chosen in Scratch Disks on this computer (remembered in
`autoSave.scratchFolders`). Otherwise FilmCraft uses the default location and says so when the
project opens. Folders on another computer or a device are never used.

Preferences are stored in `<data dir>/preferences.json`. The `prefs.*` commands read and change them.

## Crash recovery

Auto-save alone can lose up to one interval of work. FilmCraft also keeps a **recovery journal**:

- Whenever the project has unsaved changes, a snapshot is written to
  `<data dir>/Recovery/<started>-<pid>[-<n>]/snapshot.fcproj`, plus `session.json` (time, revision, project
  name and path). It is written about 1 s after the last edit, or after at most
  `recoveryIntervalSeconds` while edits keep coming. Saving the project deletes the snapshot.
- Each running FilmCraft holds an OS file lock on `<session>/lock`. When the process dies (crash,
  `kill -9`, power loss) the OS releases the lock.
- At startup, every other session directory whose lock can be taken and that has a snapshot is a
  recovery candidate. FilmCraft asks: *"FilmCraft quit unexpectedly while 'X' had unsaved changes.
  Recover unsaved changes from 2026-10-01 00:02:09?"*, with the buttons **Recover**, **Not Now** and
  **Discard**. Stale directories without a snapshot are removed. Journals of running sessions are
  never touched.
- Recovering loads the snapshot as an **unsaved** project with its original path, so Save writes
  over the original file. The recovered state goes into the new session's own journal before the old
  journal is deleted.
- A normal quit with nothing unsaved removes the session directory. Quitting with unsaved changes
  (Quit, Cmd+Q or the window's close button) first asks *"Save changes to 'X' before closing?"*
  with **Cancel**, **Don't Save** and **Save** (Save on a never-saved project opens the Save dialog);
  Don't Save discards the changes and their snapshot. A quit that does not ask (the control
  channel's `app.quit`, the OS ending the session) keeps the snapshot, marked `cleanExit`, and FilmCraft offers it on the next launch ("FilmCraft was
  closed while … had unsaved changes").

Desktop flags: `--recover` recovers the newest candidate without asking. `--no-recover` starts
without asking; the changes stay available through File ▸ Recover Unsaved Changes…. `--data-dir
<dir>` (or `FILMCRAFT_DATA_DIR`) moves the data directory. The default is `~/Library/Application
Support/FilmCraft` on macOS, `%APPDATA%\FilmCraft` on Windows, and `$XDG_DATA_HOME/filmcraft` or
`~/.local/share/filmcraft` elsewhere. A `portable.txt` file next to the executable (the Windows
portable zip ships one) makes it `data` next to the executable instead.

### Cost

The UI thread only gives the worker thread an `Arc<Project>`: the immutable snapshot the engine
already keeps for undo, passed through a channel. JSON encoding and all file I/O run on the
`filmcraft-autosave` thread. Measured on the development Mac (release build) with a 2,000-clip project
(`cargo test --release -p waltcut-format --test fixtures -- --nocapture`):

| Step | Time | Size |
|---|---|---|
| encode, compact (what is written) | 2.5 ms | 1.4 MB |
| encode, pretty (not used) | 7.1 ms | 5.6 MB |
| decode | 4.6 ms | |
| atomic write + fsync (APFS SSD) | 30 ms | |

In the running app with the demo project, a recovery write measured 0.1 ms to encode and 27 ms to
write (two fsynced files).

## Commands

| Id | |
|---|---|
| `file.recoveryList` | candidates: `id`, `projectName`, `projectPath`, `savedAt`, `revision`, `cleanExit`… |
| `file.recover {id?}` | recover (newest when no `id`). From the UI, `{}` opens the recovery dialog |
| `file.discardRecovery {id?, all?}` | delete a candidate's journal |
| `file.autoSaveNow` | write an auto-save now (if anything is unsaved) |
| `file.autoSaveStatus` | prefs, folders, last auto-save / journal times, last encode/write ms and bytes |
| `file.listAutoSaves` | auto-save files of the current project, newest first |
| `prefs.get {key?}` / `prefs.set {key, value}` or `{values:{…}}` / `prefs.reset {category?}` | settings (below) |

UI automation ids: `recovery.recover`, `recovery.later`, `recovery.discard`, `recovery.item.<n>`;
`revert.yes`, `revert.no` (File ▸ Revert asks first when invoked without params).

## Settings

The Settings dialog (app menu ▸ Settings ▸ <category> on macOS, Edit ▸ Preferences
elsewhere; General is Cmd+,; the window is titled "Preferences"). The values are user preferences,
not project data: they live in `preferences.json` in the per-user data directory (written
atomically on every change) and never touch the `.fcproj` schema. The file carries a `version`
(currently 2); older files are migrated on load, unknown dropdown values, bad colours and
out-of-range numbers are repaired to defaults/limits, and an unreadable file falls back to the
defaults.

Every value is a dotted key whose first segment is its category: `timeline.stillImageDuration`,
`labels.colors.rose.name`. `prefs.get` / `prefs.set` / `prefs.reset {category?}` reach them from the
CLI, the control channel and MCP; `prefs.set` rejects values that are not one of a dropdown's
choices. `prefs.schema {category?}` lists every category with its fields (label, kind, choices,
range, unit, current value, and `wired`: whether FilmCraft acts on it yet). `app.settings.<category>`
opens the dialog on a page.

| Category | Takes effect |
|---|---|
| General | At Startup (Show Home = demo project, Open Most Recent, empty project; recent projects are remembered on open/save), Show Tool Tips |
| Appearance | Appearance Mode `appearanceMode` (`auto` = Sync with system / `dark` (default) / `light`; the header button and View ▸ Appearance ▸ Next Appearance Mode cycle it), Dark Theme `darkTheme` (`darkest` (default) / `dark`), Light Theme `lightTheme` (`light`), highlight colour, accessible contrast. `colorTheme` (Darkest / Dark / Light) is legacy input: setting it (older files and clients, View ▸ Appearance ▸ a theme) selects that theme and fixes the mode to its family; a file without `appearanceMode` migrates to its theme's fixed mode. It is kept in line with the choices: the theme a fixed mode shows, the dark theme in Auto |
| Audio | Automatch Time, Large Volume Adjustment, automation keyframe thinning (linear, minimum time) |
| Audio Hardware | device class (cpal host), output device, I/O buffer size, sample rate, force document rate, Output Mapping (programme L/R → device channels) |
| Auto Save | the auto-save ring and the crash-recovery journal |
| Color | stored only (display colour management, EDR monitoring, HDR graphics white) |
| Graphics | new text layers: smart quotes, ligatures, default font |
| Labels | the 16 label names and colours (timeline, Project panel, Edit ▸ Label), label defaults for imported movies / video / audio / stills and new sequences, Label Color Presets: JSON files (`{"name", "labels": [16 × {"name", "color": "#rrggbb"}]}`) in `<data dir>/Label Presets`, built in: "Default" (the labels FilmCraft ships with), "Bright" (16 saturated hues) and "Roles" (B-Roll, Footage, Dialogue… named so the label defaults land on matching roles) (`labels.presets`, `labels.importPreset {path}`, `labels.exportPreset {path}`, `labels.deletePreset {name}`; `labels.preset` is the selected one) |
| Media | Indeterminate Media Timebase (frame rate of stills), Default Media Scaling (Scale to / Set to frame size when a clip of another size is edited in) |
| Media Cache | location (`<data dir>/Media Cache` by default; unsaved projects' render previews live there), automatic deletion (older than N days / beyond N GB), Delete… (`mediaCache.clean {all?}`, `mediaCache.info`) |
| Memory | frame cache budget of the monitors and thumbnails |
| Playback | preroll / postroll (Play Around, trim loop), Step forward/back many |
| Plugins | stub (no plugins) |
| Timeline | video / audio transition and still image default durations (frames or seconds), playback auto-scrolling (none / page / smooth), snap playhead, return to beginning at playback end, play after rendering previews |
| Trim | Large Trim Offset, Selection tool picks roll (on the cut) / ripple (on the edge) without a modifier, playhead determines trim loop |

The remaining fields are kept and shown, but have no effect yet (`wired: false`).

UI automation ids: `settings.category.<id>`; `settings.<key>` for every control (for example
`settings.timeline.autoScroll`, `settings.labels.colors.rose.color`); `settings.<key>.<value>` for
the items of an open dropdown; `settings.<key>.browse`, `settings.<key>.hex`;
`settings.mediaCache.clean`; `settings.help`, `settings.reset` (this page back to its defaults),
`settings.cancel`, `settings.ok`. The open dialog's page and draft are `ui.settings` in `ui.inspect`;
`ui.set {"settings": {"page": id, "values": {key: value}}}` edits the draft (OK applies it).

## Media: offline and relinking

Media is referenced by path. Each imported file also records its **identity**
(`MediaClip::identity`): the size and a 64-bit fingerprint of the size, the first MiB and the last
MiB. Reading 2 MiB is cheap on any file, and it tells a moved original from a different take with
the same name. The field is optional (`skip_serializing_if`); a project without it skips the fingerprint check
when relinking.

### Offline media

- **Opening a project** checks every file (`media.findMissing`). Missing items don't stop the open.
  They render the **offline slate** (`crates/render/src/offline.rs`, our own design: a red striped
  field, a warning triangle, "MEDIA NOT FOUND", the file name and a hint), their audio is silent,
  and the app opens the **Link Media** dialog. `file.open` returns `missingMedia`.
- **Make Offline** (`media.makeOffline {items?, deleteFiles?}`) sets `MediaClip::offline`. The clip
  then shows "MEDIA SET OFFLINE" even though the file exists, until it is linked again. Files are
  deleted only with `deleteFiles: true`.
- The media pool caches sources per item **and** per reference (path, offline flag). So relinking,
  Make Offline and undoing either take effect on the next frame.
- **Stored paths.** Media paths read from a document (the project file, a project
  template, an interchange file) are opened only when they name a file on this computer or on
  the network share the document itself is on (`crates/engine/src/doc_path.rs`, built on
  `craft-docpath`); a relative path is taken from the document's folder. Other paths (another
  computer, a device) count as missing: the item shows the offline slate, the open says why, and
  the Link Media dialog lists it. A file imported or linked in this session is opened as given.

### Link Media

| Command | |
|---|---|
| `media.findMissing` | `{missing: [{item, name, fileName, path, status: missing\|offline, duration, startTimecode}]}` |
| `media.status {item?}` | per item: `online`, `missing`, `offline`, `unreadable` or `generated`, plus identity |
| `media.linkMedia` | File ▸ Link Media… (rescans; the UI opens the dialog) |
| `media.relink {item, path, force?, relinkOthers?=true, alignTimecode?, match?}` | check, link, then the others |
| `media.autoRelink {from, to}` / `{folder}` | batch relink: a folder-prefix remap, or a search of a folder tree |
| `media.search {folder, item?, exactName?=true}` | ranked candidates with `ok`, `identityMatch`, `problems` |
| `media.replaceFootage {item, path}` | relink without checks (Replace Footage) |
| `media.offlineAll` | close the dialog and leave the rest offline |

`match` selects which properties must agree. Each defaults as shown:

- `fileName` (true): the file name stem;
- `extension` (true);
- `clipId` (true): the fingerprint; a mismatch is refused with "not the same file…";
- `duration` (true): within one frame;
- `mediaStart` (false): the start timecode;
- `metadata` (true): frame size, frame rate and audio channels.

`force: true` links regardless. **Relink others automatically** works out the folder remap from the
file just linked, by dropping the common trailing path components (`/Volumes/A/shoot/a.mov` →
`/Users/me/shoot/a.mov` gives `/Volumes/A` → `/Users/me`). It applies the remap to the other missing
files and checks each one. It also looks in the new file's folder. Windows and macOS separators both
work. **Align Timecode** keeps clips on the same timecode when the new file starts at a different
timecode: the clips' source in-points, keyframes, marks and markers move with it
(`Project::shift_media_time`). One relink, together with the files it pulls along, is one undo step.

Link Media dialog automation ids: `linkMedia.row.<n>`,
`linkMedia.match.<fileName|extension|clipId|duration|mediaStart|metadata>`, `linkMedia.alignTimecode`,
`linkMedia.relinkOthers`, `linkMedia.folder`, `linkMedia.browse`, `linkMedia.exactName`,
`linkMedia.search`, `linkMedia.candidate.<n>`, `linkMedia.preview`, `linkMedia.link`,
`linkMedia.locate`, `linkMedia.offline`, `linkMedia.offlineAll`, `linkMedia.cancel`.
Make Offline: `makeOffline.keep|delete|ok|cancel`. In the Project panel, offline items get a
broken-link badge (`project.item.<id>.offline` in Icon view).

## Effect presets

Effect presets (Effects panel ▸ Presets; Effect Controls ▸ right-click an effect ▸ Save Preset…) are
not part of the project. Built-in presets are defined in code (`waltcut_engine::presets`); user
presets live in `<data dir>/effect-presets.json`, and `presets.export` / `presets.import` read and
write the same JSON format:

```json
{"format": "filmcraft.effect-presets", "version": 1, "presets": [
  {"name": "My Fade", "description": "", "keyframes": "AnchorToIn",
   "source_duration": 2540160000000, "source_size": [1920, 1080],
   "effects": [ /* EffectInstance, exactly as in .fcproj: effect, enabled, params */ ]}
]}
```

Keyframe times are relative to the in point of the clip the preset was saved from
(`source_duration` is that clip's media length in ticks). Applying re-times them: `Scale` stretches
them over the target clip, `AnchorToIn` / `AnchorToOut` keep their distance from the target's in /
out point. Point parameters are scaled by the target / source
frame-size ratio. Intrinsic effects (Motion, Opacity…) replace the clip's own instance; other
effects are added. Applying is one undo step; saving, renaming, deleting and importing change the
library, not the project, and are not undoable. Files with another `format`, a newer `version` or
unknown effect ids are refused. Effect instances reject unknown fields, including removed mask
data, rather than silently changing their appearance. Mask-dependent built-in presets (Soft
Vignette and Spotlight) are removed; other presets remain until the later #24 catalogue slice.
