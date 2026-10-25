//! Lasso select and transform whole strokes (#66) in the Brush tool: the lasso selects, a
//! pending transform is only a preview, apply is one undo step, cancel and Escape change nothing,
//! and a refused transform says why.

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

fn key(k: egui::Key) -> [egui::Event; 2] {
    [
        egui::Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: Default::default() },
        egui::Event::Key { key: k, physical_key: Some(k), pressed: false, repeat: false, modifiers: Default::default() },
    ]
}

fn line(brush: &str, size: f64, pts: &[(f64, f64)]) -> Value {
    json!({"brush": brush, "color": "#000000", "size": size, "points": pts.iter().map(|p| json!([p.0, p.1, 1.0])).collect::<Vec<_>>()})
}

impl Driver {
    /// 128x72: stroke 0 a line at y=20, stroke 1 at y=60, stroke 2 an eraser across stroke 0.
    fn new() -> Self {
        let mut s = Session::default();
        s.execute("file.newSequence", json!({"name": "Sel", "width": 128, "height": 72, "fps": 24, "video": 2, "audio": 0})).unwrap();
        let clip = s.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
        for stroke in [
            line("Pen", 4.0, &[(10.0, 20.0), (110.0, 20.0)]),
            line("Pen", 4.0, &[(10.0, 60.0), (110.0, 60.0)]),
            line("Eraser", 6.0, &[(60.0, 10.0), (60.0, 30.0)]),
        ] {
            s.execute("drawing.addStroke", json!({"clip": clip, "frame": 0, "stroke": stroke})).unwrap();
        }
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

    /// A UI command (menu route) with its result or error.
    fn ui(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let r = self.call("ui.menu.invoke", json!({"id": id, "params": params}));
        self.frames(2);
        if r["ok"] == true { Ok(r["result"].clone()) } else { Err(r["error"].as_str().unwrap_or("").to_string()) }
    }

    fn inspect(&mut self) -> Value {
        self.call("ui.inspect", json!({}))["result"].clone()
    }

    fn selection(&mut self) -> Value {
        self.inspect()["ui"]["brush"]["selection"].clone()
    }

    fn event(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    fn canvas(&mut self) -> egui::Rect {
        let v = self.call("ui.elements", json!({"prefix": "program.brush"}))["result"].clone();
        let r = v.as_array().and_then(|a| a.iter().find(|e| e["id"] == "program.brush")).and_then(|e| e["rect"].as_array()).expect("canvas").clone();
        let f = |i: usize| r[i].as_f64().unwrap() as f32;
        egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)))
    }

    /// The screen point of canvas pixel (x, y).
    fn at(&mut self, x: f32, y: f32) -> egui::Pos2 {
        let b = self.canvas();
        b.min + egui::vec2(x / 128.0 * b.width(), y / 72.0 * b.height())
    }

    fn drag_path(&mut self, path: &[egui::Pos2]) {
        self.event(egui::Event::PointerMoved(path[0]));
        self.event(button(path[0], true));
        for p in &path[1..] {
            self.event(egui::Event::PointerMoved(*p));
        }
        self.event(button(*path.last().unwrap(), false));
        self.frames(3);
    }

    /// Lasso a canvas rectangle.
    fn lasso(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let path = [self.at(x0, y0), self.at(x1, y0), self.at(x1, y1), self.at(x0, y1), self.at(x0 + 0.5, y0 + 0.5)];
        self.drag_path(&path);
    }

    fn strokes(&self) -> Vec<waltcut_project::drawing::Stroke> {
        self.harness.state().session.project.items.values().find_map(|i| i.as_drawing()).unwrap().cels[0].strokes.to_vec()
    }

    fn untouched(&self) -> (usize, String) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.project.to_json())
    }

    fn elements(&mut self) -> Vec<String> {
        self.call("ui.elements", json!({"prefix": "program.brush.lasso"}))["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect()
    }
}

#[test]
fn a_lasso_drag_selects_the_strokes_and_the_erasers_that_belong_to_them_without_drawing() {
    let mut d = Driver::new();
    let before = d.untouched();
    assert_eq!(d.ui("drawing.lassoMode", json!({})).unwrap()["lasso"], json!(true));
    d.lasso(2.0, 8.0, 124.0, 34.0);
    let sel = d.selection();
    assert_eq!((sel["strokes"].clone(), sel["erasers"].clone(), sel["conflict"].clone()), (json!([0]), json!([2]), Value::Null), "{sel}");
    assert_eq!(d.untouched(), before, "selecting draws nothing and edits nothing");
    // a lasso around nothing clears it; Escape clears a selection
    d.lasso(2.0, 40.0, 124.0, 44.0);
    assert_eq!(d.selection(), Value::Null);
    d.lasso(2.0, 8.0, 124.0, 34.0);
    assert!(d.selection().is_object());
    for e in key(egui::Key::Escape) {
        d.event(e);
    }
    d.frames(2);
    assert_eq!(d.selection(), Value::Null, "Escape drops the selection");
    // lasso mode off: the pen draws again
    d.ui("drawing.lassoMode", json!({"enabled": false})).unwrap();
    let (a, b) = (d.at(20.0, 40.0), d.at(60.0, 45.0));
    d.drag_path(&[a, b]);
    assert_eq!(d.strokes().len(), 4);
}

