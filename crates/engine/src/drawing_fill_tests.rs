//! Region fill (#68): the commands, undo, save and load, the locked track, a refusal changing
//! nothing, and the preview and the exported still agreeing.
use std::path::PathBuf;

use serde_json::{Value, json};
use waltcut_project::ClipId;
use waltcut_project::drawing::Drawing;

use crate::Session;

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let directory = waltcut_testkit::workspace_root().join("target").join("export-tests").join(format!("fill-{name}-{}", std::process::id()));
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

fn session() -> (Session, ClipId) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Fill", "fps": 24, "width": 160, "height": 90, "video": 1, "audio": 1})).unwrap();
    let r = s.execute("drawing.new", json!({"seconds": 2})).unwrap();
    (s, ClipId(r["clip"].as_u64().unwrap()))
}

fn line(color: &str, size: f64, points: &[[f64; 2]]) -> Value {
    json!({"brush": "Pen", "color": color, "size": size, "points": points})
}

/// A closed black outline from (x0, y0) to (x1, y1).
fn outline(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": line("#000000", 4.0, &[[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]])})).unwrap();
}

fn drawing(s: &Session, clip: ClipId) -> Drawing {
    let seq = s.active_sequence().unwrap();
    crate::drawing::drawing_of(s, seq, clip).unwrap().1.clone()
}

fn stroke_count(s: &Session, clip: ClipId) -> usize {
    drawing(s, clip).cels.first().map_or(0, |c| c.strokes.len())
}

/// The preview's pixels: what the Program monitor draws for the playhead frame, 8-bit.
fn preview(s: &Session) -> (usize, usize, Vec<u8>) {
    let img = s.render_program(1.0).unwrap();
    (img.w, img.h, img.to_rgba8())
}

fn at(p: &(usize, usize, Vec<u8>), x: usize, y: usize) -> [u8; 4] {
    let i = (y * p.0 + x) * 4;
    [p.2[i], p.2[i + 1], p.2[i + 2], p.2[i + 3]]
}

#[test]
fn a_fill_colours_a_closed_shape_and_is_one_undo_step() {
    let (mut s, clip) = session();
    outline(&mut s, 40.0, 20.0, 120.0, 70.0);
    let undo = s.history.undo.len();
    let r = s.execute("drawing.fill", json!({"point": [80.5, 45.2], "color": "#ff0000"})).unwrap();
    assert_eq!((r["index"].clone(), r["changed"].clone(), r["seed"].clone(), r["tolerance"].clone()), (json!(1), json!(true), json!([80, 45]), json!(32)));
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    let shown = preview(&s);
    assert_eq!(at(&shown, 80, 45), [255, 0, 0, 255], "inside is the fill colour");
    assert_eq!(at(&shown, 5, 5)[3], 0, "outside stays transparent");
    assert_eq!(at(&shown, 40, 45), [0, 0, 0, 255], "the line is still black");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(stroke_count(&s, clip), 1, "undo removes the fill and keeps the outline");
    assert_eq!(at(&preview(&s), 80, 45)[3], 0);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(at(&preview(&s), 80, 45), [255, 0, 0, 255]);
    let r = s.execute("drawing.inspect", json!({"strokes": true})).unwrap();
    let fill = &r["cels"][0]["strokes"][1];
    assert_eq!(fill["brush"], "Fill");
    assert!(fill["fill"]["runs"].as_u64().unwrap() > 1 && fill["fill"]["pixels"].as_u64().unwrap() > 3000, "{fill}");
}

#[test]
fn colour_and_alpha_are_exact_and_the_same_every_time() {
    let (mut s, _) = session();
    outline(&mut s, 40.0, 20.0, 120.0, 70.0);
    s.execute("drawing.fill", json!({"seed": [80, 45], "color": [0.0, 0.0, 1.0, 0.5]})).unwrap();
    let a = preview(&s);
    let b = preview(&s);
    assert_eq!(a.2, b.2);
    let px = at(&a, 80, 45);
    assert_eq!((px[0], px[1], px[3]), (0, 0, 128), "straight sRGB blue at half alpha: {px:?}");
    assert!(px[2] > 250);
    // the same drawing built again gives the same pixels
    let (mut t, _) = session();
    outline(&mut t, 40.0, 20.0, 120.0, 70.0);
    t.execute("drawing.fill", json!({"seed": [80, 45], "color": [0.0, 0.0, 1.0, 0.5]})).unwrap();
    assert_eq!(preview(&t).2, a.2);
}

