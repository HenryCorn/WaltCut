//! Stroke smoothing (#57): Off stores the captured samples; Smooth de-jitters while keeping the
//! stroke's end exactly where the pen lifted; cancellation commits nothing.

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

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

fn key(k: egui::Key, pressed: bool) -> egui::Event {
    egui::Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers: Default::default() }
}

impl Driver {
    /// A 320x180 sequence with an empty drawing clip, the Brush tool selected.
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.newSequence", json!({"name": "Smooth", "fps": 24, "width": 320, "height": 180, "video": 2, "audio": 0})).unwrap();
        session.execute("drawing.new", json!({"seconds": 3.0})).unwrap();
        let (tx, rx) = channel();
        let app = WaltCutApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d.call("ui.set", json!({"tool": "Brush"}));
        d.frames(4);
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

    fn event(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    fn canvas(&mut self) -> egui::Rect {
        let v = self.call("ui.elements", json!({"prefix": "program.brush"}))["result"].clone();
        let r = v.as_array().and_then(|a| a.iter().find(|e| e["id"] == "program.brush")).and_then(|e| e["rect"].as_array()).expect("brush canvas").clone();
        let f = |i: usize| r[i].as_f64().unwrap() as f32;
        egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)))
    }

    /// Press, follow `path` (screen points), release at its last point.
    fn stroke(&mut self, path: &[egui::Pos2]) {
        self.event(egui::Event::PointerMoved(path[0]));
        self.event(button(path[0], true));
        for p in &path[1..path.len() - 1] {
            self.event(egui::Event::PointerMoved(*p));
        }
        let end = path[path.len() - 1];
        self.event(egui::Event::PointerMoved(end));
        self.event(button(end, false));
        self.frames(3);
    }

    fn strokes(&self) -> Vec<Vec<[f32; 3]>> {
        let p = &self.harness.state().session.project;
        p.items
            .values()
            .filter_map(|i| i.as_drawing())
            .flat_map(|d| d.cels.iter().flat_map(|c| c.strokes.iter().map(|s| s.points.iter().map(|q| [q.x, q.y, q.p]).collect())))
            .collect()
    }

    /// What a setting must not change: undo history, revision and the project itself.
    fn untouched(&self) -> (usize, u64, String) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.revision, serde_json::to_string(&s.project).unwrap())
    }
}

/// A horizontal pen path low in the picture with a ±2.5 point zigzag.
fn jittery(c: egui::Rect) -> Vec<egui::Pos2> {
    let y = c.min.y + c.height() * 0.8;
    (0..50).map(|i| egui::pos2(c.min.x + c.width() * 0.2 + i as f32 * 3.0, y + if i % 2 == 0 { 2.5 } else { -2.5 })).collect()
}

/// The largest departure from the middle of the stroke's settled part (after the filter has left
/// the first sample and before the pen-up catch-up).
fn wobble(points: &[[f32; 3]]) -> f32 {
    let ys: Vec<f32> = points[15..40].iter().map(|p| p[1]).collect();
    let mean = ys.iter().sum::<f32>() / ys.len() as f32;
    ys.iter().map(|y| (y - mean).abs()).fold(0.0, f32::max)
}

