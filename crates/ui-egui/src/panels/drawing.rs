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
use waltcut_project::drawing::{Brush, Drawing, GhostRole, MAX_ONION_CELS, Stroke};
use waltcut_project::{ClipId, TrackItem};

use super::canvas_view::{CanvasView, PanHold};
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
        "drawing.onionSkin" => Some(
            r#"{"enabled":bool?,"before":0-4?,"after":0-4?,"opacity":0-1?,"falloff":0-1?,"tint":0-1?,"pin":frame|"current"|null?,"loop":bool?}  set the onion skin; with no parameter, report it. `pin` ghosts the cel starting on that frame (or the one shown now) until cleared with null; `loop` ghosts the first cel"#,
        ),
        "drawing.pinPose" => Some(r#"{}  pin the cel shown now for onion-skin comparison; again to unpin it"#),
        "drawing.smoothing" => Some(
            r#"{"mode":"off"|"smooth"|"rope"?,"strength":0-1?,"pressure":0-1?,"length":0-200?}  set stroke smoothing for strokes drawn from now on (smooth: position strength; rope: pulled-string length in canvas px at 100% view; pressure strength apart); with no parameter, report it. Off stores the pen's samples as they came; a stroke always ends exactly where the pen lifts"#,
        ),
        "drawing.eraseTemporarily" => Some(
            r#"{"active":bool?}  true (default) erases until false, until the window loses focus or another tool is chosen; bound to a key it erases while the key is held"#,
        ),
        _ => None,
    }
}

/// The Brush commands act while the Brush tool is selected.
pub fn enabled(app: &WaltCutApp, id: &str) -> bool {
    params_doc(id).is_some() && (is_onion(id) || app.ui.tool == Tool::Brush)
}

/// The onion-skin commands work whatever tool is selected: they change a view setting only.
fn is_onion(id: &str) -> bool {
    matches!(id, "drawing.onionSkin" | "drawing.pinPose" | "drawing.smoothing")
}

/// The drawing clip and the start frame of the cel shown at the playhead.
fn shown_cel(app: &WaltCutApp) -> Option<OnionPin> {
    let (clip, item, d) = target(app)?;
    let t = item.source_time_at(app.session.playhead().clamp(item.start, item.end() - waltcut_time::Tick(1)));
    Some(OnionPin { clip: clip.0, frame: d.cel_at(d.frame_at(t))?.frame })
}

/// The ghosts shown now, for `ui.inspect`: `[{role, frame, opacity}]` farthest first, empty when
/// the onion skin is off or there is no drawing under the playhead.
pub fn onion_report(app: &WaltCutApp) -> Value {
    let b = &app.ui.brush;
    let (Some((clip, item, d)), true) = (target(app), b.onion) else { return json!([]) };
    let t = item.source_time_at(app.session.playhead().clamp(item.start, item.end() - waltcut_time::Tick(1)));
    let look = b.onion_look();
    let pin = b.onion_pin.filter(|p| p.clip == clip.0).map(|p| p.frame);
    let ghosts = d.onion_plan(d.frame_at(t), look.before, look.after, pin, b.onion_loop);
    Value::Array(
        ghosts
            .iter()
            .filter_map(|g| {
                let (role, n) = match g.role {
                    GhostRole::Before(n) => ("before", n),
                    GhostRole::After(n) => ("after", n),
                    GhostRole::Pin => ("pin", 1),
                    GhostRole::LoopStart => ("loop", 1),
                };
                Some(json!({"role": role, "n": n, "frame": d.cels.get(g.index)?.frame, "opacity": look.opacity_at(n)}))
            })
            .collect(),
    )
}

fn onion_state(b: &BrushSettings) -> Value {
    json!({"enabled": b.onion, "before": b.onion_look().before, "after": b.onion_look().after, "opacity": b.onion_look().opacity,
        "falloff": b.onion_look().falloff, "tint": b.onion_look().tint, "pin": b.onion_pin, "loop": b.onion_loop})
}

