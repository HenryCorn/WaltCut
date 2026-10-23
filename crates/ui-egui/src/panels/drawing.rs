//! The Brush tool: drawing frame-by-frame animation on the Program monitor.
//!
//! With the Brush tool (Shift+P) a drag on the picture paints a stroke into the drawing clip
//! under the playhead (`drawing.addStroke`, one undo step per stroke). The stroke is previewed with
//! the egui painter while the pen is down, then rendered by the engine like any other layer.
//! Pressure comes from the host's pen hook (a drawing tablet on macOS) or from touch force, else
//! it is full. The neighbouring cels show as onion skins: the one before in red, the one after
//! in green.

use egui::{Color32, Pos2, Rect, Sense, Stroke as EStroke, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use waltcut_geom::{Affine, Vec2};
use waltcut_project::drawing::{Brush, Drawing, Stroke};
use waltcut_project::{ClipId, TrackItem};

use super::drawing_input::{FrameInput, PenPhase, PenSample, PenStats, Sample, Source, StrokeBuilder, frame_samples};
use super::drawing_raster::{SharedSlot, paint_onion, paint_stroke_preview};
use crate::WaltCutApp;
use crate::state::Tool;

/// Why strokes erase for now, if they do (the selected brush and its settings are untouched, so
/// there is nothing to restore but this flag).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EraseHold {
    #[default]
    Off,
    /// The key bound to Erase While Held is down.
    Key,
    /// Switched on by `drawing.eraseTemporarily {active: true}` until `{active: false}`.
    Command,
}

/// Brush sizes the size commands step through, in canvas pixels: bounded by the Brush options
/// slider (1..=200).
const SIZE_LADDER: [f32; 25] =
    [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 20.0, 24.0, 28.0, 32.0, 40.0, 48.0, 56.0, 64.0, 80.0, 96.0, 112.0, 128.0, 160.0, 200.0];
pub const MIN_BRUSH_SIZE: f32 = 1.0;
pub const MAX_BRUSH_SIZE: f32 = 200.0;
/// Most ladder steps one command takes.
const MAX_SIZE_STEPS: u64 = 100;

/// The brush size `steps` ladder steps up or down from `current`, never past the bounds. A
/// size between two steps moves to the next step in that direction; a non-finite size counts as
/// the default.
pub fn step_size(current: f32, up: bool, steps: u32) -> f32 {
    let mut size = if current.is_finite() { current.clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE) } else { BrushSettings::default().size };
    for _ in 0..steps.min(MAX_SIZE_STEPS as u32) {
        let next = if up { SIZE_LADDER.iter().copied().find(|s| *s > size + 1e-3) } else { SIZE_LADDER.iter().rev().copied().find(|s| *s < size - 1e-3) };
        match next {
            Some(n) => size = n,
            None => break,
        }
    }
    size
}

/// Parameter documentation of the Brush commands (shown by `ui.menu.list`).
pub fn params_doc(id: &str) -> Option<&'static str> {
    match id {
        "drawing.brushSizeUp" | "drawing.brushSizeDown" => Some(r#"{"steps":int?}  steps of the size ladder 1…200 px (1–100, default 1); stops at the bounds"#),
        "drawing.eraseTemporarily" => Some(
            r#"{"active":bool?}  true (default) erases until false, until the window loses focus or another tool is chosen; bound to a key it erases while the key is held"#,
        ),
        _ => None,
    }
}

/// The Brush commands act while the Brush tool is selected.
pub fn enabled(app: &WaltCutApp, id: &str) -> bool {
    params_doc(id).is_some() && app.ui.tool == Tool::Brush
}

