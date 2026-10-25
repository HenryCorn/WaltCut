//! Light table (#64): the pinned comparison pose can be moved and turned to check proportions. It
//! is a view of the pose only: nothing is edited, dirtied, saved or exported, and the current cel
//! stays drawable and correctly mapped.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;
use waltcut_ui_egui::panels::drawing_raster::SharedSlot;

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
}

fn button(pos: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers }
}

fn key(k: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers }
}

impl Driver {
    /// Cels at frames 0, 8 and 16 of a two-second 320x180 drawing, the playhead on frame 9 (inside
    /// the hold of cel 8), the Brush tool selected and the cel at frame 16 pinned.
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.newSequence", json!({"name": "Light", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 0})).unwrap();
        let clip = session.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
        for frame in [0, 8, 16] {
            session.execute("drawing.newCel", json!({"clip": clip, "frame": frame})).unwrap();
            let x = frame as f64;
            let stroke = json!({"brush": "Pen", "color": "#000000", "size": 4.0, "points": [[10.0, 10.0, 1.0], [40.0 + x, 30.0, 1.0]]});
            session.execute("drawing.addStroke", json!({"clip": clip, "frame": frame, "stroke": stroke})).unwrap();
        }
        session.execute("playhead.set", json!({"frame": 9})).unwrap();
        session.execute("timeline.select", json!({"clips": [clip]})).unwrap();
        let (tx, rx) = channel();
        let app = WaltCutApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.command("tool.brush", json!({})).unwrap();
        // the cel before and the pinned pose show; nothing after, so the pin is its own ghost
        d.command("drawing.onionSkin", json!({"pin": 16, "before": 1, "after": 0})).unwrap();
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

    fn table(&mut self) -> Value {
        self.inspect()["ui"]["brush"]["light_table"].clone()
    }

    fn shift(&mut self) -> (f64, f64, f64) {
        let t = self.table();
        (t["shift"]["dx"].as_f64().unwrap(), t["shift"]["dy"].as_f64().unwrap(), t["shift"]["rotation"].as_f64().unwrap())
    }

    fn event(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    /// The screen rectangle of the picture (the brush canvas).
    fn bounds(&mut self) -> egui::Rect {
        let v = self.call("ui.elements", json!({"prefix": "program.brush"}))["result"].clone();
        let r = v.as_array().and_then(|a| a.iter().find(|e| e["id"] == "program.brush")).and_then(|e| e["rect"].as_array()).expect("brush canvas").clone();
        let f = |i: usize| r[i].as_f64().unwrap() as f32;
        egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)))
    }

    fn drag_with(&mut self, from: egui::Pos2, to: egui::Pos2, modifiers: egui::Modifiers) {
        self.event(egui::Event::ModifiersChanged(modifiers));
        self.event(egui::Event::PointerMoved(from));
        self.event(button(from, true, modifiers));
        for i in 1..=8 {
            self.event(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        }
        self.event(button(to, false, modifiers));
        self.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        self.frames(3);
    }

    fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        self.drag_with(from, to, egui::Modifiers::NONE);
    }

    fn strokes(&self) -> usize {
        let p = &self.harness.state().session.project;
        p.items.values().filter_map(|i| i.as_drawing()).map(|d| d.cels.iter().map(|c| c.strokes.len()).sum::<usize>()).sum()
    }

    /// What a view setting must not change: undo history, revision, dirty flag and the project itself.
    fn untouched(&self) -> (usize, u64, bool, String) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.revision, s.is_dirty(), serde_json::to_string(&s.project).unwrap())
    }

    /// The first frame of the sequence as the exporter renders it.
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

    /// Where the pinned ghost is placed on screen: the transform its texture was made with.
    fn pin_placement(&mut self) -> [f64; 6] {
        self.placement("pin")
    }

    /// The ghost rasters are made on a worker: wait until what is shown stops changing.
    fn placement(&mut self, slot: &str) -> [f64; 6] {
        let slot = self.harness.ctx.data_mut(|d| d.get_temp::<SharedSlot>(egui::Id::new(("brush-onion", slot)))).expect("the ghost has a slot");
        let (mut last, mut same) = (None, 0);
        for _ in 0..400 {
            self.frames(2);
            std::thread::sleep(std::time::Duration::from_millis(4));
            let now = slot.0.lock().unwrap().shown_key().map(|k| k.placement);
            same = if now.is_some() && now == last { same + 1 } else { 0 };
            last = now;
            if same >= 8 {
                break;
            }
        }
        last.expect("the ghost is shown").map(f64::from_bits)
    }
}