/// `drawing.onionSkin` and `drawing.pinPose`. Every parameter is checked before any is applied.
fn route_onion(app: &mut WaltCutApp, id: &str, params: &Value) -> Result<Value, String> {
    if id == "drawing.smoothing" {
        return route_smoothing(app, params);
    }
    if id == "drawing.pinPose" {
        let shown = shown_cel(app).ok_or("there is no drawing cel at the playhead to pin")?;
        let b = &mut app.ui.brush;
        b.onion_pin = if b.onion_pin == Some(shown) { None } else { Some(shown) };
        return Ok(onion_state(b));
    }
    let obj = match params {
        Value::Null => None,
        Value::Object(o) => Some(o),
        _ => return Err("parameters must be an object".into()),
    };
    let known = ["enabled", "before", "after", "opacity", "falloff", "tint", "pin", "loop"];
    if let Some(o) = obj
        && let Some(k) = o.keys().find(|k| !known.contains(&k.as_str()))
    {
        return Err(format!("unknown parameter `{k}`; expected: {}", known.join(", ")));
    }
    let get = |k: &str| obj.and_then(|o| o.get(k));
    let flag = |k: &str| match get(k) {
        None => Ok(None),
        Some(Value::Bool(v)) => Ok(Some(*v)),
        Some(_) => Err(format!("`{k}` must be true or false")),
    };
    let count = |k: &str| match get(k) {
        None => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n as usize <= MAX_ONION_CELS => Ok(Some(n as u8)),
            _ => Err(format!("`{k}` must be a whole number from 0 to {MAX_ONION_CELS}")),
        },
    };
    let unit = |k: &str| match get(k) {
        None => Ok(None),
        Some(v) => match v.as_f64() {
            Some(n) if (0.0..=1.0).contains(&n) => Ok(Some(n as f32)),
            _ => Err(format!("`{k}` must be a number from 0 to 1")),
        },
    };
    let (enabled, looped) = (flag("enabled")?, flag("loop")?);
    let (before, after) = (count("before")?, count("after")?);
    let (opacity, falloff, tint) = (unit("opacity")?, unit("falloff")?, unit("tint")?);
    let pin = match get("pin") {
        None => None,
        Some(Value::Null) | Some(Value::Bool(false)) => Some(None),
        Some(Value::String(s)) if s == "current" => Some(Some(shown_cel(app).ok_or("there is no drawing cel at the playhead to pin")?)),
        Some(v) => {
            let frame = v.as_i64().filter(|f| *f >= 0).ok_or("`pin` must be a cel's start frame, \"current\" or null")?;
            let (clip, _, d) = target(app).ok_or("there is no drawing to pin a cel of")?;
            d.cel_on(frame).ok_or_else(|| format!("no cel starts on frame {frame}"))?;
            Some(Some(OnionPin { clip: clip.0, frame }))
        }
    };
    let b = &mut app.ui.brush;
    b.onion = enabled.unwrap_or(b.onion);
    b.onion_loop = looped.unwrap_or(b.onion_loop);
    b.onion_before = before.unwrap_or(b.onion_before);
    b.onion_after = after.unwrap_or(b.onion_after);
    b.onion_opacity = opacity.unwrap_or(b.onion_opacity);
    b.onion_falloff = falloff.unwrap_or(b.onion_falloff);
    b.onion_tint = tint.unwrap_or(b.onion_tint);
    if let Some(p) = pin {
        b.onion_pin = p;
    }
    Ok(onion_state(b))
}

fn smoothing_state(s: &super::drawing_smooth::Smoothing) -> Value {
    json!({"mode": s.mode.param(), "strength": s.position(), "pressure": s.pressure(), "length": s.rope_length(), "active": s.is_active()})
}

