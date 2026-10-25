//! The cel raster cache (#71) through the commands: whatever an edit, undo, redo, retime, transform,
//! fill, save or load does, the Program frame is bit-identical to one rendered with an empty cache.

use serde_json::{Value, json};

use crate::Session;
use waltcut_render::Image;
use waltcut_render::drawing_cache::global;

fn session() -> Session {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Draw", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 1})).unwrap();
    s.execute("drawing.new", json!({"seconds": 2})).unwrap();
    s
}

/// A stroke of `n` samples (enough of them for the cache to take the cel), wandering by `seed`.
fn scribble(seed: u32, n: usize, brush: &str) -> Value {
    let pts: Vec<Value> = (0..n)
        .map(|i| {
            let t = i as f64 / n as f64 * std::f64::consts::TAU * 3.0;
            json!([160.0 + 120.0 * (t + seed as f64).sin(), 90.0 + 70.0 * (t * 1.3 + seed as f64 * 0.7).cos(), 0.3 + (i % 7) as f64 / 10.0])
        })
        .collect();
    json!({"brush": brush, "color": "#e0302a", "size": 5, "points": pts})
}

fn frame(s: &Session, f: i64) -> Image {
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let t = s.sequence_rate().tick_of(f);
    waltcut_render::render_sequence(&s.project, s.state.active_sequence.unwrap(), t, Default::default(), &provider)
}

fn bits(i: &Image) -> Vec<u32> {
    i.px.iter().map(|v| v.to_bits()).collect()
}

/// The frame as the cache serves it (warm), and as a render with nothing cached gives it.
fn warm_and_cold(s: &Session, f: i64) -> (Image, Image) {
    let _ = frame(s, f);
    let warm = frame(s, f);
    global().clear();
    let cold = frame(s, f);
    (warm, cold)
}

fn assert_same(s: &Session, f: i64, what: &str) -> Image {
    let (warm, cold) = warm_and_cold(s, f);
    assert_eq!(bits(&warm), bits(&cold), "{what}: the cached frame differs from an uncached render");
    warm
}

#[test]
fn edits_undo_redo_and_erasers_never_return_a_stale_cel() {
    let mut s = session();
    for i in 0..3 {
        s.execute("drawing.addStroke", json!({"frame": 0, "stroke": scribble(i, 900, "Pen")})).unwrap();
    }
    let one = assert_same(&s, 0, "three strokes");
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": scribble(7, 900, "Pencil")})).unwrap();
    let two = assert_same(&s, 0, "a fourth stroke (pen-up extension)");
    assert_ne!(bits(&one), bits(&two));
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": scribble(3, 400, "Eraser")})).unwrap();
    let three = assert_same(&s, 0, "an eraser");
    assert_ne!(bits(&two), bits(&three));
    // undo brings back exactly what was shown before, whether or not it is still cached
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(bits(&assert_same(&s, 0, "undo")), bits(&two));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(bits(&assert_same(&s, 0, "undo twice")), bits(&one));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(bits(&assert_same(&s, 0, "redo")), bits(&two));
    // a held frame shows the same cel
    assert_eq!(bits(&assert_same(&s, 5, "held")), bits(&two));
    // editing through undo then drawing something else is a new picture, not the redo's
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": scribble(11, 900, "Marker")})).unwrap();
    let other = assert_same(&s, 0, "a different stroke after undo");
    assert_ne!(bits(&other), bits(&two));
}

