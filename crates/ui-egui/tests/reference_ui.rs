//! Reference images (#3) in the drawing workflow: one placement action from the Brush options bar
//! (picker or clipboard), adjustable controls with automation ids, and Move with pen.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::control::ControlRequest;
use waltcut_ui_egui::{ClipboardMedia, WaltCutApp};

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
    dir: std::path::PathBuf,
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

impl Driver {
    /// `picked`: what the open dialog answers (None = cancelled); `clipboard`: the clipboard picture.
    fn new(name: &str, picked: bool, clipboard: bool) -> Self {
        let dir = std::env::temp_dir().join("waltcut-ui-tests").join(format!("reference-ui-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let picture = dir.join("ref.png");
        std::fs::write(&picture, waltcut_export::encode_png([0u8, 0, 255, 255].repeat(64 * 36), 64, 36).unwrap()).unwrap();
        let mut s = Session::default();
        s.execute("file.newSequence", json!({"name": "R", "fps": 24, "width": 320, "height": 180, "video": 2, "audio": 0})).unwrap();
        let drawing = s.execute("drawing.new", json!({"seconds": 3.0})).unwrap()["clip"].as_u64().unwrap();
        s.execute("timeline.select", json!({"clips": [drawing]})).unwrap();
        let (tx, rx) = channel();
        let mut app = WaltCutApp::new(s).with_control(rx);
        let path = picture.to_string_lossy().into_owned();
        app.hooks.pick_open_file = Some(Box::new(move |_, _| picked.then(|| path.clone())));
        app.hooks.clipboard_media =
            Some(Box::new(move || clipboard.then(|| ClipboardMedia::Image { width: 8, height: 8, rgba: [255u8, 0, 0, 255].repeat(64) })));
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, dir };
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

    fn invoke(&mut self, id: &str) -> Value {
        let r = self.call("ui.menu.invoke", json!({"id": id}));
        self.frames(4);
        r
    }

    fn list(&mut self) -> Vec<Value> {
        let r = self.call("engine.execute", json!({"command": "reference.list", "params": {}}));
        r["result"]["references"].as_array().cloned().unwrap_or_default()
    }

    fn untouched(&self) -> (usize, String) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.project.to_json())
    }