/// `drawing.smoothing`. Every parameter is checked before any is applied.
fn route_smoothing(app: &mut WaltCutApp, params: &Value) -> Result<Value, String> {
    use super::drawing_smooth::SmoothMode;
    let obj = match params {
        Value::Null => None,
        Value::Object(o) => Some(o),
        _ => return Err("parameters must be an object".into()),
    };
    if let Some(o) = obj
        && let Some(k) = o.keys().find(|k| !["mode", "strength", "pressure", "length"].contains(&k.as_str()))
    {
        return Err(format!("unknown parameter `{k}`; expected: mode, strength, pressure, length"));
    }
    let get = |k: &str| obj.and_then(|o| o.get(k)).filter(|v| !v.is_null());
    let mode = match get("mode").map(|v| v.as_str()) {
        None => None,
        Some(Some("off")) => Some(SmoothMode::Off),
        Some(Some("smooth")) => Some(SmoothMode::Smooth),
        Some(Some("rope")) => Some(SmoothMode::Rope),
        Some(_) => return Err("`mode` must be \"off\", \"smooth\" or \"rope\"".into()),
    };
    let unit = |k: &str| match get(k) {
        None => Ok(None),
        Some(v) => match v.as_f64() {
            Some(n) if (0.0..=1.0).contains(&n) => Ok(Some(n as f32)),
            _ => Err(format!("`{k}` must be a number from 0 to 1")),
        },
    };
    let (strength, pressure) = (unit("strength")?, unit("pressure")?);
    let length = match get("length") {
        None => None,
        Some(v) => match v.as_f64() {
            Some(n) if (0.0..=super::drawing_smooth::MAX_ROPE_LENGTH as f64).contains(&n) => Some(n as f32),
            _ => return Err(format!("`length` must be a number from 0 to {}", super::drawing_smooth::MAX_ROPE_LENGTH)),
        },
    };
    let st = &mut app.ui.brush.smoothing;
    st.mode = mode.unwrap_or(st.mode);
    st.strength = strength.unwrap_or(st.strength);
    st.pressure = pressure.unwrap_or(st.pressure);
    st.length = length.unwrap_or(st.length);
    Ok(smoothing_state(st))
}

/// The Feel menu of the options bar: stroke smoothing and the pressure response.
fn smoothing_menu(ui: &mut egui::Ui, b: &mut BrushSettings, auto: &mut crate::automation::Registry) {
    use super::drawing_smooth::SmoothMode;
    // the label never changes width; it turns blue while smoothing or a pressure curve is in use
    let label = egui::RichText::new("Feel");
    let label =
        if b.smoothing.mode == SmoothMode::Off && b.response.is_default() { label } else { label.strong().color(egui::Color32::from_rgb(70, 130, 230)) };
    let r = ui.menu_button(label, |ui| {
        let mut on = b.smoothing.mode == SmoothMode::Smooth;
        let r = ui.checkbox(&mut on, "Smooth strokes");
        auto.add("program.brush.smooth", r.rect, "stroke smoothing");
        if r.changed() {
            b.smoothing.mode = if on { SmoothMode::Smooth } else { SmoothMode::Off };
        }
        ui.add_enabled_ui(b.smoothing.mode == SmoothMode::Smooth, |ui| {
            let r = ui.add(egui::Slider::new(&mut b.smoothing.strength, 0.0..=1.0).text("Position"));
            auto.add("program.brush.smooth.strength", r.rect, "position smoothing");
        });
        let mut rope = b.smoothing.mode == SmoothMode::Rope;
        let r = ui.checkbox(&mut rope, "Rope (pulled string)");
        auto.add("program.brush.rope", r.rect, "pulled-string stabilizer");
        if r.changed() {
            b.smoothing.mode = if rope { SmoothMode::Rope } else { SmoothMode::Off };
        }
        ui.add_enabled_ui(b.smoothing.mode == SmoothMode::Rope, |ui| {
            let r = ui.add(egui::Slider::new(&mut b.smoothing.length, 0.0..=super::drawing_smooth::MAX_ROPE_LENGTH).text("String length"));
            auto.add("program.brush.rope.length", r.rect, "rope length in canvas pixels");
        });
        ui.add_enabled_ui(b.smoothing.mode != SmoothMode::Off, |ui| {
            let r = ui.add(egui::Slider::new(&mut b.smoothing.pressure, 0.0..=1.0).text("Pressure"));
            auto.add("program.brush.smooth.pressure", r.rect, "pressure smoothing");
        });
        ui.label(egui::RichText::new("The line trails the pen a little while smoothing or the rope is on.").small());
        super::pressure_curve::section(ui, b, auto);
    });
    auto.add("program.brush.feel.menu", r.response.rect, "pen feel: smoothing and pressure response");
}

