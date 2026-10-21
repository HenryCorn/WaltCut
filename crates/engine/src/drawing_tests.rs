use serde_json::json;

use crate::Session;
use waltcut_project::ClipId;

fn session() -> Session {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Draw", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 1})).unwrap();
    s
}

fn new_drawing(s: &mut Session) -> ClipId {
    let r = s.execute("drawing.new", json!({"seconds": 2})).unwrap();
    ClipId(r["clip"].as_u64().unwrap())
}

fn line(x: f64) -> serde_json::Value {
    json!({"brush": "Pen", "color": "#ff0000", "size": 8, "points": [[x, 20.0, 0.5], [x + 40.0, 60.0, 1.0]]})
}

#[test]
fn strokes_land_in_held_cels_and_undo_one_at_a_time() {
    let mut s = session();
    let clip = new_drawing(&mut s);
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": line(10.0)})).unwrap();
    s.execute("drawing.addStroke", json!({"frame": 3, "stroke": line(50.0)})).unwrap();
    // frame 3 still shows the cel from frame 0: both strokes are in it
    let r = s.execute("drawing.inspect", json!({"clip": clip.0, "frame": 3})).unwrap();
    assert_eq!(r["cels"], json!([{"frame": 0, "strokes": 2}]));
    s.execute("drawing.addStroke", json!({"frame": 4, "cel": "copy", "stroke": line(90.0)})).unwrap();
    let r = s.execute("drawing.inspect", json!({})).unwrap();
    assert_eq!(r["cels"], json!([{"frame": 0, "strokes": 2}, {"frame": 4, "strokes": 3}]));
    s.execute("edit.undo", json!({})).unwrap();
    let r = s.execute("drawing.inspect", json!({})).unwrap();
    assert_eq!(r["cels"], json!([{"frame": 0, "strokes": 2}]));
}

#[test]
fn drawings_render_on_the_program_frame() {
    let mut s = session();
    new_drawing(&mut s);
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": {"color": "#ffffff", "size": 20, "points": [[100, 90], [220, 90]]}})).unwrap();
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let img = waltcut_render::render_sequence(&s.project, s.state.active_sequence.unwrap(), waltcut_time::Tick::ZERO, Default::default(), &provider);
    let red = |x: usize, y: usize| img.px[(y * img.w + x) * 4];
    assert!(red(160, 90) > 0.9, "the white stroke is drawn over the black frame");
    assert!(red(160, 20) < 0.01, "and nothing far from it changes");
}

#[test]
fn in_betweens_fill_the_gap_between_cels() {
    let mut s = session();
    new_drawing(&mut s);
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": line(0.0)})).unwrap();
    s.execute("drawing.addStroke", json!({"frame": 8, "cel": "new", "stroke": line(80.0)})).unwrap();
    let r = s.execute("drawing.tween", json!({"frame": 0, "step": 2})).unwrap();
    assert_eq!(r["cels"], json!([2, 4, 6]));
    let r = s.execute("drawing.inspect", json!({"frame": 4, "strokes": true})).unwrap();
    let x0 = r["cels"][2]["strokes"][0]["points"][0]["x"].as_f64().unwrap();
    assert!((x0 - 40.0).abs() < 1e-3, "halfway: {x0}");
}

#[test]
fn hostile_parameters_are_refused_not_fatal() {
    let mut s = session();
    assert!(s.execute("drawing.addStroke", json!({"stroke": line(0.0)})).is_err(), "no drawing yet");
    new_drawing(&mut s);
    for bad in [
        json!({"stroke": {"points": []}}),
        json!({"stroke": {"size": -1, "points": [[0, 0]]}}),
        json!({"stroke": {"brush": "Airbrush", "points": [[0, 0]]}}),
        json!({"stroke": {"color": "#zzzzzz", "points": [[0, 0]]}}),
        json!({"stroke": line(0.0), "cel": "sideways"}),
        json!({"stroke": {"points": "nope"}}),
    ] {
        assert!(s.execute("drawing.addStroke", bad.clone()).is_err(), "{bad}");
    }
    // absurd numbers are clamped, never trusted
    // points beyond f32 become infinite and are dropped, which leaves nothing to draw
    assert!(s.execute("drawing.addStroke", json!({"frame": 0, "stroke": {"points": [[1e300, -1e300, 99]]}})).is_err());
    s.execute("drawing.addStroke", json!({"frame": i64::MAX, "stroke": {"size": 1e30, "points": [[1e30, -1e30, 99]]}})).unwrap();
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    waltcut_render::render_sequence(&s.project, s.state.active_sequence.unwrap(), s.playhead(), Default::default(), &provider);
    assert!(s.execute("drawing.tween", json!({"frame": 0})).is_err());
    assert!(s.execute("drawing.deleteCel", json!({"frame": -5})).is_err());
}
