//! Drawing view (#63): a turned or mirrored picture is a view setting. A stroke still lands where
//! the pen is, nothing is dirtied or exported differently, and the pan hold never draws.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;
use waltcut_ui_egui::panels::canvas_view::CanvasView;

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

fn key(k: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers }
}

impl Driver {
    /// A 320x180 sequence with an empty drawing clip, the Brush tool selected.
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.newSequence", json!({"name": "View", "fps": 24, "width": 320, "height": 180, "video": 2, "audio": 0})).unwrap();
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

    /// The screen rectangle bounding the picture (the brush canvas).
    fn bounds(&mut self) -> egui::Rect {
        let v = self.call("ui.elements", json!({"prefix": "program.brush"}))["result"].clone();
        let r = v.as_array().and_then(|a| a.iter().find(|e| e["id"] == "program.brush")).and_then(|e| e["rect"].as_array()).expect("brush canvas").clone();
        let f = |i: usize| r[i].as_f64().unwrap() as f32;
        egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)))
    }

    fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        self.event(egui::Event::PointerMoved(from));
        self.event(button(from, true));
        for i in 1..=8 {
            self.event(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        }
        self.event(button(to, false));
        self.frames(3);
    }

    fn strokes(&self) -> Vec<Vec<[f32; 2]>> {
        let p = &self.harness.state().session.project;
        p.items
            .values()
            .filter_map(|i| i.as_drawing())
            .flat_map(|d| d.cels.iter().flat_map(|c| c.strokes.iter().map(|s| s.points.iter().map(|q| [q.x, q.y]).collect())))
            .collect()
    }

    /// What a view setting must not change: undo history, revision and the project itself.
    fn untouched(&self) -> (usize, u64, String) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.revision, serde_json::to_string(&s.project).unwrap())
    }
}

/// The canvas point a screen point should land on, for a picture whose bounding rectangle is
/// `bounds`, shown under `view`: the layout rectangle has the frame's 16:9 shape.
fn expected_canvas(view: &CanvasView, bounds: egui::Rect, p: egui::Pos2) -> [f32; 2] {
    let rad = view.rotation.to_radians();
    let (c, s) = (rad.cos().abs(), rad.sin().abs());
    let unit = bounds.width() / (16.0 * c + 9.0 * s);
    let layout = egui::Rect::from_center_size(bounds.center(), egui::vec2(16.0 * unit, 9.0 * unit));
    let q = view.unshow(layout.center(), p);
    [(q.x - layout.min.x) / layout.width() * 320.0, (q.y - layout.min.y) / layout.height() * 180.0]
}

fn assert_lands(d: &mut Driver, view: CanvasView, label: &str) {
    let b = d.bounds();
    // a short stroke low in the picture, under the options bar, near its centre line so a turned
    // picture (whose corners are outside the bounds' corners) still contains it
    let (from, to) = (b.min + b.size() * egui::vec2(0.47, 0.74), b.min + b.size() * egui::vec2(0.53, 0.82));
    let before = d.strokes().len();
    d.drag(from, to);
    let strokes = d.strokes();
    assert_eq!(strokes.len(), before + 1, "{label}: a stroke was drawn");
    let pts = strokes.last().unwrap();
    let (first, last) = (pts.first().unwrap(), pts.last().unwrap());
    for (got, at) in [(first, from), (last, to)] {
        let want = expected_canvas(&view, b, at);
        assert!((got[0] - want[0]).abs() < 1.5 && (got[1] - want[1]).abs() < 1.5, "{label}: {got:?} vs {want:?}");
    }
}

#[test]
fn a_stroke_lands_under_the_pen_whatever_the_rotation_and_mirror() {
    for (rotation, mirror) in [(0.0, false), (90.0, false), (90.0, true), (-90.0, true), (180.0, false), (30.0, false), (-47.0, true)] {
        let mut d = Driver::new();
        d.command("view.canvasRotate", json!({"degrees": rotation})).unwrap();
        d.command("view.canvasMirror", json!({"enabled": mirror})).unwrap();
        d.frames(3);
        let view = CanvasView { rotation: rotation as f32, mirror, ..Default::default() };
        assert_lands(&mut d, view, &format!("{rotation}° mirror {mirror}"));
    }
}