/// `drawing.brushSizeUp`, `drawing.brushSizeDown` and `drawing.eraseTemporarily`.
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    params_doc(id)?;
    if is_onion(id) {
        return Some(route_onion(app, id, params));
    }
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
    pub(super) fn param(self) -> &'static str {
        match self {
            CelMode::Hold => "hold",
            CelMode::New => "new",
            CelMode::Copy => "copy",
        }
    }
}

/// The cel pinned for comparison: the cel starting on `frame` of the drawing clip `clip`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnionPin {
    pub clip: u64,
    pub frame: i64,
}

/// The most opaque an onion skin is ever drawn: a ghost is a guide, never a solid layer that
/// hides the picture and the cel being drawn.
pub const MAX_ONION_OPACITY: f32 = 0.6;

/// The onion-skin settings as they are used: every number inside its bounds (a hand-edited UI
/// state or `ui.set` cannot make a ghost solid, negative or endless).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnionLook {
    pub before: usize,
    pub after: usize,
    pub opacity: f32,
    pub falloff: f32,
    pub tint: f32,
}

impl OnionLook {
    /// Opacity of the `n`-th nearest ghost on its side (`n` = 1 is the nearest).
    pub fn opacity_at(&self, n: usize) -> f32 {
        let steps = i32::try_from(n.saturating_sub(1)).unwrap_or(i32::MAX);
        (self.opacity * self.falloff.powi(steps)).clamp(0.0, MAX_ONION_OPACITY)
    }
}

