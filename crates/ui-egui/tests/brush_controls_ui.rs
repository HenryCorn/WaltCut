//! Brush controls (#58): size commands with bounds, enabled state and parameter docs, their
//! default keys next to the clip-volume keys, and Erase While Held restoring the brush.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
    canvas: egui::Rect,
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

fn key(k: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers }
}

impl Driver {
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.newSequence", json!({"name": "Draw", "fps": 24, "width": 320, "height": 180, "video": 2, "audio": 0})).expect("sequence");
        session.execute("drawing.new", json!({"seconds": 3.0})).expect("drawing");
        let (tx, rx) = channel();
        let app = WaltCutApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, canvas: egui::Rect::NOTHING };
        d.frames(4);
        d.ok("ui.set", json!({"tool": "Brush"}));
        d.frames(4);
        let v = d.ok("ui.elements", json!({"prefix": "program.brush"}));
        let r = v.as_array().and_then(|a| a.iter().find(|e| e["id"] == "program.brush")).and_then(|e| e["rect"].as_array()).expect("brush canvas");
        let f = |i: usize| r.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        d.canvas = egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)));
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
        self.tx.send(req).expect("control");
        for _ in 0..600 {
            self.frames(1);
            if let Ok(r) = reply.try_recv() {
                return r;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params.clone());
        assert_eq!(r["ok"], true, "{method} {params}: {r}");
        r["result"].clone()
    }

    fn command(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let r = self.call("engine.execute", json!({"command": id, "params": params}));
        if r["ok"] == true { Ok(r["result"].clone()) } else { Err(r["error"].as_str().unwrap_or("").to_string()) }
    }

    fn at(&self, fx: f32, fy: f32) -> egui::Pos2 {
        self.canvas.min + egui::vec2(self.canvas.width() * fx, self.canvas.height() * fy)
    }

    fn event(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    fn brush(&mut self) -> Value {
        self.ok("ui.inspect", json!({}))["ui"]["brush"].clone()
    }

    /// Draw a short stroke with the left button; returns the brush of the last stroke of the drawing.
    fn draw(&mut self, y: f32) {
        let (a, b) = (self.at(0.2, y), self.at(0.7, y));
        self.event(egui::Event::PointerMoved(a));
        self.event(button(a, true));
        for k in 1..=6 {
            self.event(egui::Event::PointerMoved(a.lerp(b, k as f32 / 6.0)));
        }
        self.event(button(b, false));
        self.frames(3);
    }

    fn strokes(&self) -> Vec<waltcut_project::drawing::Brush> {
        self.harness
            .state()
            .session
            .project
            .items
            .values()
            .filter_map(|i| i.as_drawing())
            .flat_map(|d| d.cels.iter().flat_map(|c| c.strokes.iter().map(|s| s.brush)))
            .collect()
    }
}

#[test]
fn size_commands_stop_at_the_bounds_and_refuse_bad_parameters() {
    let mut d = Driver::new();
    assert_eq!(d.brush()["size"], 6.0);
    assert_eq!(d.command("drawing.brushSizeUp", json!({})).expect("up")["size"], 8.0);
    assert_eq!(d.command("drawing.brushSizeDown", json!({"steps": 2})).expect("down")["size"], 5.0);
    let mut last = Value::Null;
    for _ in 0..40 {
        last = d.command("drawing.brushSizeUp", json!({})).expect("up");
    }
    assert_eq!((last["size"].as_f64(), last["atLimit"].as_bool()), (Some(200.0), Some(true)), "{last}");
    for _ in 0..40 {
        last = d.command("drawing.brushSizeDown", json!({})).expect("down");
    }
    assert_eq!((last["size"].as_f64(), last["atLimit"].as_bool()), (Some(1.0), Some(true)), "{last}");
    for bad in [json!({"steps": 0}), json!({"steps": -1}), json!({"steps": 1000}), json!({"steps": "two"}), json!({"steps": 1.5})] {
        assert!(d.command("drawing.brushSizeUp", bad.clone()).is_err(), "{bad}");
    }
    assert_eq!(d.brush()["size"], 1.0, "refused calls change nothing");
    // size is a brush setting, not artwork: the project is not dirtied
    assert!(d.strokes().is_empty());
}

#[test]
fn size_commands_are_listed_with_docs_and_are_enabled_only_with_the_brush_tool() {
    let mut d = Driver::new();
    let item = |d: &mut Driver, id: &str| {
        d.ok("ui.menu.list", json!({})).as_array().and_then(|a| a.iter().find(|i| i["id"] == id).cloned()).unwrap_or_else(|| panic!("{id} not listed"))
    };
    for (id, label, key) in [
        ("drawing.brushSizeUp", "Increase Brush Size", "Alt+]"),
        ("drawing.brushSizeDown", "Decrease Brush Size", "Alt+["),
        ("drawing.eraseTemporarily", "Erase While Held", "Alt+E"),
    ] {
        let it = item(&mut d, id);
        assert_eq!(it["label"], label);
        assert_eq!(it["shortcut"], key);
        assert_eq!(it["enabled"], true);
        assert!(it["params"].as_str().is_some_and(|p| p.contains('{')), "{it}");
    }
    d.ok("ui.set", json!({"tool": "Selection"}));
    d.frames(2);
    for id in ["drawing.brushSizeUp", "drawing.brushSizeDown", "drawing.eraseTemporarily"] {
        assert_eq!(item(&mut d, id)["enabled"], false, "{id}");
        let e = d.command(id, json!({})).expect_err("refused");
        assert!(e.contains("Brush tool"), "{e}");
    }
    assert_eq!(d.brush()["size"], 6.0);
}

