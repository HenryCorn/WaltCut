//! Project files of the current schema open and round-trip; every earlier one is refused untouched.

use waltcut_format::{FormatError, LEGACY_RELEASE, SCHEMA_VERSION, decode, encode};
use waltcut_project::{ClipId, ItemId, ItemKind, Project};
use waltcut_time::Tick;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

/// The golden file: a project written by this schema, with media, a sequence, markers and effects.
fn golden() -> Project {
    decode(&fixture("v19-edit.waltcut")).unwrap().project
}

#[test]
fn golden_project_opens_and_round_trips_without_loss() {
    let bytes = fixture("v19-edit.waltcut");
    let l = decode(&bytes).unwrap();
    assert_eq!(l.schema_version, SCHEMA_VERSION);
    let p = &l.project;
    assert_eq!(p.name, "Legacy Edit");
    assert_eq!(p.next_id, 49);
    let media = p.item(ItemId(5)).unwrap();
    assert_eq!(media.name, "Ocean_Sunset.mp4");
    let seq = p.sequence(ItemId(21)).unwrap();
    assert_eq!((seq.video_tracks.len(), seq.audio_tracks.len(), seq.markers.len()), (1, 1, 1));
    let clip = &seq.video_tracks[0].items[0];
    assert_eq!(clip.id, ClipId(29));
    assert_eq!(clip.source_in, Tick(243_675_432_000));
    assert_eq!(clip.effects.len(), 2);
    seq.check().unwrap();
    // the drawing with a fill: its outline, then the fill's stored region, in paint order
    let d = p.item(ItemId(48)).and_then(|i| i.as_drawing()).unwrap();
    let strokes = &d.cels[0].strokes;
    assert_eq!(strokes.len(), 2);
    let fill = strokes[1].fill.as_deref().unwrap();
    assert!(strokes[1].is_fill() && strokes[1].points.is_empty());
    assert_eq!((fill.seed, fill.tolerance, fill.rect), ([10, 8], 32, [3, 2, 26, 14]));
    fill.validate(d.width, d.height).unwrap();
    assert_eq!(fill.filled_pixels(), 20 * 8);
    // saved again it is the same document: nothing added, nothing lost (the writer's name aside)
    let again = encode(p, true);
    let (old, new): (serde_json::Value, serde_json::Value) = (serde_json::from_slice(&bytes).unwrap(), serde_json::from_slice(&again).unwrap());
    assert_eq!(new["project"], old["project"]);
    assert_eq!((new["format"].clone(), new["schema_version"].clone()), (old["format"].clone(), old["schema_version"].clone()));
    assert_eq!(decode(&again).unwrap().project, *p);
    assert_eq!(encode(&decode(&again).unwrap().project, true), again, "stable on repeated saves");
}

#[test]
fn removed_audio_processing_data_is_refused_instead_of_silently_ignored() {
    let baseline: serde_json::Value = serde_json::from_slice(&fixture("v19-edit.waltcut")).unwrap();
    for (path, value) in [
        ("/project/items/21/kind/Sequence/audio_tracks/0/transitions", serde_json::json!([])),
        ("/project/settings/default_audio_transition_duration", serde_json::json!(254_016_000_000i64)),
        ("/project/items/21/kind/Sequence/settings/audio_master", serde_json::json!("Surround51")),
        ("/project/items/21/kind/Sequence/audio_tracks/0/items/0/source_channels", serde_json::json!([0, 1, 2])),
        ("/project/items/21/kind/Sequence/audio_tracks/0/items/0/effects/0/effect", serde_json::json!("parametric_eq")),
        ("/project/items/21/kind/Sequence/audio_tracks/0/items/0/effects/0/post_fader", serde_json::json!(true)),
    ] {
        let mut doc = baseline.clone();
        let (parent, key) = path.rsplit_once('/').unwrap();
        doc.pointer_mut(parent).unwrap().as_object_mut().unwrap().insert(key.into(), value);
        let bytes = serde_json::to_vec(&doc).unwrap();
        assert!(decode(&bytes).is_err(), "accepted removed data at {path}");
    }
}