fn apply(m: &[f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

#[test]
fn a_shift_is_a_view_of_the_pose_and_changes_nothing_stored_or_exported() {
    let mut d = Driver::new();
    let (before, frame) = (d.untouched(), d.render());
    for (id, p) in [
        ("drawing.lightTable", json!({"dx": 25, "dy": -12, "rotation": 15})),
        ("drawing.lightTableShift", json!({"active": true})),
        ("drawing.lightTableShift", json!({"active": false})),
        ("drawing.lightTable", json!({"rotation": 200})),
        ("drawing.lightTableReset", json!({})),
        ("drawing.lightTable", json!({"dx": 5})),
    ] {
        d.command(id, p.clone()).unwrap();
        d.frames(2);
        assert_eq!(d.untouched(), before, "{id} {p}: no undo step, revision, dirty flag or project change");
        assert_eq!(d.render(), frame, "{id} {p}: the exported frame is the same");
    }
    // the pose's source cel is where it was drawn
    let s = &d.harness.state().session;
    let stroke = s.project.items.values().filter_map(|i| i.as_drawing()).flat_map(|d| d.cels.iter()).find(|c| c.frame == 16).unwrap().strokes[0].points[1];
    assert_eq!((stroke.x, stroke.y), (56.0, 30.0));
    // undo knows nothing of it
    assert!(d.command("edit.undo", json!({})).is_err() || d.untouched().0 == before.0.saturating_sub(1));
}

#[test]
fn the_ghost_is_placed_by_the_shift_about_the_middle_of_the_canvas() {
    let mut d = Driver::new();
    let home = d.pin_placement();
    d.command("drawing.lightTable", json!({"dx": 20, "dy": -10, "rotation": 90})).unwrap();
    let moved = d.pin_placement();
    // canvas point p lands where the unshifted placement puts turn(p) + d about (160, 90)
    for (x, y) in [(10.0, 10.0), (56.0, 30.0), (160.0, 90.0), (300.0, 170.0)] {
        let (rx, ry) = (x - 160.0, y - 90.0);
        // 90° clockwise on a y-down canvas: (rx, ry) → (-ry, rx)
        let want = apply(&home, 160.0 - ry + 20.0, 90.0 + rx - 10.0);
        let got = apply(&moved, x, y);
        assert!((want.0 - got.0).abs() < 1e-3 && (want.1 - got.1).abs() < 1e-3, "{x},{y}: {want:?} vs {got:?}");
    }
    // reset: back where it was drawn
    d.command("drawing.lightTableReset", json!({})).unwrap();
    assert_eq!(d.pin_placement(), home);
}

#[test]
fn only_the_pinned_pose_moves_and_the_command_reports_what_is_shown() {
    let mut d = Driver::new();
    let r = d.command("drawing.lightTable", json!({"dx": 7, "rotation": -30})).unwrap();
    assert_eq!((r["dx"].clone(), r["dy"].clone(), r["rotation"].clone(), r["shifted"].clone()), (json!(7.0), json!(0.0), json!(-30.0), json!(true)));
    assert_eq!((r["pinned"].clone(), r["showing"].clone(), r["pose"]["frame"].clone()), (json!(true), json!(true), json!(16)));
    // each setting alone keeps the others; rotation wraps
    let r = d.command("drawing.lightTable", json!({"dy": 3, "rotation": 190})).unwrap();
    assert_eq!((r["dx"].clone(), r["dy"].clone(), r["rotation"].clone()), (json!(7.0), json!(3.0), json!(-170.0)));
    // with no parameter it reports
    assert_eq!(d.command("drawing.lightTable", json!({})).unwrap()["dx"], json!(7.0));
    // the neighbouring ghost is not shifted: the pinned cel's slot is the only one that moved
    let (pin, before) = (d.pin_placement(), d.placement("before1"));
    d.command("drawing.lightTableReset", json!({})).unwrap();
    assert_ne!(d.pin_placement(), pin);
    assert_eq!(d.placement("before1"), before);
    // a pinned cel that is also the next cel is shifted in its place as the next cel
    d.command("drawing.onionSkin", json!({"after": 1})).unwrap();
    d.command("drawing.lightTable", json!({"dx": 7})).unwrap();
    let moved = d.placement("after1");
    d.command("drawing.lightTableReset", json!({})).unwrap();
    assert_ne!(d.placement("after1"), moved);
    d.command("drawing.onionSkin", json!({"after": 0})).unwrap();
    // standing on the pinned cel itself, nothing shows to shift
    d.command("drawing.lightTable", json!({"dx": 7})).unwrap();
    d.command("playhead.set", json!({"frame": 17})).unwrap();
    d.frames(3);
    assert_eq!(d.command("drawing.lightTable", json!({})).unwrap()["showing"], json!(false));
}

#[test]
fn without_a_pinned_pose_there_is_nothing_to_shift() {
    let mut d = Driver::new();
    d.command("drawing.onionSkin", json!({"pin": null})).unwrap();
    let before = d.table();
    for (id, p) in [("drawing.lightTable", json!({"dx": 5})), ("drawing.lightTableShift", json!({"active": true}))] {
        let e = d.command(id, p).unwrap_err();
        assert!(e.contains("pin a pose"), "{e}");
        assert_eq!(d.table(), before);
    }
    // reporting and resetting are always fine
    assert_eq!(d.command("drawing.lightTable", json!({})).unwrap()["pinned"], json!(false));
    d.command("drawing.lightTableReset", json!({})).unwrap();
    // the hold needs Onion skin on: the pose is hidden without it
    d.command("drawing.onionSkin", json!({"pin": 16, "enabled": false})).unwrap();
    assert!(d.command("drawing.lightTableShift", json!({"active": true})).unwrap_err().contains("Onion skin"));
}

#[test]
fn bad_parameters_are_refused_whole_and_change_nothing() {
    let mut d = Driver::new();
    d.command("drawing.lightTable", json!({"dx": 4, "dy": 5, "rotation": 6})).unwrap();
    let before = d.table();
    for (id, p) in [
        ("drawing.lightTable", json!({"dx": "far"})),
        ("drawing.lightTable", json!({"dx": 1e12})),
        ("drawing.lightTable", json!({"dy": 1e300})),
        ("drawing.lightTable", json!({"rotation": 1e9})),
        ("drawing.lightTable", json!({"dx": [1]})),
        ("drawing.lightTable", json!({"dx": 9, "rotation": "up"})),
        ("drawing.lightTable", json!({"reset": "yes"})),
        ("drawing.lightTable", json!({"reset": true, "dx": 1})),
        ("drawing.lightTable", json!({"angle": 1})),
        ("drawing.lightTable", json!(5)),
        ("drawing.lightTableShift", json!({"active": 1})),
        ("drawing.lightTableReset", json!([1])),
    ] {
        assert!(d.command(id, p.clone()).is_err(), "{id} {p}");
        assert_eq!(d.table(), before, "{id} {p} changed something");
    }
    // a hand-edited UI state cannot send the pose to infinity or crash the monitor
    d.harness.state_mut().ui.brush.light_table.shift.dx = f64::NAN;
    d.harness.state_mut().ui.brush.light_table.shift.rotation = f64::INFINITY;
    d.harness.state_mut().ui.brush.light_table.shift.dy = 1e300;
    d.frames(4);
    let _ = d.pin_placement();
    let b = d.bounds();
    d.drag(b.center(), b.center() + egui::vec2(10.0, 5.0));
}

#[test]
fn a_drag_moves_the_pose_with_the_pen_and_never_draws_while_the_hold_is_on() {
    let mut d = Driver::new();
    d.command("drawing.lightTableShift", json!({"active": true})).unwrap();
    d.frames(2);
    assert_eq!(d.table()["hold"], json!("Command"));
    let (before, strokes) = (d.untouched(), d.strokes());
    let b = d.bounds();
    // the picture is 320 canvas pixels wide
    let k = 320.0 / b.width();
    let (from, to) = (b.center() + egui::vec2(-20.0, 10.0), b.center() + egui::vec2(40.0, -25.0));
    d.drag(from, to);
    let (dx, dy, rot) = d.shift();
    assert!((dx - 60.0 * k as f64).abs() < 1.0 && (dy + 35.0 * k as f64).abs() < 1.0 && rot == 0.0, "{dx} {dy} {rot}");
    assert_eq!(d.strokes(), strokes, "a drag while the pose is held draws nothing");
    assert_eq!(d.untouched(), before);
    // Shift-drag turns it about the middle of the canvas: a quarter turn clockwise
    d.command("drawing.lightTableReset", json!({})).unwrap();
    d.command("drawing.lightTableShift", json!({"active": true})).unwrap();
    d.frames(2);
    let c = b.center();
    d.drag_with(c + egui::vec2(60.0, 0.0), c + egui::vec2(0.0, 60.0), egui::Modifiers::SHIFT);
    let (dx, dy, rot) = d.shift();
    assert!((rot - 90.0).abs() < 8.0 && dx == 0.0 && dy == 0.0, "{dx} {dy} {rot}");
}

#[test]
fn the_pose_follows_the_pen_in_a_turned_and_mirrored_view() {
    for (rotation, mirror) in [(90.0, false), (90.0, true), (-30.0, true), (180.0, false)] {
        let mut d = Driver::new();
        d.command("view.canvasRotate", json!({"degrees": rotation})).unwrap();
        d.command("view.canvasMirror", json!({"enabled": mirror})).unwrap();
        d.command("drawing.lightTableShift", json!({"active": true})).unwrap();
        d.frames(3);
        let b = d.bounds();
        // low in the picture, under the options bar
        let from = b.min + b.size() * egui::vec2(0.45, 0.78);
        let to = from + egui::vec2(40.0, 15.0);
        let ghost_before = {
            d.command("drawing.lightTable", json!({"dx": 0.0})).unwrap();
            d.pin_placement()
        };
        d.drag(from, to);
        let (dx, dy, _) = d.shift();
        let moved = d.pin_placement();
        // a canvas point lands on the screen where the pen travelled: the screen shift of the
        // ghost is the pen's travel, whatever the view
        let (p0, p1) = (apply(&ghost_before, 100.0, 60.0), apply(&moved, 100.0, 60.0));
        assert!(
            ((p1.0 - p0.0) - 40.0).abs() < 2.0 && ((p1.1 - p0.1) - 15.0).abs() < 2.0,
            "{rotation} {mirror}: ghost travelled ({}, {}) with a pen travel of (40, 15), shift ({dx}, {dy})",
            p1.0 - p0.0,
            p1.1 - p0.1
        );
    }
}

#[test]
fn the_current_cel_stays_drawable_and_correctly_mapped_with_a_shifted_pose() {
    let mut d = Driver::new();
    let b = d.bounds();
    let (from, to) = (b.min + b.size() * egui::vec2(0.40, 0.70), b.min + b.size() * egui::vec2(0.50, 0.80));
    // draw with nothing shifted, then with a shifted and turned pose: the same pen path makes the same stroke
    let stroke_of = |d: &Driver| {
        let p = &d.harness.state().session.project;
        let cel = p.items.values().filter_map(|i| i.as_drawing()).flat_map(|d| d.cels.iter()).find(|c| c.frame == 8).unwrap();
        cel.strokes.last().unwrap().points.iter().map(|q| (q.x, q.y)).collect::<Vec<_>>()
    };
    d.drag(from, to);
    let plain = stroke_of(&d);
    d.command("drawing.lightTable", json!({"dx": 40, "dy": -20, "rotation": 33})).unwrap();
    d.frames(3);
    d.drag(from, to);
    let shifted = stroke_of(&d);
    assert_eq!(plain.len(), shifted.len());
    for (a, b) in plain.iter().zip(&shifted) {
        assert!((a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() < 0.5, "{a:?} {b:?}");
    }
    // the shift survived drawing
    assert_eq!(d.shift(), (40.0, -20.0, 33.0));
    // and the stroke is an ordinary undo step that leaves the shift alone
    d.command("edit.undo", json!({})).unwrap();
    assert_eq!(d.shift(), (40.0, -20.0, 33.0));
}

#[test]
fn letting_go_keeps_the_alignment_but_escape_and_focus_loss_give_back_the_hold() {
    let mut d = Driver::new();
    d.command("drawing.lightTable", json!({"dx": 10, "dy": 0, "rotation": 5})).unwrap();
    // let go: it stays
    d.command("drawing.lightTableShift", json!({"active": true})).unwrap();
    let b = d.bounds();
    d.drag(b.center(), b.center() + egui::vec2(30.0, 0.0));
    let moved = d.shift();
    assert_ne!(moved, (10.0, 0.0, 5.0));
    d.command("drawing.lightTableShift", json!({"active": false})).unwrap();
    d.frames(2);
    assert_eq!((d.table()["hold"].clone(), d.shift()), (json!("Off"), moved));

    // Escape, focus loss: back to what it was when the hold began
    for end in 0..3 {
        d.command("drawing.lightTable", json!({"dx": 10, "dy": 0, "rotation": 5})).unwrap();
        d.command("drawing.lightTableShift", json!({"active": true})).unwrap();
        d.frames(2);
        let b = d.bounds();
        d.drag(b.center(), b.center() + egui::vec2(30.0, 12.0));
        assert_ne!(d.shift(), (10.0, 0.0, 5.0));
        match end {
            0 => {
                d.event(key(egui::Key::Escape, true, egui::Modifiers::NONE));
                d.event(key(egui::Key::Escape, false, egui::Modifiers::NONE));
            }
            1 => d.event(egui::Event::WindowFocused(false)),
            _ => {
                // a drag in progress is abandoned too
                d.event(egui::Event::PointerMoved(b.center()));
                d.event(button(b.center(), true, egui::Modifiers::NONE));
                d.event(egui::Event::PointerMoved(b.center() + egui::vec2(20.0, 20.0)));
                d.event(key(egui::Key::Escape, true, egui::Modifiers::NONE));
                d.event(key(egui::Key::Escape, false, egui::Modifiers::NONE));
                d.event(egui::Event::PointerMoved(b.center() + egui::vec2(60.0, 60.0)));
                d.event(button(b.center() + egui::vec2(60.0, 60.0), false, egui::Modifiers::NONE));
            }
        }
        d.frames(2);
        assert_eq!(d.table()["hold"], json!("Off"), "exit {end}");
        assert_eq!(d.shift(), (10.0, 0.0, 5.0), "exit {end}: the hold's changes are given back");
    }
    // a focus loss with no hold keeps a settled alignment: the window was only put aside
    d.event(egui::Event::WindowFocused(false));
    d.frames(2);
    assert_eq!(d.shift(), (10.0, 0.0, 5.0));
}

#[test]
fn another_tool_ends_the_hold_and_keeps_the_alignment() {
    let mut d = Driver::new();
    d.command("drawing.lightTable", json!({"dx": 9})).unwrap();
    d.command("drawing.lightTableShift", json!({"active": true})).unwrap();
    d.call("ui.set", json!({"tool": "Hand"}));
    d.frames(2);
    assert_eq!(d.table()["hold"], json!("Off"));
    assert_eq!(d.shift().0, 9.0);
    // the hold is the Brush tool's
    assert!(d.command("drawing.lightTableShift", json!({"active": true})).is_err());
}

#[test]
fn reset_and_a_change_of_target_discard_the_alignment() {
    let mut d = Driver::new();
    let set = |d: &mut Driver| {
        d.command("drawing.lightTable", json!({"dx": 14, "dy": -3, "rotation": 20})).unwrap();
        assert!(d.table()["pose"]["frame"] == 16, "{}", d.table());
    };
    // Reset
    set(&mut d);
    d.command("drawing.lightTableReset", json!({})).unwrap();
    assert_eq!((d.table()["pose"].clone(), d.shift()), (json!(null), (0.0, 0.0, 0.0)));
    // unpinning, by the pin and by Pin Pose on the pinned cel itself
    set(&mut d);
    d.command("drawing.onionSkin", json!({"pin": null})).unwrap();
    d.frames(2);
    assert_eq!(d.shift(), (0.0, 0.0, 0.0), "unpinned");
    d.command("drawing.onionSkin", json!({"pin": 16})).unwrap();
    set(&mut d);
    d.command("playhead.set", json!({"frame": 17})).unwrap();
    d.command("drawing.pinPose", json!({})).unwrap();
    d.frames(2);
    assert_eq!(d.shift(), (0.0, 0.0, 0.0), "unpinned with Pin Pose");
    d.command("playhead.set", json!({"frame": 9})).unwrap();
    d.command("drawing.onionSkin", json!({"pin": 16})).unwrap();
    // pinning another cel, then the first again: it starts straight
    set(&mut d);
    d.command("drawing.onionSkin", json!({"pin": 0})).unwrap();
    d.frames(2);
    assert_eq!(d.shift(), (0.0, 0.0, 0.0), "another pose");
    d.command("drawing.onionSkin", json!({"pin": 16})).unwrap();
    d.frames(2);
    assert_eq!(d.shift(), (0.0, 0.0, 0.0), "the shift did not wait for the first pose to come back");
    // the pinned cel deleted
    set(&mut d);
    d.command("drawing.deleteCel", json!({"frame": 16})).unwrap();
    d.frames(2);
    assert_eq!(d.shift(), (0.0, 0.0, 0.0), "pinned cel gone");
    // the reset in the options bar, by its automation id
    d.command("edit.undo", json!({})).unwrap();
    d.command("drawing.onionSkin", json!({"pin": 16})).unwrap();
    set(&mut d);
    d.frames(3);
    d.call("ui.click", json!({"id": "program.brush.light.resetBadge"}));
    d.frames(3);
    assert_eq!(d.shift(), (0.0, 0.0, 0.0));
    // moving the playhead is not a change of target
    set(&mut d);
    for f in [0, 5, 17, 40] {
        d.command("playhead.set", json!({"frame": f})).unwrap();
        d.frames(2);
        assert_eq!(d.shift(), (14.0, -3.0, 20.0), "frame {f}");
    }
}

#[test]
fn a_held_key_shifts_for_as_long_as_it_is_down_and_every_exit_releases_it() {
    let mut d = Driver::new();
    let alt = egui::Modifiers::ALT;
    d.event(egui::Event::ModifiersChanged(alt));
    d.event(key(egui::Key::T, true, alt));
    assert_eq!(d.table()["hold"], json!("Key"));
    d.frames(3);
    assert_eq!(d.table()["hold"], json!("Key"), "still down");
    d.event(key(egui::Key::T, false, alt));
    d.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    d.frames(2);
    assert_eq!(d.table()["hold"], json!("Off"), "key up");

    // focus loss and Escape release a key hold too, and give the changes back
    for end in 0..2 {
        d.event(egui::Event::ModifiersChanged(alt));
        d.event(key(egui::Key::T, true, alt));
        assert_eq!(d.table()["hold"], json!("Key"));
        d.command("drawing.lightTable", json!({"dx": 33})).unwrap();
        if end == 0 {
            d.event(egui::Event::WindowFocused(false));
        } else {
            d.event(key(egui::Key::Escape, true, egui::Modifiers::NONE));
            d.event(key(egui::Key::Escape, false, egui::Modifiers::NONE));
        }
        d.frames(2);
        assert_eq!(d.table()["hold"], json!("Off"), "exit {end}");
        assert_eq!(d.shift().0, 0.0, "exit {end}");
        d.event(key(egui::Key::T, false, alt));
        d.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    }
}

#[test]
fn the_default_keys_exist_and_do_not_collide() {
    let d = Driver::new();
    let s = &d.harness.state().session.shortcuts;
    for (id, keys) in [("drawing.lightTableShift", "Alt+T"), ("drawing.lightTableReset", "Alt+Shift+T")] {
        assert_eq!(s.primary(id).as_deref(), Some(keys), "{id}");
    }
    let conflicts = s.conflicts(waltcut_engine::shortcuts::Platform::current());
    let ours: Vec<_> = conflicts.iter().filter(|c| c.2.iter().any(|id| id.starts_with("drawing.lightTable"))).collect();
    assert!(ours.is_empty(), "collisions: {ours:?}");
}

#[test]
fn the_controls_are_automatable_and_the_badge_says_the_pose_is_shifted() {
    let mut d = Driver::new();
    d.frames(2);
    let ids = |d: &mut Driver| -> Vec<String> {
        d.call("ui.elements", json!({"prefix": "program.brush.light"}))["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect()
    };
    let found = ids(&mut d);
    assert!(found.contains(&"program.brush.light.menu".to_string()), "{found:?}");
    assert!(!found.contains(&"program.brush.light.badge".to_string()), "no badge while the pose is where it was drawn");
    // the hold and the numbers, in the menu
    d.call("ui.click", json!({"id": "program.brush.light.menu"}));
    d.frames(3);
    let found = ids(&mut d);
    for id in ["shift", "dx", "dy", "rotation", "reset"] {
        assert!(found.contains(&format!("program.brush.light.{id}")), "{id}: {found:?}");
    }
    d.call("ui.click", json!({"id": "program.brush.light.shift"}));
    d.frames(3);
    assert_eq!(d.table()["hold"], json!("Command"));
    // while it is on, the bar says so and Done ends it
    let found = ids(&mut d);
    assert!(found.contains(&"program.brush.light.badge".to_string()) && found.contains(&"program.brush.light.done".to_string()), "{found:?}");
    d.call("ui.click", json!({"id": "program.brush.light.done"}));
    d.frames(3);
    assert_eq!(d.table()["hold"], json!("Off"));
    d.command("drawing.lightTable", json!({"dx": 12})).unwrap();
    d.frames(3);
    let found = ids(&mut d);
    assert!(found.contains(&"program.brush.light.badge".to_string()) && found.contains(&"program.brush.light.resetBadge".to_string()), "{found:?}");
    let items = d.call("ui.menu.list", json!({}))["result"].clone();
    let listed: Vec<&str> = items.as_array().unwrap().iter().filter_map(|i| i["id"].as_str()).collect();
    for id in ["drawing.lightTable", "drawing.lightTableShift", "drawing.lightTableReset"] {
        assert!(listed.contains(&id), "{id}");
    }
}