impl BrushSettings {
    pub fn onion_look(&self) -> OnionLook {
        let unit = |v: f32, default: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { default };
        OnionLook {
            before: usize::from(self.onion_before).min(MAX_ONION_CELS),
            after: usize::from(self.onion_after).min(MAX_ONION_CELS),
            opacity: unit(self.onion_opacity, 0.35),
            falloff: unit(self.onion_falloff, 0.6),
            tint: unit(self.onion_tint, 1.0),
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
    /// Opacity of the nearest onion skin, 0..1 (shown at most [`MAX_ONION_OPACITY`]).
    pub onion_opacity: f32,
    /// Distinct cels before / after the one shown that get a ghost, 0..=4 (default one each).
    pub onion_before: u8,
    pub onion_after: u8,
    /// Each ghost further away is this fraction of the opacity of the one before it, 0..1.
    pub onion_falloff: f32,
    /// How far the ghosts take their tint rather than their own colours, 0..1.
    pub onion_tint: f32,
    /// A pose pinned for comparison, shown whatever the playhead does.
    pub onion_pin: Option<OnionPin>,
    /// Also ghost the drawing's first cel, to compare with the end of a loop.
    pub onion_loop: bool,
    /// A drag on the picture moves the target drawing's reference instead of drawing (#3). Never
    /// loaded back: a session starts drawing.
    #[serde(skip_deserializing)]
    pub reference_move: bool,
    /// Lasso mode (#66): a drag selects strokes instead of drawing. Never loaded back.
    #[serde(skip_deserializing)]
    pub lasso: bool,
    /// Fill mode (#68): a click fills the region under it instead of a drag drawing. Never loaded
    /// back.
    #[serde(skip_deserializing)]
    pub fill: bool,
    /// How different from the clicked pixel a pixel may be and still be filled (0-255).
    pub fill_tolerance: u8,
    /// Closing radius of a fill, canvas pixels (0-8): openings up to twice this are closed (#69).
    pub fill_gap: u8,
    /// Edge pixels a fill adds under lines beyond its one-pixel shell (0-8) (#69).
    pub fill_grow: u8,
    /// The drawing clip a fill finds its region in; `None` is the drawing itself (#69). Never
    /// loaded back: clip ids belong to a project.
    #[serde(skip_deserializing)]
    pub fill_reference: Option<u64>,
    /// A click previews the region instead of filling (#69). Never loaded back.
    #[serde(skip_deserializing)]
    pub fill_preview: bool,
    /// The previewed fill waiting for Apply or Cancel. Never saved or loaded back.
    #[serde(skip_deserializing)]
    pub fill_pending: Option<super::fill::PendingFill>,
    /// The selected strokes and their pending transform. Never saved or loaded back.
    #[serde(skip_deserializing)]
    pub selection: Option<super::stroke_select::StrokeSelection>,
    /// The temporary shift of the pinned pose (#64). Never saved or loaded back.
    #[serde(skip_deserializing)]
    pub light_table: super::light_table::LightTable,
    /// The in-between assist's pairing table and last answer (#72). Never saved or loaded back.
    #[serde(skip_deserializing)]
    pub inbetween: super::inbetween::Assist,
    /// Stroke smoothing (#57): Off stores what the pen sent.
    pub smoothing: super::drawing_smooth::Smoothing,
    /// How a pen's pressure becomes width and opacity (#56); stored with each new stroke when it
    /// is not the brush's own response.
    pub response: waltcut_project::drawing::PressureResponse,
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
            onion_before: 1,
            onion_after: 1,
            onion_falloff: 0.6,
            onion_tint: 1.0,
            onion_pin: None,
            onion_loop: false,
            reference_move: false,
            lasso: false,
            fill: false,
            fill_tolerance: waltcut_engine::drawing_fill::DEFAULT_TOLERANCE,
            fill_gap: 0,
            fill_grow: 0,
            fill_reference: None,
            fill_preview: false,
            fill_pending: None,
            selection: None,
            light_table: Default::default(),
            inbetween: Default::default(),
            smoothing: Default::default(),
            response: Default::default(),
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
        response: b.stroke_response(),
        fill: None,
    }
}

/// Samples of a press that has not become a stroke (yet): the press, and the moves after it, until
/// the canvas decides the press belongs to it. Bounded, newest replacing the last when full.
const MAX_PENDING: usize = 512;

/// Picture ↔ drawing canvas mapping for one clip at the playhead.
pub(super) struct Mapping {
    /// canvas px → screen points
    pub(super) to_screen: Affine,
    from_screen: Affine,
    /// screen points per canvas pixel
    pub(super) scale: f32,
}

impl Mapping {
    /// `pic` is the picture's layout rectangle; `turn` is how the monitor shows it (#63), so a
    /// screen point maps back through the view, then the zoom, then the clip's Motion.
    fn new(app: &WaltCutApp, item: &TrackItem, pic: Rect, frame: (u32, u32), turn: &CanvasView) -> Option<Mapping> {
        let seq = app.session.active_sequence()?;
        let src = app.session.project.source_size(item.item)?;
        let par = app.session.project.source_par(item.item);
        let mt = item.effect_time_at(app.session.playhead());
        let motion = waltcut_render::motion_matrix(seq, item, src, par, mt);
        let (sx, sy) = (pic.width() as f64 / frame.0.max(1) as f64, pic.height() as f64 / frame.1.max(1) as f64);
        // canvas → sequence frame (Motion) → picture → screen; `then_apply` applies its argument first
        let layout = Affine::translate(pic.min.x as f64, pic.min.y as f64).then_apply(&Affine::scale(sx, sy)).then_apply(&motion);
        let to_screen = turn.affine(pic.center()).then_apply(&layout);
        let from_screen = to_screen.inverse()?;
        let scale = to_screen.determinant().abs().sqrt() as f32;
        (scale.is_finite() && scale > 0.0).then_some(Mapping { to_screen, from_screen, scale })
    }
    pub(super) fn canvas(&self, p: Pos2) -> (f32, f32) {
        let q = self.from_screen.apply(Vec2::new(p.x as f64, p.y as f64));
        (q.x as f32, q.y as f32)
    }
}

fn color32(c: [f32; 4], alpha: f32) -> Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(b(c[0]), b(c[1]), b(c[2]), b(c[3] * alpha))
}

