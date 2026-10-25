//! Pressure response curves (#56): a pen's pressure becomes width and opacity through curves that
//! are stored with each stroke; a mouse is unaffected; nothing drawn changes when the settings do.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_project::drawing::{Curve, PressureResponse, Stroke};
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;
use waltcut_ui_egui::panels::drawing_input::{PenPhase, PenQueueReport, PenSample};

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
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
}

type Queue = Rc<RefCell<Vec<PenSample>>>;

impl Driver {
    /// Route the host's pen queue to a list the test fills.
    fn pen_queue(&mut self) -> Queue {
        let queue: Queue = Rc::default();
        let q = queue.clone();
        self.harness.state_mut().hooks.pen_samples = Some(Box::new(move |out| {
            out.extend(q.borrow_mut().drain(..));
            PenQueueReport::default()
        }));
        queue
    }

    /// A pen stroke along a gentle wave at the middle of the canvas with pressure rising 0.1 → 1.
    fn pen_stroke(&mut self, queue: &Queue, row: f32) {
        let c = self.canvas();
        let at = |i: usize| egui::pos2(c.min.x + c.width() * (0.2 + 0.6 * i as f32 / 19.0), c.min.y + c.height() * row);
        let sample = |i: usize, phase| {
            let p = at(i);
            PenSample { t_ns: 1_000_000 * (i as u64 + 1), x: p.x, y: p.y, pressure: 0.1 + 0.9 * i as f32 / 19.0, phase, tablet: true }
        };
        self.event(egui::Event::PointerMoved(at(0)));
        queue.borrow_mut().push(sample(0, PenPhase::Down));
        self.event(button(at(0), true));
        for i in 1..19 {
            queue.borrow_mut().push(sample(i, PenPhase::Move));
            self.event(egui::Event::PointerMoved(at(i)));
        }
        queue.borrow_mut().push(sample(19, PenPhase::Up));
        self.event(egui::Event::PointerMoved(at(19)));
        self.event(button(at(19), false));
        self.frames(3);
    }

    fn all_strokes(&self) -> Vec<Stroke> {
        let p = &self.harness.state().session.project;
        p.items.values().filter_map(|i| i.as_drawing()).flat_map(|d| d.cels.iter().flat_map(|c| c.strokes.iter().cloned())).collect()
    }

    fn render(&self) -> Vec<f32> {
        let s = &self.harness.state().session;
        let seq = s.state.active_sequence.expect("sequence");
        waltcut_render::render_sequence(
            &s.project,
            seq,
            waltcut_time::Tick::ZERO,
            waltcut_render::RenderOptions::default(),
            &waltcut_render::SourceMap::default(),
        )
        .px
        .to_vec()
    }

    fn untouched(&self) -> (usize, u64, bool) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.revision, s.is_dirty())
    }
}

