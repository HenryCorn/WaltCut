//! Fill mode in the Brush tool (#68): the toggle and tolerance commands, a click that fills, a
//! drag that draws nothing, the options bar that a click cannot reach through, the edits of the
//! newest fill, a locked track, and the keys the mode takes.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
    clip: u64,
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

impl Driver {
    /// 128x72 with a closed black outline from (30, 15) to (100, 57).
    fn new() -> Self {
        let mut s = Session::default();
        s.execute("file.newSequence", json!({"name": "Fill", "width": 128, "height": 72, "fps": 24, "video": 2, "audio": 0})).unwrap();
        let clip = s.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
        let pts = [[30, 15], [100, 15], [100, 57], [30, 57], [30, 15]].map(|p| json!([p[0], p[1], 1.0]));
        let stroke = json!({"brush": "Pen", "color": "#000000", "size": 4.0, "points": pts});
        s.execute("drawing.addStroke", json!({"clip": clip, "frame": 0, "stroke": stroke})).unwrap();
        s.execute("timeline.select", json!({"clips": [clip]})).unwrap();
        let (tx, rx) = channel();
        let app = WaltCutApp::new(s).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, clip };
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

    fn ui(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let r = self.call("ui.menu.invoke", json!({"id": id, "params": params}));
        self.frames(2);
        if r["ok"] == true { Ok(r["result"].clone()) } else { Err(r["error"].as_str().unwrap_or("").to_string()) }
    }

    fn engine(&mut self, id: &str, params: Value) -> Value {
        let r = self.call("engine.execute", json!({"command": id, "params": params}));
        self.frames(2);
        assert_eq!(r["ok"], true, "{id}: {r}");
        r["result"].clone()
    }

    fn brush(&mut self) -> Value {
        self.call("ui.inspect", json!({}))["result"]["ui"]["brush"].clone()
    }

    fn event(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    fn element(&mut self, id: &str) -> egui::Rect {
        let v = self.call("ui.elements", json!({"prefix": "program.brush"}))["result"].clone();
        let r =
            v.as_array().and_then(|a| a.iter().find(|e| e["id"] == id)).and_then(|e| e["rect"].as_array()).unwrap_or_else(|| panic!("no element {id}")).clone();
        let f = |i: usize| r[i].as_f64().unwrap() as f32;
        egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)))
    }

    /// The screen point of canvas pixel (x, y).
    fn at(&mut self, x: f32, y: f32) -> egui::Pos2 {
        let b = self.element("program.brush");
        b.min + egui::vec2(x / 128.0 * b.width(), y / 72.0 * b.height())
    }

    fn click(&mut self, p: egui::Pos2) {
        self.event(egui::Event::PointerMoved(p));
        self.event(button(p, true));
        self.event(button(p, false));
        self.frames(3);
    }

    fn click_element(&mut self, id: &str) {
        let c = self.element(id).center();
        self.click(c);
    }

    fn drag(&mut self, path: &[egui::Pos2]) {
        self.event(egui::Event::PointerMoved(path[0]));
        self.event(button(path[0], true));
        for p in &path[1..] {
            self.event(egui::Event::PointerMoved(*p));
        }
        self.event(button(*path.last().unwrap(), false));
        self.frames(3);
    }

    fn strokes(&self) -> Vec<waltcut_project::drawing::Stroke> {
        self.harness.state().session.project.items.values().find_map(|i| i.as_drawing()).unwrap().cels[0].strokes.to_vec()
    }

    fn untouched(&self) -> (usize, String) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.project.to_json())
    }

    fn status(&self) -> String {
        self.harness.state().ui.status.clone()
    }

    fn fills(&self) -> usize {
        self.strokes().iter().filter(|s| s.is_fill()).count()
    }
}

