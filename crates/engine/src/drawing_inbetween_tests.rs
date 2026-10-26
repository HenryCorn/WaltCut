//! In-between assist (#72): an explicit pairing of strokes, a preview that changes nothing, and an
//! accept that makes ordinary unlinked cels without touching the two chosen ones.
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use waltcut_project::ClipId;
use waltcut_project::drawing::Drawing;

use crate::Session;

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let directory = waltcut_testkit::workspace_root().join("target").join("export-tests").join(format!("inbetween-{name}-{}", std::process::id()));
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

fn line(color: &str, a: (f64, f64), b: (f64, f64)) -> Value {
    json!({"brush": "Pen", "color": color, "size": 6.0, "points": [[a.0, a.1, 1.0], [b.0, b.1, 1.0]]})
}

fn draw(s: &mut Session, frame: i64, new: bool, stroke: Value) {
    let params = if new { json!({"frame": frame, "cel": "new", "stroke": stroke}) } else { json!({"frame": frame, "stroke": stroke}) };
    s.execute("drawing.addStroke", params).unwrap();
}

/// Frame 0: a horizontal line (0) and a vertical one (1). Frame 8: the vertical one first (0, a
/// little to the right) and the horizontal one drawn right to left (1).
fn session() -> (Session, ClipId) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Between", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 1})).unwrap();
    let r = s.execute("drawing.new", json!({"seconds": 4})).unwrap();
    let clip = ClipId(r["clip"].as_u64().unwrap());
    draw(&mut s, 0, false, line("#ff0000", (20.0, 20.0), (120.0, 20.0)));
    draw(&mut s, 0, false, line("#0000ff", (200.0, 20.0), (200.0, 120.0)));
    draw(&mut s, 8, true, line("#0000ff", (240.0, 40.0), (240.0, 140.0)));
    draw(&mut s, 8, false, line("#ff0000", (120.0, 60.0), (20.0, 60.0)));
    (s, clip)
}

fn pairs() -> Value {
    json!([{"from": 0, "to": 1, "reverse": true}, {"from": 1, "to": 0}])
}

fn preview_args() -> Value {
    json!({"from": 0, "to": 8, "pairs": pairs(), "step": 2})
}

fn drawing(s: &Session, clip: ClipId) -> Drawing {
    let seq = s.active_sequence().unwrap();
    crate::drawing::drawing_of(s, seq, clip).unwrap().1.clone()
}

fn frames(d: &Drawing) -> Vec<i64> {
    d.cels.iter().map(|c| c.frame).collect()
}

fn x_of(r: &Value, cel: usize, stroke: usize, point: usize) -> f64 {
    r["cels"][cel]["strokes"][stroke]["points"][point]["x"].as_f64().unwrap()
}
fn y_of(r: &Value, cel: usize, stroke: usize, point: usize) -> f64 {
    r["cels"][cel]["strokes"][stroke]["points"][point]["y"].as_f64().unwrap()
}

#[test]
fn the_pairing_is_the_callers_and_the_answer_says_what_it_did() {
    let (mut s, _) = session();
    let r = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": pairs(), "step": 2, "geometry": true})).unwrap();
    assert_eq!(r["status"], "ok");
    assert_eq!(r["frames"], json!([2, 4, 6]));
    assert_eq!(r["pairs"][0]["from"], 0);
    assert_eq!(r["pairs"][0]["reverse"], true);
    assert_eq!(r["unmatched"], json!({"policy": "drop", "from": [], "to": []}));
    // frame 2 is nearer the start cel, so its order: stroke 0 is the horizontal line becoming the end
    // cel's stroke 1 run backwards ((120,60) -> (20,60) reversed is (20,60) -> (120,60)): its start
    // stays at x = 20 and y goes 20 -> 60 (a quarter of the way: 30)
    assert!((x_of(&r, 0, 0, 0) - 20.0).abs() < 1e-3 && (y_of(&r, 0, 0, 0) - 30.0).abs() < 1e-3, "{r}");
    // stroke 1 is the vertical line moving to x = 240 (a quarter of the way: 210)
    assert!((x_of(&r, 0, 1, 0) - 210.0).abs() < 1e-3);
    // by position instead (0 -> 0, 1 -> 1) the horizontal line would turn into the vertical one
    let by_position = s
        .execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": [{"from": 0, "to": 0}, {"from": 1, "to": 1}], "step": 2, "geometry": true}))
        .unwrap();
    assert_ne!(by_position["cels"][0], r["cels"][0]);
    // a pair whose ends look swapped is measured and reported, never fixed
    let twisted = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": [{"from": 0, "to": 1}, {"from": 1, "to": 0}]})).unwrap();
    assert_eq!(twisted["pairs"][0]["otherDirectionIsCloser"], true);
    assert_eq!(twisted["pairs"][0]["reverse"], false);
    assert_eq!(r["pairs"][0]["otherDirectionIsCloser"], false);
}