#[test]
fn paint_fill_and_erase_apply_in_the_order_they_were_made() {
    let (mut s, _) = session();
    outline(&mut s, 40.0, 20.0, 120.0, 70.0);
    s.execute("drawing.fill", json!({"seed": [80, 45], "color": "#ff0000"})).unwrap();
    // paint after the fill covers it; an eraser after it removes it
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": line("#00ff00", 6.0, &[[50.0, 45.0], [110.0, 45.0]])})).unwrap();
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": {"brush": "Eraser", "size": 10, "points": [[80.0, 25.0], [80.0, 65.0]]}})).unwrap();
    let p = preview(&s);
    assert_eq!(at(&p, 60, 45), [0, 255, 0, 255], "the later stroke is over the fill");
    assert_eq!(at(&p, 60, 30), [255, 0, 0, 255], "the fill elsewhere");
    assert_eq!(at(&p, 80, 30)[3], 0, "the later eraser took the fill away");
    assert_eq!(at(&p, 80, 45)[3], 0, "and the stroke");
}

#[test]
fn a_cel_that_was_filled_survives_save_and_load_with_the_same_pixels() {
    let (mut s, clip) = session();
    outline(&mut s, 40.0, 20.0, 120.0, 70.0);
    s.execute("drawing.fill", json!({"seed": [80, 45], "color": "#33aa66"})).unwrap();
    s.execute("drawing.fill", json!({"seed": [5, 5], "color": [1.0, 1.0, 0.0, 0.25], "cel": "new", "frame": 6})).unwrap();
    let before = (preview(&s), drawing(&s, clip));
    let dir = Scratch::new("save");
    let path = dir.path("fill.waltcut");
    s.execute("file.save", json!({"path": path})).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("\"Fill\""));
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(drawing(&t, clip), before.1, "the stored result comes back exactly");
    assert_eq!(preview(&t).2, before.0.2, "and draws the same pixels");
    t.execute("playhead.set", json!({"frame": 6})).unwrap();
    s.execute("playhead.set", json!({"frame": 6})).unwrap();
    assert_eq!(preview(&t).2, preview(&s).2);
}

#[test]
fn the_preview_and_the_exported_still_have_the_same_pixels() {
    let (mut s, _) = session();
    outline(&mut s, 40.0, 20.0, 120.0, 70.0);
    s.execute("drawing.fill", json!({"seed": [80, 45], "color": [1.0, 0.2, 0.0, 0.8]})).unwrap();
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": {"brush": "Eraser", "size": 8, "points": [[100.0, 30.0], [100.0, 60.0]]}})).unwrap();
    let dir = Scratch::new("export");
    let png = dir.path("still.png");
    s.execute("file.exportFrame", json!({"path": png})).unwrap();
    let exported = image::load_from_memory(&std::fs::read(&png).unwrap()).unwrap().to_rgba8();
    let shown = preview(&s);
    assert_eq!((exported.width() as usize, exported.height() as usize), (shown.0, shown.1));
    let (mut max, mut painted) = (0u8, 0usize);
    for (i, px) in exported.pixels().enumerate() {
        let want = &shown.2[i * 4..i * 4 + 4];
        for c in 0..4 {
            max = max.max(px.0[c].abs_diff(want[c]));
        }
        painted += usize::from(want[3] > 0);
    }
    assert!(painted > 3000, "the still has the fill in it ({painted} pixels)");
    assert_eq!(max, 0, "export and preview differ by up to {max}/255");
}