/// The drawing clip the Brush tool paints on, with its drawing.
pub(super) fn target(app: &WaltCutApp) -> Option<(ClipId, TrackItem, std::sync::Arc<Drawing>)> {
    let clip = waltcut_engine::drawing::target_clip(&app.session, &Value::Null)?;
    let seq = app.session.active_sequence()?;
    let (_, it) = seq.find_item(clip)?;
    // the project item holds the drawing behind an `Arc`: take a handle, not a copy of every cel
    let waltcut_project::ItemKind::Drawing(d) = &app.session.project.item(it.item)?.kind else { return None };
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

/// The onion-skin settings menu of the options bar.
fn onion_menu(ui: &mut egui::Ui, b: &mut BrushSettings, auto: &mut crate::automation::Registry) {
    let r = ui.menu_button("Onion", |ui| {
        let mut row = |ui: &mut egui::Ui, id: &str, w: egui::Slider<'_>, text: &str| {
            let r = ui.add(w.text(text));
            auto.add(id, r.rect, text);
        };
        let max = MAX_ONION_CELS as u8;
        row(ui, "program.brush.onion.before", egui::Slider::new(&mut b.onion_before, 0..=max), "Cels before");
        row(ui, "program.brush.onion.after", egui::Slider::new(&mut b.onion_after, 0..=max), "Cels after");
        row(ui, "program.brush.onion.opacity", egui::Slider::new(&mut b.onion_opacity, 0.0..=1.0), "Opacity");
        row(ui, "program.brush.onion.falloff", egui::Slider::new(&mut b.onion_falloff, 0.0..=1.0), "Fade per cel");
        row(ui, "program.brush.onion.tint", egui::Slider::new(&mut b.onion_tint, 0.0..=1.0), "Tint");
        let r = ui.checkbox(&mut b.onion_loop, "First cel (loop start)");
        auto.add("program.brush.onion.loop", r.rect, "loop start ghost");
        if b.onion_pin.is_some() && ui.button("Unpin pose").clicked() {
            b.onion_pin = None;
        }
    });
    auto.add("program.brush.onion.menu", r.response.rect, "onion skin settings");
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
                let r = ui.checkbox(&mut b.onion, "Onion skin");
                app.auto.add("program.brush.onion", r.rect, "onion skin");
                onion_menu(ui, b, &mut app.auto);
                smoothing_menu(ui, b, &mut app.auto);
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
                if ui.button("Duplicate").on_hover_text("Start a cel on this frame as an independent copy of the one held").clicked() {
                    run(app, "drawing.newCel", json!({"copy": true}));
                }
                super::cel_link::controls(app, ui);
                if ui.button("Clear").clicked() {
                    run(app, "drawing.clearCel", json!({}));
                }
                if ui.button("In-between").on_hover_text(format!("Fill the frames up to the next cel with in-betweens on {step}s")).clicked() {
                    run(app, "drawing.tween", json!({"step": step}));
                }
                super::inbetween::controls(app, ui);
                super::light_table::controls(app, ui);
                super::reference::menu(app, ui);
                super::fill::controls(app, ui);
                super::stroke_select::controls(app, ui);
            });
        });
    });
    r.response.rect
}

/// Tints of the ghosts: red before, green after, blue for the pinned pose, amber for the loop start.
const TINT_BEFORE: [u8; 3] = [230, 70, 70];
const TINT_AFTER: [u8; 3] = [60, 190, 90];
const TINT_PIN: [u8; 3] = [70, 130, 230];
const TINT_LOOP: [u8; 3] = [230, 170, 50];

/// Names of the cached ghost rasters: one per place a ghost can take.
fn ghost_slots() -> Vec<(GhostRole, String)> {
    let mut v = vec![(GhostRole::LoopStart, "loop".to_string()), (GhostRole::Pin, "pin".to_string())];
    for n in 1..=MAX_ONION_CELS {
        v.push((GhostRole::Before(n), format!("before{n}")));
        v.push((GhostRole::After(n), format!("after{n}")));
    }
    v
}