    fn canvas(&mut self) -> egui::Rect {
        let v = self.call("ui.elements", json!({"prefix": "program.brush"}))["result"].clone();
        let r = v.as_array().and_then(|a| a.iter().find(|e| e["id"] == "program.brush")).and_then(|e| e["rect"].as_array()).expect("canvas").clone();
        let f = |i: usize| r[i].as_f64().unwrap() as f32;
        egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)))
    }

    fn event(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        self.event(egui::Event::PointerMoved(from));
        self.event(button(from, true));
        for i in 1..=6 {
            self.event(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 6.0)));
        }
        self.event(button(to, false));
        self.frames(3);
    }

    fn strokes(&self) -> usize {
        let p = &self.harness.state().session.project;
        p.items.values().filter_map(|i| i.as_drawing()).map(|d| d.cels.iter().map(|c| c.strokes.len()).sum::<usize>()).sum()
    }

    fn elements(&mut self) -> Vec<String> {
        self.call("ui.elements", json!({"prefix": "program.brush.reference"}))["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect()
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn one_menu_action_picks_a_picture_and_places_it_as_one_undo_step() {
    let mut d = Driver::new("place", true, false);
    let (depth, project) = d.untouched();
    d.invoke("reference.place");
    let refs = d.list();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!((refs[0]["opacity"].clone(), refs[0]["hidden"].clone(), refs[0]["locked"].clone()), (json!(50.0), json!(false), json!(false)));
    assert_eq!(d.untouched().0, depth + 1, "one undo step");
    d.invoke("edit.undo");
    assert_eq!(d.untouched(), (depth, project), "undo takes back the import, the track and the clip");
}

#[test]
fn a_cancelled_picker_and_an_empty_clipboard_change_nothing() {
    let mut d = Driver::new("cancel", false, false);
    let before = d.untouched();
    d.invoke("reference.place");
    assert_eq!(d.untouched(), before);
    assert!(!d.harness.state().ui.status.contains("error"), "{}", d.harness.state().ui.status);
    d.invoke("reference.pasteImage");
    assert_eq!(d.untouched(), before);
    assert!(d.harness.state().ui.status.contains("clipboard"), "{}", d.harness.state().ui.status);
}

#[test]
fn a_clipboard_picture_is_placed_like_a_file() {
    let mut d = Driver::new("paste", false, true);
    d.invoke("reference.pasteImage");
    assert_eq!(d.list().len(), 1);
}

#[test]
fn the_reference_controls_have_automation_ids_and_drive_the_commands() {
    let mut d = Driver::new("controls", true, false);
    d.call("ui.click", json!({"id": "program.brush.reference"}));
    d.frames(3);
    let ids = d.elements();
    assert!(ids.contains(&"program.brush.reference.place".to_string()) && ids.contains(&"program.brush.reference.paste".to_string()), "{ids:?}");
    // the Place Image… button, from the menu
    d.call("ui.click", json!({"id": "program.brush.reference.place"}));
    d.frames(4);
    assert_eq!(d.list().len(), 1);
    d.call("ui.click", json!({"id": "program.brush.reference"}));
    d.frames(3);
    let ids = d.elements();
    for id in ["list", "opacity", "scale", "rotation", "hide", "lock", "guide", "move", "nudge.left", "nudge.right", "nudge.up", "nudge.down"] {
        assert!(ids.contains(&format!("program.brush.reference.{id}")), "{id} in {ids:?}");
    }
    let x0 = d.list()[0]["x"].as_f64().unwrap();
    d.call("ui.click", json!({"id": "program.brush.reference.nudge.right"}));
    d.frames(3);
    assert_eq!(d.list()[0]["x"].as_f64().unwrap(), x0 + 10.0, "Nudge Right moves it ten pixels");
    d.call("ui.click", json!({"id": "program.brush.reference.hide"}));
    d.frames(3);
    assert_eq!(d.list()[0]["hidden"], json!(true));
    d.call("ui.click", json!({"id": "program.brush.reference.lock"}));
    d.frames(3);
    assert_eq!(d.list()[0]["locked"], json!(true));
}

#[test]
fn move_with_pen_drags_the_reference_not_a_stroke_and_each_drag_is_one_undo_step() {
    let mut d = Driver::new("move", true, false);
    d.invoke("reference.place");
    d.harness.state_mut().ui.brush.reference_move = true;
    d.frames(3);
    let b = d.canvas();
    let scale = 320.0 / b.width();
    let x0 = d.list()[0]["x"].as_f64().unwrap();
    let y0 = d.list()[0]["y"].as_f64().unwrap();
    let depth = d.untouched().0;
    let from = b.min + b.size() * egui::vec2(0.5, 0.7);
    d.drag(from, from + egui::vec2(40.0, 20.0));
    assert_eq!(d.strokes(), 0, "no stroke is drawn while moving the reference");
    let (x1, y1) = (d.list()[0]["x"].as_f64().unwrap(), d.list()[0]["y"].as_f64().unwrap());
    assert!(
        ((x1 - x0) - 40.0 * scale as f64).abs() < 2.0 && ((y1 - y0) - 20.0 * scale as f64).abs() < 2.0,
        "moved ({}, {}) for a (40, 20) drag at scale {scale}",
        x1 - x0,
        y1 - y0
    );
    assert_eq!(d.untouched().0, depth + 1, "one drag, one undo step");
    // a second drag is a step of its own
    d.drag(from, from + egui::vec2(-10.0, 0.0));
    assert_eq!(d.untouched().0, depth + 2);
    d.invoke("edit.undo");
    assert!((d.list()[0]["x"].as_f64().unwrap() - x1).abs() < 1e-6, "only the second drag is undone");
    // off: the pen draws again
    d.harness.state_mut().ui.brush.reference_move = false;
    d.frames(3);
    d.drag(from, from + egui::vec2(20.0, 5.0));
    assert_eq!(d.strokes(), 1);
}

#[test]
fn a_locked_reference_does_not_capture_the_pen() {
    let mut d = Driver::new("locked", true, false);
    d.invoke("reference.place");
    d.call("engine.execute", json!({"command": "reference.set", "params": {"locked": true}}));
    d.harness.state_mut().ui.brush.reference_move = true;
    d.frames(3);
    let b = d.canvas();
    let before = d.list()[0].clone();
    let from = b.min + b.size() * egui::vec2(0.5, 0.7);
    d.drag(from, from + egui::vec2(30.0, 10.0));
    assert_eq!(d.list()[0], before, "a locked reference is not moved");
}

#[test]
fn the_saved_ui_state_does_not_remember_move_with_pen() {
    let mut d = Driver::new("state", true, false);
    d.harness.state_mut().ui.brush.reference_move = true;
    let saved = serde_json::to_value(&d.harness.state().ui).unwrap();
    let loaded: waltcut_ui_egui::state::UiState = serde_json::from_value(saved).unwrap();
    assert!(!loaded.brush.reference_move, "a session starts drawing");
}
