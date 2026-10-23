//! `.waltcut` project files.
//!
//! A project file is UTF-8 JSON with an explicit schema version:
//!
//! ```json
//! { "format": "waltcut.project", "schema_version": 16, "generator": "WaltCut 0.1.0", "project": { … } }
//! ```
//!
//! - **One schema, no migrations.** [`SCHEMA_VERSION`] is the only version this build reads and
//!   writes. The animation-only reduction (epic #14) broke the format on purpose: a file from an
//!   earlier schema is refused with [`FormatError::TooOld`], which names the release that can still
//!   open it ([`LEGACY_RELEASE`]); a file from a newer schema with [`FormatError::TooNew`]. Reading
//!   never writes: the refused file and its auto-save siblings are left byte for byte as they were.
//! - **Nothing is dropped silently.** A field this build does not know is an error
//!   ([`FormatError::UnknownField`]), not data that vanishes on the next save.
//! - **Atomic saves.** [`atomic_write`] writes a temp file in the destination directory, fsyncs it,
//!   renames it over the target and fsyncs the directory: a crash or power cut during a save leaves
//!   either the old file or the new one, never a torn mix.
//! - **Auto-save naming/rotation** lives in [`autosave`] (`<name>-YYYY-MM-DD_HH-MM-SS.waltcut` in an
//!   `Auto-Save` folder next to the project, oldest pruned beyond the version limit). Auto-saves of
//!   another schema are neither listed nor pruned.
//!
//! Schema history:
//! - **v1–v15**: WaltCut and the first WaltCut builds, up to [`LEGACY_RELEASE`]. Not readable.
//! - **v16**: the reduction baseline. The model is unchanged from v15; the bump makes every earlier
//!   file fail with a clear message instead of loading into a model that is about to lose fields.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod atomic;
pub mod autosave;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use waltcut_project::{Project, ProjectView};

pub use atomic::atomic_write;

/// The `format` tag written in every envelope.
pub const FORMAT_ID: &str = "waltcut.project";

/// The tag files carried before the product rename. They are still recognised as projects, so an
/// older one gets the "written by an earlier build" message instead of "not a project".
pub const LEGACY_FORMAT_ID: &str = "filmcraft.project";

/// The schema version this build writes and reads. Nothing else is readable.
pub const SCHEMA_VERSION: u32 = 16;

/// The last build that opens projects written before the reduction.
pub const LEGACY_RELEASE: &str = "v0.0.1-legacy";

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("not a WaltCut project: {0}")]
    NotAProject(String),
    #[error("this project was saved by a newer version of WaltCut (project schema v{found}); this build reads up to v{supported}. Update WaltCut to open it.")]
    TooNew { found: u32, supported: u32 },
    #[error(
        "this project was created by an earlier FilmCraft or WaltCut build (project schema v{found}); this build only opens schema v{supported}. Open it with the v0.0.1-legacy release (https://github.com/HenryCorn/WaltCut/releases/tag/v0.0.1-legacy). The file has not been changed."
    )]
    TooOld { found: u32, supported: u32 },
    #[error("the project holds a field this build does not know (`{0}`); it was not opened so nothing is lost on the next save")]
    UnknownField(String),
    #[error("project file is damaged: {0}")]
    Corrupt(String),
}

/// A project read from disk.
#[derive(Debug)]
pub struct Loaded {
    pub project: Project,
    /// Schema version found in the file (always [`SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The `generator` string of the writer, if recorded.
    pub generator: Option<String>,
    /// What was open when the file was saved (sequence tabs and how each was shown), when the
    /// file records it and it can be read.
    pub view: Option<ProjectView>,
}

/// Writer identification stored in the envelope.
pub fn generator() -> String {
    format!("WaltCut {}", env!("CARGO_PKG_VERSION"))
}

#[derive(Serialize)]
struct EnvelopeOut<'a> {
    format: &'static str,
    schema_version: u32,
    generator: String,
    project: &'a Project,
    #[serde(skip_serializing_if = "Option::is_none")]
    view: Option<&'a ProjectView>,
}

