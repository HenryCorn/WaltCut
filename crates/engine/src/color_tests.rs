use serde_json::json;

use crate::Session;
use waltcut_project::ClipId;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

/// The first V1 clip under the playhead, selected.
fn pick_clip(s: &mut Session) -> ClipId {
    let q = s.active_sequence().unwrap();
    let t = s.playhead();
    // the demo opens with the playhead inside the first V1 clip (its first frames fade from black)
    let id = q.video_tracks[0].item_at(t).unwrap().id;
    s.state.selection = vec![id];
    id
}

fn mean(img: &waltcut_render::Image) -> [f32; 3] {
    assert!(img.px.iter().any(|v| *v > 0.01), "blank render");
    let mut m = [0f64; 3];
    for p in img.px.as_chunks::<4>().0 {
        for k in 0..3 {
            m[k] += p[k] as f64;
        }
    }
    let n = (img.px.len() / 4) as f64;
    m.map(|v| (v / n) as f32)
}

#[test]
fn interpret_footage_and_sequence_colour_settings() {
    let mut s = demo();
    let clip = pick_clip(&mut s);
    let item = s.active_sequence().unwrap().find_item(clip).unwrap().1.item;
    let sdr = mean(&s.render_program(0.125).unwrap());
    // metadata: the demo footage is SDR
    let info = s.execute("media.colorInfo", json!({"item": item.0})).unwrap();
    assert_eq!(info["override"], serde_json::Value::Null);
    assert_eq!(info["hdr"], false);
    // interpret it as S-Log3: decoded as log (lifted signal → brighter scene light), tone mapped
    let r = s.execute("clip.interpretFootage", json!({"colorSpace": "slog3-sgamut3cine"})).unwrap();
    assert_eq!(r["items"][0], item.0);
    let info = s.execute("media.colorInfo", json!({"item": item.0})).unwrap();
    assert_eq!(info["effective"], "slog3-sgamut3cine");
    assert_eq!(info["hdr"], true);
    let log = mean(&s.render_program(0.125).unwrap());
    assert!((log[0] - sdr[0]).abs() + (log[1] - sdr[1]).abs() > 0.02, "{sdr:?} vs {log:?}");
    assert!(log.iter().all(|v| *v <= 1.0 + 1e-3), "tone mapped into SDR: {log:?}");
    assert!(s.execute("clip.interpretFootage", json!({"colorSpace": "nope"})).is_err());
    s.execute("edit.undo", json!({})).unwrap();
    let back = mean(&s.render_program(0.125).unwrap());
    assert!((back[0] - sdr[0]).abs() < 1e-4, "undo restores the metadata interpretation");
    // a PQ sequence: SDR media keeps its values in working space; the monitor shows the same SDR
    let r = s.execute("sequence.colorSettings", json!({"workingSpace": "Rec. 2100 PQ"})).unwrap();
    assert_eq!(r["workingSpace"], "rec2100-pq");
    assert_eq!(s.active_sequence().unwrap().settings.working_space, "Rec. 2100 PQ");
    let work = mean(&s.render_program_working(0.125).unwrap());
    let disp = mean(&s.render_program(0.125).unwrap());
    assert!(work.iter().zip(&sdr).all(|(a, b)| (a - b).abs() < 0.08), "BT.2020 working values ≈ BT.709 SDR values: {work:?} vs {sdr:?}");
    assert!(disp.iter().zip(&sdr).all(|(a, b)| (a - b).abs() < 0.08), "display transform: {disp:?} vs {sdr:?}");
    let spaces = s.execute("color.spaces", json!({})).unwrap();
    assert_eq!(spaces["colorSpaces"].as_array().unwrap().len(), waltcut_color::ColorSpace::ALL.len());
    assert!(s.execute("sequence.colorSettings", json!({"workingSpace": "xyz"})).is_err());
}