/// The onion skins of the cel shown on drawing frame `f`: the neighbouring cels as the engine
/// rasterises them (erasers applied), tinted, cached. Painted over the picture only: nothing here
/// is stored in the project or reaches an export.
#[allow(clippy::too_many_arguments)]
fn paint_ghosts(app: &WaltCutApp, ui: &mut egui::Ui, painter: &egui::Painter, clip: u64, d: &Drawing, f: i64, to_screen: &Affine, pic: Rect) {
    let b = &app.ui.brush;
    let look = b.onion_look();
    let pin = b.onion_pin.filter(|p| p.clip == clip).map(|p| p.frame);
    let plan = d.onion_plan(f, look.before, look.after, pin, b.onion_loop);
    let revision = app.session.revision;
    // the pinned pose may be shifted by the light table (#64): it is placed, not edited
    let shifted = super::light_table::placement(app, d).map(|(index, m)| (index, to_screen.then_apply(&m)));
    for (role, name) in ghost_slots() {
        let slot = ui.data_mut(|dd| dd.get_temp_mut_or_default::<SharedSlot>(egui::Id::new(("brush-onion", name.as_str()))).clone());
        let ghost = plan.iter().find(|g| g.role == role);
        let cel = ghost.and_then(|g| d.cels.get(g.index));
        let place = match (ghost, &shifted) {
            (Some(g), Some((index, m))) if g.index == *index => m,
            _ => to_screen,
        };
        let (tint, n) = match role {
            GhostRole::Before(n) => (TINT_BEFORE, n),
            GhostRole::After(n) => (TINT_AFTER, n),
            GhostRole::Pin => (TINT_PIN, 1),
            GhostRole::LoopStart => (TINT_LOOP, 1),
        };
        paint_onion(painter, &slot, revision, clip, cel.map(|c| (c.frame, c.strokes.as_slice())), tint, look.tint, look.opacity_at(n), place, pic);
    }
}