#[derive(Deserialize)]
struct EnvelopeIn {
    #[serde(default)]
    generator: Option<String>,
    project: Project,
    /// Kept as JSON until the project itself has loaded: a view that cannot be read is dropped,
    /// it never stops a project from opening.
    #[serde(default)]
    view: Option<Value>,
}

impl EnvelopeIn {
    fn loaded(self, schema_version: u32) -> Loaded {
        let view = self.view.and_then(|v| serde_json::from_value(v).ok());
        Loaded { project: self.project, schema_version, generator: self.generator, view }
    }
}

/// Just enough of a document to know which schema it is.
#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    schema_version: Option<u32>,
    /// Bare (pre-envelope) project: its `version` field.
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    items: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    root: Option<serde::de::IgnoredAny>,
}

/// Serialize a project in the current schema. `pretty` = indented (human-diffable; used for the
/// project file itself), compact otherwise (recovery snapshots).
pub fn encode(project: &Project, pretty: bool) -> Vec<u8> {
    encode_with_view(project, None, pretty)
}

/// [`encode`], with what is open in the editor (`view`) stored beside the project. The view is
/// outside the schema's promises: builds that do not know it ignore it, and it is dropped rather
/// than migrated when it cannot be read.
pub fn encode_with_view(project: &Project, view: Option<&ProjectView>, pretty: bool) -> Vec<u8> {
    let env = EnvelopeOut { format: FORMAT_ID, schema_version: SCHEMA_VERSION, generator: generator(), project, view };
    let r = if pretty { serde_json::to_vec_pretty(&env) } else { serde_json::to_vec(&env) };
    // Serializing plain data (string keys, finite numbers or null) cannot fail.
    r.unwrap_or_default()
}

fn probe_version(p: &Probe) -> Result<u32, FormatError> {
    if let Some(v) = p.schema_version {
        if let Some(f) = &p.format
            && f != FORMAT_ID
            && f != LEGACY_FORMAT_ID
        {
            return Err(FormatError::NotAProject(format!("format is `{f}`, expected `{FORMAT_ID}`")));
        }
        return Ok(v);
    }
    // Before the envelope (schema 1) a file was the bare Project object, with a `version` field
    // that only counts as newer when it is beyond this build.
    if p.items.is_some() && p.root.is_some() {
        let v = p.version.unwrap_or(1);
        return Ok(if v > SCHEMA_VERSION { v } else { 1 });
    }
    Err(FormatError::NotAProject("no `schema_version` and not a legacy project".into()))
}

/// The schema version a project file declares, without loading it. `None` when it is not a
/// project. Reads the front of the file first (the envelope is written before the project).
pub fn peek_schema(bytes: &[u8]) -> Option<u32> {
    let front = bytes.get(..PEEK_BYTES.min(bytes.len())).unwrap_or_default();
    scan_schema(front).or_else(|| {
        let probe: Probe = serde_json::from_slice(bytes).ok()?;
        probe_version(&probe).ok()
    })
}

/// [`peek_schema`] for a file on disk; reads only its front unless that is not enough.
pub fn peek_file_schema(path: &Path) -> Option<u32> {
    use std::io::Read;
    let mut front = Vec::new();
    std::fs::File::open(path).ok()?.take(PEEK_BYTES as u64).read_to_end(&mut front).ok()?;
    scan_schema(&front).or_else(|| peek_schema(&std::fs::read(path).ok()?))
}

/// Is `path` a project file this build can open?
pub fn is_current_file(path: &Path) -> bool {
    peek_file_schema(path) == Some(SCHEMA_VERSION)
}

const PEEK_BYTES: usize = 4096;

/// `"schema_version": N` in the front of a document.
fn scan_schema(front: &[u8]) -> Option<u32> {
    const KEY: &[u8] = b"\"schema_version\"";
    let at = front.windows(KEY.len()).position(|w| w == KEY)?;
    let rest = front.get(at + KEY.len()..)?;
    let mut digits = rest.iter().copied().skip_while(|b| b.is_ascii_whitespace() || *b == b':').take_while(u8::is_ascii_digit);
    let first = digits.next()?;
    digits.try_fold(u32::from(first - b'0'), |n, d| n.checked_mul(10)?.checked_add(u32::from(d - b'0')))
}

