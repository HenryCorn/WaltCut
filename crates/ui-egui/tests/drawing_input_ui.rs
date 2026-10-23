//! Brush-tool input (#54): every pointer sample keeps its own position and pressure, the stroke
//! ends where the pen did however events were batched into UI frames, and focus loss or Escape
//! abandons a stroke without committing it.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;
use waltcut_ui_egui::panels::drawing_input::{PenPhase, PenQueueReport, PenSample};

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
    canvas: egui::Rect,
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

impl Driver {
    /// An empty drawing clip with the Brush tool selected.
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

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).expect("control");
        for _ in 0..600 {
            self.frames(1);
            if let Ok(r) = reply.try_recv() {
                assert_eq!(r["ok"], true, "{method} {params}: {r}");
                return r["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }

    /// A point on the canvas, `fx` and `fy` as fractions of its size.
    fn at(&self, fx: f32, fy: f32) -> egui::Pos2 {
        self.canvas.min + egui::vec2(self.canvas.width() * fx, self.canvas.height() * fy)
    }

    /// Queue events for the next frame and run it.
    fn frame_with(&mut self, events: Vec<egui::Event>) {
        self.harness.input_mut().events.extend(events);
        self.frames(1);
    }

    fn pen_stats(&mut self) -> Value {
        self.ok("ui.inspect", json!({}))["ui"]["brush"]["pen"].clone()
    }
}

fn stroke_points(d: &mut Driver) -> Vec<[f32; 3]> {
    let project = d.harness.state().session.project.clone();
    for item in project.items.values() {
        if let Some(s) = item.as_drawing().and_then(|dr| dr.cels.iter().flat_map(|c| c.strokes.iter()).last()) {
            return s.points.iter().map(|p| [p.x, p.y, p.p]).collect();
        }
    }
    Vec::new()
}

fn stroke_count(d: &Driver) -> usize {
    d.harness.state().session.project.items.values().filter_map(|i| i.as_drawing()).map(|dr| dr.cels.iter().map(|c| c.strokes.len()).sum::<usize>()).sum()
}

#[test]
fn the_release_frame_keeps_the_moves_that_arrived_with_it() {
    // press in one frame; the last moves and the release arrive together in the next
    let mut d = Driver::new();
    let (a, b, c, e) = (d.at(0.2, 0.5), d.at(0.4, 0.5), d.at(0.6, 0.5), d.at(0.8, 0.5));
    d.frame_with(vec![egui::Event::PointerMoved(a)]);
    d.frame_with(vec![button(a, true)]);
    d.frame_with(vec![egui::Event::PointerMoved(b)]);
    d.frame_with(vec![egui::Event::PointerMoved(c), egui::Event::PointerMoved(e), button(e, false)]);
    d.frames(3);
    assert_eq!(stroke_count(&d), 1);
    let pts = stroke_points(&mut d);
    let last = pts.last().copied().expect("points");
    // the endpoint is where the pen came up, in canvas pixels, and the move before it is there too
    let canvas_x = |p: egui::Pos2| (p.x - d.canvas.min.x) / d.canvas.width();
    let fx = last[0] / pts.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
    assert!((fx - 1.0).abs() < 1e-3, "the last point is the right-most: {pts:?}");
    assert!(pts.len() >= 4, "press, move, move and the release-frame moves: {pts:?}");
    assert!(canvas_x(e) > canvas_x(c));
}

#[test]
fn replay_is_the_same_whatever_the_frame_batching() {
    let path = |d: &Driver| (0..30).map(|i| d.at(0.1 + i as f32 * 0.02, 0.5 + (i as f32 * 0.4).sin() * 0.1)).collect::<Vec<_>>();
    let draw = |per_frame: usize| {
        let mut d = Driver::new();
        let pts = path(&d);
        d.frame_with(vec![egui::Event::PointerMoved(pts[0])]);
        d.frame_with(vec![button(pts[0], true)]);
        for chunk in pts[1..].chunks(per_frame) {
            d.frame_with(chunk.iter().map(|p| egui::Event::PointerMoved(*p)).collect());
        }
        d.frame_with(vec![button(*pts.last().expect("path"), false)]);
        d.frames(3);
        stroke_points(&mut d)
    };
    let one = draw(1);
    assert!(one.len() > 20);
    for n in [2, 5, 29] {
        assert_eq!(draw(n), one, "{n} events per frame");
    }
}

#[test]
fn each_platform_sample_keeps_its_own_pressure_within_one_frame() {
    let mut d = Driver::new();
    let queue: Rc<RefCell<Vec<PenSample>>> = Rc::default();
    let q = queue.clone();
    d.harness.state_mut().hooks.pen_samples = Some(Box::new(move |out| {
        out.extend(q.borrow_mut().drain(..));
        PenQueueReport::default()
    }));
    let pts = [d.at(0.2, 0.5), d.at(0.35, 0.55), d.at(0.5, 0.5), d.at(0.65, 0.45), d.at(0.8, 0.5)];
    let pressures = [0.15, 0.4, 0.95, 0.6, 0.3];
    let sample = |i: usize, phase| PenSample { t_ns: 1_000_000 * (i as u64 + 1), x: pts[i].x, y: pts[i].y, pressure: pressures[i], phase, tablet: true };
    d.frame_with(vec![egui::Event::PointerMoved(pts[0])]);
    queue.borrow_mut().push(sample(0, PenPhase::Down));
    d.frame_with(vec![button(pts[0], true)]);
    // four samples with distinct pressures and the pen-up, all inside one UI frame
    {
        let mut q = queue.borrow_mut();
        for i in 1..4 {
            q.push(sample(i, PenPhase::Move));
        }
        q.push(sample(4, PenPhase::Up));
    }
    let mut ev: Vec<egui::Event> = pts[1..].iter().map(|p| egui::Event::PointerMoved(*p)).collect();
    ev.push(button(pts[4], false));
    d.frame_with(ev);
    d.frames(3);
    assert_eq!(stroke_count(&d), 1);
    let got: Vec<f32> = stroke_points(&mut d).iter().map(|p| p[2]).collect();
    // the Up carries the pressure of the sample before it, so the endpoint is the last move's pressure
    assert_eq!(got, vec![0.15, 0.4, 0.95, 0.6, 0.3], "{got:?}");
    let stats = d.pen_stats();
    assert_eq!(stats["source"], "Pen");
    assert_eq!(stats["last_stroke_points"], 5);
    assert!(stats["last_stroke_hz"].as_f64().unwrap_or(0.0) > 900.0, "1 kHz timestamps: {stats}");
}

#[test]
fn queue_overflow_and_a_mismatching_queue_are_reported() {
    let mut d = Driver::new();
    let far = [PenSample { t_ns: 1, x: 5000.0, y: 5000.0, pressure: 0.2, phase: PenPhase::Move, tablet: true }];
    let calls = Rc::new(RefCell::new(0));
    let c = calls.clone();
    d.harness.state_mut().hooks.pen_samples = Some(Box::new(move |out| {
        *c.borrow_mut() += 1;
        if *c.borrow() == 5 {
            out.extend(far);
            return PenQueueReport { dropped: 7, invalid: 2 };
        }
        PenQueueReport::default()
    }));
    let (a, b) = (d.at(0.3, 0.5), d.at(0.6, 0.5));
    d.frame_with(vec![egui::Event::PointerMoved(a)]);
    d.frame_with(vec![button(a, true)]);
    d.frame_with(vec![egui::Event::PointerMoved(b)]);
    d.frame_with(vec![button(b, false)]);
    d.frames(3);
    let stats = d.pen_stats();
    assert_eq!(stats["queue_dropped"], 7, "{stats}");
    assert_eq!(stats["invalid"], 2, "{stats}");
    assert!(*calls.borrow() >= 5);
    // the queue's sample is nowhere near the pointer, so that frame used the UI's events
    assert!(stats["position_mismatches"].as_u64().unwrap_or(0) >= 1, "{stats}");
}

#[test]
fn losing_focus_or_pressing_escape_abandons_the_stroke_without_drawing() {
    for cancel in [
        egui::Event::WindowFocused(false),
        egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() },
    ] {
        let mut d = Driver::new();
        let (a, b, c) = (d.at(0.2, 0.5), d.at(0.5, 0.5), d.at(0.7, 0.5));
        d.frame_with(vec![egui::Event::PointerMoved(a)]);
        d.frame_with(vec![button(a, true)]);
        d.frame_with(vec![egui::Event::PointerMoved(b)]);
        d.frame_with(vec![egui::Event::PointerMoved(c)]);
        d.frame_with(vec![cancel.clone()]);
        // the button comes up later (maybe in another app): still nothing is drawn
        d.frame_with(vec![button(c, false)]);
        d.frames(3);
        assert_eq!(stroke_count(&d), 0, "{cancel:?}");
        let stats = d.pen_stats();
        assert_eq!(stats["cancelled"], 1, "{stats}");
        // and drawing works again afterwards
        d.frame_with(vec![egui::Event::PointerMoved(a)]);
        d.frame_with(vec![button(a, true)]);
        d.frame_with(vec![egui::Event::PointerMoved(c)]);
        d.frame_with(vec![button(c, false)]);
        d.frames(3);
        assert_eq!(stroke_count(&d), 1, "{cancel:?}");
    }
}

#[test]
fn a_mouse_press_outside_the_canvas_never_becomes_a_stroke() {
    let mut d = Driver::new();
    let outside = egui::pos2(d.canvas.min.x - 30.0, d.canvas.center().y);
    d.frame_with(vec![egui::Event::PointerMoved(outside)]);
    d.frame_with(vec![button(outside, true)]);
    d.frame_with(vec![egui::Event::PointerMoved(d.at(0.5, 0.5))]);
    d.frame_with(vec![egui::Event::PointerMoved(d.at(0.6, 0.5))]);
    d.frame_with(vec![button(d.at(0.6, 0.5), false)]);
    d.frames(3);
    assert_eq!(stroke_count(&d), 0);
}

#[test]
fn samples_queued_while_another_tool_was_active_are_not_a_stroke_or_a_loss_report() {
    let mut d = Driver::new();
    d.ok("ui.set", json!({"tool": "Selection"}));
    d.frames(6);
    let queue: Rc<RefCell<Vec<PenSample>>> = Rc::default();
    let q = queue.clone();
    let first = Rc::new(RefCell::new(true));
    d.harness.state_mut().hooks.pen_samples = Some(Box::new(move |out| {
        out.extend(q.borrow_mut().drain(..));
        // the host's queue reports its overflow once, with the first drain after the overflow
        PenQueueReport { dropped: if std::mem::take(&mut *first.borrow_mut()) { 4000 } else { 0 }, invalid: 0 }
    }));
    // a pen press and stroke elsewhere in the app, never seen by the Brush tool
    queue.borrow_mut().extend((0..10).map(|i| PenSample {
        t_ns: i + 1,
        x: 100.0 + i as f32,
        y: 100.0,
        pressure: 0.5,
        phase: if i == 0 { PenPhase::Down } else { PenPhase::Move },
        tablet: true,
    }));
    d.frames(3);
    d.ok("ui.set", json!({"tool": "Brush"}));
    d.frames(4);
    let stats = d.pen_stats();
    assert_eq!(stats["queue_dropped"], 0, "{stats}");
    assert_eq!(stroke_count(&d), 0);
}
