//! The `drawing` benchmark section (#70): what the retained drawing workload costs, split into
//! cel rasterisation, committing a stroke (pen-up), scrubbing and playback over reference video, and
//! the UI work of the Brush tool (onion skins, a live stroke). Workloads are generated here from a
//! fixed seed (nothing is committed): see [`WORKLOADS`].
//!
//! `WALTCUT_BENCH_SAVE=<dir>` also saves each workload as a project there.
//!
//! Not measured here (needs the running app): audio timing and peak memory of the whole app. The
//! orchestrator reports peak RSS per section; `tools/bench-drawing-native.py` drives the real app.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_geom::Affine;
use waltcut_project::drawing::{Brush, Cel, Drawing, Stroke, StrokePoint};
use waltcut_project::{ItemId, ItemKind};
use waltcut_render::Image;

use crate::Opts;
use crate::playback::{self, Display, load_avg, ms, pct};

/// A named, reproducible drawing workload on the 1080p reference-video sequence.
pub struct Workload {
    pub name: &'static str,
    /// Unique cels.
    pub cels: i64,
    /// Strokes in each cel.
    pub strokes: usize,
    /// Samples per stroke (a 240 Hz pen at a moderate pace).
    pub points: usize,
    /// Frames each cel is held (2 = on twos).
    pub hold: i64,
}

pub const WORKLOADS: &[Workload] = &[
    Workload { name: "small", cels: 12, strokes: 20, points: 40, hold: 2 },
    Workload { name: "medium", cels: 48, strokes: 150, points: 120, hold: 2 },
    Workload { name: "stress", cels: 96, strokes: 1500, points: 200, hold: 1 },
];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// 0..1
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// One wandering stroke of `n` samples inside a `w`×`h` canvas, with a pressure ramp.
fn gen_stroke(rng: &mut Rng, w: f32, h: f32, n: usize, index: usize) -> Stroke {
    let brush = match index % 10 {
        9 => Brush::Eraser,
        i if i % 3 == 0 => Brush::Pencil,
        i if i % 3 == 1 => Brush::Pen,
        _ => Brush::Marker,
    };
    let (mut x, mut y) = (rng.unit() * w, rng.unit() * h);
    let mut a = rng.unit() * std::f32::consts::TAU;
    let points = (0..n)
        .map(|i| {
            a += (rng.unit() - 0.5) * 0.3;
            x = (x + a.cos() * 3.0).clamp(0.0, w);
            y = (y + a.sin() * 3.0).clamp(0.0, h);
            let t = i as f32 / n.max(1) as f32;
            StrokePoint { x, y, p: (std::f32::consts::PI * t).sin().clamp(0.05, 1.0) }
        })
        .collect();
    Stroke { brush, color: [rng.unit(), rng.unit(), rng.unit(), 1.0], size: 4.0 + rng.unit() * 20.0, points }
}

pub fn gen_drawing(wl: &Workload, w: u32, h: u32, rate: waltcut_time::FrameRate) -> Drawing {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut d = Drawing::new(w, h, rate);
    d.cels = (0..wl.cels)
        .map(|c| Cel {
            frame: c * wl.hold,
            strokes: (0..wl.strokes).map(|i| gen_stroke(&mut rng, w as f32, h as f32, wl.points, i)).collect::<Vec<_>>().into(),
        })
        .collect();
    d
}

/// The reference-video sequence with a drawing clip of the workload above it.
fn session(wl: &Workload) -> Option<(Session, ItemId, i64)> {
    let (mut s, seq) = playback::scenario_session("h264-1080")?;
    let r = s.execute("drawing.new", json!({"seconds": 20.0})).ok()?;
    let item = ItemId(r["item"].as_u64()?);
    let clip = r["clip"].as_u64()?;
    let rate = s.project.sequence(seq)?.settings.frame_rate;
    let drawing = gen_drawing(wl, 1920, 1080, rate);
    s.edit("bench drawing", |p, _| {
        p.item_mut(item).ok_or_else(|| waltcut_engine::EngineError::Other("no drawing item".into()))?.kind = ItemKind::Drawing(Arc::new(drawing));
        Ok(())
    })
    .ok()?;
    // WALTCUT_BENCH_SAVE=<dir>: keep the workload as a project, for `tools/bench-drawing-native.py --project`
    if let Some(dir) = std::env::var_os("WALTCUT_BENCH_SAVE") {
        let path = std::path::Path::new(&dir).join(format!("drawing-{}.waltcut", wl.name));
        match s.execute("file.saveAs", json!({"path": path.to_string_lossy()})) {
            Ok(_) => eprintln!("saved {}", path.display()),
            Err(e) => eprintln!("could not save {}: {e}", path.display()),
        }
    }
    Some((s, seq, clip as i64))
}

