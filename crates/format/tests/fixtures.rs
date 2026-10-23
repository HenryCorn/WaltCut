//! Project files of the current schema open and round-trip; every earlier one is refused untouched.

use waltcut_format::{FormatError, LEGACY_RELEASE, SCHEMA_VERSION, decode, encode};
use waltcut_project::{ClipId, ItemId, ItemKind, Project};
use waltcut_time::Tick;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

/// The golden file: a project written by this schema, with media, a sequence, markers and effects.
fn golden() -> Project {
    decode(&fixture("v16-edit.fcproj")).unwrap().project
}

#[test]
fn golden_project_opens_and_round_trips_without_loss() {
    let bytes = fixture("v16-edit.fcproj");
    let l = decode(&bytes).unwrap();
    assert_eq!(l.schema_version, SCHEMA_VERSION);
    let p = &l.project;
    assert_eq!(p.name, "Legacy Edit");
    assert_eq!(p.next_id, 48);
    let media = p.item(ItemId(5)).unwrap();
    assert_eq!(media.name, "Ocean_Sunset.mp4");
    let seq = p.sequence(ItemId(21)).unwrap();
    assert_eq!((seq.video_tracks.len(), seq.audio_tracks.len(), seq.markers.len()), (1, 1, 1));
    let clip = &seq.video_tracks[0].items[0];
    assert_eq!(clip.id, ClipId(29));
    assert_eq!(clip.source_in, Tick(243_675_432_000));
    assert_eq!(clip.effects.len(), 2);
    seq.check().unwrap();
    // saved again it is the same document: nothing added, nothing lost (the writer's name aside)
    let again = encode(p, true);
    let (old, new): (serde_json::Value, serde_json::Value) = (serde_json::from_slice(&bytes).unwrap(), serde_json::from_slice(&again).unwrap());
    assert_eq!(new["project"], old["project"]);
    assert_eq!((new["format"].clone(), new["schema_version"].clone()), (old["format"].clone(), old["schema_version"].clone()));
    assert_eq!(decode(&again).unwrap().project, *p);
    assert_eq!(encode(&decode(&again).unwrap().project, true), again, "stable on repeated saves");
}

/// A project written by the build before the reduction fails with the actionable error, and
/// reading it changes nothing: the bytes are hashed before and after, on disk.
#[test]
fn pre_reduction_project_is_refused_and_left_untouched() {
    let dir = std::env::temp_dir().join(format!("filmcraft-old-project-{}", std::process::id()));
    let autosave = dir.join(waltcut_format::autosave::AUTO_SAVE_DIR);
    std::fs::create_dir_all(&autosave).unwrap();
    let files = [dir.join("old.fcproj"), autosave.join("old-2026-10-01_10-00-00.fcproj"), dir.join("old (schema v1 backup).fcproj")];
    for f in &files {
        std::fs::write(f, fixture("v15-edit.fcproj")).unwrap();
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

#[test]
fn bare_pre_envelope_project_is_refused() {
    let e = decode(&fixture("v1-minimal.fcproj")).unwrap_err();
    assert!(matches!(e, FormatError::TooOld { found: 1, .. }), "{e}");
}

#[test]
fn future_schema_is_refused_with_a_clear_error() {
    let mut v: serde_json::Value = serde_json::from_slice(&fixture("v1-minimal.fcproj")).unwrap();
    // A bare v1-shaped project claiming a future `version` is refused too.
    v["version"] = 99.into();
    let e = decode(&serde_json::to_vec(&v).unwrap()).unwrap_err();
    assert!(matches!(e, FormatError::TooNew { found: 99, .. }), "{e}");
    assert!(e.to_string().contains("Update FilmCraft"));
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
    let dir = std::env::temp_dir().join(format!("filmcraft-big-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    waltcut_format::atomic_write(&dir.join("big.fcproj"), &compact).unwrap();
    let write_ms = t0.elapsed().as_secs_f64() * 1000.0;
    std::fs::remove_dir_all(&dir).unwrap();
    println!(
        "2000-clip project: pretty {:.1} KB in {pretty_ms:.2} ms, compact {:.1} KB in {compact_ms:.2} ms, decode {decode_ms:.2} ms, atomic write+fsync {write_ms:.2} ms",
        pretty.len() as f64 / 1024.0,
        compact.len() as f64 / 1024.0
    );
}
