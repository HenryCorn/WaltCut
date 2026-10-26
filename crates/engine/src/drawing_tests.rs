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
fn reference_and_drawing_clips_keep_motion_opacity_blending_and_keyframes() {
    use waltcut_time::Tick;

    let mut s = session();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("fixtures")
        .join(format!("retained-reference-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("reference.png");
    image::RgbaImage::from_pixel(320, 180, image::Rgba([255, 0, 0, 255])).save(&path).unwrap();
    let imported = s.execute("file.import", json!({"paths": [path.to_string_lossy()]})).unwrap();
    let placed = s.execute("timeline.place", json!({"item": imported["items"][0], "track": "V1", "time": 0})).unwrap();
    let reference = placed["clips"][0].as_u64().unwrap();
    let drawing = new_drawing(&mut s);
    s.execute("drawing.addStroke", json!({"clip": drawing.0, "frame": 0, "stroke": {"color": "#00ff00", "size": 20, "points": [[160, 90]]}})).unwrap();
    let before = s.project.clone();
    for clip in [reference, drawing.0] {
        for (param, value) in [("anchor", json!([160, 90])), ("scale", json!(50)), ("rotation", json!(90))] {
            s.execute("effects.setParam", json!({"clip": clip, "effect": "motion", "param": param, "value": value})).unwrap();
        }
        for (time, position, opacity) in [(Tick::ZERO, [100, 90], 100), (s.sequence_rate().tick_of(24), [140, 90], 0)] {
            for (effect, param, value) in [("motion", "position", json!(position)), ("opacity", "opacity", json!(opacity))] {
                s.execute("effects.setParam", json!({"clip": clip, "effect": effect, "param": param, "value": value, "keyframe": true, "time": time.0}))
                    .unwrap();
            }
        }
    }
    let render = |s: &Session, t| {
        let provider = s.media.provider(s.project.clone(), s.services.clone());
        waltcut_render::render_sequence(&s.project, s.state.active_sequence.unwrap(), t, Default::default(), &provider)
    };
    let half = s.sequence_rate().tick_of(12);
    // The reference has moved, rotated and scaled. Away from the stroke it is half opaque.
    let img = render(&s, half);
    let p = img.get(120, 120);
    assert!((p[0] - 0.5).abs() < 0.02 && (p[3] - 0.5).abs() < 0.02, "{p:?}");
    assert!(img.get(20, 20)[3] < 0.01, "the reference no longer covers the frame");
    s.execute("effects.setParam", json!({"clip": drawing.0, "effect": "opacity", "param": "blend", "value": "Screen"})).unwrap();
    let screen = render(&s, half).get(120, 90);
    s.execute("effects.setParam", json!({"clip": drawing.0, "effect": "opacity", "param": "blend", "value": "Multiply"})).unwrap();
    let multiply = render(&s, half).get(120, 90);
    assert!(screen[0] > multiply[0] + 0.1 && screen[1] > multiply[1] + 0.1, "{screen:?} vs {multiply:?}");
    assert!(render(&s, s.sequence_rate().tick_of(24)).get(140, 90)[3] < 0.01, "both clips fade to transparent");
    let after = s.project.clone();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(*s.project, *after);
    assert_ne!(*s.project, *before);
    std::fs::remove_file(path).unwrap();
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
fn retiming_and_hold_edits_preserve_cels_and_undo_once_each() {
    let mut s = session();
    let clip = new_drawing(&mut s);
    s.execute("drawing.addStroke", json!({"clip": clip.0, "frame": 0, "stroke": line(10.0)})).unwrap();
    s.execute("drawing.newCel", json!({"clip": clip.0, "frame": 4})).unwrap();
    s.execute("drawing.addStroke", json!({"clip": clip.0, "frame": 4, "stroke": line(50.0)})).unwrap();
    s.execute("drawing.newCel", json!({"clip": clip.0, "frame": 8})).unwrap();

    let before_retime = s.history.undo.len();
    s.execute("drawing.retimeCel", json!({"clip": clip.0, "from": 4, "to": 6})).unwrap();
    let inspected = s.execute("drawing.inspect", json!({"clip": clip.0})).unwrap();
    assert_eq!(inspected["cels"], json!([{"frame": 0, "strokes": 1}, {"frame": 6, "strokes": 1}, {"frame": 8, "strokes": 0}]));
    assert_eq!(s.history.undo.len(), before_retime + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("drawing.inspect", json!({"clip": clip.0})).unwrap()["cels"][1]["frame"], 4);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.execute("drawing.inspect", json!({"clip": clip.0})).unwrap()["cels"][1]["frame"], 6);

    let before_hold = s.history.undo.len();
    s.execute("drawing.setHold", json!({"clip": clip.0, "frame": 0, "endFrame": 5})).unwrap();
    let hold = s.execute("drawing.inspect", json!({"clip": clip.0})).unwrap();
    assert_eq!(hold["cels"][1]["frame"], 5);
    assert_eq!(s.history.undo.len(), before_hold + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("drawing.inspect", json!({"clip": clip.0})).unwrap()["cels"][1]["frame"], 6);
    s.execute("edit.redo", json!({})).unwrap();

    let blank = s.execute("drawing.setHold", json!({"clip": clip.0, "frame": 8, "endFrame": 10})).unwrap();
    assert_eq!(blank["blank"], true);
    assert_eq!(s.execute("drawing.inspect", json!({"clip": clip.0})).unwrap()["cels"][3], json!({"frame": 10, "strokes": 0}));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("drawing.inspect", json!({"clip": clip.0})).unwrap()["cels"].as_array().map(Vec::len), Some(3));
}

#[test]
fn retiming_rejects_collisions_locked_tracks_and_frames_outside_trimmed_source() {
    let mut s = session();
    let clip = new_drawing(&mut s);
    s.execute("drawing.newCel", json!({"clip": clip.0, "frame": 4})).unwrap();
    let before = s.history.undo.len();
    for params in [json!({"clip": clip.0, "from": 0, "to": 4}), json!({"clip": clip.0, "from": 0, "to": 48}), json!({"clip": clip.0, "from": 1, "to": 2})] {
        assert!(s.execute("drawing.retimeCel", params.clone()).is_err(), "{params}");
        assert_eq!(s.history.undo.len(), before);
    }

    s.execute("timeline.trim", json!({"clip": clip.0, "edge": "in", "mode": "regular", "deltaFrames": 4})).unwrap();
    assert!(s.execute("drawing.retimeCel", json!({"clip": clip.0, "from": 0, "to": 2})).is_err());
    s.execute("drawing.retimeCel", json!({"clip": clip.0, "from": 4, "to": 5})).unwrap();

    let track = s.active_sequence().and_then(|seq| seq.find_item(clip)).map(|(id, _)| id).unwrap();
    s.execute("timeline.setTrack", json!({"track": track.0, "locked": true})).unwrap();
    let before = s.history.undo.len();
    let err = s.execute("drawing.retimeCel", json!({"clip": clip.0, "from": 5, "to": 6})).unwrap_err();
    assert!(err.to_string().contains("locked"), "{err}");
    assert_eq!(s.history.undo.len(), before);
}

#[test]
fn exposures_retime_on_twos_and_threes_as_one_undo_step() {
    let mut s = session();
    let clip = new_drawing(&mut s);
    for f in [0, 1, 2, 3] {
        s.execute("drawing.newCel", json!({"clip": clip.0, "frame": f})).unwrap();
        s.execute("drawing.addStroke", json!({"clip": clip.0, "frame": f, "stroke": line(f as f64 * 10.0)})).unwrap();
    }
    s.execute("drawing.newCel", json!({"clip": clip.0, "frame": 10})).unwrap();
    let frames = |s: &mut Session| -> Vec<i64> {
        s.execute("drawing.inspect", json!({"clip": clip.0})).unwrap()["cels"].as_array().unwrap().iter().map(|c| c["frame"].as_i64().unwrap()).collect()
    };
    let before = s.history.undo.len();
    let r = s.execute("drawing.retimeExposures", json!({"clip": clip.0, "frames": [0, 1, 2, 3], "on": "twos"})).unwrap();
    assert_eq!(r["moved"], 4);
    assert_eq!(frames(&mut s), vec![0, 2, 4, 6, 13]);
    assert_eq!(s.history.undo.len(), before + 1);
    s.execute("drawing.retimeExposures", json!({"clip": clip.0, "frames": [0, 2, 4, 6], "spacing": 3})).unwrap();
    assert_eq!(frames(&mut s), vec![0, 3, 6, 9, 16]);
    // the strokes went with their cels
    let full = s.execute("drawing.inspect", json!({"clip": clip.0, "strokes": true})).unwrap();
    assert_eq!(full["cels"][2]["strokes"][0]["points"][0]["x"], 20.0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(frames(&mut s), vec![0, 2, 4, 6, 13]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(frames(&mut s), vec![0, 1, 2, 3, 10]);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(frames(&mut s), vec![0, 2, 4, 6, 13]);
}

#[test]
fn retiming_exposures_refuses_what_does_not_fit_and_leaves_history_alone() {
    let mut s = session();
    let clip = new_drawing(&mut s);
    for f in [0, 1, 2, 40] {
        s.execute("drawing.newCel", json!({"clip": clip.0, "frame": f})).unwrap();
    }
    let before = s.history.undo.len();
    for params in [
        json!({"clip": clip.0, "frames": [0, 2], "on": "twos"}),     // not consecutive
        json!({"clip": clip.0, "frames": [0, 1, 2], "on": "fours"}), // unknown spacing
        json!({"clip": clip.0, "frames": [0, 1, 2], "spacing": 0}),
        json!({"clip": clip.0, "frames": [0, 1, 2], "spacing": 100}),
        json!({"clip": clip.0, "frames": [0, 1, 2], "spacing": 9_223_372_036_854_775_807i64}),
        json!({"clip": clip.0, "frames": [0, 1, 2, 40], "spacing": 20}), // the last would start past the end
        json!({"clip": clip.0, "frames": [0, 1, 2], "spacing": 20, "ripple": false}), // runs into frame 40
        json!({"clip": clip.0, "frames": [-4], "on": "twos"}),
        json!({"clip": clip.0, "frames": "all", "on": "twos"}),
        json!({"clip": clip.0, "on": "twos"}),
    ] {
        assert!(s.execute("drawing.retimeExposures", params.clone()).is_err(), "{params}");
        assert_eq!(s.history.undo.len(), before, "{params}");
    }
    // a dry run reports the result and changes nothing
    let r = s.execute("drawing.retimeExposures", json!({"clip": clip.0, "frames": [0, 1, 2], "on": "twos", "dryRun": true})).unwrap();
    assert_eq!(r["cels"], json!([0, 2, 4, 42]));
    assert_eq!(r["dryRun"], true);
    assert_eq!(s.history.undo.len(), before);
    // already on ones: nothing to do, no undo step
    s.execute("drawing.retimeExposures", json!({"clip": clip.0, "frames": [0, 1, 2], "on": "ones"})).unwrap();
    assert_eq!(s.history.undo.len(), before);
}

#[test]
fn retiming_exposures_respects_the_trimmed_range_and_locked_tracks() {
    let mut s = session();
    let clip = new_drawing(&mut s);
    for f in [0, 4, 6, 8] {
        s.execute("drawing.newCel", json!({"clip": clip.0, "frame": f})).unwrap();
    }
    s.execute("timeline.trim", json!({"clip": clip.0, "edge": "in", "mode": "regular", "deltaFrames": 4})).unwrap();
    let before = s.history.undo.len();
    // frame 0 is trimmed away: not part of what can be retimed
    assert!(s.execute("drawing.retimeExposures", json!({"clip": clip.0, "frames": [0, 4], "on": "twos"})).is_err());
    let r = s.execute("drawing.retimeExposures", json!({"clip": clip.0, "frames": [4, 6, 8], "on": "threes"})).unwrap();
    assert_eq!(r["cels"], json!([0, 4, 7, 10]));
    assert_eq!(s.history.undo.len(), before + 1);
    let track = s.active_sequence().and_then(|seq| seq.find_item(clip)).map(|(id, _)| id).unwrap();
    s.execute("timeline.setTrack", json!({"track": track.0, "locked": true})).unwrap();
    let before = s.history.undo.len();
    let err = s.execute("drawing.retimeExposures", json!({"clip": clip.0, "frames": [4, 7], "on": "twos"})).unwrap_err();
    assert!(err.to_string().contains("locked"), "{err}");
    assert_eq!(s.history.undo.len(), before);
}

/// Cels at 0 (strokes), 4 (blank), 8 (strokes) and 12 (strokes) on a two-second drawing.
fn poses(s: &mut Session) -> ClipId {
    let clip = new_drawing(s);
    for (f, draw) in [(0, true), (4, false), (8, true), (12, true)] {
        s.execute("drawing.newCel", json!({"clip": clip.0, "frame": f})).unwrap();
        if draw {
            s.execute("drawing.addStroke", json!({"clip": clip.0, "frame": f, "stroke": line(f as f64)})).unwrap();
        }
    }
    clip
}

fn playhead_frame(s: &Session) -> i64 {
    s.sequence_rate().frame_at(s.playhead())
}

#[test]
fn previous_and_next_cel_skip_holds_and_report_missing_neighbours() {
    let mut s = session();
    let clip = poses(&mut s);
    let (undo, revision) = (s.history.undo.len(), s.revision);
    // in the middle of the hold of cel 4
    s.execute("playhead.set", json!({"frame": 6})).unwrap();
    let r = s.execute("drawing.nextCel", json!({"clip": clip.0})).unwrap();
    assert_eq!((r["cel"].as_i64(), playhead_frame(&s)), (Some(8), 8));
    s.execute("drawing.nextCel", json!({})).unwrap();
    assert_eq!(playhead_frame(&s), 12);
    // the last cel holds to the end of the clip: no next one, and the playhead stays
    let err = s.execute("drawing.nextCel", json!({})).unwrap_err();
    assert!(err.to_string().contains("no next cel"), "{err}");
    assert_eq!(playhead_frame(&s), 12);
    s.execute("playhead.set", json!({"frame": 10})).unwrap();
    s.execute("drawing.previousCel", json!({})).unwrap();
    assert_eq!(playhead_frame(&s), 4, "a held pose counts once: from inside cel 8 the previous drawing is cel 4");
    let r = s.execute("drawing.previousCel", json!({})).unwrap();
    assert_eq!((r["frame"].as_i64(), playhead_frame(&s)), (Some(0), 0));
    assert!(s.execute("drawing.previousCel", json!({})).unwrap_err().to_string().contains("no previous cel"));
    // nothing was edited
    assert_eq!((s.history.undo.len(), s.revision), (undo, revision));
}

#[test]
fn blank_cels_are_poses_unless_skipped() {
    let mut s = session();
    let clip = poses(&mut s);
    s.execute("playhead.set", json!({"frame": 0})).unwrap();
    let r = s.execute("drawing.nextCel", json!({"clip": clip.0})).unwrap();
    assert_eq!((playhead_frame(&s), r["blank"].clone()), (4, json!(true)));
    s.execute("playhead.set", json!({"frame": 0})).unwrap();
    s.execute("drawing.nextCel", json!({"clip": clip.0, "skipBlank": true})).unwrap();
    assert_eq!(playhead_frame(&s), 8);
    s.execute("drawing.previousCel", json!({"clip": clip.0, "skipBlank": true})).unwrap();
    assert_eq!(playhead_frame(&s), 0);
}

#[test]
fn cel_navigation_respects_trims_and_a_hold_that_reaches_into_the_clip() {
    let mut s = session();
    let clip = poses(&mut s);
    // the clip now starts at drawing frame 6, inside the hold of cel 4
    s.execute("timeline.trim", json!({"clip": clip.0, "edge": "in", "mode": "regular", "deltaFrames": 6})).unwrap();
    let start = s.active_sequence().and_then(|seq| seq.find_item(clip)).map(|(_, it)| it.start).unwrap();
    s.set_playhead(start);
    // cel 0 ends before the clip begins: not a previous drawing; cel 4 is the one shown at the start
    assert!(s.execute("drawing.previousCel", json!({})).is_err());
    let r = s.execute("drawing.nextCel", json!({})).unwrap();
    assert_eq!(r["cel"], 8);
    // from cel 8, the previous drawing is cel 4, shown from the clip's first frame, not before it
    let r = s.execute("drawing.previousCel", json!({})).unwrap();
    assert_eq!((r["cel"].as_i64(), r["frame"].as_i64()), (Some(4), Some(6)));
    assert_eq!(s.playhead(), start);
}

#[test]
fn cel_navigation_follows_the_timeline_on_a_reversed_clip() {
    let mut s = session();
    let clip = poses(&mut s);
    s.execute("clip.speedDuration", json!({"clips": [clip.0], "speed": 100, "reverse": true})).unwrap();
    let start = s.active_sequence().and_then(|seq| seq.find_item(clip)).map(|(_, it)| it.start).unwrap();
    s.set_playhead(start);
    // the clip opens on its last cel; "next" in time is the cel before it in the drawing
    let r = s.execute("drawing.nextCel", json!({})).unwrap();
    assert_eq!(r["cel"], 8);
    let r = s.execute("drawing.previousCel", json!({})).unwrap();
    assert_eq!(r["cel"], 12);
}

#[test]
fn cel_navigation_refuses_hostile_parameters() {
    let mut s = session();
    let clip = poses(&mut s);
    let before = s.playhead();
    for params in
        [json!({"clip": clip.0, "frame": -5}), json!({"clip": clip.0, "frame": 9_000_000}), json!({"clip": 987_654}), json!({"clip": clip.0, "frame": "next"})]
    {
        assert!(s.execute("drawing.nextCel", params.clone()).is_err() || params["frame"] == "next", "{params}");
        assert!(s.execute("drawing.previousCel", params.clone()).is_err() || params["frame"] == "next", "{params}");
    }
    assert_eq!(s.playhead(), before);
}

#[test]
fn retimed_cels_and_navigation_agree_at_a_fractional_sequence_rate() {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "NTSC", "fps": 23.976, "width": 320, "height": 180, "video": 1, "audio": 0})).unwrap();
    let clip = new_drawing(&mut s);
    for f in [0, 1, 2, 3, 20] {
        s.execute("drawing.newCel", json!({"clip": clip.0, "frame": f})).unwrap();
    }
    assert_eq!(s.sequence_rate(), waltcut_time::FrameRate::FPS_23_976);
    s.execute("drawing.retimeExposures", json!({"clip": clip.0, "frames": [0, 1, 2, 3], "on": "threes"})).unwrap();
    let starts: Vec<i64> =
        s.execute("drawing.inspect", json!({"clip": clip.0})).unwrap()["cels"].as_array().unwrap().iter().map(|c| c["frame"].as_i64().unwrap()).collect();
    assert_eq!(starts, vec![0, 3, 6, 9, 26]);
    // stepping through them lands on the exact frame of each, in exact ticks
    s.execute("playhead.set", json!({"frame": 0})).unwrap();
    for want in &starts[1..] {
        s.execute("drawing.nextCel", json!({})).unwrap();
        assert_eq!(s.playhead(), s.sequence_rate().tick_of(*want), "frame {want}");
    }
    assert!(s.execute("drawing.nextCel", json!({})).is_err());
    // the same cels survive a save-format round trip
    let seq = s.active_sequence().unwrap();
    let (_, d) = crate::drawing::drawing_of(&s, seq, clip).unwrap();
    let back: waltcut_project::drawing::Drawing = serde_json::from_slice(&serde_json::to_vec(d).unwrap()).unwrap();
    assert_eq!(back.cels.iter().map(|c| c.frame).collect::<Vec<_>>(), starts);
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

fn cels(s: &Session) -> Vec<std::sync::Arc<Vec<waltcut_project::drawing::Stroke>>> {
    s.project.items.values().find_map(|i| i.as_drawing()).map(|d| d.cels.iter().map(|c| c.strokes.clone()).collect()).unwrap_or_default()
}

#[test]
fn adding_a_stroke_copies_only_its_own_cel_and_undo_restores_the_shared_strokes() {
    let mut s = session();
    new_drawing(&mut s);
    for f in [0, 2, 4] {
        s.execute("drawing.addStroke", json!({"frame": f, "cel": "new", "stroke": line(f as f64 * 10.0)})).unwrap();
    }
    let before = cels(&s);
    s.execute("drawing.addStroke", json!({"frame": 2, "stroke": line(99.0)})).unwrap();
    let after = cels(&s);
    // the cels that were not drawn on still share their strokes with the snapshot before the edit
    assert!(std::sync::Arc::ptr_eq(&before[0], &after[0]) && std::sync::Arc::ptr_eq(&before[2], &after[2]), "untouched cels are not copied");
    assert!(!std::sync::Arc::ptr_eq(&before[1], &after[1]));
    assert_eq!((before[1].len(), after[1].len()), (1, 2), "the snapshot keeps the old strokes");
    s.execute("edit.undo", json!({})).unwrap();
    let undone = cels(&s);
    assert!(std::sync::Arc::ptr_eq(&before[1], &undone[1]) && undone[1].len() == 1);
    // clearing and replacing also leave the other cels alone
    s.execute("drawing.clearCel", json!({"frame": 4})).unwrap();
    let cleared = cels(&s);
    assert!(std::sync::Arc::ptr_eq(&before[0], &cleared[0]) && cleared[2].is_empty());
}

#[test]
fn a_copied_cel_shares_strokes_until_either_is_edited_and_then_they_diverge() {
    let mut s = session();
    new_drawing(&mut s);
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": line(10.0)})).unwrap();
    s.execute("drawing.newCel", json!({"frame": 3, "copy": true})).unwrap();
    let both = cels(&s);
    assert!(std::sync::Arc::ptr_eq(&both[0], &both[1]), "a copy is free");
    s.execute("drawing.addStroke", json!({"frame": 3, "stroke": line(60.0)})).unwrap();
    let r = s.execute("drawing.inspect", json!({})).unwrap();
    assert_eq!(r["cels"], json!([{"frame": 0, "strokes": 1}, {"frame": 3, "strokes": 2}]), "the original is not changed by drawing on the copy");
    s.execute("drawing.clearCel", json!({"frame": 0})).unwrap();
    let r = s.execute("drawing.inspect", json!({})).unwrap();
    assert_eq!(r["cels"], json!([{"frame": 0, "strokes": 0}, {"frame": 3, "strokes": 2}]));
}

#[test]
fn the_project_file_format_of_cels_is_unchanged() {
    let mut s = session();
    new_drawing(&mut s);
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": line(10.0)})).unwrap();
    let d = s.project.items.values().find_map(|i| i.as_drawing()).unwrap().clone();
    let v = serde_json::to_value(&d).unwrap();
    assert!(v["cels"][0]["strokes"].is_array(), "strokes are a plain list: {v}");
    let back: waltcut_project::drawing::Drawing = serde_json::from_value(v).unwrap();
    assert_eq!(back, d);
    // an old file without `strokes` still loads
    let old: waltcut_project::drawing::Drawing =
        serde_json::from_value(json!({"width": 8, "height": 8, "rate": {"num": 24, "den": 1}, "cels": [{"frame": 5}]})).unwrap();
    assert!(old.cels[0].strokes.is_empty() && old.cels[0].link.is_none());
    // no cel is linked, so no `link` or `links` key is written
    let text = serde_json::to_string(&d).unwrap();
    assert!(!text.contains("link"), "{text}");
}

fn stored_response(s: &Session) -> Vec<Option<waltcut_project::drawing::PressureResponse>> {
    s.project.items.values().filter_map(|i| i.as_drawing()).flat_map(|d| d.cels.iter().flat_map(|c| c.strokes.iter().map(|st| st.response.clone()))).collect()
}

#[test]
fn a_stroke_keeps_the_pressure_response_it_was_drawn_with_through_undo_and_save() {
    use waltcut_project::drawing::Curve;
    let mut s = session();
    new_drawing(&mut s);
    let mut with = line(10.0);
    with["response"] = json!({"width": "soft", "opacity": [[0.0, 0.0], [0.5, 0.2], [1.0, 1.0]]});
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": with})).unwrap();
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": line(60.0)})).unwrap();
    let r = stored_response(&s);
    assert_eq!(r[0].as_ref().and_then(|r| r.width.clone()), Curve::preset("soft"));
    assert_eq!(r[0].as_ref().and_then(|r| r.opacity.as_ref()).map(|c| c.0.len()), Some(3));
    assert_eq!(r[1], None, "a stroke drawn without a response is stored without one");
    // the identity, written out, is stored as nothing
    let mut ident = line(90.0);
    ident["response"] = json!({"width": "linear", "opacity": [[0, 0], [1, 1]]});
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": ident})).unwrap();
    assert_eq!(stored_response(&s)[2], None);
    // undo and redo bring back the same response
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(stored_response(&s).is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(stored_response(&s)[0].as_ref().and_then(|r| r.width.clone()), Curve::preset("soft"));
}

#[test]
fn malformed_responses_are_refused_and_change_nothing() {
    let mut s = session();
    new_drawing(&mut s);
    s.execute("drawing.addStroke", json!({"frame": 0, "stroke": line(10.0)})).unwrap();
    let before = serde_json::to_string(&s.project).unwrap();
    for bad in [
        json!("soft"),
        json!([[0, 0], [1, 1]]),
        json!({"width": "bouncy"}),
        json!({"width": [[0, 0]]}),
        json!({"width": [[0, 0], [1, 1], [2, 2]]}),
        json!({"width": [[0, 0], [0.5, 7], [1, 1]]}),
        json!({"width": [[0, 0], [0.6, 0.5], [0.4, 0.6], [1, 1]]}),
        json!({"width": [[0.2, 0], [1, 1]]}),
        json!({"width": [[0, 0], [1, 1]], "hue": 1}),
        json!({"opacity": "firm", "width": [[0, 0, 0], [1, 1]]}),
        json!({"opacity": 5}),
        json!({"opacity": [[0, 0], [null, 1]]}),
    ] {
        let mut stroke = line(30.0);
        stroke["response"] = bad.clone();
        let e = s.execute("drawing.addStroke", json!({"frame": 0, "stroke": stroke})).unwrap_err().to_string();
        assert!(!e.is_empty(), "{bad}");
        assert_eq!(serde_json::to_string(&s.project).unwrap(), before, "{bad} changed the project");
    }
}