#[test]
fn a_pending_transform_is_a_preview_that_apply_makes_one_undo_step_and_cancel_drops() {
    let mut d = Driver::new();
    d.ui("drawing.lassoMode", json!({})).unwrap();
    d.lasso(2.0, 8.0, 124.0, 34.0);
    let before = d.untouched();
    let r = d.ui("drawing.selectionTransform", json!({"dy": 20})).unwrap();
    assert_eq!((r["pending"]["dy"].clone(), r["refused"].clone()), (json!(20.0), Value::Null), "{r}");
    d.frames(3);
    assert_eq!(d.untouched(), before, "the pending transform edits nothing");
    // cancel changes nothing and drops the pending numbers
    d.ui("drawing.selectionTransform", json!({"cancel": true})).unwrap();
    assert_eq!(d.selection()["pending"]["dy"], json!(0.0));
    assert_eq!(d.untouched(), before);
    // apply: one undo step, selection gone
    d.ui("drawing.selectionTransform", json!({"dy": 20, "rotation": 0})).unwrap();
    d.ui("drawing.selectionTransform", json!({"apply": true})).unwrap();
    let (depth, _) = d.untouched();
    assert_eq!(depth, before.0 + 1);
    let moved = d.strokes();
    assert!((moved[0].points[0].y - 40.0).abs() < 1e-3 && (moved[2].points[0].y - 30.0).abs() < 1e-3 && (moved[1].points[0].y - 60.0).abs() < 1e-3);
    assert_eq!(d.selection(), Value::Null);
    d.ui("edit.undo", json!({})).unwrap();
    assert_eq!(d.untouched(), before, "one undo restores the project exactly");
}

#[test]
fn dragging_inside_the_selection_moves_the_pending_transform_and_alt_enter_applies_it() {
    let mut d = Driver::new();
    d.ui("drawing.lassoMode", json!({})).unwrap();
    // the lower line, away from the options bar that covers the top of the picture
    d.lasso(2.0, 50.0, 124.0, 70.0);
    assert_eq!(d.selection()["strokes"], json!([1]));
    let before = d.untouched();
    let (a, b) = (d.at(30.0, 60.0), d.at(30.0, 45.0));
    let path: Vec<egui::Pos2> = (0..=10).map(|i| a + (b - a) * (i as f32 / 10.0)).collect();
    d.drag_path(&path);
    let sel = d.selection();
    assert!((sel["pending"]["dy"].as_f64().unwrap() + 15.0).abs() < 1.5, "dragged 15 canvas pixels up: {sel}");
    assert_eq!(d.untouched(), before, "still only a preview");
    // the apply chord (Return itself is Render In to Out)
    let alt = egui::Modifiers::ALT;
    d.event(egui::Event::ModifiersChanged(alt));
    d.event(egui::Event::Key { key: egui::Key::Enter, physical_key: Some(egui::Key::Enter), pressed: true, repeat: false, modifiers: alt });
    d.event(egui::Event::Key { key: egui::Key::Enter, physical_key: Some(egui::Key::Enter), pressed: false, repeat: false, modifiers: alt });
    d.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    d.frames(2);
    assert_eq!(d.untouched().0, before.0 + 1, "Alt+Enter applies");
    assert_eq!(d.strokes().len(), 3, "no stroke was drawn by the drags");
    assert_eq!(d.selection(), Value::Null);
}

#[test]
fn escape_drops_the_pending_transform_first_and_the_selection_second() {
    let mut d = Driver::new();
    d.ui("drawing.lassoMode", json!({})).unwrap();
    d.lasso(2.0, 8.0, 124.0, 34.0);
    d.ui("drawing.selectionTransform", json!({"dx": 10})).unwrap();
    for e in key(egui::Key::Escape) {
        d.event(e);
    }
    d.frames(2);
    assert_eq!(d.selection()["pending"]["dx"], json!(0.0));
    assert!(d.selection().is_object(), "the selection stays");
    for e in key(egui::Key::Escape) {
        d.event(e);
    }
    d.frames(2);
    assert_eq!(d.selection(), Value::Null);
}

#[test]
fn a_refused_transform_says_why_and_cannot_be_applied() {
    let mut d = Driver::new();
    d.ui("drawing.lassoMode", json!({})).unwrap();
    d.lasso(2.0, 8.0, 124.0, 34.0);
    let before = d.untouched();
    // onto the second line: the eraser would erase it
    let r = d.ui("drawing.selectionTransform", json!({"dy": 40})).unwrap();
    assert!(r["refused"].as_str().is_some_and(|s| s.contains("erase stroke 1")), "{r}");
    let e = d.ui("drawing.selectionTransform", json!({"apply": true})).unwrap_err();
    assert!(e.contains("erase stroke 1"), "{e}");
    assert_eq!(d.untouched(), before);
    // bad numbers are refused whole
    for bad in [
        json!({"scale": 0}),
        json!({"scale": 1000}),
        json!({"rotation": "x"}),
        json!({"dx": 1e12}),
        json!({"apply": true, "cancel": true}),
        json!({"apply": 1}),
    ] {
        assert!(d.ui("drawing.selectionTransform", bad.clone()).is_err(), "{bad}");
    }
    assert_eq!(d.untouched(), before);
    // nothing to apply
    d.ui("drawing.selectionTransform", json!({"cancel": true})).unwrap();
    assert!(d.ui("drawing.selectionTransform", json!({"apply": true})).is_err());
}