/// This process's resident set size now, in MB (`ps`).
fn rss_mb() -> f64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn dist(name: &str, v: &[f64]) -> Value {
    json!({ format!("{name}_p50_ms"): pct(v, 0.5), format!("{name}_p95_ms"): pct(v, 0.95), format!("{name}_max_ms"): pct(v, 1.0) })
}

fn merge(a: &mut Value, b: Value) {
    if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
        a.extend(b.clone());
    }
}

/// `--only small/play`: one workload and one phase (`raster`, `pen_up`, `scrub`, `play`, `ui`), so a
/// phase can run in its own process and its peak RSS is its own. `--only small`: every phase.
fn selected(o: &Opts) -> (Option<String>, Option<String>) {
    match o.only.as_deref() {
        Some(s) => match s.split_once('/') {
            Some((w, p)) => (Some(w.to_string()), Some(p.to_string())),
            None => (Some(s.to_string()), None),
        },
        None => (None, None),
    }
}

pub fn drawing(o: &Opts) -> Vec<Value> {
    let mut rows = Vec::new();
    let (only_wl, only_phase) = selected(o);
    let phase = |name: &str| only_phase.as_deref().is_none_or(|p| p == name);
    for wl in WORKLOADS.iter().filter(|w| only_wl.as_deref().is_none_or(|s| w.name.contains(s))) {
        let t_gen = Instant::now();
        let Some((mut s, seq, clip)) = session(wl) else {
            eprintln!("drawing {}: skipped (fixtures unavailable)", wl.name);
            continue;
        };
        let total_strokes = wl.cels as usize * wl.strokes;
        let total_points = total_strokes * wl.points;
        eprintln!(
            "drawing {}: {} cels × {} strokes × {} points = {} M points (built in {:.1} s)",
            wl.name,
            wl.cels,
            wl.strokes,
            wl.points,
            total_points as f64 / 1e6,
            t_gen.elapsed().as_secs_f64()
        );
        for _ in 0..o.repeat {
            let mut row = json!({
                "workload": wl.name, "cels": wl.cels, "strokes_per_cel": wl.strokes, "points_per_stroke": wl.points, "hold": wl.hold,
                "total_points": total_points, "canvas": "1920x1080", "sequence_fps": 23.976,
            });
            row["rss_built_mb"] = json!(rss_mb());
            if phase("raster") {
                merge(&mut row, raster(&s));
                row["rss_after_raster_mb"] = json!(rss_mb());
            }
            if phase("pen_up") {
                merge(&mut row, pen_up(&mut s, seq, clip, wl, o));
                row["rss_after_pen_up_mb"] = json!(rss_mb());
            }
            if phase("scrub") {
                merge(&mut row, scrub(&s, seq, o));
                row["rss_after_scrub_mb"] = json!(rss_mb());
            }
            if phase("play") {
                merge(&mut row, play(&s, seq, o));
                row["rss_after_play_mb"] = json!(rss_mb());
            }
            if phase("ui") {
                merge(&mut row, crate::drawing_ui::ui(&s, wl, o));
                row["rss_after_ui_mb"] = json!(rss_mb());
            }
            row["load"] = json!(load_avg());
            rows.push(row);
        }
    }
    rows
}

/// CPU rasterisation of one cel at 1080p (what a frame with a drawing clip adds), first call
/// (cold: page faults, allocation) and then warm.
fn raster(s: &Session) -> Value {
    let d = s.project.items.values().find_map(|i| match &i.kind {
        ItemKind::Drawing(d) => Some(d.clone()),
        _ => None,
    });
    let Some(d) = d else { return json!({}) };
    let id = Affine::scale(1.0, 1.0);
    let mut cold = Vec::new();
    let mut warm = Vec::new();
    let step = (d.cels.len() / 8).max(1);
    for (n, cel) in d.cels.iter().step_by(step).enumerate() {
        let mut img = Image::new(d.width as usize, d.height as usize);
        let t = Instant::now();
        waltcut_render::drawing::render_strokes(&cel.strokes, &id, &mut img);
        let first = ms(t.elapsed());
        if n == 0 {
            cold.push(first);
        } else {
            warm.push(first);
        }
        let mut img = Image::new(d.width as usize, d.height as usize);
        let t = Instant::now();
        waltcut_render::drawing::render_strokes(&cel.strokes, &id, &mut img);
        warm.push(ms(t.elapsed()));
    }
    let mut v = dist("raster_cel", &warm);
    merge(&mut v, json!({"raster_cel_cold_ms": cold.first().copied().unwrap_or(0.0)}));
    v
}

