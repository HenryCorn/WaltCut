//! Onion skin (#62): several distinct ghosts, a pinned pose and the loop start. The onion skin is
//! a view setting: it never edits, dirties or exports anything.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    /// Cels at frames 0, 4 (blank), 8, 12 and 16 on a two-second drawing, the playhead on frame 9
    /// (inside the hold of cel 8); frame 20 repeats the strokes of 16 as a copy.
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.newSequence", json!({"name": "Onion", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 0})).unwrap();
        let clip = session.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
        for frame in [0, 4, 8, 12, 16, 20] {
            session.execute("drawing.newCel", json!({"clip": clip, "frame": frame})).unwrap();
            if frame != 4 {
                let x = if frame == 20 { 16.0 } else { frame as f64 };
                let stroke = json!({"brush": "Pen", "color": "#000000", "size": 4.0, "points": [[10.0, 10.0, 1.0], [40.0 + x, 30.0, 1.0]]});
                session.execute("drawing.addStroke", json!({"clip": clip, "frame": frame, "stroke": stroke})).unwrap();
            }
        }
        session.execute("playhead.set", json!({"frame": 9})).unwrap();
        session.execute("timeline.select", json!({"clips": [clip]})).unwrap();
        let (tx, rx) = channel();
        let app = WaltCutApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.command("tool.brush", json!({})).unwrap();
        d.frames(6);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(r) = reply.try_recv() {
                return r;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn command(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let r = self.call("engine.execute", json!({"command": id, "params": params}));
        if r["ok"] == true { Ok(r["result"].clone()) } else { Err(r["error"].as_str().unwrap_or("").to_string()) }
    }

    fn inspect(&mut self) -> Value {
        self.call("ui.inspect", json!({}))["result"].clone()
    }

    fn ghosts(&mut self) -> Vec<(String, i64)> {
        self.inspect()["ghosts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| (format!("{}{}", g["role"].as_str().unwrap(), g["n"]), g["frame"].as_i64().unwrap()))
            .collect()
    }

    /// What a view setting must not change: the undo history, the revision and the project itself.
    fn untouched(&self) -> (usize, u64, String) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.revision, serde_json::to_string(&s.project).unwrap())
    }
}

fn g(role: &str, frame: i64) -> (String, i64) {
    (role.to_string(), frame)
}

#[test]
fn by_default_one_cel_before_and_after_and_a_hold_never_repeats() {
    let mut d = Driver::new();
    for frame in [8, 9, 10, 11] {
        d.command("playhead.set", json!({"frame": frame})).unwrap();
        assert_eq!(d.ghosts(), vec![g("before1", 4), g("after1", 12)], "frame {frame}");
    }
    let state = d.command("drawing.onionSkin", json!({})).unwrap();
    assert_eq!((state["before"].clone(), state["after"].clone(), state["opacity"].clone()), (json!(1), json!(1), json!(0.35f32 as f64)));
}

#[test]
fn several_cels_fade_with_distance_and_the_blank_pose_is_honoured() {
    let mut d = Driver::new();
    let before = d.untouched();
    let r = d.command("drawing.onionSkin", json!({"before": 3, "after": 2, "opacity": 0.5, "falloff": 0.5, "tint": 0.25})).unwrap();
    assert_eq!((r["before"].clone(), r["after"].clone(), r["tint"].clone()), (json!(3), json!(2), json!(0.25)));
    // before the shown cel 8: the blank cel 4, then 0; after: 12, 16 (20 repeats 16 and is passed over)
    assert_eq!(d.ghosts(), vec![g("before2", 0), g("after2", 16), g("before1", 4), g("after1", 12)]);
    let report = d.inspect()["ghosts"].clone();
    let opacity: Vec<f64> = report.as_array().unwrap().iter().map(|x| x["opacity"].as_f64().unwrap()).collect();
    assert_eq!(opacity, vec![0.25, 0.25, 0.5, 0.5]);
    d.frames(3);
    assert_eq!(d.untouched(), before, "a view setting: no undo step, no revision, the project is unchanged");
    // off: no ghosts
    d.command("drawing.onionSkin", json!({"enabled": false})).unwrap();
    assert_eq!(d.ghosts(), vec![]);
}