/// Read a project file written by this schema.
///
/// Refuses, without touching anything: damaged JSON, files that are not projects, earlier and
/// newer schemas, and projects that hold a field this build does not know.
pub fn decode(bytes: &[u8]) -> Result<Loaded, FormatError> {
    let doc: Value = serde_json::from_slice(bytes).map_err(|e| FormatError::Corrupt(e.to_string()))?;
    let probe = Probe::deserialize(&doc).map_err(|e| FormatError::NotAProject(e.to_string()))?;
    let found = probe_version(&probe)?;
    if found > SCHEMA_VERSION {
        return Err(FormatError::TooNew { found, supported: SCHEMA_VERSION });
    }
    if found < SCHEMA_VERSION {
        return Err(FormatError::TooOld { found, supported: SCHEMA_VERSION });
    }
    let env = EnvelopeIn::deserialize(&doc).map_err(|e| FormatError::Corrupt(e.to_string()))?;
    check_no_unknown_fields(&doc, &env.project)?;
    validate_loaded(env.loaded(found))
}

/// Every key the file holds under `project` must come back when the loaded project is written
/// again. A key that does not is one the model dropped while loading (a field a later build
/// removed, or a typo): refuse the file rather than lose that data on the next save.
fn check_no_unknown_fields(doc: &Value, project: &Project) -> Result<(), FormatError> {
    let Some(original) = doc.get("project") else { return Err(FormatError::Corrupt("no `project`".into())) };
    let written = serde_json::to_value(project).map_err(|e| FormatError::Corrupt(e.to_string()))?;
    let mut path = String::from("project");
    match first_dropped_key(original, &written, &mut path) {
        Some(at) => Err(FormatError::UnknownField(at)),
        None => Ok(()),
    }
}

/// The path of the first object key of `original` that `written` lacks.
fn first_dropped_key(original: &Value, written: &Value, path: &mut String) -> Option<String> {
    let len = path.len();
    let found = match (original, written) {
        (Value::Object(o), Value::Object(w)) => o.iter().find_map(|(k, v)| {
            path.truncate(len);
            path.push('.');
            path.push_str(k);
            match w.get(k) {
                Some(back) => first_dropped_key(v, back, path),
                None => Some(path.clone()),
            }
        }),
        (Value::Array(o), Value::Array(w)) => o.iter().zip(w).enumerate().find_map(|(i, (v, back))| {
            path.truncate(len);
            path.push_str(&format!("[{i}]"));
            first_dropped_key(v, back, path)
        }),
        _ => None,
    };
    path.truncate(len);
    found
}

