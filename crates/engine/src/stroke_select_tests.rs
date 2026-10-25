//! Lasso selection and whole-stroke transforms (#66): what is selected, that an accepted transform
//! is one undo step whose rendered result agrees with drawing the strokes there, and that every
//! refusal changes nothing.
use serde_json::{Value, json};
use waltcut_project::ClipId;

use crate::Session;

fn line(brush: &str, size: f64, pts: &[(f64, f64)]) -> Value {
    json!({"brush": brush, "color": "#000000", "size": size, "points": pts.iter().map(|p| json!([p.0, p.1, 1.0])).collect::<Vec<_>>()})
}

/// A 128x72 sequence with a drawing: stroke 0 a pen line at y=20, stroke 1 one at y=60, and
/// stroke 2 an eraser across stroke 0 at x=60 (a gap). The cel starts on frame 0.
fn session() -> (Session, ClipId) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Sel", "width": 128, "height": 72, "fps": 24, "video": 2, "audio": 0})).unwrap();
    let clip = s.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
    for stroke in
        [line("Pen", 4.0, &[(10.0, 20.0), (110.0, 20.0)]), line("Pen", 4.0, &[(10.0, 60.0), (110.0, 60.0)]), line("Eraser", 6.0, &[(60.0, 10.0), (60.0, 30.0)])]
    {
        s.execute("drawing.addStroke", json!({"clip": clip, "frame": 0, "stroke": stroke})).unwrap();
    }
    s.execute("timeline.select", json!({"clips": [clip]})).unwrap();
    (s, ClipId(clip))
}

fn strokes(s: &Session) -> Vec<waltcut_project::drawing::Stroke> {
    s.project.items.values().find_map(|i| i.as_drawing()).unwrap().cels[0].strokes.to_vec()
}

fn state(s: &Session) -> (String, usize) {
    (s.project.to_json(), s.history.undo.len())
}

fn frame(s: &Session) -> Vec<f32> {
    s.render_program(1.0).unwrap().px
}

fn alpha(s: &Session, x: usize, y: usize) -> f32 {
    s.render_program(1.0).unwrap().get(x, y)[3]
}