/// A project written by the build before the reduction fails with the actionable error, and
/// reading it changes nothing: the bytes are hashed before and after, on disk.
#[test]
fn pre_reduction_project_is_refused_and_left_untouched() {
    let dir = std::env::temp_dir().join(format!("waltcut-old-project-{}", std::process::id()));
    let autosave = dir.join(waltcut_format::autosave::AUTO_SAVE_DIR);
    std::fs::create_dir_all(&autosave).unwrap();
    let files = [dir.join("old.waltcut"), autosave.join("old-2026-10-01_10-00-00.waltcut"), dir.join("old (schema v1 backup).waltcut")];
    for f in &files {
        std::fs::write(f, fixture("v15-edit.waltcut")).unwrap();
    }
    let hash = |p: &std::path::Path| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::fs::read(p).unwrap().hash(&mut h);
        (h.finish(), std::fs::metadata(p).unwrap().modified().unwrap())
    };
    let before: Vec<_> = files.iter().map(|f| hash(f)).collect();
    let e = decode(&std::fs::read(&files[0]).unwrap()).unwrap_err();
    assert!(matches!(e, FormatError::TooOld { found: 15, supported } if supported == SCHEMA_VERSION), "{e}");
    let text = e.to_string();
    assert!(text.contains("earlier FilmCraft or WaltCut build") && text.contains(LEGACY_RELEASE) && text.contains("schema v15"), "{text}");
    // its auto-save sibling is not offered for recovery, and not pruned either
    assert!(waltcut_format::autosave::list_auto_saves(&autosave, "old").is_empty());
    waltcut_format::autosave::rotate(&autosave, "old", 1).unwrap();
    assert_eq!(files.iter().map(|f| hash(f)).collect::<Vec<_>>(), before);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// v18 was the last schema before a cel could hold a fill: it is refused like any earlier one.
#[test]
fn the_schema_before_fills_is_refused_with_the_legacy_release() {
    let e = decode(&fixture("v18-edit.waltcut")).unwrap_err();
    assert!(matches!(e, FormatError::TooOld { found: 18, supported } if supported == SCHEMA_VERSION), "{e}");
    assert!(e.to_string().contains(LEGACY_RELEASE));
}

/// A fill survives a save and a load exactly; a drawing without one has no `fill` key; a damaged
/// fill is dropped on load and the rest of the cel stays.
#[test]
fn fills_round_trip_and_plain_cels_are_unchanged() {
    use waltcut_project::drawing::{Brush, Drawing, Fill, Stroke, StrokePoint};
    use waltcut_project::{ItemKind, Label};
    let pen = || Stroke { brush: Brush::Pen, color: [0.0, 0.0, 0.0, 1.0], size: 2.0, points: vec![StrokePoint { x: 1.0, y: 2.0, p: 1.0 }], response: None, fill: None };
    let mut p = golden();
    let mut d = Drawing::new(16, 16, waltcut_time::FrameRate::FPS_24);
    d.ensure_cel(0, false).unwrap();
    if let Some(c) = d.cels.first_mut() {
        c.strokes_mut().push(pen());
    }
    let plain = p.add_item("plain", Label::Iris, ItemKind::Drawing(d.clone().into()), None);
    let mut bare = Project::new("bare");
    bare.add_item("plain", Label::Iris, ItemKind::Drawing(d.clone().into()), None);
    let written = String::from_utf8_lossy(&encode(&bare, true)).into_owned();
    assert!(!written.contains("\"Fill\"") && !written.contains("\"fill\""), "no fill key without a fill");
    let fill = Fill::encode([4, 4], 7, [2, 2, 8, 8], &[1u8; 64]).unwrap();
    let good = Stroke::fill_op([0.2, 0.4, 0.6, 0.5], fill);
    let bad = Stroke::fill_op([0.2, 0.4, 0.6, 0.5], Fill { seed: [0, 0], tolerance: 0, rect: [0, 0, 40, 40], runs: vec![[1, 1600]] });
    if let Some(c) = d.cels.first_mut() {
        c.strokes_mut().push(good.clone());
        c.strokes_mut().push(bad);
        c.strokes_mut().push(pen());
    }
    let filled = p.add_item("filled", Label::Iris, ItemKind::Drawing(d.into()), None);
    let bytes = encode(&p, true);
    let back = decode(&bytes).unwrap().project;
    let strokes = |p: &Project, id| p.item(id).and_then(|i| i.as_drawing()).map(|d| d.cels[0].strokes.to_vec());
    assert_eq!(strokes(&back, plain).map(|s| s.len()), Some(1));
    let kept = strokes(&back, filled).unwrap();
    assert_eq!(kept.len(), 3, "the fill outside the 16 × 16 canvas is dropped, the strokes and the good fill stay");
    assert_eq!(kept[1], good);
    // and the stored region is bit for bit what was saved
    assert_eq!(kept[1].fill, good.fill);
}

/// v17 was the last schema before strokes could carry a pressure response: it is refused like any
/// earlier one.
#[test]
fn the_schema_before_pressure_responses_is_refused_with_the_legacy_release() {
    let e = decode(&fixture("v17-edit.waltcut")).unwrap_err();
    assert!(matches!(e, FormatError::TooOld { found: 17, supported } if supported == SCHEMA_VERSION), "{e}");
    assert!(e.to_string().contains(LEGACY_RELEASE));
}