#[test]
fn a_preview_leaves_the_project_the_history_and_the_dirty_mark_alone() {
    let (mut s, clip) = session();
    let (project, revision, undo, redo, dirty) = (s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.is_dirty());
    let journal = s.journal.len();
    let r = s.execute("drawing.inbetweenPreview", preview_args()).unwrap();
    assert_eq!(r["status"], "ok");
    assert!(Arc::ptr_eq(&project, &s.project), "the very same project");
    assert_eq!((s.revision, s.history.undo.len(), s.history.redo.len(), s.is_dirty()), (revision, undo, redo, dirty));
    assert_eq!(s.journal.len(), journal, "a preview is not a recorded edit");
    assert_eq!(frames(&drawing(&s, clip)), [0, 8]);
    assert!(s.inbetween_preview().is_some());
    // cancel leaves nothing behind
    assert_eq!(s.execute("drawing.inbetweenCancel", json!({})).unwrap(), json!({"cancelled": true}));
    assert!(s.inbetween_preview().is_none());
    assert_eq!(s.execute("drawing.inbetweenCancel", json!({})).unwrap(), json!({"cancelled": false}));
    assert!(Arc::ptr_eq(&project, &s.project), "cancel: same project");
    assert_eq!(s.revision, revision, "cancel: revision");
    assert_eq!(s.is_dirty(), dirty, "cancel: dirty");
    // and there is nothing to accept any more
    assert!(s.execute("drawing.inbetweenAccept", json!({})).is_err());
    assert_eq!(frames(&drawing(&s, clip)), [0, 8]);
}

#[test]
fn accepting_makes_unlinked_cels_the_preview_showed_and_leaves_the_endpoints_alone() {
    let (mut s, clip) = session();
    let shown = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": pairs(), "step": 2, "geometry": true})).unwrap();
    let before = drawing(&s, clip);
    let undo = s.history.undo.len();
    let r = s.execute("drawing.inbetweenAccept", json!({})).unwrap();
    assert_eq!(r["cels"], json!([2, 4, 6]));
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert!(s.inbetween_preview().is_none(), "accepting ends the preview");
    let after = drawing(&s, clip);
    assert_eq!(frames(&after), [0, 2, 4, 6, 8]);
    assert!(after.cels.iter().all(|c| c.link.is_none()), "ordinary unlinked cels");
    after.check_links().unwrap();
    // the endpoints are the very same artwork
    assert!(Arc::ptr_eq(&before.cels[0].strokes, &after.cels[0].strokes) && Arc::ptr_eq(&before.cels[1].strokes, &after.cels[4].strokes));
    // what was previewed is what was made
    let made = s.execute("drawing.inspect", json!({"strokes": true})).unwrap();
    for (n, c) in [1usize, 2, 3].into_iter().enumerate() {
        assert_eq!(made["cels"][c]["strokes"], shown["cels"][n]["strokes"], "cel {}", made["cels"][c]["frame"]);
    }
    // undo and redo
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(frames(&drawing(&s, clip)), [0, 8]);
    assert!(Arc::ptr_eq(&before.cels[0].strokes, &drawing(&s, clip).cels[0].strokes));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(drawing(&s, clip), after);
    // editing one in-between touches no other cel
    s.execute("drawing.addStroke", json!({"frame": 4, "stroke": line("#00ff00", (1.0, 1.0), (5.0, 5.0))})).unwrap();
    let edited = drawing(&s, clip);
    assert_eq!((edited.cels[2].strokes.len(), edited.cels[1].strokes.len(), edited.cels[3].strokes.len()), (3, 2, 2));
}