/// Refuse only values that would overflow or exhaust memory later (`Sequence::check_bounds`);
/// structural rules such as overlaps are left to the editor, so projects that open today still open.
fn validate_loaded(mut loaded: Loaded) -> Result<Loaded, FormatError> {
    // Out-of-range audio stream data is repaired, not refused: failing the load would lose the
    // whole project over one bad index. A media item keeps its first `MAX_AUDIO_STREAMS` streams; a
    // clip pointing past that plays stream 0.
    let max = waltcut_media::MAX_AUDIO_STREAMS;
    for item in loaded.project.items.values_mut() {
        let name = item.name.clone();
        match &mut item.kind {
            waltcut_project::ItemKind::Media(m) if m.info.audio_streams.len() > max => {
                log::warn!("media `{name}`: {} audio streams, keeping the first {max}", m.info.audio_streams.len());
                m.info.audio_streams.truncate(max);
            }
            waltcut_project::ItemKind::Sequence(sequence) => {
                let sequence = std::sync::Arc::make_mut(sequence);
                for clip in sequence.audio_tracks.iter_mut().flat_map(|t| t.items.iter_mut()) {
                    if clip.audio_stream >= max {
                        log::warn!("sequence `{name}`: clip `{}` uses audio stream {}, reset to 0", clip.name, clip.audio_stream);
                        clip.audio_stream = 0;
                    }
                }
            }
            _ => {}
        }
    }
    for item in loaded.project.sequences() {
        if let waltcut_project::ItemKind::Sequence(sequence) = &item.kind {
            sequence.check_bounds().map_err(|e| FormatError::Corrupt(format!("sequence `{}`: {e}", item.name)))?;
        }
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_is_the_reduction_baseline() {
        assert_eq!(SCHEMA_VERSION, 16);
        assert!(FormatError::TooOld { found: 15, supported: SCHEMA_VERSION }.to_string().contains(LEGACY_RELEASE));
    }

    #[test]
    fn damaged_sequence_settings_are_rejected_before_rendering() {
        let mut p = Project::new("Invalid sequence");
        let id = p.new_sequence("Bad", waltcut_project::SequenceSettings::default(), 1, 1, None);
        for (width, height, sample_rate) in [(0, 90, 48000), (160, 0, 48000), (160, 90, 0), (65536, 90, 48000), (32768, 16384, 48000)] {
            let settings = &mut p.sequence_mut(id).unwrap().settings;
            (settings.width, settings.height, settings.sample_rate) = (width, height, sample_rate);
            assert!(matches!(decode(&encode(&p, false)), Err(FormatError::Corrupt(_))));
        }
    }

    /// #210 review: load-time validation refuses only overflowing values. A project that breaks a
    /// structural rule an older build may have written (overlapping or zero-length clips) still
    /// opens, as it did before; and 16K sequences are not "too large".
    #[test]
    fn structural_rule_breaks_still_open() {
        use waltcut_project::{ItemKind, SequenceSettings, TrackKind};
        use waltcut_time::{FrameRate, Tick, TimeRange};
        let mut p = Project::new("Legacy");
        let rate = FrameRate::FPS_24;
        let layer = p.add_item("Layer", waltcut_project::Label::Iris, ItemKind::Graphic { width: 1920, height: 1080, rate }, None);
        let settings = SequenceSettings { width: 15360, height: 8640, ..Default::default() };
        let seq = p.new_sequence("Sequence 01", settings, 1, 1, None);
        let range = TimeRange::new(Tick::ZERO, rate.tick_of(48));
        let a = p.make_track_item(layer, TrackKind::Video, Tick::ZERO, range, rate).unwrap();
        let b = p.make_track_item(layer, TrackKind::Video, rate.tick_of(24), range, rate).unwrap();
        let mut c = p.make_track_item(layer, TrackKind::Video, rate.tick_of(200), range, rate).unwrap();
        c.duration = Tick::ZERO;
        p.sequence_mut(seq).unwrap().video_tracks[0].items = vec![a, b, c];
        assert!(p.sequence(seq).unwrap().check().is_err(), "the sequence breaks the editor's invariants");
        let loaded = decode(&encode(&p, false)).unwrap_or_else(|e| panic!("a project that opened before is refused: {e}"));
        assert_eq!(loaded.project.sequence(seq).unwrap().video_tracks[0].items.len(), 3);
    }

    #[test]
    fn roundtrip_current() {
        let p = Project::new("Round Trip");
        for pretty in [true, false] {
            let b = encode(&p, pretty);
            let l = decode(&b).unwrap();
            assert_eq!(l.project, p);
            assert_eq!(l.schema_version, SCHEMA_VERSION);
            assert_eq!(l.generator.as_deref(), Some(generator().as_str()));
        }
    }

    /// What is open is stored beside the project. A file without it, or with one that cannot be
    /// read, opens all the same.
    #[test]
    fn the_view_is_kept_beside_the_project_and_never_stops_a_file_from_opening() {
        use waltcut_project::{ItemId, SequenceSettings, SequenceView};
        let mut p = Project::new("Tabs");
        let seq = p.new_sequence("Sequence 01", SequenceSettings::default(), 1, 1, None);
        let view = ProjectView {
            open_sequences: vec![seq],
            active_sequence: Some(seq),
            sequences: [(seq, SequenceView { pps: 80.0, scroll: 1.5, v_scroll: 0.0, a_scroll: 4.0, video_track_h: 60.0, audio_track_h: 56.0 })].into(),
            playheads: [(seq, waltcut_time::Tick(3 * waltcut_time::TICKS_PER_SECOND))].into(),
        };
        for pretty in [true, false] {
            let l = decode(&encode_with_view(&p, Some(&view), pretty)).unwrap();
            assert_eq!((l.view.as_ref(), &l.project), (Some(&view), &p));
            assert_eq!(decode(&encode(&p, pretty)).unwrap().view, None);
        }
        // the project is the same bytes with or without a view beside it
        let mut doc: Value = serde_json::from_slice(&encode_with_view(&p, Some(&view), false)).unwrap();
        assert_eq!(doc["project"], serde_json::from_slice::<Value>(&encode(&p, false)).unwrap()["project"]);
        // a view that is not one: dropped, the project loads
        for junk in [serde_json::json!(7), serde_json::json!({"open_sequences": {"a": 1}}), serde_json::json!({"sequences": [1]}), Value::Null] {
            doc["view"] = junk;
            let l = decode(&serde_json::to_vec(&doc).unwrap()).unwrap();
            assert_eq!((l.view, &l.project), (None, &p));
        }
        // unknown ids are not this crate's business (the editor checks them against the project)
        doc["view"] = serde_json::json!({"open_sequences": [424242]});
        assert_eq!(decode(&serde_json::to_vec(&doc).unwrap()).unwrap().view.unwrap().open_sequences, [ItemId(424242)]);
    }

    /// A clip with unresolved "auto" points (NaN) is written with `null` coordinates. Such a file
    /// used to be refused as damaged; it opens, and the points are still "auto".
    #[test]
    fn auto_points_written_as_null_open_again() {
        use waltcut_project::{ItemKind, SequenceSettings, TrackKind};
        use waltcut_time::{FrameRate, Tick, TimeRange};
        let mut p = Project::new("Imported");
        let layer = p.add_item("Layer", waltcut_project::Label::Iris, ItemKind::Graphic { width: 3840, height: 2160, rate: FrameRate::FPS_24 }, None);
        let seq = p.new_sequence("Sequence 01", SequenceSettings::default(), 1, 1, None);
        let ti = p.make_track_item(layer, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(48)), FrameRate::FPS_24).unwrap();
        p.sequence_mut(seq).unwrap().video_tracks[0].items.push(ti);
        for pretty in [true, false] {
            let bytes = encode(&p, pretty);
            let text = String::from_utf8(bytes.clone()).unwrap();
            assert!(text.replace([' ', '\n'], "").contains(r#"{"Vec2":{"x":null,"y":null}}"#), "the file holds NaN points as null");
            let l = decode(&bytes).unwrap_or_else(|e| panic!("a file with `null` points is refused: {e}"));
            let motion = l.project.sequence(seq).unwrap().video_tracks[0].items[0].effect("motion").unwrap().clone();
            let anchor = motion.params["anchor"].value.as_vec2().unwrap();
            assert!(anchor.x.is_nan() && anchor.y.is_nan(), "still auto");
            assert_eq!(motion.params["scale"].value.as_f64(), Some(100.0));
            assert_eq!(encode(&l.project, pretty), bytes, "and the file is written back the same");
            // a `null` where no "auto" exists is still a damaged file
            let broken = text.replacen("3840", "null", 1);
            assert!(matches!(decode(broken.as_bytes()), Err(FormatError::Corrupt(_))));
        }
    }

    #[test]
    fn envelope_has_explicit_schema_version() {
        let v: Value = serde_json::from_slice(&encode(&Project::new("x"), true)).unwrap();
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        assert_eq!(v["format"], FORMAT_ID);
        assert!(v["project"]["items"].is_object());
        assert!(v["project"].get("version").is_none());
    }

    #[test]
    fn refuses_newer() {
        let mut v: Value = serde_json::from_slice(&encode(&Project::new("x"), false)).unwrap();
        v["schema_version"] = Value::from(SCHEMA_VERSION + 1);
        let e = decode(&serde_json::to_vec(&v).unwrap()).unwrap_err();
        assert!(matches!(e, FormatError::TooNew { .. }));
        assert!(e.to_string().contains("newer version of WaltCut"), "{e}");
    }

    #[test]
    fn rejects_foreign_and_damaged() {
        assert!(matches!(decode(br#"{"hello": 1}"#), Err(FormatError::NotAProject(_))));
        assert!(matches!(decode(br#"{"format":"other","schema_version":1}"#), Err(FormatError::NotAProject(_))));
        let b = encode(&Project::new("x"), false);
        assert!(matches!(decode(&b[..b.len() / 2]), Err(FormatError::Corrupt(_))));
    }

    fn doc_of(p: &Project) -> Value {
        serde_json::from_slice(&encode(p, false)).unwrap()
    }

    fn with_sequence() -> Project {
        let mut p = Project::new("Fields");
        p.new_sequence("Sequence 01", waltcut_project::SequenceSettings::default(), 1, 1, None);
        p
    }

    #[test]
    fn every_earlier_schema_is_refused_with_the_legacy_release() {
        for old in [0, 1, 8, 14, 15] {
            let mut v = doc_of(&Project::new("x"));
            v["schema_version"] = Value::from(old);
            let e = decode(&serde_json::to_vec(&v).unwrap()).unwrap_err();
            assert!(matches!(e, FormatError::TooOld { found, supported } if found == old && supported == SCHEMA_VERSION), "{e}");
            let text = e.to_string();
            assert!(text.contains("earlier FilmCraft or WaltCut build") && text.contains(LEGACY_RELEASE) && text.contains("has not been changed"), "{text}");
        }
        // the pre-envelope shape is an earlier schema too, whatever its `version` says
        for bare in [
            br#"{"version":1,"items":{},"root":{}}"#.as_slice(),
            br#"{"items":{},"root":{}}"#,
            br#"{"version":2,"items":{},"root":{}}"#,
            br#"{"version":9,"items":{},"root":{}}"#,
        ] {
            assert!(matches!(decode(bare), Err(FormatError::TooOld { found: 1, .. })), "{}", String::from_utf8_lossy(bare));
        }
        assert!(matches!(decode(br#"{"version":99,"items":{},"root":{}}"#), Err(FormatError::TooNew { found: 99, .. })));
    }

    #[test]
    fn hostile_documents_return_errors() {
        let good = encode(&with_sequence(), false);
        for cut in [0, 1, good.len() / 3, good.len() - 1] {
            assert!(decode(&good[..cut]).is_err(), "truncated at {cut}");
        }
        for junk in [
            &b"null"[..],
            b"[]",
            b"7",
            b"\"x\"",
            b"{}",
            b"\xff\xfe",
            br#"{"schema_version":"16"}"#,
            br#"{"schema_version":16}"#,
            br#"{"schema_version":-1}"#,
            br#"{"schema_version":99999999999}"#,
            br#"{"format":"filmcraft.project","schema_version":16,"project":7}"#,
        ] {
            assert!(decode(junk).is_err(), "{}", String::from_utf8_lossy(junk));
        }
    }

    #[test]
    fn unknown_fields_are_refused_not_dropped() {
        let p = with_sequence();
        let seq = p.sequences().next().unwrap().id;
        let base = doc_of(&p);
        assert!(decode(&serde_json::to_vec(&base).unwrap()).is_ok());
        let sequence_key = seq.0.to_string();
        let edits: [(&str, Box<dyn Fn(&mut Value)>); 4] = [
            ("project.removed_feature", Box::new(|d| d["project"]["removed_feature"] = Value::from(1))),
            ("project.settings.removed_feature", Box::new(|d| d["project"]["settings"]["removed_feature"] = Value::from("x"))),
            ("project.root.removed_feature", Box::new(|d| d["project"]["root"]["removed_feature"] = Value::from(true))),
            ("project.items.", Box::new(move |d| d["project"]["items"][&sequence_key]["removed_feature"] = Value::Null)),
        ];
        for (path, edit) in edits {
            let mut d = base.clone();
            edit(&mut d);
            let e = decode(&serde_json::to_vec(&d).unwrap()).unwrap_err();
            assert!(matches!(&e, FormatError::UnknownField(at) if at.starts_with(path) && at.ends_with("removed_feature")), "{path}: {e}");
        }
        // inside sequences, whose clips live deeper
        let mut d = base.clone();
        let items = d["project"]["items"].as_object_mut().unwrap();
        let seq_doc = items.values_mut().find(|v| v.to_string().contains("video_tracks")).unwrap();
        let found = seq_doc.to_string().contains("removed_feature");
        assert!(!found);
        fn plant(v: &mut Value) -> bool {
            match v {
                Value::Object(o) => {
                    if o.contains_key("video_tracks")
                        && let Some(Value::Array(tracks)) = o.get_mut("video_tracks")
                        && let Some(Value::Object(t)) = tracks.first_mut()
                    {
                        t.insert("removed_feature".into(), Value::from(1));
                        return true;
                    }
                    o.values_mut().any(plant)
                }
                Value::Array(a) => a.iter_mut().any(plant),
                _ => false,
            }
        }
        assert!(plant(seq_doc), "the sequence has a video track");
        let e = decode(&serde_json::to_vec(&d).unwrap()).unwrap_err();
        // a track refuses a key it does not know (naming it) rather than dropping it on the next save
        assert!(
            matches!(&e, FormatError::UnknownField(at) if at.contains("video_tracks[0].removed_feature"))
                || matches!(&e, FormatError::Corrupt(msg) if msg.contains("unknown field `removed_feature`")),
            "{e}"
        );
    }

    /// What is open is not part of the schema: a `view` this build does not fully know still opens.
    #[test]
    fn unknown_view_fields_do_not_stop_a_project_opening() {
        let mut d = doc_of(&with_sequence());
        d["view"] = serde_json::json!({"open_sequences": [], "from_the_future": 1});
        assert!(decode(&serde_json::to_vec(&d).unwrap()).is_ok());
        d["unknown_envelope_key"] = Value::from(1);
        assert!(decode(&serde_json::to_vec(&d).unwrap()).is_ok());
    }

    #[test]
    fn peek_reads_the_schema_without_loading() {
        let p = with_sequence();
        for pretty in [true, false] {
            assert_eq!(peek_schema(&encode(&p, pretty)), Some(SCHEMA_VERSION));
        }
        assert_eq!(peek_schema(br#"{"format":"filmcraft.project","schema_version": 15,"project":{}}"#), Some(15));
        assert_eq!(peek_schema(b"{\"schema_version\"\n :\t7 }"), Some(7));
        assert_eq!(peek_schema(br#"{"items":{},"root":{}}"#), Some(1));
        // behind the first 4 KiB: found by a full parse
        let late = format!(r#"{{"padding":"{}","schema_version":16}}"#, "x".repeat(PEEK_BYTES * 2));
        assert_eq!(peek_schema(late.as_bytes()), Some(16));
        for junk in [&b""[..], b"{", b"[1]", br#"{"schema_version":"x"}"#, br#"{"schema_version":99999999999}"#] {
            assert_eq!(peek_schema(junk), None, "{}", String::from_utf8_lossy(junk));
        }
        let d = std::env::temp_dir().join(format!("waltcut-peek-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (cur, old, none) = (d.join("cur.waltcut"), d.join("old.waltcut"), d.join("none.waltcut"));
        std::fs::write(&cur, encode(&p, false)).unwrap();
        std::fs::write(&old, br#"{"format":"filmcraft.project","schema_version":15,"project":{}}"#).unwrap();
        assert!(is_current_file(&cur) && !is_current_file(&old) && !is_current_file(&none));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