#[test]
fn a_lasso_names_the_strokes_it_hits_and_the_erasers_that_belong_to_them() {
    let (mut s, _) = session();
    let around_first = json!([[0, 5], [120, 5], [120, 35], [0, 35]]);
    let r = s.execute("drawing.lassoSelect", json!({"lasso": around_first})).unwrap();
    assert_eq!((r["strokes"].clone(), r["erasers"].clone(), r["conflict"].clone()), (json!([0]), json!([2]), Value::Null));
    assert!(r["centre"][0].as_f64().unwrap() > 50.0 && r["centre"][1].as_f64().unwrap() < 30.0);
    // everything: both lines, and the eraser still belongs to the first
    let all = s.execute("drawing.lassoSelect", json!({"lasso": [[0, 0], [128, 0], [128, 72], [0, 72]]})).unwrap();
    assert_eq!(all["strokes"], json!([0, 1]));
    // nothing hit, and a read-only command changes nothing
    let before = state(&s);
    let none = s.execute("drawing.lassoSelect", json!({"lasso": [[0, 40], [128, 40], [128, 45]]})).unwrap();
    assert_eq!((none["strokes"].clone(), none["centre"].clone()), (json!([]), Value::Null));
    assert_eq!(state(&s), before);
    for bad in [
        json!({}),
        json!({"lasso": [[0, 0], [1, 1]]}),
        json!({"lasso": "all"}),
        json!({"lasso": [[0, 0], [1, 1], ["a", 2]]}),
        json!({"lasso": [[0, 0], [1, 1], [2, 5]], "colour": 1}),
    ] {
        assert!(s.execute("drawing.lassoSelect", bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn an_accepted_transform_is_one_undo_step_and_renders_like_drawing_the_strokes_there() {
    let (mut s, clip) = session();
    let (before, depth) = state(&s);
    let r = s.execute("drawing.transformStrokes", json!({"strokes": [0], "dx": 0, "dy": 20})).unwrap();
    assert_eq!((r["strokes"].clone(), r["erasers"].clone(), r["applied"].clone()), (json!([0]), json!([2]), json!(true)));
    assert_eq!(s.history.undo.len(), depth + 1, "one undo step");
    let moved = strokes(&s);
    assert!((moved[0].points[0].y - 40.0).abs() < 1e-4 && (moved[2].points[0].y - 30.0).abs() < 1e-4, "paint and its eraser moved; stroke 1 stayed");
    assert!((moved[1].points[0].y - 60.0).abs() < 1e-4);
    // the erased gap travelled with the paint: a hole at x = 60 on the new line, none on the old spot
    assert!(alpha(&s, 30, 40) > 0.9 && alpha(&s, 60, 40) < 0.05, "gap at the new place");
    assert!(alpha(&s, 30, 20) < 0.05 && alpha(&s, 60, 20) < 0.05, "nothing left behind");
    // what was drawn straight there renders the same pixels
    let (mut t, _) = {
        let mut t = Session::default();
        t.execute("file.newSequence", json!({"name": "Sel", "width": 128, "height": 72, "fps": 24, "video": 2, "audio": 0})).unwrap();
        let c = t.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
        (t, c)
    };
    let c = t.state.active_sequence.map(|_| t.active_sequence().unwrap().video_tracks.iter().flat_map(|x| x.items.iter()).next().unwrap().id.0).unwrap();
    for stroke in
        [line("Pen", 4.0, &[(10.0, 40.0), (110.0, 40.0)]), line("Pen", 4.0, &[(10.0, 60.0), (110.0, 60.0)]), line("Eraser", 6.0, &[(60.0, 30.0), (60.0, 50.0)])]
    {
        t.execute("drawing.addStroke", json!({"clip": c, "frame": 0, "stroke": stroke})).unwrap();
    }
    let _ = (&mut t, clip);
    let (a, b) = (s.render_program(1.0).unwrap(), t.render_program(1.0).unwrap());
    for y in 34..46 {
        for x in 0..128 {
            assert!((a.get(x, y)[3] - b.get(x, y)[3]).abs() < 1e-4, "({x},{y})");
        }
    }
    // undo restores the project exactly, redo brings it back
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(state(&s), (before, depth));
    s.execute("edit.redo", json!({})).unwrap();
    assert!((strokes(&s)[0].points[0].y - 40.0).abs() < 1e-4);
}

#[test]
fn rotate_and_scale_about_the_selection_centre_and_a_lasso_can_stand_in_for_indices() {
    let (mut s, _) = session();
    let by_lasso = json!({"lasso": [[0, 5], [120, 5], [120, 35], [0, 35]], "rotation": 90, "scale": 0.5});
    let dry = s.execute("drawing.transformStrokes", json!({"lasso": [[0, 5], [120, 5], [120, 35], [0, 35]], "rotation": 90, "dryRun": true})).unwrap();
    assert_eq!(dry["applied"], json!(false));
    let before = strokes(&s);
    s.execute("drawing.transformStrokes", by_lasso).unwrap();
    let now = strokes(&s);
    let first = &now[0];
    // a horizontal line turned a quarter and halved is a vertical line half as long, brush halved too
    assert!((first.points[0].x - first.points[1].x).abs() < 1e-3, "vertical: {:?}", first.points);
    assert!((first.points[0].y - first.points[1].y).abs() - 50.0 < 1e-3);
    assert_eq!(first.size, 2.0);
    assert_eq!(now[1], before[1], "the other stroke is untouched");
    // the identity does nothing and adds no undo step
    let depth = s.history.undo.len();
    let r = s.execute("drawing.transformStrokes", json!({"strokes": [1]})).unwrap();
    assert_eq!((r["applied"].clone(), s.history.undo.len()), (json!(false), depth));
}

#[test]
fn only_the_cel_shown_changes_and_clip_motion_stays() {
    let (mut s, clip) = session();
    s.execute("drawing.newCel", json!({"clip": clip.0, "frame": 12, "copy": true})).unwrap();
    let motion_before = s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.clone();
    s.execute("drawing.transformStrokes", json!({"clip": clip.0, "frame": 12, "strokes": [1], "dx": 20})).unwrap();
    let d = s.project.items.values().find_map(|i| i.as_drawing()).unwrap().clone();
    assert!((d.cels[0].strokes[1].points[0].x - 10.0).abs() < 1e-4, "the first cel is untouched");
    assert!((d.cels[1].strokes[1].points[0].x - 30.0).abs() < 1e-4);
    assert_eq!(s.active_sequence().unwrap().find_item(clip).unwrap().1.effects, motion_before, "clip Motion is not touched");
}

#[test]
fn every_refusal_changes_nothing() {
    let (mut s, clip) = session();
    let before = state(&s);
    let refused = |s: &mut Session, p: Value| {
        let e = s.execute("drawing.transformStrokes", p.clone());
        assert!(e.is_err(), "{p}");
        assert_eq!(state(s), before, "{p} changed something");
    };
    for p in [
        json!({"strokes": [9], "dx": 1}),
        json!({"strokes": [2], "dx": 1}),
        json!({"strokes": [0, 0], "dx": 1}),
        json!({"strokes": [], "dx": 1}),
        json!({"strokes": "all"}),
        json!({"strokes": [-1]}),
        json!({"dx": 1}),
        json!({"strokes": [0], "lasso": [[0, 0], [1, 1], [2, 5]]}),
        json!({"strokes": [0], "scale": 0}),
        json!({"strokes": [0], "scale": 1000}),
        json!({"strokes": [0], "scale": "big"}),
        json!({"strokes": [0], "rotation": 1e12}),
        json!({"strokes": [0], "dx": 1e12}),
        json!({"strokes": [0], "dx": 999999.0, "scale": 90}),
        json!({"strokes": [0], "pivot": [1]}),
        json!({"strokes": [0], "dryRun": 1}),
        json!({"strokes": [0], "colour": 1}),
        // moving the first line and its eraser onto the second line would erase it
        json!({"strokes": [0], "dy": 40}),
        // a lasso that hits nothing selects nothing
        json!({"lasso": [[0, 40], [128, 40], [128, 45]], "dx": 1}),
    ] {
        refused(&mut s, p);
    }
    // an eraser crossing selected and unselected paint is ambiguous
    s.execute("drawing.addStroke", json!({"clip": clip.0, "frame": 0, "stroke": line("Eraser", 6.0, &[(30.0, 10.0), (30.0, 65.0)])})).unwrap();
    let after_eraser = state(&s);
    let e = s.execute("drawing.transformStrokes", json!({"strokes": [0], "dx": 5})).unwrap_err().to_string();
    assert!(e.contains("crosses"), "{e}");
    assert_eq!(state(&s), after_eraser);
    // selecting both resolves it
    s.execute("drawing.transformStrokes", json!({"strokes": [0, 1], "dx": 5})).unwrap();
}

#[test]
fn a_locked_track_and_a_missing_cel_are_refused() {
    let (mut s, clip) = session();
    let seq = s.state.active_sequence.unwrap();
    let ti = s.active_sequence().unwrap().video_tracks.iter().position(|t| t.items.iter().any(|i| i.id == clip)).unwrap();
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[ti].locked = true;
    let before = state(&s);
    let e = s.execute("drawing.transformStrokes", json!({"strokes": [0], "dx": 1})).unwrap_err().to_string();
    assert!(e.contains("locked"), "{e}");
    assert_eq!(state(&s), before);
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[ti].locked = false;
    // a cel starts on frame 0 only: a drawing with no cel shown before frame 5 has nothing to select
    let (mut empty, _) = {
        let mut t = Session::default();
        t.execute("file.newSequence", json!({"name": "E", "width": 64, "height": 36, "fps": 24, "video": 1, "audio": 0})).unwrap();
        let c = t.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
        t.execute("timeline.select", json!({"clips": [c]})).unwrap();
        (t, c)
    };
    assert!(empty.execute("drawing.transformStrokes", json!({"strokes": [0], "dx": 1})).is_err());
    assert!(empty.execute("drawing.lassoSelect", json!({"lasso": [[0, 0], [9, 0], [9, 9]]})).is_err());
}

#[test]
fn a_transformed_cel_survives_save_and_load() {
    let (mut s, _) = session();
    s.execute("drawing.transformStrokes", json!({"strokes": [1], "dx": 7, "rotation": 15, "scale": 1.5})).unwrap();
    let want = strokes(&s);
    let dir = waltcut_testkit::workspace_root().join("target").join("export-tests").join(format!("select-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.waltcut").to_string_lossy().into_owned();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(strokes(&t), want);
    assert_eq!(frame(&s), frame(&t), "rendered output agrees after loading");
    let _ = std::fs::remove_dir_all(dir);
}