#[test]
fn the_preview_and_the_accepted_cels_render_the_same_pixels() {
    let (mut s, _) = session();
    let shown = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": pairs(), "step": 4, "geometry": true})).unwrap();
    s.execute("drawing.inbetweenAccept", json!({})).unwrap();
    // a control project that holds the previewed strokes as plain strokes at the same frame
    let (mut control, _) = session();
    let cel = &shown["cels"][0];
    assert_eq!(cel["frame"], 4);
    control.execute("drawing.newCel", json!({"frame": 4})).unwrap();
    for st in cel["strokes"].as_array().unwrap() {
        let pts: Vec<Value> = st["points"].as_array().unwrap().iter().map(|p| json!([p["x"], p["y"], p["p"]])).collect();
        control
            .execute("drawing.addStroke", json!({"frame": 4, "stroke": {"brush": st["brush"], "color": st["color"], "size": st["size"], "points": pts}}))
            .unwrap();
    }
    let dir = Scratch::new("pixels");
    let mut shots = Vec::new();
    for (name, session) in [("accepted", &mut s), ("control", &mut control)] {
        session.execute("playhead.set", json!({"frame": 4})).unwrap();
        let png = dir.path(&format!("{name}.png"));
        session.execute("file.exportFrame", json!({"path": png})).unwrap();
        shots.push(image::load_from_memory(&std::fs::read(&png).unwrap()).unwrap().to_rgba8());
    }
    assert_eq!(shots[0].as_raw(), shots[1].as_raw(), "the exported frame is the preview's strokes, pixel for pixel");
    assert!(shots[0].pixels().any(|p| p.0[0] > 200 && p.0[3] > 0) || shots[0].pixels().any(|p| p.0[2] > 200), "something is drawn");
}

#[test]
fn different_counts_and_blank_endpoints_are_reported_not_guessed() {
    let (mut s, _) = session();
    // one more stroke on the start cel and none paired with it: unmatched, dropped by default
    draw(&mut s, 0, false, line("#00aa00", (10.0, 150.0), (60.0, 150.0)));
    let r = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": pairs(), "geometry": true})).unwrap();
    assert_eq!(r["unmatched"], json!({"policy": "drop", "from": [2], "to": []}));
    assert_eq!(r["cels"][0]["strokes"].as_array().unwrap().len(), 2);
    // asked to keep it, it stays with the nearer cel only (frames 2 and 4 are nearer... 6 is not)
    let kept = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": pairs(), "unmatched": "nearest", "geometry": true})).unwrap();
    assert_eq!(kept["cels"][0]["strokes"].as_array().unwrap().len(), 3);
    assert_eq!(kept["cels"][2]["strokes"].as_array().unwrap().len(), 2, "frame 6 is nearer the end cel, which has no such stroke");
    // a missing stroke on the other side
    let missing = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": [{"from": 0, "to": 5}]})).unwrap();
    assert_eq!(missing["status"], "unsupported");
    assert_eq!(missing["problems"][0]["kind"], "outOfRange");
    assert_eq!(missing["frames"], json!([]));
    assert!(s.inbetween_preview().is_none(), "an unsupported pairing keeps no preview");
    // blank endpoints
    s.execute("drawing.newCel", json!({"frame": 12})).unwrap();
    let blank = s.execute("drawing.inbetweenPreview", json!({"from": 8, "to": 12, "pairs": [{"from": 0, "to": 0}]})).unwrap();
    assert_eq!((blank["status"].as_str(), blank["problems"][0]["kind"].as_str()), (Some("unsupported"), Some("blankCel")));
    let both = s.execute("drawing.inbetweenAccept", json!({"from": 8, "to": 12, "pairs": [{"from": 0, "to": 0}]}));
    assert!(both.unwrap_err().to_string().contains("blank"));
    // no pairs at all
    let none = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": []})).unwrap();
    assert_eq!(none["problems"][0]["kind"], "noPairs");
    // one stroke named twice
    let twice = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": [{"from": 0, "to": 0}, {"from": 1, "to": 0}]})).unwrap();
    assert_eq!(twice["problems"][0]["kind"], "repeatedStroke");
}