#[test]
fn a_pinned_pose_and_the_loop_start_are_ghosts_that_follow_neither_the_playhead_nor_each_other() {
    let mut d = Driver::new();
    d.command("drawing.onionSkin", json!({"before": 0, "after": 0})).unwrap();
    d.command("playhead.set", json!({"frame": 17})).unwrap();
    // pin the pose shown now (cel 16), then move on
    let pinned = d.command("drawing.pinPose", json!({})).unwrap();
    assert_eq!(pinned["pin"]["frame"], 16);
    d.command("playhead.set", json!({"frame": 9})).unwrap();
    assert_eq!(d.ghosts(), vec![g("pin1", 16)]);
    d.command("drawing.onionSkin", json!({"loop": true})).unwrap();
    assert_eq!(d.ghosts(), vec![g("loop1", 0), g("pin1", 16)]);
    // on the pinned cel itself, or on the first cel, nothing doubles
    d.command("playhead.set", json!({"frame": 17})).unwrap();
    assert_eq!(d.ghosts(), vec![g("loop1", 0)]);
    d.command("playhead.set", json!({"frame": 1})).unwrap();
    assert_eq!(d.ghosts(), vec![g("pin1", 16)]);
    // pinning again unpins; a pin by frame; null clears
    d.command("drawing.pinPose", json!({})).unwrap();
    assert_eq!(d.ghosts(), vec![], "unpinned, and the loop start is the cel shown");
    d.command("drawing.onionSkin", json!({"pin": 12})).unwrap();
    assert_eq!(d.ghosts(), vec![g("pin1", 12)]);
    d.command("drawing.onionSkin", json!({"pin": null})).unwrap();
    assert_eq!(d.ghosts(), vec![]);
}

#[test]
fn invalid_onion_settings_are_refused_whole() {
    let mut d = Driver::new();
    let before = d.inspect()["ui"]["brush"].clone();
    for bad in [
        json!({"before": 5}),
        json!({"after": -1}),
        json!({"before": 1.5}),
        json!({"opacity": 1.5}),
        json!({"falloff": -0.1}),
        json!({"tint": "full"}),
        json!({"enabled": 1}),
        json!({"pin": 7}),
        json!({"pin": -3}),
        json!({"pin": "later"}),
        json!({"colour": 1}),
        json!({"before": 2, "opacity": 9}),
    ] {
        let e = d.command("drawing.onionSkin", bad.clone()).unwrap_err();
        assert!(!e.is_empty(), "{bad}");
        assert_eq!(d.inspect()["ui"]["brush"], before, "{bad} changed something");
    }
    // a hand-edited UI state cannot make a ghost solid or unbounded
    d.harness.state_mut().ui.brush.onion_before = 200;
    d.harness.state_mut().ui.brush.onion_falloff = f32::NAN;
    d.harness.state_mut().ui.brush.onion_opacity = 3.0;
    d.frames(2);
    let ghosts = d.inspect()["ghosts"].clone();
    assert!(ghosts.as_array().unwrap().len() <= 9);
    for x in ghosts.as_array().unwrap() {
        let o = x["opacity"].as_f64().unwrap();
        assert!((0.0..=0.600001).contains(&o), "{o}");
    }
}

#[test]
fn the_onion_controls_are_automatable() {
    let mut d = Driver::new();
    d.frames(2);
    let ids: Vec<String> = d.call("ui.elements", json!({"prefix": "program.brush.onion"}))["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&"program.brush.onion".to_string()) && ids.contains(&"program.brush.onion.menu".to_string()), "{ids:?}");
    d.call("ui.click", json!({"id": "program.brush.onion.menu"}));
    d.frames(3);
    let ids: Vec<String> = d.call("ui.elements", json!({"prefix": "program.brush.onion"}))["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect();
    for id in ["before", "after", "opacity", "falloff", "tint", "loop"] {
        assert!(ids.contains(&format!("program.brush.onion.{id}")), "{id} in {ids:?}");
    }
}
