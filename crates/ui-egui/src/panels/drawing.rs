//! The Brush tool: drawing frame-by-frame animation on the Program monitor.
//!
//! With the Brush tool (Shift+P) a drag on the picture paints a stroke into the drawing clip
//! under the playhead (`drawing.addStroke`, one undo step per stroke). The stroke is previewed with
//! the egui painter while the pen is down, then rendered by the engine like any other layer.
//! Pressure comes from the host's pen hook (a drawing tablet on macOS) or from touch force, else
//! it is full. The neighbouring cels show as onion skins: the one before in red, the one after
//! in green.

use egui::{Color32, Pos2, Rect, Sense, Stroke as EStroke, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use waltcut_geom::{Affine, Vec2};
use waltcut_project::drawing::{Brush, Drawing, Stroke};
use waltcut_project::{ClipId, TrackItem};

use super::drawing_input::{FrameInput, PenPhase, PenSample, PenStats, Sample, Source, StrokeBuilder, frame_samples};
use crate::WaltCutApp;

/// What a stroke on a frame that does not start a cel draws into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CelMode {
    /// Add to the cel being held on this frame.
    #[default]
    Hold,
    /// Start a blank cel on this frame.
    New,
    /// Start a cel on this frame as a copy of the one held.
    Copy,
}

impl CelMode {
    const ALL: [CelMode; 3] = [CelMode::Hold, CelMode::New, CelMode::Copy];
    fn label(self) -> &'static str {
        match self {
            CelMode::Hold => "Draw on held cel",
            CelMode::New => "New blank cel",
            CelMode::Copy => "New cel from held",
        }
    }
    fn param(self) -> &'static str {
        match self {
            CelMode::Hold => "hold",
            CelMode::New => "new",
            CelMode::Copy => "copy",
        }
    }
}

/// Brush tool options (saved with the UI state).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushSettings {
    pub brush: Brush,
    /// sRGB, straight alpha.
    pub color: [f32; 4],
    /// Diameter in drawing pixels at full pressure.
    pub size: f32,
    pub pressure: bool,
    pub cel_mode: CelMode,
    pub onion: bool,
    /// Onion skin opacity, 0..1.
    pub onion_opacity: f32,
    /// In-between spacing for In-between to Next Cel (2 = on twos).
    pub tween_step: u32,
    /// Input diagnostics (read-only: shown by `ui.inspect`, never loaded back).
    #[serde(skip_deserializing)]
    pub pen: PenStats,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            brush: Brush::Pen,
            color: [0.07, 0.07, 0.07, 1.0],
            size: 6.0,
            pressure: true,
            cel_mode: CelMode::Hold,
            onion: true,
            onion_opacity: 0.35,
            tween_step: 2,
            pen: PenStats::default(),
        }
    }
}

/// The stroke being drawn.
#[derive(Clone, Debug, Default)]
struct Live {
    clip: u64,
    stroke: StrokeBuilder,
    /// Drawn with the pen's barrel (secondary) button: erases whatever the brush is.
    eraser: bool,
}

/// Samples of a press that has not become a stroke (yet): the press, and the moves after it, until
/// the canvas decides the press belongs to it. Bounded, newest replacing the last when full.
const MAX_PENDING: usize = 512;

/// Picture ↔ drawing canvas mapping for one clip at the playhead.
struct Mapping {
    /// canvas px → screen points
    to_screen: Affine,
    from_screen: Affine,
    /// screen points per canvas pixel
    scale: f32,
}

impl Mapping {
    fn new(app: &WaltCutApp, item: &TrackItem, pic: Rect, frame: (u32, u32)) -> Option<Mapping> {
        let seq = app.session.active_sequence()?;
        let src = app.session.project.source_size(item.item)?;
        let par = app.session.project.source_par(item.item);
        let mt = item.effect_time_at(app.session.playhead());
        let motion = waltcut_render::motion_matrix(seq, item, src, par, mt);
        let (sx, sy) = (pic.width() as f64 / frame.0.max(1) as f64, pic.height() as f64 / frame.1.max(1) as f64);
        // canvas → sequence frame (Motion) → picture → screen; `then_apply` applies its argument first
        let to_screen = Affine::translate(pic.min.x as f64, pic.min.y as f64).then_apply(&Affine::scale(sx, sy)).then_apply(&motion);
        let from_screen = to_screen.inverse()?;
        let scale = to_screen.determinant().abs().sqrt() as f32;
        (scale.is_finite() && scale > 0.0).then_some(Mapping { to_screen, from_screen, scale })
    }
    fn screen(&self, x: f32, y: f32) -> Pos2 {
        let p = self.to_screen.apply(Vec2::new(x as f64, y as f64));
        pos2(p.x as f32, p.y as f32)
    }
    fn canvas(&self, p: Pos2) -> (f32, f32) {
        let q = self.from_screen.apply(Vec2::new(p.x as f64, p.y as f64));
        (q.x as f32, q.y as f32)
    }
}