/// Pen-up: `drawing.addStroke` of a 150-point stroke into the cel under the playhead, then the time
/// until the frame server has the exact frame at the new revision (commit-to-preview), and undo.
fn pen_up(s: &mut Session, seq: ItemId, clip: i64, wl: &Workload, o: &Opts) -> Value {
    let trials = if wl.name == "stress" {
        5
    } else if o.quick {
        8
    } else {
        20
    };
    let rate = s.project.sequence(seq).map(|q| q.settings.frame_rate).unwrap_or(waltcut_time::FrameRate::FPS_23_976);
    let frame = (wl.cels / 2) * wl.hold;
    s.state.playheads.insert(seq, rate.tick_of(frame));
    let bench = playback::Bench {
        server: waltcut_ui_egui::frames::FrameServer::new(
            s.media.clone(),
            s.services.clone(),
            s.previews.clone(),
            waltcut_ui_egui::frames::FrameServer::default_workers(),
        ),
        refresh_hz: 60.0,
        seconds: 0.0,
        draft: false,
    };
    let mut display = Display::new(o.gpu);
    let mut rng = Rng(77);
    let (mut commit, mut ready, mut undo) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..trials {
        let st = gen_stroke(&mut rng, 1920.0, 1080.0, 150, 1);
        let pts: Vec<[f32; 3]> = st.points.iter().map(|p| [p.x, p.y, p.p]).collect();
        let params = json!({"clip": clip, "cel": "hold", "stroke": {"brush": "Pen", "color": [0.1, 0.1, 0.1, 1.0], "size": st.size, "points": pts}});
        let t = Instant::now();
        if s.execute("drawing.addStroke", params).is_err() {
            break;
        }
        commit.push(ms(t.elapsed()));
        let target = if display_gpu(&display) { waltcut_ui_egui::frames::Target::SequencePlan(seq) } else { waltcut_ui_egui::frames::Target::Sequence(seq) };
        let key = waltcut_ui_egui::frames::FrameKey { target, frame, size: 1000, revision: s.revision, draft: false };
        let project = s.project.clone();
        let t2 = Instant::now();
        loop {
            bench.server.request(key, rate.tick_of(frame), 1.0, &project, 0);
            if display_exact(&mut display, &bench.server, key) || t2.elapsed() > Duration::from_secs(10) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        ready.push(ms(t.elapsed()));
        let t3 = Instant::now();
        let _ = s.execute("edit.undo", json!({}));
        undo.push(ms(t3.elapsed()));
    }
    let mut v = dist("commit", &commit);
    merge(&mut v, dist("commit_to_preview", &ready));
    merge(&mut v, dist("undo", &undo));
    v
}

fn display_gpu(d: &Display) -> bool {
    d.gpu_available()
}

fn display_exact(d: &mut Display, server: &waltcut_ui_egui::frames::FrameServer, key: waltcut_ui_egui::frames::FrameKey) -> bool {
    d.refresh(server, key)
}

fn scrub(s: &Session, seq: ItemId, o: &Opts) -> Value {
    let (seeks, drags) = if o.quick { (8, 3) } else { (24, 8) };
    let bench = playback::Bench {
        server: waltcut_ui_egui::frames::FrameServer::new(
            s.media.clone(),
            s.services.clone(),
            s.previews.clone(),
            waltcut_ui_egui::frames::FrameServer::default_workers(),
        ),
        refresh_hz: 60.0,
        seconds: 0.0,
        draft: false,
    };
    bench.server.set_profiling(true);
    let mut display = Display::new(o.gpu);
    let r = bench.scrub(s, seq, &mut display, 1.0, seeks, drags, 24, Duration::from_secs(20));
    json!({
        "scrub_seek_p50_ms": pct(&r.seek_ms, 0.5), "scrub_seek_p95_ms": pct(&r.seek_ms, 0.95), "scrub_seek_max_ms": pct(&r.seek_ms, 1.0),
        "scrub_settle_p50_ms": pct(&r.settle_ms, 0.5), "scrub_settle_p95_ms": pct(&r.settle_ms, 0.95),
        "scrub_drag_shown_pct": 100.0 * r.drag_shown as f64 / r.drag_steps.max(1) as f64, "scrub_timeouts": r.timeouts,
        "scrub_process_cpu_ms": r.process_cpu_ms, "gpu": display.gpu_available(),
    })
}

fn play(s: &Session, seq: ItemId, o: &Opts) -> Value {
    let seconds = if o.quick { 4.0 } else { 8.0 };
    let mut bench = playback::Bench {
        server: waltcut_ui_egui::frames::FrameServer::new(
            s.media.clone(),
            s.services.clone(),
            s.previews.clone(),
            waltcut_ui_egui::frames::FrameServer::default_workers(),
        ),
        refresh_hz: 60.0,
        seconds,
        draft: false,
    };
    bench.server.set_profiling(true);
    bench.draft = false;
    let mut display = Display::new(o.gpu);
    let r = bench.play(s, seq, &mut display, 1.0, waltcut_time::Tick::ZERO, "drawing");
    let frames = (r.shown + r.dropped).max(1) as f64;
    json!({
        "play_shown": r.shown, "play_dropped": r.dropped, "play_drop_pct": 100.0 * r.dropped as f64 / frames,
        "play_on_time": r.on_time, "play_due": r.due, "play_preroll_ms": r.preroll_ms,
        "play_latency_p50_ms": pct(&r.latency, 0.5), "play_latency_p95_ms": pct(&r.latency, 0.95),
        "play_service_p50_ms": pct(&r.service, 0.5), "play_service_p95_ms": pct(&r.service, 0.95),
        "play_decode_p50_ms": pct(&r.src_wall, 0.5), "play_present_p95_ms": pct(&r.present, 0.95),
        "play_process_cpu_ms": r.process_cpu,
    })
}

/// One table, a column per workload row.
pub fn markdown(rows: &[Value]) -> String {
    const METRICS: &[(&str, &str)] = &[
        ("total_points", "stroke samples in the drawing"),
        ("raster_cel_cold_ms", "rasterise one cel at 1080p, first call (ms)"),
        ("raster_cel_p50_ms", "rasterise one cel, warm p50 (ms)"),
        ("raster_cel_max_ms", "rasterise one cel, warm max (ms)"),
        ("commit_p50_ms", "pen-up: `drawing.addStroke` p50 (ms)"),
        ("commit_p95_ms", "pen-up: `drawing.addStroke` p95 (ms)"),
        ("commit_to_preview_p50_ms", "pen-up to exact frame ready, p50 (ms)"),
        ("commit_to_preview_p95_ms", "pen-up to exact frame ready, p95 (ms)"),
        ("undo_p50_ms", "undo of the stroke p50 (ms)"),
        ("scrub_seek_p50_ms", "random seek to exact frame p50 (ms)"),
        ("scrub_seek_p95_ms", "random seek to exact frame p95 (ms)"),
        ("scrub_settle_p50_ms", "drag-stop to exact frame p50 (ms)"),
        ("scrub_drag_shown_pct", "frames shown during a drag (%)"),
        ("play_drop_pct", "playback: frames dropped (%)"),
        ("play_service_p50_ms", "playback: frame job service p50 (ms)"),
        ("play_service_p95_ms", "playback: frame job service p95 (ms)"),
        ("play_decode_p50_ms", "playback: reference-video decode p50 (ms/job)"),
        ("ui_idle_p50_ms", "UI frame, Brush tool, onion on, idle, p50 (ms)"),
        ("ui_idle_p95_ms", "UI frame, Brush tool, onion on, idle, p95 (ms)"),
        ("ui_noonion_p50_ms", "UI frame, same, onion off, p50 (ms)"),
        ("ui_live_p50_ms", "UI frame while drawing a 600-sample live stroke, p50 (ms)"),
        ("ui_live_p95_ms", "UI frame while drawing a 600-sample live stroke, p95 (ms)"),
        ("ui_live_last_ms", "UI frame at the last sample of that stroke (ms)"),
        ("rss_built_mb", "process RSS after building the project (MB)"),
        ("rss_after_raster_mb", "RSS after rasterising cels (MB)"),
        ("rss_after_pen_up_mb", "RSS after the pen-up trials (MB)"),
        ("rss_after_scrub_mb", "RSS after scrubbing (MB)"),
        ("rss_after_play_mb", "RSS after playing (MB)"),
        ("rss_after_ui_mb", "RSS after the UI frames (MB)"),
    ];
    let num = |r: &Value, k: &str| {
        r.get(k).and_then(Value::as_f64).map(|v| if v.abs() >= 100.0 { format!("{v:.0}") } else { format!("{v:.2}") }).unwrap_or_else(|| "-".into())
    };
    let mut s = format!(
        "| metric | {} |\n|---|{}|\n",
        rows.iter().map(|r| r["workload"].as_str().unwrap_or("?")).collect::<Vec<_>>().join(" | "),
        rows.iter().map(|_| "---").collect::<Vec<_>>().join("|")
    );
    for (k, label) in METRICS {
        s += &format!("| {label} | {} |\n", rows.iter().map(|r| num(r, k)).collect::<Vec<_>>().join(" | "));
    }
    s
}