#[test]
fn off_keeps_the_captured_samples_and_smooth_reduces_the_jitter_but_ends_where_the_pen_lifted() {
    let mut d = Driver::new();
    let c = d.canvas();
    let path = jittery(c);
    d.stroke(&path);
    d.command("drawing.smoothing", json!({"mode": "smooth", "strength": 0.9})).unwrap();
    d.stroke(&path);
    let strokes = d.strokes();
    assert_eq!(strokes.len(), 2);
    let (off, smooth) = (&strokes[0], &strokes[1]);
    // Off: one point per sample (3 pt steps are above the thinning step)
    assert_eq!(off.len(), path.len(), "Off keeps every captured sample");
    // ±2.5 screen points are about ±1.4 canvas pixels here
    assert!(wobble(off) > 1.2, "the raw trace jitters: {}", wobble(off));
    assert!(wobble(smooth) < wobble(off) * 0.6, "smoothed {} vs raw {}", wobble(smooth), wobble(off));
    // the same pen path ends at the same canvas point, with or without smoothing
    assert_eq!(smooth.last(), off.last(), "the stroke ends exactly where the pen lifted");
    // the smoothed stroke starts where the pen came down too
    assert_eq!(smooth.first(), off.first());
    // no long straight finishing segment: the last step is no longer than a normal step
    let step = |p: &[[f32; 3]], i: usize| (p[i][0] - p[i - 1][0]).hypot(p[i][1] - p[i - 1][1]);
    let normal = (1..off.len()).map(|i| step(off, i)).fold(0.0f32, f32::max);
    let n = smooth.len();
    assert!(step(smooth, n - 1) <= normal, "last step {} vs largest raw step {normal}", step(smooth, n - 1));
}

#[test]
fn smoothing_is_a_setting_not_an_edit_and_applies_to_strokes_drawn_after_it() {
    let mut d = Driver::new();
    let before = d.untouched();
    let r = d.command("drawing.smoothing", json!({})).unwrap();
    assert_eq!((r["mode"].clone(), r["strength"].clone(), r["pressure"].clone(), r["active"].clone()), (json!("off"), json!(0.5), json!(0.0), json!(false)));
    let r = d.command("drawing.smoothing", json!({"mode": "smooth", "strength": 0.25, "pressure": 0.75})).unwrap();
    assert_eq!(
        (r["mode"].clone(), r["strength"].clone(), r["pressure"].clone(), r["active"].clone()),
        (json!("smooth"), json!(0.25), json!(0.75), json!(true))
    );
    assert_eq!(d.untouched(), before, "no undo step, revision or project change");
    assert_eq!(d.inspect()["ui"]["brush"]["smoothing"]["mode"], json!("Smooth"));
    // partial updates keep the rest
    let r = d.command("drawing.smoothing", json!({"strength": 0.8})).unwrap();
    assert_eq!((r["mode"].clone(), r["pressure"].clone()), (json!("smooth"), json!(0.75)));
    // a stroke already drawn is not changed by a later setting
    let c = d.canvas();
    d.command("drawing.smoothing", json!({"mode": "off"})).unwrap();
    d.stroke(&jittery(c));
    let first = d.strokes();
    d.command("drawing.smoothing", json!({"mode": "smooth", "strength": 1.0})).unwrap();
    assert_eq!(d.strokes(), first);
    // works with any tool, like the onion skin
    d.call("ui.set", json!({"tool": "Hand"}));
    assert!(d.command("drawing.smoothing", json!({"strength": 0.1})).is_ok());
}

#[test]
fn bad_smoothing_parameters_are_refused_whole_and_change_nothing() {
    let mut d = Driver::new();
    d.command("drawing.smoothing", json!({"mode": "smooth", "strength": 0.4, "pressure": 0.2})).unwrap();
    let before = d.inspect()["ui"]["brush"]["smoothing"].clone();
    for bad in [
        json!({"mode": "trailing"}),
        json!({"mode": 1}),
        json!({"strength": 1.5}),
        json!({"strength": -0.1}),
        json!({"strength": "high"}),
        json!({"pressure": 2}),
        json!({"pressure": [0.1]}),
        json!({"colour": 1}),
        json!({"mode": "off", "strength": 9}),
        json!(5),
    ] {
        assert!(d.command("drawing.smoothing", bad.clone()).is_err(), "{bad}");
        assert_eq!(d.inspect()["ui"]["brush"]["smoothing"], before, "{bad} changed something");
    }
    // a hand-edited state cannot make the filter misbehave
    d.harness.state_mut().ui.brush.smoothing.strength = f32::NAN;
    d.harness.state_mut().ui.brush.smoothing.pressure = f32::INFINITY;
    d.frames(2);
    let c = d.canvas();
    d.stroke(&jittery(c));
    assert!(d.strokes().iter().flatten().all(|p| p.iter().all(|v| v.is_finite())));
}