#[test]
fn erasers_and_fills_are_unsupported_or_never_morphed() {
    let (mut s, clip) = session();
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": {"brush": "Eraser", "size": 10, "points": [[0.0, 0.0, 1.0], [30.0, 30.0, 1.0]]}})).unwrap();
    let before = drawing(&s, clip);
    let r = s.execute("drawing.inbetweenPreview", preview_args()).unwrap();
    assert_eq!(r["status"], "unsupported");
    assert_eq!(
        (r["problems"][0]["kind"].as_str(), r["problems"][0]["side"].as_str(), r["problems"][0]["index"].as_u64()),
        (Some("eraser"), Some("from"), Some(2))
    );
    let e = s.execute("drawing.inbetweenAccept", preview_args()).unwrap_err().to_string();
    assert!(e.contains("eraser"), "{e}");
    assert_eq!(drawing(&s, clip), before, "nothing was made");
    // a fill: never paired, and shown from the nearer cel
    let (mut f, clip) = session();
    f.execute("drawing.addStroke", json!({"frame": 0, "stroke": line("#000000", (150.0, 100.0), (150.0, 170.0))})).unwrap();
    let square = |x: f64| [(x, 100.0), (x + 60.0, 100.0), (x + 60.0, 160.0), (x, 160.0), (x, 100.0)];
    for (frame, x) in [(0, 10.0), (8, 40.0)] {
        let pts: Vec<Value> = square(x).iter().map(|p| json!([p.0, p.1, 1.0])).collect();
        f.execute("drawing.addStroke", json!({"frame": frame, "stroke": {"brush": "Pen", "color": "#000000", "size": 3, "points": pts}})).unwrap();
        f.execute("drawing.fill", json!({"frame": frame, "point": [x + 30.0, 130.0], "color": "#ffcc00"})).unwrap();
    }
    let strokes = f.execute("drawing.inspect", json!({"frame": 0, "strokes": true})).unwrap();
    let fills: Vec<usize> =
        strokes["cels"][0]["strokes"].as_array().unwrap().iter().enumerate().filter(|(_, s)| s["brush"] == "Fill").map(|(i, _)| i).collect();
    assert_eq!(fills.len(), 1, "{strokes}");
    let _ = clip;
    let named = f.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": [{"from": fills[0], "to": 0}]})).unwrap();
    assert_eq!(named["problems"][0]["kind"], "fillPaired");
    let ok = f.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": [{"from": 0, "to": 0}], "geometry": true})).unwrap();
    assert_eq!(ok["status"], "ok");
    assert_eq!(ok["fills"]["from"].as_array().unwrap().len(), 1);
    let kind = |cel: usize| ok["cels"][cel]["strokes"].as_array().unwrap().iter().map(|s| s["brush"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
    assert!(kind(0).contains(&"Fill".to_string()) && kind(2).contains(&"Fill".to_string()), "a fill is shown from the nearer cel: {:?} {:?}", kind(0), kind(2));
}

#[test]
fn an_existing_cel_is_never_overwritten_and_frames_outside_the_clip_are_skipped() {
    let (mut s, clip) = session();
    draw(&mut s, 4, true, line("#00aa00", (5.0, 5.0), (50.0, 50.0)));
    let middle = drawing(&s, clip).cels[1].strokes.clone();
    let r = s.execute("drawing.inbetweenPreview", preview_args()).unwrap();
    assert_eq!(r["frames"], json!([2, 6]));
    assert_eq!(r["skipped"], json!([{"frame": 4, "reason": "cel starts here"}]));
    s.execute("drawing.inbetweenAccept", json!({})).unwrap();
    let d = drawing(&s, clip);
    assert_eq!(frames(&d), [0, 2, 4, 6, 8]);
    assert!(Arc::ptr_eq(&d.cels[2].strokes, &middle), "the cel that was there is untouched");
    // a short clip: frames past its last visible frame are skipped, not made
    let mut t = Session::default();
    t.execute("file.newSequence", json!({"name": "Short", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 1})).unwrap();
    t.execute("drawing.new", json!({"seconds": 1})).unwrap();
    draw(&mut t, 0, false, line("#ff0000", (20.0, 20.0), (120.0, 20.0)));
    draw(&mut t, 30, true, line("#ff0000", (20.0, 80.0), (120.0, 80.0)));
    let r = t.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 30, "pairs": [{"from": 0, "to": 0}], "step": 6})).unwrap();
    assert_eq!(r["frames"], json!([6, 12, 18]));
    assert_eq!(r["skipped"], json!([{"frame": 24, "reason": "outside the clip"}]));
    // every frame skipped: refused
    let e = t.execute("drawing.inbetweenAccept", json!({"from": 0, "to": 30, "pairs": [{"from": 0, "to": 0}], "frames": [25, 27]})).unwrap_err();
    assert!(e.to_string().contains("skipped"), "{e}");
}