/// `drawing.brushSizeUp`, `drawing.brushSizeDown` and `drawing.eraseTemporarily`.
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    params_doc(id)?;
    if app.ui.tool != Tool::Brush {
        return Some(Err(format!("`{id}` is available while the Brush tool is selected")));
    }
    Some(match id {
        "drawing.eraseTemporarily" => {
            let active = match params.get("active") {
                None | Some(Value::Null) => true,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Some(Err("`active` must be true or false".into())),
            };
            // from a key press the hold lasts as long as the key; from a command, until told otherwise
            let held_by_key = params.get("key").and_then(Value::as_bool).unwrap_or(false);
            app.ui.brush.erase_held = match (active, held_by_key) {
                (false, _) => EraseHold::Off,
                (true, true) => EraseHold::Key,
                (true, false) => EraseHold::Command,
            };
            Ok(json!({"erasing": active}))
        }
        _ => {
            let steps = match params.get("steps") {
                None | Some(Value::Null) => 1,
                Some(v) => match v.as_u64() {
                    Some(n) if (1..=MAX_SIZE_STEPS).contains(&n) => n as u32,
                    _ => return Some(Err(format!("`steps` must be a whole number from 1 to {MAX_SIZE_STEPS}"))),
                },
            };
            let before = app.ui.brush.size;
            let size = step_size(before, id == "drawing.brushSizeUp", steps);
            app.ui.brush.size = size;
            let at_limit = if id == "drawing.brushSizeUp" { size >= MAX_BRUSH_SIZE } else { size <= MIN_BRUSH_SIZE };
            Ok(json!({"size": size, "previous": before, "atLimit": at_limit}))
        }
    })
}

/// End a hold that no longer applies: its key came up, the window lost focus, or the Brush tool is
/// not selected any more. Called every frame before shortcuts are handled.
pub fn release_stale_erase(app: &mut WaltCutApp, ctx: &egui::Context) {
    if app.ui.brush.erase_held == EraseHold::Off {
        return;
    }
    // the loss itself, not the state: a window driven over the control channel is never focused
    let focus_lost = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))));
    let key_up = app.ui.brush.erase_held == EraseHold::Key
        && !app.bindings.iter().any(|(_, k, id, _)| id == "drawing.eraseTemporarily" && ctx.input(|i| i.key_down(*k)));
    if app.ui.tool != Tool::Brush || focus_lost || key_up {
        app.ui.brush.erase_held = EraseHold::Off;
    }
}

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
    /// Whether strokes erase for now (Erase While Held), whatever brush is selected. Never loaded
    /// back: the app starts with the brush it was left with.
    #[serde(skip_deserializing)]
    pub erase_held: EraseHold,
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
            erase_held: EraseHold::Off,
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

/// A stroke that was committed and whose frame the Program monitor has not shown yet.
#[derive(Clone, Debug)]
struct Committed {
    clip: u64,
    stroke: Stroke,
    /// The project revision that contains it.
    revision: u64,
    /// UI time of the commit, seconds.
    time: f64,
}

/// Give up waiting for the engine's frame after this long.
const COMMITTED_TIMEOUT_S: f64 = 3.0;

/// The live stroke as the project stroke it will become.
fn live_stroke(b: &BrushSettings, l: &Live) -> Stroke {
    Stroke {
        brush: if l.eraser { Brush::Eraser } else { b.brush },
        color: b.color,
        size: b.size,
        points: l.stroke.points.iter().map(|p| waltcut_project::drawing::StrokePoint { x: p[0], y: p[1], p: p[2] }).collect(),
    }
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
    fn canvas(&self, p: Pos2) -> (f32, f32) {
        let q = self.from_screen.apply(Vec2::new(p.x as f64, p.y as f64));
        (q.x as f32, q.y as f32)
    }
}