#[test]
fn it_still_lands_when_the_picture_is_magnified_and_panned() {
    let mut d = Driver::new();
    d.command("view.canvasRotate", json!({"degrees": 90})).unwrap();
    d.command("view.canvasMirror", json!({"enabled": true})).unwrap();
    d.call("engine.execute", json!({"command": "view.magnification.200", "params": {}}));
    d.frames(3);
    let view = CanvasView { rotation: 90.0, mirror: true, ..Default::default() };
    // the magnified picture is larger than the viewport: aim at its centre, which the layout keeps
    let b = d.bounds();
    let (from, to) = (b.center() + egui::vec2(-20.0, -10.0), b.center() + egui::vec2(25.0, 15.0));
    d.drag(from, to);
    let pts = d.strokes().pop().expect("a stroke");
    for (got, at) in [(pts.first().unwrap(), from), (pts.last().unwrap(), to)] {
        let want = expected_canvas(&view, b, at);
        assert!((got[0] - want[0]).abs() < 1.5 && (got[1] - want[1]).abs() < 1.5, "{got:?} vs {want:?}");
    }
}

#[test]
fn view_changes_are_not_edits_and_do_not_change_the_rendered_frame() {
    let mut d = Driver::new();
    let b = d.bounds();
    d.drag(b.min + b.size() * egui::vec2(0.3, 0.4), b.min + b.size() * egui::vec2(0.7, 0.6));
    let render = |d: &Driver| {
        let s = &d.harness.state().session;
        let seq = s.state.active_sequence.expect("sequence");
        waltcut_render::render_sequence(
            &s.project,
            seq,
            waltcut_time::Tick::ZERO,
            waltcut_render::RenderOptions::default(),
            &waltcut_render::SourceMap::default(),
        )
    };
    let (before, frame) = (d.untouched(), render(&d));
    for (id, p) in [
        ("view.canvasRotateRight", json!({})),
        ("view.canvasRotateRight", json!({"degrees": 40})),
        ("view.canvasMirror", json!({})),
        ("view.canvasPan", json!({"active": true})),
        ("view.canvasPan", json!({"active": false})),
        ("view.canvasRotate", json!({"degrees": 123.5})),
    ] {
        d.command(id, p).unwrap();
        d.frames(2);
        assert_eq!(d.untouched(), before, "{id}");
        assert_eq!(render(&d).px, frame.px, "{id}: the exported frame is the same");
    }
    // undo does not know about the view: it undoes the stroke, and the view stays turned
    d.command("edit.undo", json!({})).unwrap();
    assert_eq!(d.strokes().len(), 0);
    assert_ne!(d.inspect()["ui"]["program"]["canvas"]["rotation"], 0.0);
}

#[test]
fn rotation_steps_wrap_and_reset_returns_to_the_documented_default() {
    let mut d = Driver::new();
    let r = d.command("view.canvasRotateRight", json!({})).unwrap();
    assert_eq!((r["rotation"].clone(), r["mirror"].clone()), (json!(15.0), json!(false)));
    d.command("view.canvasRotateLeft", json!({"degrees": 200})).unwrap();
    assert_eq!(d.inspect()["ui"]["program"]["canvas"]["rotation"], json!(175.0), "15 - 200 wraps to 175");
    d.command("view.canvasRotate", json!({"degrees": 180})).unwrap();
    d.command("view.canvasRotateRight", json!({"degrees": 1})).unwrap();
    assert_eq!(d.inspect()["ui"]["program"]["canvas"]["rotation"], json!(-179.0));
    d.command("view.canvasMirror", json!({})).unwrap();
    d.command("view.magnification.400", json!({})).unwrap();
    d.command("view.canvasReset", json!({})).unwrap();
    let ui = d.inspect()["ui"]["program"].clone();
    assert_eq!(
        (ui["canvas"]["rotation"].clone(), ui["canvas"]["mirror"].clone(), ui["zoom"].clone(), ui["pan"].clone()),
        (json!(0.0), json!(false), json!(null), json!([0.0, 0.0]))
    );
}

#[test]
fn bad_view_parameters_are_refused_and_change_nothing() {
    let mut d = Driver::new();
    d.command("view.canvasRotate", json!({"degrees": 33})).unwrap();
    let before = d.inspect()["ui"]["program"].clone();
    for (id, p) in [
        ("view.canvasRotate", json!({})),
        ("view.canvasRotate", json!({"degrees": "ninety"})),
        ("view.canvasRotate", json!({"degrees": 1e12})),
        ("view.canvasRotateRight", json!({"degrees": [1]})),
        ("view.canvasMirror", json!({"enabled": "yes"})),
        ("view.canvasPan", json!({"active": 1})),
        ("view.canvasReset", json!(5)),
    ] {
        assert!(d.command(id, p.clone()).is_err(), "{id} {p}");
        assert_eq!(d.inspect()["ui"]["program"], before, "{id} {p} changed something");
    }
}

