//! Reference images (#3): one placement action, atomic and undoable; adjustable; exported like
//! ordinary artwork unless hidden or marked as a guide.
use std::path::PathBuf;

use serde_json::{Value, json};
use waltcut_project::ClipId;
use waltcut_time::FrameRate;

use crate::Session;

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let directory = waltcut_testkit::workspace_root().join("target").join("export-tests").join(format!("reference-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
    /// A solid-colour PNG of the sequence's size.
    fn picture(&self, name: &str, rgba: [u8; 4]) -> String {
        let path = self.path(name);
        std::fs::write(&path, waltcut_export::encode_png(rgba.repeat(64 * 36), 64, 36).unwrap()).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const BLUE: [u8; 4] = [0, 0, 255, 255];

/// A 64x36 sequence with a two-second drawing on V2 (V1 empty); the playhead on frame 5.
fn session() -> (Session, ClipId) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Ref", "width": 64, "height": 36, "fps": 24, "video": 2, "audio": 0})).unwrap();
    let drawing = s.execute("drawing.new", json!({"seconds": 2, "track": 2})).unwrap()["clip"].as_u64().unwrap();
    s.execute("playhead.set", json!({"frame": 5})).unwrap();
    s.execute("timeline.select", json!({"clips": [drawing]})).unwrap();
    (s, ClipId(drawing))
}

fn tracks(s: &Session) -> Vec<(String, Vec<u64>)> {
    s.active_sequence().unwrap().video_tracks.iter().map(|t| (t.name.clone(), t.items.iter().map(|i| i.id.0).collect())).collect()
}

fn centre(s: &mut Session, dir: &Scratch, name: &str, extra: Value) -> [u8; 4] {
    let mut p = json!({"path": dir.path(name), "overwrite": true});
    if let (Some(a), Some(b)) = (p.as_object_mut(), extra.as_object()) {
        a.extend(b.clone());
    }
    s.execute("file.exportFrame", p).unwrap();
    let img = image::load_from_memory(&std::fs::read(dir.path(name)).unwrap()).unwrap().to_rgba8();
    img.get_pixel(img.width() / 2, img.height() / 2).0
}

#[test]
fn one_action_places_the_picture_below_the_drawing_for_its_duration() {
    let dir = Scratch::new("place");
    let (mut s, drawing) = session();
    let (playhead, selection) = (s.playhead(), s.state.selection.clone());
    let items_before = s.project.items.len();
    let r = s.execute("reference.place", json!({"path": dir.picture("ref.png", BLUE)})).unwrap();
    let reference = ClipId(r["clip"].as_u64().unwrap());
    let q = s.active_sequence().unwrap();
    let (rt, rc) = q.find_item(reference).unwrap();
    let (dt, dc) = q.find_item(drawing).unwrap();
    assert_eq!(q.track(rt).unwrap().name, "Reference");
    let at = |id| q.video_tracks.iter().position(|t| t.id == id).unwrap();
    assert_eq!(at(rt) + 1, at(dt), "a Reference track right below the drawing's");
    assert_eq!(q.video_tracks.len(), 3);
    assert_eq!((rc.start, rc.duration), (dc.start, dc.duration), "it lasts as long as the drawing");
    assert!(rc.enabled && !rc.guide);
    assert_eq!(s.execute("reference.list", json!({})).unwrap()["references"][0]["opacity"], json!(50.0), "starts at the documented tracing opacity");
    assert_eq!((s.playhead(), s.state.selection.clone()), (playhead, selection), "the playhead and selection stay");
    // one undo step takes back the import, the track and the clip
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.items.len(), items_before);
    assert_eq!(tracks(&s).len(), 2);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(tracks(&s).len(), 3);
}

#[test]
fn a_second_reference_goes_on_a_new_track_and_a_free_reference_track_is_reused() {
    let dir = Scratch::new("tracks");
    let (mut s, drawing) = session();
    let a = s.execute("reference.place", json!({"path": dir.picture("a.png", BLUE)})).unwrap();
    let b = s.execute("reference.place", json!({"path": dir.picture("b.png", BLUE)})).unwrap();
    assert_eq!(b["newTrack"], json!(true), "the first reference still occupies the track below");
    let names: Vec<_> = tracks(&s).into_iter().map(|t| t.0.starts_with("Reference")).collect();
    assert_eq!(names.iter().filter(|r| **r).count(), 2, "two Reference tracks");
    let q = s.active_sequence().unwrap();
    let pos = |t: &Value| q.video_tracks.iter().position(|x| x.id.0 == t.as_u64().unwrap()).unwrap();
    let dt = q.find_item(drawing).unwrap().0;
    let dpos = q.video_tracks.iter().position(|t| t.id == dt).unwrap();
    assert_eq!((pos(&a["track"]) + 1, pos(&b["track"]) + 1), (pos(&b["track"]), dpos), "the newest sits right below the drawing, above the older one");
    // free the track right below the drawing, then place again: reused
    s.execute("timeline.select", json!({"clips": [b["clip"].as_u64().unwrap()]})).unwrap();
    s.execute("edit.clear", json!({})).unwrap();
    s.execute("timeline.select", json!({"clips": [drawing.0]})).unwrap();
    let c = s.execute("reference.place", json!({"item": a["item"]})).unwrap();
    assert_eq!(c["newTrack"], json!(false));
    assert_eq!(c["track"], b["track"], "the same Reference track again");
}

#[test]
fn refusals_leave_nothing_behind() {
    let dir = Scratch::new("refuse");
    let (mut s, drawing) = session();
    let picture = dir.picture("ref.png", BLUE);
    let not_an_image = dir.path("notes.txt");
    std::fs::write(&not_an_image, "hello").unwrap();
    let before = s.project.to_json();
    let depth = s.history.undo.len();
    let state = |s: &Session| (s.project.to_json(), s.history.undo.len());
    // a missing file, a file that is not a picture, bad parameters
    for p in [
        json!({"path": dir.path("missing.png")}),
        json!({"path": not_an_image}),
        json!({"path": ""}),
        json!({}),
        json!({"path": picture, "item": 1}),
        json!({"path": picture, "opacity": 101}),
        json!({"path": picture, "opacity": "half"}),
        json!({"path": picture, "guide": 1}),
        json!({"path": picture, "colour": 1}),
        json!({"item": 9_999_999}),
        json!({"path": picture, "target": 9_999_999}),
    ] {
        assert!(s.execute("reference.place", p.clone()).is_err(), "{p}");
        assert_eq!(state(&s), (before.clone(), depth), "{p} left something behind");
    }
    // a locked drawing track
    let di = s.active_sequence().unwrap().video_tracks.iter().position(|t| t.items.iter().any(|i| i.id == drawing)).unwrap();
    Arc_lock(&mut s, di, true);
    let locked = state(&s);
    assert!(s.execute("reference.place", json!({"path": picture})).unwrap_err().to_string().contains("locked"));
    assert_eq!(state(&s), locked, "no import, no track");
    Arc_lock(&mut s, di, false);
    // a locked Reference track below
    s.execute("reference.place", json!({"path": picture})).unwrap();
    let after_place = s.active_sequence().unwrap().video_tracks.iter().find(|t| t.name == "Reference").unwrap().id;
    s.execute("reference.set", json!({"locked": true})).unwrap();
    let locked = state(&s);
    assert!(s.execute("reference.place", json!({"path": picture})).is_err());
    assert_eq!(state(&s), locked);
    assert!(s.active_sequence().unwrap().video_tracks.iter().any(|t| t.id == after_place && t.locked));
}

#[allow(non_snake_case)]
fn Arc_lock(s: &mut Session, track: usize, locked: bool) {
    let seq = s.state.active_sequence.unwrap();
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[track].locked = locked;
}

#[test]
fn a_reference_can_be_adjusted_hidden_locked_and_a_drag_is_one_undo_step() {
    let dir = Scratch::new("adjust");
    let (mut s, _) = session();
    s.execute("reference.place", json!({"path": dir.picture("ref.png", BLUE)})).unwrap();
    let get = |s: &mut Session| s.execute("reference.list", json!({})).unwrap()["references"][0].clone();
    let r = s.execute("reference.set", json!({"opacity": 30, "scale": 150, "rotation": 12.5, "x": 40, "y": 10})).unwrap();
    assert_eq!(
        (r["opacity"].clone(), r["scale"].clone(), r["rotation"].clone(), r["x"].clone(), r["y"].clone()),
        (json!(30.0), json!(150.0), json!(12.5), json!(40.0), json!(10.0))
    );
    // relative moves from the current position
    s.execute("reference.set", json!({"dx": 5, "dy": -2.5})).unwrap();
    assert_eq!((get(&mut s)["x"].clone(), get(&mut s)["y"].clone()), (json!(45.0), json!(7.5)));
    let depth = s.history.undo.len();
    // a drag: the first call is a step, the following ones merge into it
    s.execute("reference.set", json!({"dx": 1, "merge": "start"})).unwrap();
    for _ in 0..5 {
        s.execute("reference.set", json!({"dx": 1, "dy": 1, "merge": true})).unwrap();
    }
    assert_eq!(s.history.undo.len(), depth + 1, "a drag is one undo step");
    assert_eq!(get(&mut s)["x"], json!(51.0));
    // the next drag is a step of its own, not part of the first
    s.execute("reference.set", json!({"dx": 2, "merge": "start"})).unwrap();
    s.execute("reference.set", json!({"dx": 2, "merge": true})).unwrap();
    assert_eq!(s.history.undo.len(), depth + 2);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(get(&mut s)["x"], json!(51.0), "only the second drag is undone");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(get(&mut s)["x"], json!(45.0), "the whole first drag is undone");
    // hide, guide, lock
    let r = s.execute("reference.set", json!({"hidden": true, "guide": true})).unwrap();
    assert_eq!((r["hidden"].clone(), r["guide"].clone()), (json!(true), json!(true)));
    s.execute("reference.set", json!({"hidden": false, "guide": false, "locked": true})).unwrap();
    assert_eq!(get(&mut s)["locked"], json!(true));
    // locked: edits are refused, unlocking is allowed
    let locked = s.project.to_json();
    assert!(s.execute("reference.set", json!({"opacity": 10})).unwrap_err().to_string().contains("locked"));
    assert_eq!(s.project.to_json(), locked);
    s.execute("reference.set", json!({"locked": false})).unwrap();
    s.execute("reference.set", json!({"opacity": 10})).unwrap();
}

#[test]
fn bad_adjustments_are_refused_whole() {
    let dir = Scratch::new("bad");
    let (mut s, drawing) = session();
    s.execute("reference.place", json!({"path": dir.picture("ref.png", BLUE)})).unwrap();
    let before = (s.project.to_json(), s.history.undo.len());
    for p in [
        json!({"opacity": -1}),
        json!({"opacity": 1e9}),
        json!({"scale": 0}),
        json!({"scale": 1e9}),
        json!({"rotation": "left"}),
        json!({"x": 1e12}),
        json!({"x": 1, "dx": 2}),
        json!({"hidden": "yes"}),
        json!({"merge": 3}),
        json!({"colour": 1}),
        json!({"opacity": 20, "scale": -4}),
        json!({"clip": drawing.0, "opacity": 20}),
        json!({"clip": 9_999_999, "opacity": 20}),
    ] {
        assert!(s.execute("reference.set", p.clone()).is_err(), "{p}");
        assert_eq!((s.project.to_json(), s.history.undo.len()), before, "{p} changed something");
    }
    // a drawing without a reference
    let (mut other, _) = session();
    assert!(other.execute("reference.set", json!({"opacity": 20})).is_err());
    assert_eq!(other.execute("reference.list", json!({})).unwrap()["references"], json!([]));
}

#[test]
fn an_ordinary_reference_is_exported_a_hidden_or_guide_one_is_not() {
    let dir = Scratch::new("export");
    let (mut s, _) = session();
    let empty = centre(&mut s, &dir, "empty.png", json!({}));
    assert_eq!(empty[3], 0, "an empty drawing exports transparent");
    s.execute("reference.place", json!({"path": dir.picture("ref.png", BLUE), "opacity": 100})).unwrap();
    assert_eq!(centre(&mut s, &dir, "visible.png", json!({})), BLUE, "a visible reference is ordinary exportable artwork");
    s.execute("reference.set", json!({"hidden": true})).unwrap();
    assert_eq!(centre(&mut s, &dir, "hidden.png", json!({}))[3], 0, "hidden: not exported");
    s.execute("reference.set", json!({"hidden": false, "guide": true})).unwrap();
    assert_eq!(centre(&mut s, &dir, "guide.png", json!({}))[3], 0, "a guide stays out of a default export (#65)");
    assert_eq!(centre(&mut s, &dir, "included.png", json!({"includeGuides": true})), BLUE);
    // and the preview shows it (hidden is the only way to drop it from the monitor)
    assert_eq!(s.render_program(1.0).unwrap().get(32, 18)[2], 1.0);
}

#[test]
fn the_reference_survives_save_and_load() {
    let dir = Scratch::new("save");
    let (mut s, drawing) = session();
    s.execute("reference.place", json!({"path": dir.picture("ref.png", BLUE), "opacity": 40})).unwrap();
    s.execute("reference.set", json!({"x": 12, "y": 8, "scale": 80, "rotation": 5, "hidden": true})).unwrap();
    let want = s.execute("reference.list", json!({})).unwrap()["references"][0].clone();
    let path = dir.path("r.waltcut");
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path})).unwrap();
    let got = t.execute("reference.list", json!({"target": drawing.0})).unwrap()["references"][0].clone();
    assert_eq!(got, want);
}

#[test]
fn the_frame_rate_of_the_drawing_decides_nothing_about_the_reference_length() {
    // 12 fps sequence: the reference still covers exactly the drawing clip
    let dir = Scratch::new("rate");
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "R", "width": 64, "height": 36, "fps": 12, "video": 2, "audio": 0})).unwrap();
    let drawing = s.execute("drawing.new", json!({"seconds": 1.5, "track": 2})).unwrap()["clip"].as_u64().unwrap();
    s.execute("timeline.select", json!({"clips": [drawing]})).unwrap();
    let r = s.execute("reference.place", json!({"path": dir.picture("ref.png", BLUE)})).unwrap();
    let q = s.active_sequence().unwrap();
    let (_, rc) = q.find_item(ClipId(r["clip"].as_u64().unwrap())).unwrap();
    let (_, dc) = q.find_item(ClipId(drawing)).unwrap();
    assert_eq!((rc.start, rc.end()), (dc.start, dc.end()));
    assert_eq!(FrameRate::new(12, 1).frame_at(rc.duration), 18);
}