#[test]
fn a_locked_track_and_every_bad_request_change_nothing() {
    let (mut s, clip) = session();
    outline(&mut s, 40.0, 20.0, 120.0, 70.0);
    s.execute("drawing.fill", json!({"seed": [80, 45], "color": "#ff0000"})).unwrap();
    let snapshot = |s: &Session| (s.revision, s.history.undo.len(), s.history.redo.len(), drawing(s, clip));
    let before = snapshot(&s);
    for params in [
        json!({"point": [-1.0, 5.0]}),
        json!({"point": [160.0, 5.0]}),
        json!({"point": [5.0, 90.0]}),
        json!({"seed": [5.5, 5.0]}),
        json!({"seed": [5, 5], "point": [5.0, 5.0]}),
        json!({}),
        json!({"point": [f64::NAN, 5.0]}),
        json!({"point": [1e300, 5.0]}),
        json!({"point": [5.0, 5.0], "tolerance": 256}),
        json!({"point": [5.0, 5.0], "tolerance": -1}),
        json!({"point": [5.0, 5.0], "tolerance": "high"}),
        json!({"point": [5.0, 5.0], "color": [1.0, 0.0, 0.0, 0.0]}),
        json!({"point": [5.0, 5.0], "color": "red"}),
        json!({"point": [5.0, 5.0], "cel": "bogus"}),
        json!({"point": [5.0, 5.0], "unknown": 1}),
    ] {
        assert!(s.execute("drawing.fill", params.clone()).is_err(), "{params}");
        assert_eq!(snapshot(&s), before, "{params} must change nothing");
    }
    let track = s.active_sequence().and_then(|seq| seq.find_item(clip)).map(|(id, _)| id).unwrap();
    s.execute("timeline.setTrack", json!({"track": track.0, "locked": true})).unwrap();
    let locked = snapshot(&s);
    for (id, params) in [
        ("drawing.fill", json!({"point": [5.0, 5.0]})),
        ("drawing.fillEdit", json!({"index": 1, "color": "#00ff00"})),
        ("drawing.fillRemove", json!({"index": 1})),
    ] {
        let e = s.execute(id, params).unwrap_err();
        assert!(e.to_string().contains("locked"), "{id}: {e}");
        assert_eq!(snapshot(&s), locked);
    }
    assert!(!s.is_enabled("drawing.fill"), "disabled while the track is locked");
}

#[test]
fn a_cancelled_or_oversized_fill_leaves_the_cel_and_history_alone() {
    let (mut s, clip) = session();
    outline(&mut s, 40.0, 20.0, 120.0, 70.0);
    let before = (s.revision, s.history.undo.len(), drawing(&s, clip));
    let e = crate::drawing_fill::fill_with(&mut s, &json!({"seed": [80, 45]}), &|| true).unwrap_err();
    assert!(e.to_string().contains("cancelled"), "{e}");
    assert_eq!((s.revision, s.history.undo.len(), drawing(&s, clip)), before);
    // a drawing whose canvas is far too large to fill
    s.edit("huge", |p, _| {
        let id = p.items.iter().find(|(_, i)| i.as_drawing().is_some()).map(|(id, _)| *id).unwrap();
        let d = p.item_mut(id).and_then(|i| i.as_drawing_mut()).unwrap();
        d.width = 16_384;
        d.height = 16_384;
        Ok(())
    })
    .unwrap();
    let before = (s.revision, s.history.undo.len());
    let e = s.execute("drawing.fill", json!({"seed": [80, 45]})).unwrap_err();
    assert!(e.to_string().contains("too large"), "{e}");
    assert_eq!((s.revision, s.history.undo.len()), before);
}