/// A stroke's pressure response survives a save and a load, a stroke without one has no `response`
/// key.
#[test]
fn pressure_responses_round_trip_and_plain_strokes_are_unchanged() {
    use waltcut_project::drawing::{Brush, Curve, Drawing, PressureResponse, Stroke, StrokePoint};
    use waltcut_project::{ItemKind, Label};
    let stroke = |response| Stroke {
        brush: Brush::Pencil,
        color: [0.0, 0.0, 0.0, 1.0],
        size: 6.0,
        points: vec![StrokePoint { x: 1.0, y: 2.0, p: 0.3 }, StrokePoint { x: 9.0, y: 4.0, p: 0.8 }],
        response,
        fill: None,
    };
    let mut p = golden();
    let mut d = Drawing::new(64, 32, waltcut_time::FrameRate::FPS_24);
    d.ensure_cel(0, false).unwrap();
    if let Some(c) = d.cels.first_mut() {
        c.strokes_mut().push(stroke(None));
    }
    let id = p.add_item("plain", Label::Iris, ItemKind::Drawing(d.clone().into()), None);
    let plain = encode(&p, true);
    assert!(!String::from_utf8_lossy(&plain).contains("\"response\""), "the key is written only when set");
    let response = PressureResponse { width: Curve::preset("soft"), opacity: Curve::preset("firm") };
    if let Some(c) = d.cels.first_mut() {
        c.strokes_mut().push(stroke(Some(response.clone())));
    }
    let id2 = p.add_item("curved", Label::Iris, ItemKind::Drawing(d.into()), None);
    let bytes = encode(&p, true);
    let back = decode(&bytes).unwrap().project;
    let stored = |p: &Project, id| p.item(id).and_then(|i| i.as_drawing()).map(|d| d.cels[0].strokes.iter().map(|s| s.response.clone()).collect::<Vec<_>>());
    assert_eq!(stored(&back, id), Some(vec![None]));
    assert_eq!(stored(&back, id2), Some(vec![None, Some(response)]));
}

/// v16 was the last schema before clips could be guides: it is refused like any earlier one.
#[test]
fn the_previous_schema_is_refused_with_the_legacy_release() {
    let e = decode(&fixture("v16-edit.waltcut")).unwrap_err();
    assert!(matches!(e, FormatError::TooOld { found: 16, supported } if supported == SCHEMA_VERSION), "{e}");
    assert!(e.to_string().contains(LEGACY_RELEASE));
}

/// A guide clip survives a save and a load, and an unmarked clip's JSON has no `guide` key.
#[test]
fn guide_clips_round_trip_and_unmarked_clips_are_unchanged() {
    let mut p = golden();
    let plain = encode(&p, true);
    assert!(!String::from_utf8_lossy(&plain).contains("\"guide\""), "the key is written only when set");
    let seq = p.sequence_mut(ItemId(21)).unwrap();
    seq.video_tracks[0].items[0].guide = true;
    let bytes = encode(&p, true);
    assert!(String::from_utf8_lossy(&bytes).contains("\"guide\": true"));
    assert!(decode(&bytes).unwrap().project.sequence(ItemId(21)).unwrap().video_tracks[0].items[0].guide);
}

#[test]
fn bare_pre_envelope_project_is_refused() {
    let e = decode(&fixture("v1-minimal.waltcut")).unwrap_err();
    assert!(matches!(e, FormatError::TooOld { found: 1, .. }), "{e}");
}

#[test]
fn future_schema_is_refused_with_a_clear_error() {
    let mut v: serde_json::Value = serde_json::from_slice(&fixture("v1-minimal.waltcut")).unwrap();
    // A bare v1-shaped project claiming a future `version` is refused too.
    v["version"] = 99.into();
    let e = decode(&serde_json::to_vec(&v).unwrap()).unwrap_err();
    assert!(matches!(e, FormatError::TooNew { found: 99, .. }), "{e}");
    assert!(e.to_string().contains("Update WaltCut"));
}

#[test]
fn golden_audio_field_is_the_primary_stream_and_clips_default_to_zero() {
    let p = golden();
    let media = p.item(ItemId(5)).unwrap().as_media().unwrap();
    assert_eq!(media.info.audio_streams.len(), 1);
    assert!(media.info.has_audio());
    let clip = &p.sequence(ItemId(21)).unwrap().audio_tracks[0].items[0];
    assert_eq!(clip.audio_stream, 0);
}