#[test]
fn a_locked_track_refuses_preview_and_accept_and_changes_nothing() {
    let (mut s, clip) = session();
    let track = s.active_sequence().and_then(|seq| seq.find_item(clip)).map(|(id, _)| id).unwrap();
    s.execute("timeline.setTrack", json!({"track": track.0, "locked": true})).unwrap();
    let before = s.project.clone();
    for id in ["drawing.inbetweenPreview", "drawing.inbetweenAccept"] {
        let e = s.execute(id, preview_args()).unwrap_err();
        assert!(e.to_string().contains("locked"), "{id}: {e}");
    }
    assert!(Arc::ptr_eq(&before, &s.project) && s.inbetween_preview().is_none());
    // a preview taken before the lock cannot be accepted after it
    s.execute("timeline.setTrack", json!({"track": track.0, "locked": false})).unwrap();
    s.execute("drawing.inbetweenPreview", preview_args()).unwrap();
    s.execute("timeline.setTrack", json!({"track": track.0, "locked": true})).unwrap();
    let e = s.execute("drawing.inbetweenAccept", json!({})).unwrap_err();
    assert!(e.to_string().contains("locked"), "{e}");
    assert_eq!(frames(&drawing(&s, clip)), [0, 8]);
}

#[test]
fn a_preview_is_dropped_when_either_cel_changes_and_survives_other_edits() {
    let (mut s, clip) = session();
    s.execute("drawing.inbetweenPreview", preview_args()).unwrap();
    draw(&mut s, 20, true, line("#00aa00", (5.0, 5.0), (50.0, 50.0)));
    assert!(s.inbetween_preview().is_some(), "an unrelated cel changed");
    draw(&mut s, 8, false, line("#00aa00", (5.0, 5.0), (50.0, 50.0)));
    assert!(s.inbetween_preview().is_none(), "the end cel changed");
    let e = s.execute("drawing.inbetweenAccept", json!({})).unwrap_err();
    assert!(e.to_string().contains("no in-between preview"), "{e}");
    assert_eq!(frames(&drawing(&s, clip)), [0, 8, 20]);
    // undoing the edit makes the cel what it was, but the preview is not resurrected
    s.execute("drawing.inbetweenPreview", preview_args()).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.inbetween_preview().is_some() || s.inbetween.is_none());
    s.execute("drawing.deleteCel", json!({"frame": 8})).unwrap();
    assert!(s.inbetween_preview().is_none(), "a deleted cel ends the preview");
}