#[test]
fn cancelling_a_smoothed_stroke_commits_nothing_and_a_dot_is_one_point() {
    let mut d = Driver::new();
    d.command("drawing.smoothing", json!({"mode": "smooth", "strength": 0.9})).unwrap();
    let c = d.canvas();
    let path = jittery(c);
    // Escape in the middle of a stroke
    d.event(egui::Event::PointerMoved(path[0]));
    d.event(button(path[0], true));
    for p in &path[1..20] {
        d.event(egui::Event::PointerMoved(*p));
    }
    d.event(key(egui::Key::Escape, true));
    d.event(key(egui::Key::Escape, false));
    d.event(egui::Event::PointerMoved(path[30]));
    d.event(button(path[30], false));
    d.frames(3);
    assert_eq!(d.strokes().len(), 0, "nothing committed");
    assert_eq!(d.inspect()["ui"]["brush"]["pen"]["cancelled"], json!(1));
    // focus loss cancels the same way
    d.event(egui::Event::PointerMoved(path[0]));
    d.event(button(path[0], true));
    d.event(egui::Event::PointerMoved(path[5]));
    d.event(egui::Event::WindowFocused(false));
    d.event(button(path[5], false));
    d.frames(3);
    assert_eq!(d.strokes().len(), 0);
    // a tap: one point, where the pen was
    d.event(egui::Event::WindowFocused(true));
    d.stroke(&[path[0], path[0]]);
    let strokes = d.strokes();
    assert_eq!(strokes.len(), 1);
    assert_eq!(strokes[0].len(), 1, "a dot keeps its one point");
}

#[test]
fn batching_does_not_change_a_smoothed_stroke() {
    let mut a = Driver::new();
    let mut b = Driver::new();
    for d in [&mut a, &mut b] {
        d.command("drawing.smoothing", json!({"mode": "smooth", "strength": 0.7, "pressure": 0.4})).unwrap();
    }
    let (ca, cb) = (a.canvas(), b.canvas());
    assert_eq!(ca, cb);
    let path = jittery(ca);
    a.stroke(&path);
    // the same path with all the moves in one frame
    b.event(egui::Event::PointerMoved(path[0]));
    b.event(button(path[0], true));
    for p in &path[1..path.len() - 1] {
        b.harness.input_mut().events.push(egui::Event::PointerMoved(*p));
    }
    b.harness.input_mut().events.push(egui::Event::PointerMoved(path[path.len() - 1]));
    b.harness.input_mut().events.push(button(path[path.len() - 1], false));
    b.frames(4);
    assert_eq!(a.strokes(), b.strokes());
}

#[test]
fn the_controls_are_automatable_and_the_menu_lists_the_command() {
    let mut d = Driver::new();
    d.frames(2);
    let ids = |d: &mut Driver| -> Vec<String> {
        d.call("ui.elements", json!({"prefix": "program.brush.smooth"}))["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(ids(&mut d).contains(&"program.brush.smooth.menu".to_string()));
    d.call("ui.click", json!({"id": "program.brush.smooth.menu"}));
    d.frames(3);
    let found = ids(&mut d);
    for id in ["smooth", "smooth.strength", "smooth.pressure"] {
        assert!(found.contains(&format!("program.brush.{id}")), "{id}: {found:?}");
    }
    d.call("ui.click", json!({"id": "program.brush.smooth"}));
    d.frames(3);
    assert_eq!(d.inspect()["ui"]["brush"]["smoothing"]["mode"], json!("Smooth"));
    let items = d.call("ui.menu.list", json!({}))["result"].clone();
    assert!(items.as_array().unwrap().iter().any(|i| i["id"] == "drawing.smoothing" && i["enabled"] == true));
}