/// Out-of-range audio stream data is repaired on load, never a reason to refuse the project.
#[test]
fn hostile_audio_stream_counts_and_indices_are_repaired_not_rejected() {
    let mut p = golden();
    let seq = ItemId(21);
    for bad in [waltcut_media::MAX_AUDIO_STREAMS, usize::MAX] {
        p.sequence_mut(seq).unwrap().audio_tracks[0].items[0].audio_stream = bad;
        let loaded = decode(&encode(&p, false)).unwrap().project;
        assert_eq!(loaded.sequence(seq).unwrap().audio_tracks[0].items[0].audio_stream, 0);
    }
    p.sequence_mut(seq).unwrap().audio_tracks[0].items[0].audio_stream = waltcut_media::MAX_AUDIO_STREAMS - 1;
    let loaded = decode(&encode(&p, false)).unwrap().project;
    assert_eq!(loaded.sequence(seq).unwrap().audio_tracks[0].items[0].audio_stream, waltcut_media::MAX_AUDIO_STREAMS - 1, "in range: kept");
    let media = p.item_mut(ItemId(5)).unwrap().as_media_mut().unwrap();
    let stream = media.info.audio_streams[0].clone();
    media.info.audio_streams.resize(waltcut_media::MAX_AUDIO_STREAMS + 1, stream);
    let loaded = decode(&encode(&p, false)).unwrap().project;
    assert_eq!(loaded.item(ItemId(5)).unwrap().as_media().unwrap().info.audio_streams.len(), waltcut_media::MAX_AUDIO_STREAMS);
}

/// Build a project with `n` clips (half video, half linked audio) on 4+4 tracks.
pub fn big_project(n: usize) -> Project {
    let mut p = golden();
    let seq_id = ItemId(21);
    let mut next = 1_000_000u64;
    let seq = match &mut p.items.get_mut(&seq_id).unwrap().kind {
        ItemKind::Sequence(s) => std::sync::Arc::make_mut(s),
        _ => unreachable!(),
    };
    let v0 = seq.video_tracks[0].items[0].clone();
    let a0 = seq.audio_tracks[0].items[0].clone();
    for t in seq.video_tracks.iter_mut().chain(seq.audio_tracks.iter_mut()) {
        t.items.clear();
    }
    let mut vt = seq.video_tracks[0].clone();
    let mut at = seq.audio_tracks[0].clone();
    for k in 1..4 {
        vt.id = waltcut_project::TrackId(500 + k);
        vt.name = format!("Video {}", k + 1);
        seq.video_tracks.push(vt.clone());
        at.id = waltcut_project::TrackId(600 + k);
        at.name = format!("Audio {}", k + 1);
        seq.audio_tracks.push(at.clone());
    }
    let dur = Tick(254_016_000_000); // 1 s each
    for i in 0..n / 2 {
        let track = i % 4;
        let slot = (i / 4) as i64;
        let mut v = v0.clone();
        v.id = ClipId(next);
        v.start = Tick(dur.0 * slot);
        v.duration = dur;
        v.link = Some(next);
        let mut a = a0.clone();
        a.id = ClipId(next + 1);
        a.start = v.start;
        a.duration = dur;
        a.link = Some(next);
        next += 2;
        seq.video_tracks[track].items.push(v);
        seq.audio_tracks[track].items.push(a);
    }
    p.next_id = next;
    p
}

#[test]
fn large_project_roundtrip_and_timing() {
    let p = big_project(2000);
    let clips: usize = p.sequence(ItemId(21)).unwrap().all_tracks().map(|t| t.items.len()).sum();
    assert_eq!(clips, 2000);
    let reps = 5;
    let t0 = std::time::Instant::now();
    let mut pretty = Vec::new();
    for _ in 0..reps {
        pretty = encode(&p, true);
    }
    let pretty_ms = t0.elapsed().as_secs_f64() * 1000.0 / reps as f64;
    let t0 = std::time::Instant::now();
    let mut compact = Vec::new();
    for _ in 0..reps {
        compact = encode(&p, false);
    }
    let compact_ms = t0.elapsed().as_secs_f64() * 1000.0 / reps as f64;
    let t0 = std::time::Instant::now();
    let back = decode(&compact).unwrap();
    let decode_ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(back.project, p);
    let t0 = std::time::Instant::now();
    let dir = std::env::temp_dir().join(format!("waltcut-big-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    waltcut_format::atomic_write(&dir.join("big.waltcut"), &compact).unwrap();
    let write_ms = t0.elapsed().as_secs_f64() * 1000.0;
    std::fs::remove_dir_all(&dir).unwrap();
    println!(
        "2000-clip project: pretty {:.1} KB in {pretty_ms:.2} ms, compact {:.1} KB in {compact_ms:.2} ms, decode {decode_ms:.2} ms, atomic write+fsync {write_ms:.2} ms",
        pretty.len() as f64 / 1024.0,
        compact.len() as f64 / 1024.0
    );
}