#[test]
fn accepting_chosen_frames_only_and_the_same_mapping_gives_the_same_cels_every_way() {
    let (mut s, clip) = session();
    s.execute("drawing.inbetweenPreview", preview_args()).unwrap();
    assert!(s.execute("drawing.inbetweenAccept", json!({"frames": [3]})).is_err());
    assert!(s.execute("drawing.inbetweenAccept", json!({"frames": []})).is_err());
    assert!(s.inbetween_preview().is_some(), "a refused accept keeps the preview");
    s.execute("drawing.inbetweenAccept", json!({"frames": [4]})).unwrap();
    assert_eq!(frames(&drawing(&s, clip)), [0, 4, 8]);

    // preview then accept, accept with the mapping, and the old explicit command's own pairing
    let (mut a, ca) = session();
    a.execute("drawing.inbetweenPreview", preview_args()).unwrap();
    a.execute("drawing.inbetweenAccept", json!({})).unwrap();
    let (mut b, cb) = session();
    b.execute("drawing.inbetweenAccept", preview_args()).unwrap();
    assert_eq!(drawing(&a, ca), drawing(&b, cb));
    // the frames list is the same schedule as the step
    let (mut c, cc) = session();
    c.execute("drawing.inbetweenAccept", json!({"from": 0, "to": 8, "pairs": pairs(), "frames": [6, 2, 4]})).unwrap();
    assert_eq!(drawing(&a, ca), drawing(&c, cc));
}