/// Draw the overlay and handle the Brush tool on the Program monitor picture `pic`.
pub fn monitor_overlay(app: &mut WaltCutApp, ui: &mut egui::Ui, layout_pic: Rect, frame: (u32, u32)) {
    let turn = app.ui.program.canvas;
    // what the pen sees: the rectangle that bounds the (possibly turned) picture
    let pic = turn.bounds(layout_pic);
    let panning = turn.pan_held != PanHold::Off;
    // Shift pose: a drag moves the pinned onion-skin pose instead of drawing (#64)
    let shifting = !panning && app.ui.brush.light_table.hold != super::light_table::ShiftHold::Off;
    // Move with pen: a drag moves the reference instead of drawing (#3)
    let moving_ref = !panning && !shifting && super::reference::moving(app);
    // Lasso mode: a drag selects strokes (or moves the pending transform) instead of drawing (#66)
    let lassoing = !panning && !shifting && !moving_ref && app.ui.brush.lasso;
    // Fill mode (#68): a click fills the region under it, a drag draws nothing
    let filling = !panning && !shifting && !moving_ref && !lassoing && app.ui.brush.fill;
    let tgt = target(app);
    // the options bar sits where it does on an upright picture: a turned one is narrow
    let bar = options_bar(app, ui, layout_pic, tgt.is_some());
    let Some((clip, item, d)) = tgt else {
        return;
    };
    let Some(map) = Mapping::new(app, &item, layout_pic, frame, &turn) else { return };
    let painter = ui.painter().with_clip_rect(pic);
    let f = item.source_time_at(app.session.playhead().clamp(item.start, item.end() - waltcut_time::Tick(1)));
    let f = d.frame_at(f);
    if app.ui.brush.onion {
        paint_ghosts(app, ui, &painter, clip.0, &d, f, &map.to_screen, pic);
    }
    super::light_table::paint_indicator(app, &painter, &map, &d, bar.left_bottom());
    super::inbetween::paint_overlay(app, &painter, &map, bar.left_bottom() + vec2(0.0, 24.0));
    super::inbetween::window(app, ui.ctx(), clip.0, &d, f);
    let id = egui::Id::new("brush-overlay");
    // while the pan is held the drag belongs to the pan, not to a stroke
    let resp = ui.interact(pic, id, if panning { Sense::hover() } else { Sense::click_and_drag() });
    app.auto.add("program.brush", pic, "brush canvas");
    if panning {
        let grabbing = ui.input(|i| i.pointer.primary_down());
        ui.ctx().set_cursor_icon(if grabbing { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    } else if shifting {
        super::light_table::drag(app, ui, &resp, &map, &d);
    } else if lassoing {
        super::stroke_select::overlay(app, ui, &painter, &resp, &map, &d, f);
    } else if filling {
        super::fill::overlay(app, ui, &painter, &resp, &map, bar, clip.0);
    } else if moving_ref {
        let grabbing = ui.input(|i| i.pointer.primary_down());
        ui.ctx().set_cursor_icon(if grabbing { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        // one drag is one undo step: its first move starts it, the rest join it
        let first_id = id.with("reference-first");
        if resp.drag_started_by(egui::PointerButton::Primary) {
            ui.data_mut(|dd| dd.insert_temp(first_id, true));
        }
        let moved = resp.drag_delta();
        if resp.dragged_by(egui::PointerButton::Primary) && (moved.x != 0.0 || moved.y != 0.0) {
            // screen → picture → sequence pixels, through the view (the reference is placed in the sequence's pixels)
            let c = layout_pic.center();
            let (a, b) = (turn.unshow(c, c), turn.unshow(c, c + moved));
            let (sx, sy) = (frame.0 as f32 / layout_pic.width().max(1.0), frame.1 as f32 / layout_pic.height().max(1.0));
            let first = ui.data_mut(|dd| dd.remove_temp::<bool>(first_id)).unwrap_or(false);
            let merge = if first { json!("start") } else { json!(true) };
            run(app, "reference.set", json!({"dx": ((b.x - a.x) * sx) as f64, "dy": ((b.y - a.y) * sy) as f64, "merge": merge}));
        }
    } else if resp.hovered() {
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
    let begin_wanted = !panning
        && !shifting
        && !moving_ref
        && !lassoing
        && !filling
        && (resp.drag_started_by(egui::PointerButton::Primary)
            || resp.drag_started_by(egui::PointerButton::Secondary)
            || resp.clicked_by(egui::PointerButton::Primary)
            || resp.clicked_by(egui::PointerButton::Secondary)
            || (resp.is_pointer_button_down_on() && live.is_none() && !middle));
    // Every pointer sample of the frame, in order, with the pressure it was made at.
    let stream = frame_input(app, ui, live.is_some());
    // a drag that pans is not a stroke: its pen samples are dropped
    let stream = if (panning || shifting || moving_ref || lassoing || filling) && live.is_none() { Vec::new() } else { stream };
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
        if begin_wanted && !cancelled && start.is_some_and(|p| !bar.contains(p) && layout_pic.contains(turn.unshow(layout_pic.center(), p))) {
            let mut l = Live {
                clip: clip.0,
                stroke: StrokeBuilder::with_smoothing(&app.ui.brush.smoothing, map.scale),
                eraser: secondary || app.ui.brush.erase_held != EraseHold::Off,
            };
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
        let mut stroke = json!({
            "brush": if l.eraser { Brush::Eraser.label() } else { b.brush.label() },
            "color": b.color,
            "size": b.size,
            "points": l.stroke.points,
        });
        if let (Some(r), Some(o)) = (b.stroke_response(), stroke.as_object_mut()) {
            o.insert("response".into(), serde_json::to_value(r).unwrap_or(Value::Null));
        }
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
    let label = if moving_ref { format!("{label} · drag to move the reference") } else { label };
    let label = if shifting { format!("{label} · drag to shift the pinned pose (Shift-drag turns it)") } else { label };
    let label = if lassoing { format!("{label} · lasso: drag around strokes") } else { label };
    let label = if filling { format!("{label} · fill: click inside a shape") } else { label };
    painter.text(pic.left_bottom() + vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM, label, egui::FontId::proportional(12.0), Color32::from_white_alpha(200));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghost_opacity_fades_with_distance_and_is_never_solid() {
        let look = BrushSettings::default().onion_look();
        assert_eq!((look.before, look.after, look.opacity_at(1)), (1, 1, 0.35));
        assert!((look.opacity_at(2) - 0.21).abs() < 1e-6 && look.opacity_at(3) < look.opacity_at(2));
        let wild =
            BrushSettings { onion_opacity: 9.0, onion_falloff: f32::NAN, onion_tint: -4.0, onion_before: 255, onion_after: 255, ..BrushSettings::default() }
                .onion_look();
        assert_eq!((wild.before, wild.after, wild.tint), (MAX_ONION_CELS, MAX_ONION_CELS, 0.0));
        for n in [0, 1, 2, usize::MAX] {
            assert!(wild.opacity_at(n) <= MAX_ONION_OPACITY && wild.opacity_at(n) >= 0.0);
        }
        // settings saved before these existed load with the defaults
        let old: BrushSettings = serde_json::from_value(json!({"onion": true, "onion_opacity": 0.5})).unwrap();
        assert_eq!((old.onion_before, old.onion_after, old.onion_pin, old.onion_loop), (1, 1, None, false));
    }

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