fn color32(c: [f32; 4], alpha: f32) -> Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(b(c[0]), b(c[1]), b(c[2]), b(c[3] * alpha))
}

/// Paint a stroke preview: round-capped segments, width following pressure.
fn paint_stroke(painter: &egui::Painter, map: &Mapping, brush: Brush, size: f32, pts: &[[f32; 3]], color: Color32) {
    let radius = |p: f32| (size * 0.5 * brush.response(p).0 * map.scale).max(0.5);
    if color.a() < 255 && pts.len() > 1 {
        // translucent (onion skins): bare segments, no round joints, so overlapping joints don't
        // stack their alpha into beads (and no polyline, whose miters spike where samples double back)
        for w in pts.windows(2) {
            if let [a, b] = w {
                painter.line_segment([map.screen(a[0], a[1]), map.screen(b[0], b[1])], EStroke::new(radius(a[2]) + radius(b[2]), color));
            }
        }
        return;
    }
    let mut prev: Option<(Pos2, f32)> = None;
    for &[x, y, p] in pts {
        let s = map.screen(x, y);
        let r = radius(p);
        if let Some((q, rq)) = prev {
            painter.line_segment([q, s], EStroke::new(r + rq, color));
        }
        painter.circle_filled(s, r, color);
        prev = Some((s, r));
    }
}

fn paint_cel(painter: &egui::Painter, map: &Mapping, strokes: &[Stroke], tint: Option<Color32>, alpha: f32) {
    for s in strokes {
        let pts: Vec<[f32; 3]> = s.points.iter().map(|p| [p.x, p.y, p.p]).collect();
        let c = match (s.brush, tint) {
            (Brush::Eraser, _) => continue,
            (_, Some(t)) => t.gamma_multiply(alpha),
            (_, None) => color32(s.color, alpha),
        };
        paint_stroke(painter, map, s.brush, s.size, &pts, c);
    }
}

/// The drawing clip the Brush tool paints on, with its drawing.
fn target(app: &WaltCutApp) -> Option<(ClipId, TrackItem, Drawing)> {
    let clip = waltcut_engine::drawing::target_clip(&app.session, &Value::Null)?;
    let seq = app.session.active_sequence()?;
    let (_, it) = seq.find_item(clip)?;
    let (_, d) = waltcut_engine::drawing::drawing_of(&app.session, seq, clip)?;
    Some((clip, it.clone(), d.clone()))
}

/// Current pen pressure: the host's tablet hook, else touch force from this frame, else full.
fn pressure(app: &WaltCutApp, ui: &egui::Ui) -> f32 {
    if !app.ui.brush.pressure {
        return 1.0;
    }
    if let Some(p) = app.hooks.pen_pressure.as_ref().and_then(|f| f()) {
        return p;
    }
    ui.input(|i| {
        i.events.iter().rev().find_map(|e| match e {
            egui::Event::Touch { force: Some(f), .. } => Some(*f),
            _ => None,
        })
    })
    .unwrap_or(1.0)
}

/// UI clock in nanoseconds.
fn now_ns(ui: &egui::Ui) -> u64 {
    let t = ui.input(|i| i.time);
    if t.is_finite() && t > 0.0 { (t * 1e9) as u64 } else { 0 }
}