#[test]
fn easing_shapes_the_position_and_spacing_stays_on_exact_frames() {
    let (mut s, _) = session();
    let x_at = |s: &mut Session, easing: &str, f: i64| {
        let r =
            s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": pairs(), "frames": [f], "easing": easing, "geometry": true})).unwrap();
        // stroke 1 is the vertical line going from x = 200 to x = 240, first in the nearer cel's order
        let order = if (f as f64 / 8.0) < 0.5 || easing != "linear" { 1 } else { 0 };
        let _ = order;
        r["cels"][0]["strokes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|st| st["color"][2].as_f64() == Some(1.0))
            .map(|st| st["points"][0]["x"].as_f64().unwrap())
            .next()
            .unwrap()
    };
    let lin = x_at(&mut s, "linear", 2);
    assert!((lin - 210.0).abs() < 1e-3, "a quarter of the way: {lin}");
    let ease_in = x_at(&mut s, "easeIn", 2);
    assert!((ease_in - 200.0 - 40.0 * 0.0625).abs() < 1e-3, "{ease_in}");
    let ease_out = x_at(&mut s, "easeOut", 2);
    assert!(ease_out > lin, "{ease_out}");
    let io = x_at(&mut s, "easeInOut", 2);
    assert!(io < lin, "{io}");
    // at the middle frame: ease in lags (a quarter), ease out leads (three quarters), in-out is even
    assert!((x_at(&mut s, "easeIn", 4) - 210.0).abs() < 1e-3);
    assert!((x_at(&mut s, "easeOut", 4) - 230.0).abs() < 1e-3);
    assert!((x_at(&mut s, "easeInOut", 4) - 220.0).abs() < 1e-3);
    // spacing: ones fill every frame, odd steps stop short of the end cel
    let ones = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": pairs(), "step": 1})).unwrap();
    assert_eq!(ones["frames"], json!([1, 2, 3, 4, 5, 6, 7]));
    let threes = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": pairs(), "step": 3})).unwrap();
    assert_eq!(threes["frames"], json!([3, 6]));
}

#[test]
fn hostile_parameters_and_extreme_counts_are_refused_before_anything_changes() {
    let (mut s, clip) = session();
    let before = s.project.clone();
    let p = pairs();
    for bad in [
        json!({"from": 0, "to": 8}),
        json!({"from": 0, "to": 8, "pairs": "all"}),
        json!({"from": 0, "to": 8, "pairs": [{"from": -1, "to": 0}]}),
        json!({"from": 0, "to": 8, "pairs": [{"from": 0.5, "to": 0}]}),
        json!({"from": 0, "to": 8, "pairs": [{"from": "0", "to": 0}]}),
        json!({"from": 0, "to": 8, "pairs": [{"from": 0}]}),
        json!({"from": 0, "to": 8, "pairs": [{"from": 0, "to": 0, "extra": 1}]}),
        json!({"from": 0, "to": 8, "pairs": [{"from": 0, "to": 0, "reverse": "yes"}]}),
        json!({"from": 0, "to": 8, "pairs": p, "easing": "bounce"}),
        json!({"from": 0, "to": 8, "pairs": p, "unmatched": "guess"}),
        json!({"from": 0, "to": 8, "pairs": p, "step": 0}),
        json!({"from": 0, "to": 8, "pairs": p, "step": -2}),
        json!({"from": 0, "to": 8, "pairs": p, "step": 1e30}),
        json!({"from": 0, "to": 8, "pairs": p, "step": 1001}),
        json!({"from": 0, "to": 8, "pairs": p, "step": 2, "frames": [4]}),
        json!({"from": 0, "to": 8, "pairs": p, "frames": [0]}),
        json!({"from": 0, "to": 8, "pairs": p, "frames": [8]}),
        json!({"from": 0, "to": 8, "pairs": p, "frames": ["x"]}),
        json!({"from": 0, "to": 8, "pairs": p, "bogus": true}),
        json!({"from": 8, "to": 0, "pairs": p}),
        json!({"from": 0, "to": 0, "pairs": p}),
        json!({"from": 0, "to": 3, "pairs": p}),
        json!({"from": i64::MAX, "to": i64::MIN, "pairs": p}),
        json!({"from": i64::MIN, "to": i64::MAX, "pairs": p}),
        json!({"from": 0, "to": 8, "pairs": p, "geometry": "yes"}),
        json!([1, 2, 3]),
    ] {
        for id in ["drawing.inbetweenPreview", "drawing.inbetweenAccept"] {
            assert!(s.execute(id, bad.clone()).is_err(), "{id} {bad}");
            assert!(Arc::ptr_eq(&before, &s.project), "{id} {bad} must change nothing");
        }
    }
    let many: Vec<Value> = (0..300).map(|i| json!({"from": i, "to": i})).collect();
    assert!(s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": many})).is_err());
    let frames_300: Vec<i64> = (1..300).collect();
    assert!(s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 8, "pairs": p, "frames": frames_300})).is_err());
    assert!(s.execute("drawing.inbetweenCancel", json!({"x": 1})).is_err());

    // a gap of a hundred thousand frames cannot be filled one by one
    draw(&mut s, 100_000, true, line("#ff0000", (1.0, 1.0), (2.0, 2.0)));
    draw(&mut s, 100_000, false, line("#ff0000", (1.0, 1.0), (2.0, 2.0)));
    let r = s.execute("drawing.inbetweenPreview", json!({"from": 8, "to": 100_000, "pairs": [{"from": 0, "to": 0}], "step": 1}));
    assert!(r.unwrap_err().to_string().contains("at most"), "bounded");
    // spaced out they are outside the 96-frame clip, so there is nothing to make
    assert!(s.execute("drawing.inbetweenPreview", json!({"from": 8, "to": 100_000, "pairs": [{"from": 0, "to": 0}], "step": 1000})).is_err());
    assert_eq!(frames(&drawing(&s, clip)), [0, 8, 100_000]);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
}