#[test]
fn the_curve_command_sets_reports_and_refuses_bad_curves_whole() {
    let mut d = Driver::new();
    let before = d.untouched();
    let r = d.command("drawing.pressureCurve", json!({})).unwrap();
    assert_eq!((r["width"].clone(), r["opacity"].clone(), r["default"].clone()), (json!(null), json!(null), json!(true)));
    let r = d.command("drawing.pressureCurve", json!({"preset": "soft"})).unwrap();
    assert_eq!(r["default"], false);
    assert_eq!(r["width"], r["opacity"]);
    // each curve apart; null is the brush's own response; the identity is stored as nothing
    let r = d.command("drawing.pressureCurve", json!({"width": [[0, 0], [0.5, 0.9], [1, 1]], "opacity": null})).unwrap();
    assert_eq!((r["width"].clone(), r["opacity"].clone()), (json!([[0.0, 0.0], [0.5, 0.9f32 as f64], [1.0, 1.0]]), json!(null)));
    let r = d.command("drawing.pressureCurve", json!({"width": "linear"})).unwrap();
    assert_eq!((r["width"].clone(), r["default"].clone()), (json!(null), json!(true)));
    d.command("drawing.pressureCurve", json!({"preset": "firm"})).unwrap();
    let r = d.command("drawing.pressureCurve", json!({"reset": true})).unwrap();
    assert_eq!(r["default"], true);
    assert_eq!(d.untouched(), before, "a setting: no undo step, no revision, nothing marked changed");
    assert_eq!(d.inspect()["ui"]["brush"]["response"], json!({}), "the default is stored as nothing");

    d.command("drawing.pressureCurve", json!({"preset": "scurve"})).unwrap();
    let saved = d.inspect()["ui"]["brush"]["response"].clone();
    for bad in [
        json!({"preset": "bouncy"}),
        json!({"preset": 3}),
        json!({"width": [[0, 0]]}),
        json!({"width": [[0, 0], [1, 1], [2, 2]]}),
        json!({"width": [[0, 0], [0.5, 3], [1, 1]]}),
        json!({"width": [[0, 0], [0.7, 0.5], [0.3, 0.6], [1, 1]]}),
        json!({"width": [[0.1, 0], [1, 1]]}),
        json!({"width": "soft", "opacity": [[0, 0], [1, "x"]]}),
        json!({"opacity": 7}),
        json!({"reset": "yes"}),
        json!({"reset": true, "width": "soft"}),
        json!({"colour": 1}),
        json!(5),
    ] {
        assert!(d.command("drawing.pressureCurve", bad.clone()).is_err(), "{bad}");
        assert_eq!(d.inspect()["ui"]["brush"]["response"], saved, "{bad} changed something");
    }
    // a hand-edited state with a NaN curve cannot crash the preview or the stroke
    d.harness.state_mut().ui.brush.response =
        PressureResponse { width: Some(Curve(vec![[0.0, f32::NAN], [1.0, 1.0]])), opacity: Some(Curve(vec![[0.5, 0.5]])) };
    d.frames(3);
    let q = d.pen_queue();
    d.pen_stroke(&q, 0.7);
    assert!(d.all_strokes().iter().all(|s| s.response.is_none() || s.response.as_ref().is_some_and(|r| r.validate().is_ok())));
}

#[test]
fn a_pen_stroke_keeps_the_curves_it_was_drawn_with_and_later_settings_never_change_it() {
    let mut d = Driver::new();
    let q = d.pen_queue();
    // the brush's own response: nothing is stored
    d.pen_stroke(&q, 0.55);
    let plain_frame = d.render();
    d.command("drawing.pressureCurve", json!({"preset": "soft"})).unwrap();
    d.pen_stroke(&q, 0.75);
    let strokes = d.all_strokes();
    assert_eq!(strokes.len(), 2);
    assert_eq!(strokes[0].response, None);
    let soft = Curve::preset("soft");
    assert_eq!(strokes[1].response, Some(PressureResponse { width: soft.clone(), opacity: soft.clone() }));
    // the stored pressures are the pen's own: the curve is applied when the stroke is drawn
    assert!((strokes[1].points[0].p - 0.1).abs() < 1e-3 && (strokes[1].points.last().map_or(0.0, |p| p.p) - 1.0).abs() < 1e-3);
    // a later change of settings leaves both strokes, and so the frame, as they were
    let frame = d.render();
    assert_ne!(frame, plain_frame, "the soft stroke is on the frame");
    for p in [json!({"preset": "firm"}), json!({"reset": true}), json!({"width": [[0, 0], [0.5, 0.1], [1, 1]]})] {
        d.command("drawing.pressureCurve", p).unwrap();
        d.frames(2);
        assert_eq!(d.all_strokes(), strokes, "stored strokes do not follow the settings");
        assert_eq!(d.render(), frame);
    }
    // undo takes a stroke away; redo brings it back with its curves
    d.command("edit.undo", json!({})).unwrap();
    d.command("edit.redo", json!({})).unwrap();
    assert_eq!(d.all_strokes(), strokes);
}

#[test]
fn the_same_pen_path_draws_differently_through_different_curves() {
    let draw = |curve: Value| {
        let mut d = Driver::new();
        let q = d.pen_queue();
        d.command("drawing.pressureCurve", curve).unwrap();
        d.pen_stroke(&q, 0.7);
        d.render()
    };
    let (own, soft, firm) = (draw(json!({"reset": true})), draw(json!({"preset": "soft"})), draw(json!({"preset": "firm"})));
    assert_ne!(own, soft);
    assert_ne!(own, firm);
    assert_ne!(soft, firm);
    // width and opacity apart
    assert_ne!(draw(json!({"width": "soft"})), draw(json!({"opacity": "soft"})));
    // and the same settings draw the same, every time
    assert_eq!(draw(json!({"preset": "soft"})), soft);
}