fn color32(c: [f32; 4], alpha: f32) -> Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(b(c[0]), b(c[1]), b(c[2]), b(c[3] * alpha))
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
                ui.add(egui::Slider::new(&mut b.size, MIN_BRUSH_SIZE..=MAX_BRUSH_SIZE).logarithmic(true).text("px"));
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
                if b.erase_held != EraseHold::Off {
                    // the selected brush is unchanged and comes back when the hold ends
                    let r = ui.label(egui::RichText::new(format!("Erasing (back to {})", b.brush.label())).strong());
                    app.auto.add("program.brush.erasing", r.rect, "temporary eraser");
                }
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
        // the neighbouring cels as the engine rasterises them (erasers applied), tinted, cached
        let (prev, next) = d.neighbours(f);
        let a = app.ui.brush.onion_opacity.clamp(0.0, 1.0);
        let slot = |name: &str| ui.data_mut(|dd| dd.get_temp_mut_or_default::<SharedSlot>(egui::Id::new(("brush-onion", name))).clone());
        let revision = app.session.revision;
        paint_onion(&painter, &slot("prev"), revision, clip.0, prev.map(|c| (c.frame, c.strokes.as_slice())), [230, 70, 70], a, &map.to_screen, pic);
        paint_onion(&painter, &slot("next"), revision, clip.0, next.map(|c| (c.frame, c.strokes.as_slice())), [60, 190, 90], a, &map.to_screen, pic);
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
            let mut l = Live { clip: clip.0, stroke: StrokeBuilder::default(), eraser: secondary || app.ui.brush.erase_held != EraseHold::Off };
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
    // The stroke just committed stays on screen until the engine's frame with it arrives.
    let committed_id = id.with("committed");
    let committed: Option<Committed> = ui.data(|dd| dd.get_temp(committed_id));
    if let Some(c) = committed {
        let arrived = app.program_picture().is_some_and(|k| k.revision >= c.revision);
        if arrived || ui.input(|i| i.time) - c.time > COMMITTED_TIMEOUT_S || c.clip != clip.0 {
            ui.data_mut(|dd| dd.remove::<Committed>(committed_id));
        } else {
            paint_stroke_preview(&painter, "brush-committed", &c.stroke, &map.to_screen, pic);
            ui.ctx().request_repaint();
        }
    }
    if let Some(l) = live.as_ref() {
        paint_stroke_preview(&painter, "brush-live", &live_stroke(&app.ui.brush, l), &map.to_screen, pic);
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
        let shown = live_stroke(&b, &l);
        run(app, "drawing.addStroke", params);
        let c = Committed { clip: l.clip, stroke: shown, revision: app.session.revision, time: ui.input(|i| i.time) };
        ui.data_mut(|dd| dd.insert_temp(committed_id, c));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_ladder_is_monotonic_bounded_and_stops_at_both_ends() {
        assert!(SIZE_LADDER.windows(2).all(|w| w[0] < w[1]));
        assert_eq!((SIZE_LADDER[0], SIZE_LADDER[SIZE_LADDER.len() - 1]), (MIN_BRUSH_SIZE, MAX_BRUSH_SIZE));
        let (mut up, mut down) = (6.0, 6.0);
        for _ in 0..200 {
            up = step_size(up, true, 1);
            down = step_size(down, false, 1);
        }
        assert_eq!((up, down), (MAX_BRUSH_SIZE, MIN_BRUSH_SIZE));
        assert_eq!(step_size(6.0, true, 3), 12.0);
        assert_eq!(step_size(6.0, false, 2), 4.0);
        // a size between two steps goes to the next step in that direction
        assert_eq!(step_size(7.0, true, 1), 8.0);
        assert_eq!(step_size(7.0, false, 1), 6.0);
        // a huge step count is bounded, not a long loop
        assert_eq!(step_size(6.0, true, u32::MAX), MAX_BRUSH_SIZE);
    }

    #[test]
    fn hostile_sizes_are_brought_back_in_range() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -5.0, 0.0, 1e30] {
            let s = step_size(bad, true, 1);
            assert!((MIN_BRUSH_SIZE..=MAX_BRUSH_SIZE).contains(&s), "{bad} -> {s}");
            let s = step_size(bad, false, 1);
            assert!((MIN_BRUSH_SIZE..=MAX_BRUSH_SIZE).contains(&s), "{bad} -> {s}");
        }
    }
}