#[test]
fn the_default_keys_do_not_collide_and_the_clip_volume_keys_are_untouched() {
    let d = Driver::new();
    let s = &d.harness.state().session.shortcuts;
    let primary = |id: &str| s.primary(id);
    assert_eq!(primary("clip.volumeUp").as_deref(), Some("]"));
    assert_eq!(primary("clip.volumeDown").as_deref(), Some("["));
    assert_eq!(primary("trim.extendToPlayhead").as_deref(), Some("E"));
    let conflicts = s.conflicts(waltcut_engine::shortcuts::Platform::current());
    let ours: Vec<_> = conflicts.iter().filter(|c| c.2.iter().any(|id| id.starts_with("drawing."))).collect();
    assert!(ours.is_empty(), "the drawing keys collide: {ours:?}");
}

#[test]
fn the_size_keys_change_the_size_and_the_plain_bracket_keys_do_not() {
    let mut d = Driver::new();
    let alt = egui::Modifiers::ALT;
    d.event(egui::Event::ModifiersChanged(alt));
    d.event(key(egui::Key::CloseBracket, true, alt));
    d.event(key(egui::Key::CloseBracket, false, alt));
    assert_eq!(d.brush()["size"], 8.0);
    d.event(key(egui::Key::OpenBracket, true, alt));
    d.event(key(egui::Key::OpenBracket, false, alt));
    d.event(key(egui::Key::OpenBracket, true, alt));
    d.event(key(egui::Key::OpenBracket, false, alt));
    assert_eq!(d.brush()["size"], 5.0);
    d.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    d.event(key(egui::Key::CloseBracket, true, egui::Modifiers::NONE));
    d.event(key(egui::Key::CloseBracket, false, egui::Modifiers::NONE));
    assert_eq!(d.brush()["size"], 5.0, "] alone is still the clip volume key");
}

#[test]
fn erase_while_held_erases_and_then_brings_the_brush_back() {
    let mut d = Driver::new();
    let alt = egui::Modifiers::ALT;
    let before = d.brush();
    d.draw(0.3);
    d.event(egui::Event::ModifiersChanged(alt));
    d.event(key(egui::Key::E, true, alt));
    assert_eq!(d.brush()["erase_held"], "Key");
    d.draw(0.3);
    d.event(key(egui::Key::E, false, alt));
    d.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    d.frames(2);
    assert_eq!(d.brush()["erase_held"], "Off");
    d.draw(0.5);
    use waltcut_project::drawing::Brush;
    assert_eq!(d.strokes(), vec![Brush::Pen, Brush::Eraser, Brush::Pen], "the eraser stroke, then the Pen is back");
    let after = d.brush();
    for k in ["brush", "size", "color", "pressure", "cel_mode", "onion", "onion_opacity"] {
        assert_eq!(before[k], after[k], "{k} is unchanged");
    }
}

#[test]
fn erase_while_held_ends_on_focus_loss_tool_change_and_command_and_draws_nothing_by_itself() {
    use waltcut_project::drawing::Brush;
    let mut d = Driver::new();
    // as a command: on until off
    assert_eq!(d.command("drawing.eraseTemporarily", json!({})).expect("on")["erasing"], true);
    assert_eq!(d.brush()["erase_held"], "Command");
    d.draw(0.3);
    assert_eq!(d.strokes(), vec![Brush::Eraser]);
    // regression: a window driven over the control channel is never focused, and that must not
    // end a hold (only the loss of focus does)
    d.harness.input_mut().focused = false;
    d.frames(3);
    assert_eq!(d.brush()["erase_held"], "Command");
    d.harness.input_mut().focused = true;
    d.command("drawing.eraseTemporarily", json!({"active": false})).expect("off");
    assert_eq!(d.brush()["erase_held"], "Off");
    assert!(d.command("drawing.eraseTemporarily", json!({"active": "yes"})).is_err());
    // focus loss while a key hold is on, mid-stroke: the hold ends and the stroke is abandoned
    let alt = egui::Modifiers::ALT;
    d.event(egui::Event::ModifiersChanged(alt));
    d.event(key(egui::Key::E, true, alt));
    let (a, b, c) = (d.at(0.2, 0.6), d.at(0.5, 0.6), d.at(0.7, 0.6));
    d.event(egui::Event::PointerMoved(a));
    d.event(button(a, true));
    d.event(egui::Event::PointerMoved(b));
    d.event(egui::Event::PointerMoved(c));
    d.event(egui::Event::WindowFocused(false));
    d.event(button(c, false));
    d.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    d.frames(3);
    assert_eq!(d.brush()["erase_held"], "Off");
    assert_eq!(d.strokes(), vec![Brush::Eraser], "no new stroke from the abandoned one");
    // a hold does not survive leaving the Brush tool
    d.command("drawing.eraseTemporarily", json!({})).expect("on");
    d.ok("ui.set", json!({"tool": "Selection"}));
    d.frames(3);
    assert_eq!(d.brush()["erase_held"], "Off");
}