#[test]
fn the_fill_mode_command_toggles_sets_the_tolerance_and_refuses_bad_values_whole() {
    let mut d = Driver::new();
    assert_eq!((d.brush()["fill"].clone(), d.brush()["fill_tolerance"].clone()), (json!(false), json!(32)));
    assert_eq!(d.ui("drawing.fillMode", json!({})).unwrap(), json!({"fill": true, "tolerance": 32}));
    assert_eq!(d.ui("drawing.fillMode", json!({"tolerance": 90})).unwrap(), json!({"fill": true, "tolerance": 90}), "a tolerance alone keeps the mode");
    assert_eq!(d.ui("drawing.fillMode", json!({"enabled": false, "tolerance": 5})).unwrap(), json!({"fill": false, "tolerance": 5}));
    for bad in [
        json!({"tolerance": 256}),
        json!({"tolerance": -1}),
        json!({"tolerance": 1.5}),
        json!({"tolerance": "x"}),
        json!({"enabled": "yes"}),
        json!({"colour": 1}),
    ] {
        let before = d.brush();
        assert!(d.ui("drawing.fillMode", bad.clone()).is_err(), "{bad}");
        assert_eq!(d.brush(), before, "{bad} changes nothing");
    }
    // Lasso and Fill exclude each other
    d.ui("drawing.fillMode", json!({"enabled": true})).unwrap();
    d.ui("drawing.lassoMode", json!({"enabled": true})).unwrap();
    assert_eq!((d.brush()["fill"].clone(), d.brush()["lasso"].clone()), (json!(false), json!(true)));
    d.ui("drawing.fillMode", json!({"enabled": true})).unwrap();
    assert_eq!((d.brush()["fill"].clone(), d.brush()["lasso"].clone()), (json!(true), json!(false)));
    // the mode is never loaded back from saved interface state
    let saved = serde_json::to_value(&d.harness.state().ui.brush).unwrap();
    assert_eq!(saved["fill"], json!(true));
    let loaded: waltcut_ui_egui::panels::drawing::BrushSettings = serde_json::from_value(saved).unwrap();
    assert!(!loaded.fill, "a session starts drawing");
}

#[test]
fn a_click_inside_a_closed_shape_fills_it_as_one_undo_step() {
    let mut d = Driver::new();
    d.ui("drawing.fillMode", json!({"enabled": true})).unwrap();
    let before = d.untouched().0;
    let p = d.at(64.0, 36.0);
    d.click(p);
    assert_eq!(d.fills(), 1, "{}", d.status());
    assert_eq!(d.untouched().0, before + 1, "one undo step");
    assert!(d.status().starts_with("Filled "), "{}", d.status());
    let fill = d.strokes().pop().unwrap();
    let region = fill.fill.as_deref().unwrap();
    assert_eq!(region.seed, [64, 36]);
    assert!(region.filled_pixels() > 2000 && region.filled_pixels() < 128 * 72 / 2, "only the inside: {}", region.filled_pixels());
    assert_eq!(fill.color, d.harness.state().ui.brush.color, "the brush colour");
    // undo takes it away, the outline stays
    d.engine("edit.undo", json!({}));
    assert_eq!(d.strokes().len(), 1);
    // outside the shape fills the surround, and tolerance reaches the command
    d.ui("drawing.fillMode", json!({"tolerance": 7})).unwrap();
    // (the options bar covers the picture's top left corner, so a point down in the corner)
    let outside = d.at(6.0, 66.0);
    d.click(outside);
    let f = d.strokes().pop().unwrap();
    assert_eq!((f.fill.as_deref().unwrap().tolerance, f.fill.as_deref().unwrap().seed), (7, [6, 66]));
}