#[test]
fn a_mouse_and_pressure_off_ignore_the_curves() {
    let mut d = Driver::new();
    d.command("drawing.pressureCurve", json!({"preset": "firm"})).unwrap();
    let c = d.canvas();
    // a mouse: full pressure, nothing stored
    d.stroke(&[
        egui::pos2(c.min.x + c.width() * 0.2, c.min.y + c.height() * 0.6),
        egui::pos2(c.min.x + c.width() * 0.5, c.min.y + c.height() * 0.62),
        egui::pos2(c.min.x + c.width() * 0.8, c.min.y + c.height() * 0.6),
    ]);
    let s = d.all_strokes();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].response, None);
    assert!(s[0].points.iter().all(|p| p.p == 1.0));
    // with Pressure switched off a pen stroke stores no response either (the pen queue still sends
    // its own pressures; the checkbox only governs the pointer fallback)
    let q = d.pen_queue();
    d.harness.state_mut().ui.brush.pressure = false;
    d.pen_stroke(&q, 0.8);
    assert_eq!(d.all_strokes()[1].response, None);
}

#[test]
fn the_editor_has_presets_handles_and_a_reset_that_all_have_automation_ids() {
    let mut d = Driver::new();
    d.frames(2);
    d.call("ui.click", json!({"id": "program.brush.feel.menu"}));
    d.frames(3);
    let ids = |d: &mut Driver| -> Vec<String> {
        d.call("ui.elements", json!({"prefix": "program.brush.pressure"}))["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect()
    };
    let found = ids(&mut d);
    for id in ["preset.linear", "preset.soft", "preset.firm", "preset.scurve", "width.h0", "width.h4", "opacity.h2", "reset"] {
        assert!(found.contains(&format!("program.brush.pressure.{id}")), "{id}: {found:?}");
    }
    // a preset by its id (choosing one closes the menu)
    d.call("ui.click", json!({"id": "program.brush.pressure.preset.soft"}));
    d.frames(3);
    assert_eq!(d.inspect()["ui"]["brush"]["response"]["width"], json!(Curve::preset("soft").map(|c| c.0)));
    d.call("ui.click", json!({"id": "program.brush.feel.menu"}));
    d.frames(3);
    // drag the middle width handle down: the curve changes, the opacity curve does not
    let opacity = d.inspect()["ui"]["brush"]["response"]["opacity"].clone();
    let v = d.call("ui.elements", json!({"prefix": "program.brush.pressure.width.h2"}))["result"].clone();
    let r = v[0]["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect::<Vec<_>>();
    let centre = egui::pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    d.event(egui::Event::PointerMoved(centre));
    d.event(button(centre, true));
    for i in 1..=6 {
        d.event(egui::Event::PointerMoved(centre + egui::vec2(0.0, 6.0 * i as f32)));
    }
    d.event(button(centre + egui::vec2(0.0, 36.0), false));
    d.frames(3);
    let after = d.inspect()["ui"]["brush"]["response"].clone();
    let mid = after["width"][2][1].as_f64().unwrap();
    assert!(mid < 0.5, "the handle moved down from the soft curve's 0.5 ... {after}");
    assert_eq!(after["opacity"], opacity);
    // reset (the menu may have closed after the drag)
    if !ids(&mut d).contains(&"program.brush.pressure.reset".to_string()) {
        d.call("ui.click", json!({"id": "program.brush.feel.menu"}));
        d.frames(3);
    }
    d.call("ui.click", json!({"id": "program.brush.pressure.reset"}));
    d.frames(3);
    assert_eq!(d.inspect()["ui"]["brush"]["response"], json!({}));
    let items = d.call("ui.menu.list", json!({}))["result"].clone();
    assert!(items.as_array().unwrap().iter().any(|i| i["id"] == "drawing.pressureCurve" && i["enabled"] == true));
}
