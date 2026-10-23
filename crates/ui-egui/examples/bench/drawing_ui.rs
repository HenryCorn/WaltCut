//! The UI half of the `drawing` section: frame time of the real app (under `egui_kittest`, no
//! window) with the Brush tool on a drawing clip, with the onion skin on and off, and while a
//! live stroke grows. Per frame: app update (layout and the egui painter that draws the onion skins
//! and the live stroke) plus tessellation; the wgpu draw is not included.

use std::time::Instant;

use serde_json::{Value, json};
use waltcut_engine::Session;

use crate::Opts;
use crate::drawing::Workload;
use crate::playback::{ms, pct};
use crate::sections::Ui;

pub fn ui(s: &Session, wl: &Workload, o: &Opts) -> Value {
    let mut s2 = Session::default();
    s2.project = s.project.clone();
    s2.state = s.state.clone();
    let Some(seq) = s2.state.active_sequence else { return json!({}) };
    let rate = s2.project.sequence(seq).map(|q| q.settings.frame_rate).unwrap_or(waltcut_time::FrameRate::FPS_23_976);
    // a cel with a neighbour on each side
    s2.state.playheads.insert(seq, rate.tick_of((wl.cels / 2) * wl.hold));
    let (tx, rx) = std::sync::mpsc::channel();
    let app = waltcut_ui_egui::WaltCutApp::new(s2).with_control(rx);
    let harness = egui_kittest::Harness::builder().with_size(egui::vec2(1920.0, 1080.0)).with_max_steps(100_000).build_eframe(move |_cc| app);
    let mut ui = Ui { harness, tx };
    for _ in 0..8 {
        ui.frame();
    }
    ui.call("ui.set", json!({"tool": "brush"}));
    for _ in 0..10 {
        ui.frame();
    }
    let n = if o.quick { 30 } else { 100 };
    let time_frames = |ui: &mut Ui, n: usize| -> Vec<f64> {
        (0..n)
            .map(|_| {
                let t = Instant::now();
                ui.frame();
                tessellated(ui, t)
            })
            .collect()
    };
    let idle = time_frames(&mut ui, n);
    let idle_shapes = ui.harness.output().shapes.len();
    ui.harness.state_mut().ui.brush.onion = false;
    ui.frame();
    let noonion = time_frames(&mut ui, n);
    ui.harness.state_mut().ui.brush.onion = true;
    ui.frame();
    // a live stroke: press, then move a little each frame (the sample count grows)
    let rect = ui.call("ui.elements", json!({"prefix": "program.brush"}));
    let rc: Vec<f64> = rect
        .get("result")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|e| e["id"] == "program.brush"))
        .and_then(|e| e["rect"].as_array())
        .map(|r| r.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let g = |i: usize, d: f64| rc.get(i).copied().unwrap_or(d) as f32;
    let (cx, cy) = (g(0, 600.0) + g(2, 400.0) * 0.5, g(1, 400.0) + g(3, 300.0) * 0.5);
    let strokes_before = strokes_in(&ui);
    let at = |i: usize| egui::pos2(cx + 150.0 * (i as f32 * 0.05).cos(), cy + 100.0 * (i as f32 * 0.05).sin());
    ui.harness.hover_at(at(0));
    ui.frame();
    ui.harness.input_mut().events.push(egui::Event::PointerButton {
        pos: at(0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    ui.frame();
    let mut live = Vec::new();
    let samples = 600;
    for i in 1..=samples {
        ui.harness.input_mut().events.push(egui::Event::PointerMoved(at(i)));
        let t = Instant::now();
        ui.frame();
        live.push(tessellated(&ui, t));
    }
    let live_shapes = ui.harness.output().shapes.len();
    ui.harness.input_mut().events.push(egui::Event::PointerButton {
        pos: at(samples),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    ui.frame();
    let committed = strokes_in(&ui) - strokes_before;
    let last_points = last_stroke_points(&ui);
    let mut v = json!({"ui_idle_shapes": idle_shapes, "ui_live_end_shapes": live_shapes, "ui_live_committed_strokes": committed, "ui_live_committed_points": last_points, "ui_live_samples": samples, "ui_live_last_ms": live.last().copied().unwrap_or(0.0)});
    for (name, d) in [("ui_idle", &idle), ("ui_noonion", &noonion), ("ui_live", &live)] {
        v[format!("{name}_p50_ms")] = json!(pct(d, 0.5));
        v[format!("{name}_p95_ms")] = json!(pct(d, 0.95));
    }
    v
}

fn strokes_in(ui: &Ui) -> usize {
    let st = ui.harness.state();
    st.session.project.items.values().filter_map(|i| i.as_drawing().map(|d| d.cels.iter().map(|c| c.strokes.len()).sum::<usize>())).sum()
}

/// Milliseconds since `t0` (the frame's update), plus the tessellation of its shapes: the CPU
/// cost of a UI frame before the GPU draws it.
fn tessellated(ui: &Ui, t0: Instant) -> f64 {
    let out = ui.harness.output();
    let prims = ui.harness.ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
    std::hint::black_box(&prims);
    ms(t0.elapsed())
}

/// Samples of the stroke most recently added (the last of the last cel by start frame).
fn last_stroke_points(ui: &Ui) -> usize {
    let st = ui.harness.state();
    let d = st.session.project.items.values().find_map(|i| i.as_drawing());
    // the live stroke was drawn on the cel held at the playhead
    d.and_then(|d| d.cels.iter().map(|c| c.strokes.last().map_or(0, |s| s.points.len())).max()).unwrap_or(0)
}