#[test]
fn an_ambiguous_selection_is_flagged_and_a_locked_track_refuses_the_apply() {
    let mut d = Driver::new();
    // an eraser across both lines
    d.harness
        .state_mut()
        .session
        .execute("drawing.addStroke", json!({"clip": d.clip, "frame": 0, "stroke": line("Eraser", 6.0, &[(30.0, 10.0), (30.0, 65.0)])}))
        .unwrap();
    d.ui("drawing.lassoMode", json!({})).unwrap();
    d.frames(2);
    d.lasso(2.0, 8.0, 124.0, 34.0);
    let sel = d.selection();
    assert!(sel["conflict"].as_str().is_some_and(|c| c.contains("crosses")), "{sel}");
    let e = d.ui("drawing.selectionTransform", json!({"dx": 4, "apply": true})).unwrap_err();
    assert!(e.contains("crosses"), "{e}");
    // select both lines: fine
    d.lasso(2.0, 8.0, 124.0, 70.0);
    assert_eq!(d.selection()["conflict"], Value::Null);
    // lock the drawing's track: the engine refuses and nothing changes
    let seq = d.harness.state().session.state.active_sequence.unwrap();
    let ti = d.harness.state().session.active_sequence().unwrap().video_tracks.iter().position(|t| !t.items.is_empty()).unwrap();
    std::sync::Arc::make_mut(&mut d.harness.state_mut().session.project).sequence_mut(seq).unwrap().video_tracks[ti].locked = true;
    d.frames(2);
    d.lasso(2.0, 8.0, 124.0, 70.0);
    let before = d.untouched();
    d.ui("drawing.selectionTransform", json!({"dx": 4})).unwrap();
    let e = d.ui("drawing.selectionTransform", json!({"apply": true})).unwrap_err();
    assert!(e.contains("locked"), "{e}");
    assert_eq!(d.untouched(), before);
}

#[test]
fn the_selection_goes_when_the_project_changes_under_it() {
    let mut d = Driver::new();
    d.ui("drawing.lassoMode", json!({})).unwrap();
    d.lasso(2.0, 8.0, 124.0, 34.0);
    assert!(d.selection().is_object());
    d.call(
        "engine.execute",
        json!({"command": "drawing.addStroke", "params": {"clip": d.clip, "frame": 0, "stroke": line("Pen", 3.0, &[(5.0, 5.0), (9.0, 9.0)])}}),
    );
    d.frames(3);
    assert_eq!(d.selection(), Value::Null, "an edit invalidates the stroke numbers");
}

#[test]
fn the_controls_have_automation_ids_and_none_of_this_is_remembered() {
    let mut d = Driver::new();
    d.ui("drawing.lassoMode", json!({})).unwrap();
    d.lasso(2.0, 8.0, 124.0, 34.0);
    d.frames(2);
    let ids = d.elements();
    for id in ["lasso", "lasso.status", "lasso.dx", "lasso.dy", "lasso.rotation", "lasso.scale", "lasso.apply", "lasso.cancel", "lasso.clear"] {
        assert!(ids.contains(&format!("program.brush.{id}")), "{id} in {ids:?}");
    }
    d.ui("drawing.selectionTransform", json!({"dy": 12})).unwrap();
    d.frames(2);
    d.call("ui.click", json!({"id": "program.brush.lasso.apply"}));
    d.frames(3);
    assert_eq!(d.selection(), Value::Null, "the Apply button applied it");
    assert!((d.strokes()[0].points[0].y - 32.0).abs() < 1e-3);
    // saved UI state neither writes the selection back on nor breaks loading
    d.lasso(2.0, 8.0, 124.0, 50.0);
    let saved = serde_json::to_value(&d.harness.state().ui).unwrap();
    let loaded: waltcut_ui_egui::state::UiState = serde_json::from_value(saved).unwrap();
    assert!(!loaded.brush.lasso && loaded.brush.selection.is_none());
}

#[test]
fn the_apply_chord_exists_and_does_not_collide() {
    let d = Driver::new();
    let s = &d.harness.state().session.shortcuts;
    assert_eq!(s.primary("drawing.selectionApply").as_deref(), Some("Alt+Enter"));
    assert_eq!(s.primary("drawing.lassoMode").as_deref(), Some("Alt+L"));
    let conflicts = s.conflicts(waltcut_engine::shortcuts::Platform::current());
    let ours: Vec<_> = conflicts.iter().filter(|c| c.2.iter().any(|id| id.starts_with("drawing."))).collect();
    assert!(ours.is_empty(), "collisions: {ours:?}");
}