#[test]
fn a_fill_can_be_recoloured_redone_with_another_tolerance_and_removed() {
    let (mut s, clip) = session();
    // a translucent line splits the canvas
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": {"brush": "Marker", "color": "#000000", "size": 6, "points": [[80.0, 0.0], [80.0, 89.0]]}})).unwrap();
    let r = s.execute("drawing.fill", json!({"seed": [10, 45], "tolerance": 0, "color": "#ff0000"})).unwrap();
    let half = r["pixels"].as_u64().unwrap();
    assert!(half < 160 * 90 / 2 + 400, "tolerance 0 stops at the marker: {half}");
    // recolour: same region, one undo step
    let undo = s.history.undo.len();
    let r = s.execute("drawing.fillEdit", json!({"index": 1, "color": "#0000ff"})).unwrap();
    assert_eq!((r["changed"].clone(), r["pixels"].as_u64()), (json!(true), Some(half)));
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(at(&preview(&s), 10, 45), [0, 0, 255, 255]);
    // asking for what it already is makes no history
    let r = s.execute("drawing.fillEdit", json!({"index": 1, "color": "#0000ff", "tolerance": 0})).unwrap();
    assert_eq!((r["changed"].clone(), s.history.undo.len()), (json!(false), undo + 1));
    // redo the region with a tolerance that crosses the marker
    let r = s.execute("drawing.fillEdit", json!({"index": 1, "tolerance": 200})).unwrap();
    assert_eq!(r["pixels"], json!(160 * 90), "{r}");
    assert_eq!(at(&preview(&s), 150, 45), [0, 0, 255, 255]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("drawing.inspect", json!({"strokes": true})).unwrap()["cels"][0]["strokes"][1]["fill"]["pixels"], json!(half));
    // nothing to change, the wrong stroke, a bad index, unknown keys
    for params in [json!({"index": 1}), json!({"index": 0, "color": "#00ff00"}), json!({"index": 9, "color": "#00ff00"}), json!({"index": 1, "colour": "#00ff00"})] {
        let before = (s.revision, s.history.undo.len());
        assert!(s.execute("drawing.fillEdit", params.clone()).is_err(), "{params}");
        assert_eq!((s.revision, s.history.undo.len()), before);
    }
    // remove: only a fill, one undo step
    assert!(s.execute("drawing.fillRemove", json!({"index": 0})).is_err());
    s.execute("drawing.fillRemove", json!({"index": 1})).unwrap();
    assert_eq!(stroke_count(&s, clip), 1);
    assert_eq!(at(&preview(&s), 10, 45)[3], 0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(stroke_count(&s, clip), 2);
}

#[test]
fn a_new_cel_starts_from_nothing_and_a_copy_from_the_cel_held() {
    let (mut s, clip) = session();
    outline(&mut s, 40.0, 20.0, 120.0, 70.0);
    // on a new cel the outline is not there, so the whole canvas is one region
    let r = s.execute("drawing.fill", json!({"seed": [80, 45], "frame": 4, "cel": "new", "color": "#ff0000"})).unwrap();
    assert_eq!((r["cel"].clone(), r["pixels"].clone()), (json!(4), json!(160 * 90)));
    // on a copy it is, so only the inside is
    let r = s.execute("drawing.fill", json!({"seed": [80, 45], "frame": 2, "cel": "copy", "color": "#ff0000"})).unwrap();
    assert_eq!(r["cel"], json!(2));
    assert!(r["pixels"].as_u64().unwrap() < 160 * 90 / 2);
    let d = drawing(&s, clip);
    assert_eq!(d.cels.iter().map(|c| (c.frame, c.strokes.len())).collect::<Vec<_>>(), vec![(0, 1), (2, 2), (4, 1)]);
}

/// Regression: `cel: "new"` on a frame that already starts a cel adds to that cel, so the region
/// must be found in its paint (it was found against an empty cel).
#[test]
fn a_new_cel_request_on_a_frame_that_already_has_one_fills_against_that_cels_paint() {
    let (mut s, clip) = session();
    outline(&mut s, 40.0, 20.0, 120.0, 70.0);
    let r = s.execute("drawing.fill", json!({"seed": [80, 45], "frame": 0, "cel": "new", "color": "#ff0000"})).unwrap();
    assert!(r["pixels"].as_u64().unwrap() < 160 * 90 / 2, "only the inside of the outline: {r}");
    let d = drawing(&s, clip);
    assert_eq!(d.cels.len(), 1);
    assert_eq!(d.cels[0].strokes.len(), 2);
    assert_eq!(at(&preview(&s), 5, 5)[3], 0);
}

#[test]
fn filling_is_enabled_for_a_drawing_and_listed_with_its_parameters() {
    let (s, _) = session();
    for id in ["drawing.fill", "drawing.fillEdit", "drawing.fillRemove"] {
        assert!(s.is_enabled(id), "{id}");
    }
    assert!(!Session::default().is_enabled("drawing.fill"), "nothing to fill without a drawing");
}