/// The frame's pointer samples (see `drawing_input`), updating the input diagnostics.
fn frame_input(app: &mut WaltCutApp, ui: &egui::Ui, was_down: bool) -> Vec<Sample> {
    let mut platform: Vec<PenSample> = Vec::new();
    // The host queues pointer events whatever tool is active. When the Brush tool was not drawn on
    // the frame before (another tool, a dialog), what is queued is old: drop it and its loss count.
    let frame = ui.ctx().cumulative_frame_nr();
    let key = egui::Id::new("brush-last-input-frame");
    let stale = ui.data(|d| d.get_temp::<u64>(key)).is_none_or(|last| frame.saturating_sub(last) > 2);
    ui.data_mut(|d| d.insert_temp(key, frame));
    if let Some(f) = app.hooks.pen_samples.as_mut() {
        let r = f(&mut platform);
        if stale {
            platform.clear();
        } else {
            let st = &mut app.ui.brush.pen;
            st.queue_dropped += r.dropped;
            st.invalid += r.invalid;
        }
    }
    let events: Vec<(Pos2, Option<bool>)> = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::PointerMoved(p) => Some((*p, None)),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary | egui::PointerButton::Secondary, pressed, .. } => {
                    Some((*pos, Some(*pressed)))
                }
                _ => None,
            })
            .collect()
    });
    let input = FrameInput {
        platform: &platform,
        events: &events,
        was_down,
        pointer_now: ui.input(|i| i.pointer.latest_pos()),
        zoom: ui.ctx().zoom_factor(),
        fallback_pressure: pressure(app, ui),
        now_ns: now_ns(ui),
    };
    let (samples, source) = frame_samples(&input);
    let st = &mut app.ui.brush.pen;
    if source == Source::PenFallback {
        st.position_mismatches += 1;
    }
    if !samples.is_empty() {
        st.source = source;
    }
    samples
}

/// Add a sample to the live stroke. Returns whether it ended the stroke (the pen came up).
fn feed(l: &mut Live, s: &Sample, map: &Mapping) -> bool {
    let canvas = map.canvas(s.pos);
    match s.phase {
        PenPhase::Up => {
            l.stroke.finish(s, canvas);
            true
        }
        PenPhase::Down | PenPhase::Move => {
            l.stroke.push(s, canvas);
            false
        }
        PenPhase::Hover => false,
    }
}

fn run(app: &mut WaltCutApp, cmd: &str, p: Value) {
    if let Err(e) = app.session.execute(cmd, p) {
        app.ui.status = e.to_string();
    }
}

/// The Brush tool options bar, top-left over the picture.
fn options_bar(app: &mut WaltCutApp, ui: &mut egui::Ui, pic: Rect, has_target: bool) -> Rect {
    let area = egui::Area::new(egui::Id::new("brush-options")).fixed_pos(pic.min + vec2(8.0, 8.0)).order(egui::Order::Foreground);
    let pen = app.hooks.pen_pressure.as_ref().and_then(|f| f());
    let tablet = pen.is_some();
    let r = area.show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width((pic.width() - 32.0).max(120.0));
                let b = &mut app.ui.brush;
                for br in Brush::ALL {
                    ui.selectable_value(&mut b.brush, br, br.label());
                }
                ui.separator();
                let mut c32 = color32(b.color, 1.0);
                if egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::OnlyBlend).changed() {
                    let [r, g, bb, a] = c32.to_srgba_unmultiplied();
                    b.color = [r as f32 / 255.0, g as f32 / 255.0, bb as f32 / 255.0, a as f32 / 255.0];
                }
                ui.add(egui::Slider::new(&mut b.size, 1.0..=200.0).logarithmic(true).text("px"));
                ui.checkbox(&mut b.pressure, if tablet { "Pressure (tablet)" } else { "Pressure" });
                // live pen readout: confirms the tablet's pressure and buttons reach the app
                let (pb, sb, mb) = ui.input(|i| (i.pointer.primary_down(), i.pointer.secondary_down(), i.pointer.middle_down()));
                let held: Vec<&str> = [(pb, "tip"), (sb, "lower"), (mb, "upper")].iter().filter(|x| x.0).map(|x| x.1).collect();
                let readout = format!(
                    "{} · {}",
                    pen.map_or("mouse".to_string(), |p| format!("pen {:.0}%", p * 100.0)),
                    if held.is_empty() { "no buttons".to_string() } else { held.join("+") }
                );
                let r = ui.label(egui::RichText::new(readout).monospace().small());
                app.auto.add("program.brush.readout", r.rect, "pen readout");
                ui.separator();
                egui::ComboBox::from_id_salt("brush-cel-mode").selected_text(b.cel_mode.label()).show_ui(ui, |ui| {
                    for m in CelMode::ALL {
                        ui.selectable_value(&mut b.cel_mode, m, m.label());
                    }
                });
                ui.checkbox(&mut b.onion, "Onion skin");
                ui.separator();
                let step = b.tween_step;
                if !has_target {
                    if ui.button("New Drawing").on_hover_text("Add a drawing layer above the clips at the playhead").clicked() {
                        run(app, "drawing.new", json!({}));
                    }
                    return;
                }
                if ui.button("New Cel").clicked() {
                    run(app, "drawing.newCel", json!({}));
                }
                if ui.button("Duplicate").on_hover_text("Start a cel on this frame as a copy of the one held").clicked() {
                    run(app, "drawing.newCel", json!({"copy": true}));
                }
                if ui.button("Clear").clicked() {
                    run(app, "drawing.clearCel", json!({}));
                }
                if ui.button("In-between").on_hover_text(format!("Fill the frames up to the next cel with in-betweens on {step}s")).clicked() {
                    run(app, "drawing.tween", json!({"step": step}));
                }
            });
        });
    });
    r.response.rect
}