#[test]
fn a_drag_in_fill_mode_draws_nothing_and_the_options_bar_is_not_clicked_through() {
    let mut d = Driver::new();
    d.ui("drawing.fillMode", json!({"enabled": true})).unwrap();
    let before = d.untouched();
    let (a, b) = (d.at(40.0, 30.0), d.at(90.0, 45.0));
    d.drag(&[a, b]);
    assert_eq!(d.untouched(), before, "a drag neither draws nor fills");
    // the tolerance slider sits inside the picture's rectangle: using it fills nothing
    d.click_element("program.brush.fill.tolerance");
    assert_eq!(d.untouched().0, before.0, "a click on the options bar is not a click on the picture");
    assert_eq!(d.fills(), 0);
}

#[test]
fn the_controls_are_registered_and_the_newest_fill_can_be_recoloured_and_removed() {
    let mut d = Driver::new();
    // the toggle is there before the mode is on; the rest appear with it
    d.element("program.brush.fill");
    d.click_element("program.brush.fill");
    assert_eq!(d.brush()["fill"], json!(true), "the button turns the mode on");
    for id in ["program.brush.fill.tolerance", "program.brush.fill.recolor", "program.brush.fill.remove"] {
        d.element(id);
    }
    let p = d.at(64.0, 36.0);
    d.click(p);
    assert_eq!(d.fills(), 1);
    // recolour with the brush colour
    d.harness.state_mut().ui.brush.color = [0.1, 0.8, 0.2, 1.0];
    d.frames(2);
    let undo = d.untouched().0;
    d.click_element("program.brush.fill.recolor");
    let color = d.strokes().pop().unwrap().color;
    assert!((color[1] - 0.8).abs() < 1e-6, "{color:?}");
    assert_eq!(d.untouched().0, undo + 1, "one undo step");
    // remove the newest fill
    d.click_element("program.brush.fill.remove");
    assert_eq!(d.fills(), 0);
    assert_eq!(d.untouched().0, undo + 2);
    // with no fill left both are disabled: a click changes nothing
    let before = d.untouched();
    d.click_element("program.brush.fill.remove");
    d.click_element("program.brush.fill.recolor");
    assert_eq!(d.untouched(), before);
}

#[test]
fn a_locked_track_and_a_click_outside_the_canvas_say_why_and_change_nothing() {
    let mut d = Driver::new();
    d.ui("drawing.fillMode", json!({"enabled": true})).unwrap();
    let track = {
        let s = &d.harness.state().session;
        s.active_sequence().and_then(|q| q.find_item(waltcut_project::ClipId(d.clip))).map(|(t, _)| t.0).unwrap()
    };
    d.engine("timeline.setTrack", json!({"track": track, "locked": true}));
    let before = d.untouched();
    let p = d.at(64.0, 36.0);
    d.click(p);
    assert_eq!(d.untouched(), before, "nothing changed");
    assert!(d.status().contains("locked"), "{}", d.status());
    d.engine("timeline.setTrack", json!({"track": track, "locked": false}));
    // a point past the canvas is refused by the engine with its reason
    let before = d.untouched();
    let r = d.harness.state_mut().session.execute("drawing.fill", json!({"point": [500.0, 5.0]}));
    assert!(r.unwrap_err().to_string().contains("outside"));
    assert_eq!(d.untouched(), before);
}

#[test]
fn the_mode_has_a_key_that_nothing_else_uses() {
    let mut d = Driver::new();
    let items = d.call("ui.menu.list", json!({}))["result"].clone();
    let all = items.as_array().unwrap();
    let mine = all.iter().find(|m| m["id"] == "drawing.fillMode").expect("in the menu");
    assert_eq!(mine["shortcut"], json!("Alt+F"));
    assert_eq!(mine["enabled"], json!(true));
    let same: Vec<&str> = all.iter().filter(|m| m["shortcut"] == mine["shortcut"]).filter_map(|m| m["id"].as_str()).collect();
    assert_eq!(same, vec!["drawing.fillMode"], "no other command has Alt+F");
    // not available with another tool selected
    d.call("ui.set", json!({"tool": "Selection"}));
    d.frames(3);
    assert!(d.ui("drawing.fillMode", json!({})).is_err());
}
