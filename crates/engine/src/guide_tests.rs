//! Guide clips (#65): the command, and that a guide never reaches a delivery output by default
//! (frame, PNG sequence, movie, AI Restyle input) while the preview shows it.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use waltcut_genai::{GenError, Picture, RestyleRequest, Restyler};
use waltcut_media::Generator;
use waltcut_media::generators::GeneratorSource;
use waltcut_project::{ClipId, ItemId, Label};
use waltcut_time::FrameRate;

use crate::Session;

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let directory = waltcut_testkit::workspace_root().join("target").join("export-tests").join(format!("guides-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

fn matte(s: &mut Session, color: [f32; 4], name: &str) -> ItemId {
    let source = GeneratorSource::new(Generator::ColorMatte { color }, 32, 18, FrameRate::FPS_24, FrameRate::FPS_24.tick_of(24));
    crate::demo::add_generator(Arc::make_mut(&mut s.project), &s.media, source, name, Label::Iris, None)
}

/// V1 a green clip, V2 a red one over it (the reference), 24 frames each.
fn session() -> (Session, ClipId, ClipId) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Guides", "width": 32, "height": 18, "fps": 24, "video": 2, "audio": 1})).unwrap();
    let green = matte(&mut s, [0.0, 1.0, 0.0, 1.0], "Green");
    let red = matte(&mut s, [1.0, 0.0, 0.0, 1.0], "Red");
    s.execute("timeline.place", json!({"item": green.0, "track": "V1"})).unwrap();
    s.execute("timeline.place", json!({"item": red.0, "track": "V2"})).unwrap();
    let q = s.active_sequence().unwrap();
    let (base, reference) = (q.video_tracks[0].items[0].id, q.video_tracks[1].items[0].id);
    (s, base, reference)
}

fn is_guide(s: &Session, c: ClipId) -> bool {
    s.active_sequence().unwrap().find_item(c).unwrap().1.guide
}

fn centre(png: &[u8]) -> [u8; 4] {
    let img = image::load_from_memory(png).unwrap().to_rgba8();
    img.get_pixel(img.width() / 2, img.height() / 2).0
}

#[test]
fn the_command_marks_toggles_and_undoes() {
    let (mut s, base, reference) = session();
    assert!(!is_guide(&s, reference));
    let r = s.execute("clip.setGuide", json!({"clips": [reference.0]})).unwrap();
    assert_eq!((r["guide"].clone(), r["changed"].clone()), (json!(true), json!(1)));
    assert!(is_guide(&s, reference) && !is_guide(&s, base));
    // a second call with no `guide` flips it back; explicit values are idempotent
    s.execute("clip.setGuide", json!({"clips": [reference.0]})).unwrap();
    assert!(!is_guide(&s, reference));
    assert_eq!(s.execute("clip.setGuide", json!({"clips": [reference.0], "guide": false})).unwrap()["changed"], json!(0));
    s.execute("clip.setGuide", json!({"clips": [reference.0], "guide": true})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!is_guide(&s, reference), "one undo step");
    s.execute("edit.redo", json!({})).unwrap();
    assert!(is_guide(&s, reference));
    // the selection is the default target
    s.execute("timeline.select", json!({"clips": [base.0]})).unwrap();
    s.execute("clip.setGuide", json!({})).unwrap();
    assert!(is_guide(&s, base));
}

#[test]
fn refusals_change_nothing() {
    let (mut s, base, reference) = session();
    let audio = s.active_sequence().unwrap().audio_tracks.iter().flat_map(|t| t.items.iter()).map(|i| i.id).next();
    let before = s.project.to_json();
    let depth = s.history.undo.len();
    // a locked track, a missing clip, bad parameters, a mixed list where one clip is refused
    Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap().video_tracks[1].locked = true;
    let locked = s.project.to_json();
    assert!(s.execute("clip.setGuide", json!({"clips": [reference.0]})).unwrap_err().to_string().contains("locked"));
    assert!(s.execute("clip.setGuide", json!({"clips": [base.0, reference.0]})).is_err());
    assert_eq!(s.project.to_json(), locked, "the unlocked clip of a refused list was not changed either");
    Arc::make_mut(&mut s.project).sequence_mut(s.state.active_sequence.unwrap()).unwrap().video_tracks[1].locked = false;
    for p in [
        json!({"clips": [9_999_999]}),
        json!({"clips": "all"}),
        json!({"clips": [reference.0], "guide": "yes"}),
        json!({"clips": [reference.0], "colour": 1}),
        json!({"clips": []}),
    ] {
        assert!(s.execute("clip.setGuide", p.clone()).is_err(), "{p}");
    }
    if let Some(a) = audio {
        assert!(s.execute("clip.setGuide", json!({"clips": [a.0]})).is_err(), "audio clips cannot be guides");
    }
    assert_eq!(s.project.to_json(), before);
    assert_eq!(s.history.undo.len(), depth);
}

#[test]
fn the_preview_shows_a_guide_and_a_frame_export_does_not_unless_asked() {
    let (mut s, _, reference) = session();
    let dir = Scratch::new("frame");
    s.execute("clip.setGuide", json!({"clips": [reference.0]})).unwrap();
    // the Program picture (monitor, render_frame) shows the guide
    let preview = s.render_program(1.0).unwrap();
    assert_eq!(preview.get(16, 9)[0], 1.0, "red in the preview");
    let default = dir.path("default.png");
    let included = dir.path("included.png");
    s.execute("file.exportFrame", json!({"path": default})).unwrap();
    s.execute("file.exportFrame", json!({"path": included, "includeGuides": true})).unwrap();
    assert_eq!(centre(&std::fs::read(&default).unwrap()), GREEN, "a default still leaves the guide out");
    assert_eq!(centre(&std::fs::read(&included).unwrap()), RED);
    assert!(s.execute("file.exportFrame", json!({"path": dir.path("x.png"), "includeGuides": "yes"})).is_err());
    // an ordinary clip is in both
    s.execute("clip.setGuide", json!({"clips": [reference.0], "guide": false})).unwrap();
    s.execute("file.exportFrame", json!({"path": dir.path("plain.png")})).unwrap();
    assert_eq!(centre(&std::fs::read(dir.path("plain.png")).unwrap()), RED);
}

fn read_all(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
        .collect();
    v.sort();
    v
}

#[test]
fn sequence_and_movie_exports_leave_guides_out_by_default() {
    let dir = Scratch::new("movies");
    let (mut s, _, reference) = session();
    s.execute("clip.setGuide", json!({"clips": [reference.0]})).unwrap();
    let range = json!({"range": "custom", "startFrame": 0, "endFrame": 2, "wait": true});
    let run = |s: &mut Session, name: &str, format: &str, include: bool| -> String {
        let path = dir.path(name);
        let mut p = range.clone();
        p["path"] = json!(path);
        p["format"] = json!(format);
        if include {
            p["includeGuides"] = json!(true);
        }
        s.execute("file.exportMedia", p).unwrap();
        path
    };
    // the same edit without the reference clip at all is what a default export must equal
    let (mut bare, _, bare_ref) = session();
    bare.execute("timeline.select", json!({"clips": [bare_ref.0]})).unwrap();
    bare.execute("edit.clear", json!({})).unwrap();

    // PNG sequence
    for (what, include, session_) in [("default", false, 0), ("included", true, 0), ("bare", false, 1)] {
        let base = run(if session_ == 0 { &mut s } else { &mut bare }, &format!("{what}.png"), "png", include);
        let _ = base;
    }
    let frames =
        |what: &str| -> Vec<Vec<u8>> { read_all(&dir.0).into_iter().filter(|(n, _)| n.starts_with(what) && n.ends_with(".png")).map(|(_, b)| b).collect() };
    let (default, included, bare_frames) = (frames("default"), frames("included"), frames("bare"));
    assert!(!default.is_empty() && default.len() == included.len() && default.len() == bare_frames.len());
    assert!(default.iter().all(|f| centre(f) == GREEN), "default frames leave the guide out");
    assert!(included.iter().all(|f| centre(f) == RED), "includeGuides draws it");
    assert_eq!(default, bare_frames, "a default export of a guided project is the export of the project without the guide");

    // movie: ProRes bytes are deterministic here, so the same comparison holds
    let (d, i, b) = (run(&mut s, "default.mov", "prores", false), run(&mut s, "included.mov", "prores", true), run(&mut bare, "bare.mov", "prores", false));
    let (d, i, b) = (std::fs::read(d).unwrap(), std::fs::read(i).unwrap(), std::fs::read(b).unwrap());
    assert_ne!(d, i, "including the guide changes the movie");
    assert_eq!(d.len(), b.len(), "same size as the unguided edit");
    // the quick export has the same switch
    let q = dir.path("quick.png");
    s.execute("export.quick", json!({"path": q, "format": "png", "range": "custom", "startFrame": 0, "endFrame": 1, "wait": true})).unwrap();
    assert!(read_all(&dir.0).iter().filter(|(n, _)| n.starts_with("quick")).all(|(_, b)| centre(b) == GREEN));
}

struct Capture(Mutex<Vec<Vec<u8>>>);
impl Restyler for Capture {
    fn name(&self) -> &str {
        "capture"
    }
    fn restyle(&self, picture: &Picture, _: &RestyleRequest) -> waltcut_genai::Result<Vec<u8>> {
        self.0.lock().unwrap().push(picture.png.clone());
        Err(GenError::Provider("stop".into()))
    }
}

#[test]
fn the_picture_sent_to_ai_restyle_has_no_guide_unless_asked() {
    let (mut s, _, reference) = session();
    s.execute("clip.setGuide", json!({"clips": [reference.0]})).unwrap();
    let cap = Arc::new(Capture(Mutex::new(Vec::new())));
    s.restyler_override = Some(cap.clone());
    let ask = |s: &mut Session, extra: Value| {
        let mut p = json!({"prompt": "oil painting", "range": "playhead", "wait": true});
        if let (Some(a), Some(b)) = (p.as_object_mut(), extra.as_object()) {
            a.extend(b.clone());
        }
        let _ = s.execute("ai.restyle", p);
    };
    ask(&mut s, json!({}));
    ask(&mut s, json!({"includeGuides": true}));
    let sent = cap.0.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(centre(&sent[0]), GREEN, "the default request is the picture without the guide");
    assert_eq!(centre(&sent[1]), RED);
    assert!(s.execute("ai.restyle", json!({"prompt": "x", "includeGuides": 3})).is_err());
}

#[test]
fn guide_status_survives_save_and_load_and_unguided_projects_are_unchanged() {
    let dir = Scratch::new("save");
    let (mut s, _, reference) = session();
    let plain = s.project.to_json();
    assert!(!plain.contains("\"guide\""), "no guide, no key: projects without guides are byte-compatible");
    s.execute("clip.setGuide", json!({"clips": [reference.0]})).unwrap();
    let path = dir.path("g.waltcut");
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    assert!(String::from_utf8_lossy(&std::fs::read(&path).unwrap()).contains("\"guide\":true"));
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path})).unwrap();
    let flagged = t.active_sequence().unwrap().video_tracks.iter().flat_map(|tr| tr.items.iter()).filter(|i| i.guide).count();
    assert_eq!(flagged, 1);
    // and the sequence is checked like any other edit (a split keeps the flag on both halves)
    let seq = t.active_sequence().unwrap();
    let clip = seq.video_tracks.iter().flat_map(|tr| tr.items.iter()).find(|i| i.guide).unwrap().id;
    t.execute("timeline.razor", json!({"time": FrameRate::FPS_24.tick_of(12).0, "clip": clip.0})).unwrap();
    let halves = t.active_sequence().unwrap().video_tracks.iter().flat_map(|tr| tr.items.iter()).filter(|i| i.guide).count();
    assert_eq!(halves, 2, "a split of a guide is two guides");
}