#[test]
fn the_pan_hold_pans_instead_of_drawing_and_every_exit_releases_it() {
    let mut d = Driver::new();
    d.call("engine.execute", json!({"command": "view.magnification.400", "params": {}}));
    d.frames(3);
    let b = d.bounds();
    let (a, z) = (b.center(), b.center() + egui::vec2(-60.0, 25.0));
    d.command("view.canvasPan", json!({"active": true})).unwrap();
    d.frames(2);
    d.drag(a, z);
    assert_eq!(d.strokes().len(), 0, "a drag while the pan is held draws nothing");
    let pan = d.inspect()["ui"]["program"]["pan"].clone();
    assert!(pan[0].as_f64().unwrap() < -1.0 && pan[1].as_f64().unwrap() > 1.0, "the picture followed the pen: {pan}");
    // let go: drawing works again
    d.command("view.canvasPan", json!({"active": false})).unwrap();
    d.frames(2);
    let b = d.bounds();
    d.drag(b.center(), b.center() + egui::vec2(30.0, 10.0));
    assert_eq!(d.strokes().len(), 1);

    // Escape, focus loss and another tool all end a command-held pan
    for end in [0, 1, 2] {
        d.command("view.canvasPan", json!({"active": true})).unwrap();
        assert_eq!(d.inspect()["ui"]["program"]["canvas"]["panHeld"], json!("Command"));
        match end {
            0 => {
                d.event(key(egui::Key::Escape, true, egui::Modifiers::NONE));
                d.event(key(egui::Key::Escape, false, egui::Modifiers::NONE));
            }
            1 => d.event(egui::Event::WindowFocused(false)),
            _ => {
                d.call("ui.set", json!({"tool": "Hand"}));
                d.call("ui.set", json!({"tool": "Brush"}));
            }
        }
        d.frames(2);
        assert_eq!(d.inspect()["ui"]["program"]["canvas"]["panHeld"], json!("Off"), "exit {end}");
    }
}

#[test]
fn the_default_keys_exist_and_do_not_collide() {
    let d = Driver::new();
    let s = &d.harness.state().session.shortcuts;
    for (id, keys) in [
        ("view.canvasRotateRight", "Alt+."),
        ("view.canvasRotateLeft", "Alt+,"),
        ("view.canvasMirror", "Alt+M"),
        ("view.canvasPan", "Alt+H"),
        ("view.canvasReset", "Alt+0"),
    ] {
        assert_eq!(s.primary(id).as_deref(), Some(keys), "{id}");
    }
    let conflicts = s.conflicts(waltcut_engine::shortcuts::Platform::current());
    let ours: Vec<_> = conflicts.iter().filter(|c| c.2.iter().any(|id| id.starts_with("view.canvas") || id.starts_with("drawing."))).collect();
    assert!(ours.is_empty(), "collisions: {ours:?}");
}

#[test]
fn a_held_key_pans_for_as_long_as_it_is_down() {
    let mut d = Driver::new();
    let alt = egui::Modifiers::ALT;
    d.event(egui::Event::ModifiersChanged(alt));
    d.event(key(egui::Key::H, true, alt));
    assert_eq!(d.inspect()["ui"]["program"]["canvas"]["panHeld"], json!("Key"));
    d.frames(3);
    assert_eq!(d.inspect()["ui"]["program"]["canvas"]["panHeld"], json!("Key"), "still down");
    d.event(key(egui::Key::H, false, alt));
    d.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    d.frames(2);
    assert_eq!(d.inspect()["ui"]["program"]["canvas"]["panHeld"], json!("Off"));
}

#[test]
fn a_singular_view_state_cannot_crash_the_monitor() {
    let mut d = Driver::new();
    d.harness.state_mut().ui.program.canvas = CanvasView { rotation: f32::NAN, mirror: true, ..Default::default() };
    d.frames(4);
    let b = d.bounds();
    d.drag(b.center(), b.center() + egui::vec2(10.0, 5.0));
    d.harness.state_mut().ui.program.canvas = CanvasView { rotation: f32::INFINITY, ..Default::default() };
    d.frames(4);
}

#[test]
fn the_controls_have_automation_ids_in_the_menu() {
    let mut d = Driver::new();
    let items = d.call("ui.menu.list", json!({}))["result"].clone();
    let ids: Vec<&str> = items.as_array().unwrap().iter().filter_map(|i| i["id"].as_str()).collect();
    for id in ["view.canvasRotateRight", "view.canvasRotateLeft", "view.canvasRotate", "view.canvasMirror", "view.canvasPan", "view.canvasReset"] {
        assert!(ids.contains(&id), "{id}");
    }
}