#[test]
fn transform_retime_clear_and_motion_are_never_stale() {
    let mut s = session();
    for i in 0..3 {
        s.execute("drawing.addStroke", json!({"frame": 0, "stroke": scribble(i, 900, "Pen")})).unwrap();
    }
    s.execute("drawing.addStroke", json!({"frame": 4, "cel": "copy", "stroke": scribble(9, 900, "Pen")})).unwrap();
    let a0 = assert_same(&s, 0, "cel 0");
    let a4 = assert_same(&s, 4, "cel 4");
    assert_ne!(bits(&a0), bits(&a4));
    s.execute("drawing.transformStrokes", json!({"frame": 4, "strokes": [0, 1], "dx": 12, "dy": -9})).unwrap();
    let moved = assert_same(&s, 4, "transformed strokes");
    assert_ne!(bits(&a4), bits(&moved));
    assert_eq!(bits(&assert_same(&s, 0, "the other cel is untouched")), bits(&a0));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(bits(&assert_same(&s, 4, "undo transform")), bits(&a4));
    s.execute("drawing.retimeCel", json!({"from": 4, "to": 8})).unwrap();
    assert_same(&s, 4, "after retime (held cel 0)");
    assert_same(&s, 8, "after retime (moved cel)");
    // clip Motion changes the placement: scale and position each give the fresh render
    let clip = s.execute("drawing.inspect", json!({})).unwrap()["clip"].as_u64().unwrap();
    let plain = assert_same(&s, 0, "before motion");
    s.execute("effects.setParam", json!({"clip": clip, "effect": "motion", "param": "scale", "value": 60})).unwrap();
    let scaled = assert_same(&s, 0, "motion scale");
    assert_ne!(bits(&plain), bits(&scaled));
    s.execute("effects.setParam", json!({"clip": clip, "effect": "motion", "param": "position", "value": [100, 60]})).unwrap();
    assert_same(&s, 0, "motion position");
    s.execute("effects.setParam", json!({"clip": clip, "effect": "motion", "param": "rotation", "value": 33})).unwrap();
    assert_same(&s, 0, "motion rotation");
    s.execute("drawing.clearCel", json!({"frame": 0})).unwrap();
    let cleared = assert_same(&s, 0, "cleared cel");
    assert!(cleared.px.iter().all(|v| *v == 0.0) || bits(&cleared) != bits(&scaled));
}

#[test]
fn fills_including_reference_fills_are_never_stale() {
    let mut s = session();
    let ring = json!({"brush": "Pen", "color": "#000000", "size": 6, "points": [[60, 40], [260, 40], [260, 140], [60, 140], [60, 40]]});
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": ring})).unwrap();
    for i in 0..2 {
        s.execute("drawing.addStroke", json!({"frame": 0, "stroke": scribble(i + 20, 1200, "Pencil")})).unwrap();
    }
    let before = assert_same(&s, 0, "before the fill");
    let made = s.execute("drawing.fill", json!({"point": [70.5, 50.5], "color": "#2060ff"})).unwrap();
    let filled = assert_same(&s, 0, "a fill");
    assert_ne!(bits(&before), bits(&filled));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(bits(&assert_same(&s, 0, "undo fill")), bits(&before));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(bits(&assert_same(&s, 0, "redo fill")), bits(&filled));
    s.execute("drawing.fillRemove", json!({"index": made["index"]})).unwrap();
    assert_same(&s, 0, "fill removed");
}

#[test]
fn save_and_load_render_the_same_frame() {
    let mut s = session();
    for i in 0..3 {
        s.execute("drawing.addStroke", json!({"frame": 0, "stroke": scribble(i + 40, 900, "Pen")})).unwrap();
    }
    let want = assert_same(&s, 0, "before saving");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures").join(format!("drawing-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cache.waltcut");
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let mut t = Session::default();
    t.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    let got = assert_same(&t, 0, "after loading");
    assert_eq!(bits(&got), bits(&want));
    // the loaded project's undo history and strokes are untouched by clearing the cache
    let strokes_before = serde_json::to_string(&t.project.items).unwrap();
    global().clear();
    assert_eq!(serde_json::to_string(&t.project.items).unwrap(), strokes_before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rapid_scrubbing_across_cels_is_correct() {
    let mut s = session();
    for (f, seed) in [(0, 1), (3, 2), (6, 3), (9, 4)] {
        s.execute("drawing.addStroke", json!({"frame": f, "cel": "new", "stroke": scribble(seed, 2100, "Pen")})).ok();
    }
    let mut seen: Vec<Vec<u32>> = Vec::new();
    for round in 0..3 {
        for f in [0, 9, 3, 6, 1, 8, 4, 7, 2, 5, 0, 6] {
            let cached = frame(&s, f);
            if round == 0 {
                seen.push(bits(&cached));
            }
            let (warm, cold) = warm_and_cold(&s, f);
            assert_eq!(bits(&warm), bits(&cold), "frame {f}, round {round}");
        }
    }
    assert!(!seen.is_empty());
}