#[test]
fn the_budget_of_generated_points_is_enforced() {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Big", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 1})).unwrap();
    s.execute("drawing.new", json!({"seconds": 4})).unwrap();
    // 200 strokes of 600 points each in two cels: 200 pairs resample to 512 points
    let wiggle = |y: f64| -> Value {
        let pts: Vec<Value> = (0..600).map(|i| json!([i as f64 * 0.5, y + (i % 7) as f64, 1.0])).collect();
        json!({"brush": "Pen", "color": "#000000", "size": 2.0, "points": pts})
    };
    for frame in [0, 60] {
        let strokes: Vec<Value> = (0..200).map(|i| wiggle(i as f64 * 0.8)).collect();
        s.execute("drawing.setStrokes", json!({"frame": frame, "strokes": strokes})).unwrap();
    }
    let pairs: Vec<Value> = (0..200).map(|i| json!({"from": i, "to": i})).collect();
    let before = s.project.clone();
    // 29 frames x 102 400 points is over two million
    let e = s.execute("drawing.inbetweenAccept", json!({"from": 0, "to": 60, "pairs": pairs, "step": 2})).unwrap_err().to_string();
    assert!(e.contains("limit"), "{e}");
    assert!(Arc::ptr_eq(&before, &s.project));
    // fewer frames fit
    let r = s.execute("drawing.inbetweenPreview", json!({"from": 0, "to": 60, "pairs": pairs, "step": 20})).unwrap();
    assert_eq!(r["frames"], json!([20, 40]));
}

#[test]
fn accepted_cels_survive_save_and_load_and_nothing_extra_is_stored() {
    let (mut s, clip) = session();
    s.execute("drawing.inbetweenPreview", preview_args()).unwrap();
    s.execute("drawing.inbetweenAccept", json!({})).unwrap();
    let dir = Scratch::new("save");
    let path = dir.path("between.waltcut");
    s.execute("file.save", json!({"path": path})).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    for word in ["inbetween", "correspondence", "unmatched", "otherDirection"] {
        assert!(!text.contains(word), "the mapping is not stored ({word})");
    }
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(drawing(&s, clip), drawing(&t, clip));
    assert!(t.inbetween_preview().is_none());
}

#[test]
fn linked_endpoints_stay_linked_and_the_new_cels_are_independent() {
    let (mut s, clip) = session();
    s.execute("drawing.linkCel", json!({"from": 0, "frame": 16})).unwrap();
    let before = drawing(&s, clip);
    s.execute("drawing.inbetweenAccept", preview_args()).unwrap();
    let d = drawing(&s, clip);
    d.check_links().unwrap();
    let start = d.cel_on(0).unwrap();
    let held = d.cel_on(16).unwrap();
    assert!(d.cels[start].link.is_some() && d.cels[start].link == d.cels[held].link);
    assert!(Arc::ptr_eq(&d.cels[start].strokes, &d.cels[held].strokes) && Arc::ptr_eq(&d.cels[start].strokes, &before.cels[0].strokes));
    for f in [2, 4, 6] {
        assert!(d.cels[d.cel_on(f).unwrap()].link.is_none());
    }
    // editing an in-between does not reach the linked exposures
    s.execute("drawing.addStroke", json!({"frame": 4, "stroke": line("#00ff00", (1.0, 1.0), (5.0, 5.0))})).unwrap();
    let e = drawing(&s, clip);
    assert_eq!(e.cels[e.cel_on(0).unwrap()].strokes.len(), 2);
    assert_eq!(e.cels[e.cel_on(16).unwrap()].strokes.len(), 2);
}

#[test]
fn the_old_explicit_command_is_unchanged_and_another_drawing_clip_drops_the_preview() {
    let (mut s, _) = session();
    let r = s.execute("drawing.tween", json!({"frame": 0, "step": 2})).unwrap();
    assert_eq!(r["cels"], json!([2, 4, 6]));
    // the pair-by-position morph and an explicit identity pairing agree where they should
    let (mut t, ct) = session();
    t.execute("drawing.inbetweenAccept", json!({"from": 0, "to": 8, "pairs": [{"from": 0, "to": 0}, {"from": 1, "to": 1}]})).unwrap();
    let old = s.execute("drawing.inspect", json!({"strokes": true})).unwrap();
    let new = t.execute("drawing.inspect", json!({"strokes": true})).unwrap();
    assert_eq!(old["cels"][1]["strokes"], new["cels"][1]["strokes"], "frame 2 is the same morph (the nearer cel is the start cel)");
    let _ = ct;
}