/// Draw the overlay and handle the Brush tool on the Program monitor picture `pic`.
pub fn monitor_overlay(app: &mut WaltCutApp, ui: &mut egui::Ui, pic: Rect, frame: (u32, u32)) {
    let tgt = target(app);
    let bar = options_bar(app, ui, pic, tgt.is_some());
    let Some((clip, item, d)) = tgt else {
        return;
    };
    let Some(map) = Mapping::new(app, &item, pic, frame) else { return };
    let painter = ui.painter().with_clip_rect(pic);
    let f = item.source_time_at(app.session.playhead().clamp(item.start, item.end() - waltcut_time::Tick(1)));
    let f = d.frame_at(f);
    if app.ui.brush.onion {
        let (prev, next) = d.neighbours(f);
        let a = app.ui.brush.onion_opacity.clamp(0.0, 1.0);
        if let Some(c) = prev {
            paint_cel(&painter, &map, &c.strokes, Some(Color32::from_rgb(230, 70, 70)), a);
        }
        if let Some(c) = next {
            paint_cel(&painter, &map, &c.strokes, Some(Color32::from_rgb(60, 190, 90)), a);
        }
    }
    let id = egui::Id::new("brush-overlay");
    let resp = ui.interact(pic, id, Sense::click_and_drag());
    app.auto.add("program.brush", pic, "brush canvas");
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        // brush outline under the pointer
        if let Some(p) = resp.hover_pos() {
            painter.circle_stroke(p, (app.ui.brush.size * 0.5 * map.scale).max(1.0), EStroke::new(1.0, Color32::from_white_alpha(160)));
        }
    }
    let live_id = id.with("live");
    let pending_id = id.with("pending");
    let mut live: Option<Live> = ui.data(|dd| dd.get_temp(live_id));
    let mut pending: Vec<Sample> = ui.data(|dd| dd.get_temp(pending_id)).unwrap_or_default();
    // the pen's lower barrel button reaches the app as the secondary button: it erases
    let secondary = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
    let middle = ui.input(|i| i.pointer.middle_down());
    let begin_wanted = resp.drag_started_by(egui::PointerButton::Primary)
        || resp.drag_started_by(egui::PointerButton::Secondary)
        || resp.clicked_by(egui::PointerButton::Primary)
        || resp.clicked_by(egui::PointerButton::Secondary)
        || (resp.is_pointer_button_down_on() && live.is_none() && !middle);
    // Every pointer sample of the frame, in order, with the pressure it was made at.
    let stream = frame_input(app, ui, live.is_some());
    // a stroke is abandoned when the window loses focus or Escape is pressed: nothing is committed
    let cancelled =
        ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false) | egui::Event::Key { key: egui::Key::Escape, pressed: true, .. })));
    let mut released = false;
    for s in stream {
        if let Some(l) = live.as_mut() {
            released |= feed(l, &s, &map);
        } else {
            match s.phase {
                PenPhase::Down => {
                    pending.clear();
                    pending.push(s);
                }
                _ if pending.is_empty() => {}
                _ => {
                    if pending.len() >= MAX_PENDING {
                        if let Some(last) = pending.last_mut() {
                            *last = s;
                        }
                    } else {
                        pending.push(s);
                    }
                }
            }
        }
        if released {
            break;
        }
    }
    if live.is_none() {
        let ended = pending.last().is_some_and(|s| s.phase == PenPhase::Up);
        let start = ui.input(|i| i.pointer.press_origin()).or_else(|| ui.input(|i| i.pointer.interact_pos()));
        if begin_wanted && !cancelled && start.is_some_and(|p| !bar.contains(p) && pic.contains(p)) {
            let mut l = Live { clip: clip.0, stroke: StrokeBuilder::default(), eraser: secondary };
            let mut samples = std::mem::take(&mut pending);
            if samples.first().is_none_or(|s| s.phase != PenPhase::Down) {
                // the press itself was not seen (no hook, or a drag that began elsewhere): start where it was pressed
                if let Some(p) = start {
                    samples.insert(0, Sample { t_ns: now_ns(ui), pos: p, pressure: pressure(app, ui), phase: PenPhase::Down });
                }
            }
            for s in samples {
                released |= feed(&mut l, &s, &map);
                if released {
                    break;
                }
            }
            live = Some(l);
        } else if ended || !ui.input(|i| i.pointer.any_down()) {
            // a press that is over and was not ours
            pending.clear();
        }
    }
    if cancelled && let Some(l) = live.take() {
        let st = &mut app.ui.brush.pen;
        st.cancelled += 1;
        st.samples += l.stroke.received;
        st.decimated += l.stroke.decimated;
        app.ui.status = "Stroke cancelled: nothing was drawn".to_string();
        released = false;
    }
    if let Some(l) = live.as_ref() {
        let b = &app.ui.brush;
        let brush = if l.eraser { Brush::Eraser } else { b.brush };
        let c = if brush == Brush::Eraser { Color32::from_white_alpha(120) } else { color32(b.color, 1.0) };
        paint_stroke(&painter, &map, brush, b.size, &l.stroke.points, c);
        ui.ctx().request_repaint();
    }
    // The stroke ends on the pen-up sample, or when the UI says the button is up (no sample seen).
    if live.is_some() && (released || !(resp.is_pointer_button_down_on() || resp.dragged())) {
        let l = live.take().unwrap_or_default();
        let b = app.ui.brush.clone();
        let st = &mut app.ui.brush.pen;
        st.samples += l.stroke.received;
        st.decimated += l.stroke.decimated;
        st.truncated += u64::from(l.stroke.truncated > 0);
        st.last_stroke_points = l.stroke.points.len() as u64;
        st.last_stroke_ms = l.stroke.duration_ms();
        st.last_stroke_hz = if st.last_stroke_ms > 0.0 { (st.last_stroke_points.saturating_sub(1)) as f64 * 1000.0 / st.last_stroke_ms } else { 0.0 };
        if l.stroke.truncated > 0 {
            app.ui.status = format!(
                "Stroke reached the {} sample limit: its end follows the pen but detail in between was lost",
                waltcut_project::drawing::MAX_STROKE_POINTS
            );
        }
        let stroke = json!({
            "brush": if l.eraser { Brush::Eraser.label() } else { b.brush.label() },
            "color": b.color,
            "size": b.size,
            "points": l.stroke.points,
        });
        let params = json!({"clip": l.clip, "cel": b.cel_mode.param(), "stroke": stroke});
        run(app, "drawing.addStroke", params);
    }
    ui.data_mut(|dd| {
        if let Some(l) = live {
            dd.insert_temp(live_id, l);
        } else {
            dd.remove::<Live>(live_id);
        }
        if pending.is_empty() {
            dd.remove::<Vec<Sample>>(pending_id);
        } else {
            dd.insert_temp(pending_id, pending);
        }
    });
    // which cel is showing, bottom-left
    let label = match d.cel_at(f) {
        Some(c) if c.frame == f => format!("Frame {f} · cel {}", c.frame),
        Some(c) => format!("Frame {f} · holding cel {}", c.frame),
        None => format!("Frame {f} · no cel yet"),
    };
    painter.text(pic.left_bottom() + vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM, label, egui::FontId::proportional(12.0), Color32::from_white_alpha(200));
}
