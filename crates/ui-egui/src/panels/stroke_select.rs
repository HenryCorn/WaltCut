//! Lasso-select and transform whole strokes (#66) in the Brush tool. The rules and the stroke
//! maths are in `waltcut_project::drawing_select`; the edit is the engine's
//! `drawing.transformStrokes` (one undo step). This module is the view state around them: the
//! Lasso mode, the selection, the **pending** transform that is only previewed, and the controls.
//!
//! * **Lasso** (Brush options bar, `drawing.lassoMode`, `Alt+L`): while it is on, a drag that
//!   starts outside the selection draws a lasso; letting go selects the paint strokes of the cel
//!   shown with more than half of their points inside it (`drawing.selectStrokes {lasso}`). A drag
//!   that starts inside the selection's box moves the pending transform. Nothing is drawn.
//! * **Pending transform** (`drawing.selectionTransform {dx?, dy?, rotation?, scale?}`): move
//!   (canvas pixels), rotate (degrees, clockwise) and uniformly scale about the selection's centre.
//!   It is previewed over the picture (the selection in blue, the result in orange) and changes no
//!   artwork. A transform the rules refuse is shown with the reason and cannot be applied.
//! * **Apply** (the Apply button, `drawing.selectionApply` / `Alt+Enter`, or
//!   `drawing.selectionTransform {apply: true}`) makes it one undo step;
//!   **Cancel** (`{cancel: true}`, Escape) drops it; **Clear** (`drawing.selectStrokes {clear:
//!   true}`, a second Escape) drops the selection. The selection also goes when the project
//!   changes under it, and none of this state is saved or loaded back.
//!
//! Automation ids: `program.brush.lasso`, `program.brush.lasso.<dx|dy|rotation|scale|apply|cancel|
//! clear|status>`. `ui.inspect` reports `ui.brush.lasso` and `ui.brush.selection`.

use egui::{Color32, Painter, Pos2, Rect, Stroke as EStroke, Ui, vec2};
use serde::Serialize;
use serde_json::{Value, json};
use waltcut_geom::Vec2;
use waltcut_project::drawing::{Drawing, Stroke};
use waltcut_project::drawing_select::{MAX_LASSO_POINTS, Transform, transform_selection};

use super::drawing::Mapping;
use crate::WaltCutApp;
use crate::state::Tool;

/// The pending transform, in canvas pixels and degrees.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Pending {
    pub dx: f64,
    pub dy: f64,
    pub rotation: f64,
    pub scale: f64,
}

impl Default for Pending {
    fn default() -> Self {
        Self { dx: 0.0, dy: 0.0, rotation: 0.0, scale: 1.0 }
    }
}

impl Pending {
    pub fn is_identity(&self) -> bool {
        *self == Pending::default()
    }
    fn transform(&self, pivot: (f64, f64)) -> Transform {
        Transform { dx: self.dx, dy: self.dy, rotation: self.rotation, scale: self.scale, pivot }
    }
}

/// The strokes of one cel that are selected, with what moves with them.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct StrokeSelection {
    pub clip: u64,
    /// Start frame of the cel the indices refer to.
    pub cel: i64,
    /// The project revision the selection was made on: any edit drops it.
    pub revision: u64,
    pub strokes: Vec<usize>,
    pub erasers: Vec<usize>,
    pub conflict: Option<String>,
    pub centre: Option<[f64; 2]>,
    pub pending: Pending,
}

pub fn params_doc(id: &str) -> Option<&'static str> {
    match id {
        "drawing.lassoMode" => Some(
            r#"{"enabled":bool?}  drags on the picture lasso strokes (or move the pending transform) instead of drawing; toggles when `enabled` is absent"#,
        ),
        "drawing.selectStrokes" => {
            Some(r#"{"lasso":[[x,y],...]|"clear":true}  select the paint strokes of the cel shown that the lasso (canvas pixels) hits, or drop the selection"#)
        }
        "drawing.selectionApply" => {
            Some("{}  apply the pending transform of the selection as one undo step (the same as `drawing.selectionTransform {apply: true}`)")
        }
        "drawing.selectionTransform" => Some(
            r#"{"dx":px?,"dy":px?,"rotation":deg?,"scale":f?,"apply":bool?,"cancel":bool?}  set the pending transform of the selection (previewed only); `apply: true` makes it one undo step, `cancel: true` drops it"#,
        ),
        _ => None,
    }
}

pub fn enabled(app: &WaltCutApp, id: &str) -> bool {
    params_doc(id).is_some() && app.ui.tool == Tool::Brush
}

/// The target drawing's strokes of the cel shown: `(clip, cel start, strokes)`.
fn current(app: &WaltCutApp) -> Option<(u64, i64, std::sync::Arc<Vec<Stroke>>)> {
    let (clip, item, d) = super::drawing::target(app)?;
    let t = item.source_time_at(app.session.playhead().clamp(item.start, item.end() - waltcut_time::Tick(1)));
    let cel = d.cel_at(d.frame_at(t))?;
    Some((clip.0, cel.frame, cel.strokes.clone()))
}

/// Drop the selection when the project, clip or cel changed under it. Called every frame.
pub fn validate(app: &mut WaltCutApp) {
    let Some(sel) = &app.ui.brush.selection else { return };
    let keep = current(app).is_some_and(|(clip, cel, _)| clip == sel.clip && cel == sel.cel) && sel.revision == app.session.revision;
    if !keep {
        app.ui.brush.selection = None;
    }
}

fn num(params: &Value, key: &str, lo: f64, hi: f64) -> Result<Option<f64>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().filter(|n| n.is_finite() && (lo..=hi).contains(n)).map(Some).ok_or_else(|| format!("`{key}` must be a number from {lo} to {hi}")),
    }
}

fn flag(params: &Value, key: &str) -> Result<Option<bool>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("`{key}` must be true or false")),
    }
}

/// What the pending transform would do: the refusal, if any.
pub fn refusal(app: &WaltCutApp) -> Option<String> {
    let sel = app.ui.brush.selection.as_ref()?;
    if let Some(c) = &sel.conflict {
        return Some(c.clone());
    }
    let (_, _, strokes) = current(app)?;
    let centre = sel.centre.map(|c| (c[0], c[1]))?;
    transform_selection(&strokes, &sel.strokes, &sel.pending.transform(centre)).err()
}

fn report(app: &WaltCutApp) -> Value {
    match &app.ui.brush.selection {
        None => json!({"selected": 0}),
        Some(s) => {
            json!({"selected": s.strokes.len(), "strokes": s.strokes, "erasers": s.erasers, "conflict": s.conflict, "pending": s.pending, "refused": refusal(app)})
        }
    }
}

/// `drawing.lassoMode`, `drawing.selectStrokes` and `drawing.selectionTransform`.
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    params_doc(id)?;
    if app.ui.tool != Tool::Brush {
        return Some(Err(format!("`{id}` is available while the Brush tool is selected")));
    }
    if !matches!(params, Value::Null | Value::Object(_)) {
        return Some(Err("parameters must be an object".into()));
    }
    Some(match id {
        "drawing.lassoMode" => flag(params, "enabled").map(|e| {
            let on = e.unwrap_or(!app.ui.brush.lasso);
            app.ui.brush.lasso = on;
            if !on {
                app.ui.brush.selection = None;
            }
            json!({"lasso": on})
        }),
        "drawing.selectStrokes" => select(app, params),
        "drawing.selectionApply" => transform(app, &json!({"apply": true})),
        _ => transform(app, params),
    })
}

fn select(app: &mut WaltCutApp, params: &Value) -> Result<Value, String> {
    if flag(params, "clear")? == Some(true) {
        app.ui.brush.selection = None;
        return Ok(report(app));
    }
    let lasso = params.get("lasso").ok_or("give `lasso` (canvas points) or `clear: true`")?.clone();
    let r = app.session.execute("drawing.lassoSelect", json!({"lasso": lasso})).map_err(|e| e.to_string())?;
    let strokes: Vec<usize> = r["strokes"].as_array().map(|a| a.iter().filter_map(Value::as_u64).map(|n| n as usize).collect()).unwrap_or_default();
    if strokes.is_empty() {
        app.ui.brush.selection = None;
        app.ui.status = "The lasso selected no strokes".into();
        return Ok(report(app));
    }
    let (clip, cel, _) = current(app).ok_or("no cel is shown")?;
    let erasers = r["erasers"].as_array().map(|a| a.iter().filter_map(Value::as_u64).map(|n| n as usize).collect()).unwrap_or_default();
    let centre = r["centre"].as_array().and_then(|c| Some([c.first()?.as_f64()?, c.get(1)?.as_f64()?]));
    app.ui.brush.selection = Some(StrokeSelection {
        clip,
        cel,
        revision: app.session.revision,
        strokes,
        erasers,
        conflict: r["conflict"].as_str().map(str::to_string),
        centre,
        pending: Pending::default(),
    });
    Ok(report(app))
}

fn transform(app: &mut WaltCutApp, params: &Value) -> Result<Value, String> {
    let sel = app.ui.brush.selection.clone().ok_or("nothing is selected: lasso some strokes first")?;
    let (dx, dy) = (num(params, "dx", -1.0e6, 1.0e6)?, num(params, "dy", -1.0e6, 1.0e6)?);
    let (rotation, scale) = (num(params, "rotation", -36_000.0, 36_000.0)?, num(params, "scale", 0.01, 100.0)?);
    let (apply, cancel) = (flag(params, "apply")?.unwrap_or(false), flag(params, "cancel")?.unwrap_or(false));
    if apply && cancel {
        return Err("`apply` and `cancel` together make no sense".into());
    }
    let mut pending = sel.pending;
    if cancel {
        pending = Pending::default();
    }
    pending.dx = dx.unwrap_or(pending.dx);
    pending.dy = dy.unwrap_or(pending.dy);
    pending.rotation = rotation.unwrap_or(pending.rotation);
    pending.scale = scale.unwrap_or(pending.scale);
    if let Some(s) = app.ui.brush.selection.as_mut() {
        s.pending = pending;
    }
    if !apply {
        return Ok(report(app));
    }
    if let Some(why) = refusal(app) {
        return Err(why);
    }
    if pending.is_identity() {
        return Err("nothing to apply: move, rotate or scale the selection first".into());
    }
    let p = json!({"strokes": sel.strokes, "dx": pending.dx, "dy": pending.dy, "rotation": pending.rotation, "scale": pending.scale, "pivot": sel.centre});
    app.session.execute("drawing.transformStrokes", p).map_err(|e| e.to_string())?;
    app.ui.brush.selection = None;
    Ok(json!({"applied": true, "strokes": sel.strokes.len()}))
}

fn polyline(painter: &Painter, to_screen: &waltcut_geom::Affine, s: &Stroke, width: f32, color: Color32) {
    let pts: Vec<Pos2> = s
        .points
        .iter()
        .map(|p| {
            let q = to_screen.apply(Vec2::new(f64::from(p.x), f64::from(p.y)));
            Pos2::new(q.x as f32, q.y as f32)
        })
        .collect();
    match pts.len() {
        0 => {}
        1 => {
            painter.circle_filled(pts[0], (width * 0.5).max(1.0), color);
        }
        _ => {
            painter.add(egui::Shape::line(pts, EStroke::new(width.max(1.0), color)));
        }
    }
}

fn screen(to_screen: &waltcut_geom::Affine, x: f64, y: f64) -> Pos2 {
    let q = to_screen.apply(Vec2::new(x, y));
    Pos2::new(q.x as f32, q.y as f32)
}

/// Lasso, move-by-drag, preview and the Escape key, over the picture. `d` is the target
/// drawing and `f` the frame shown.
#[allow(clippy::too_many_arguments)]
pub(super) fn overlay(app: &mut WaltCutApp, ui: &mut Ui, painter: &Painter, resp: &egui::Response, map: &Mapping, d: &Drawing, f: i64) {
    validate(app);
    let accent = Color32::from_rgb(0x4d, 0x9d, 0xff);
    let result = Color32::from_rgb(0xff, 0x9a, 0x2e);
    let strokes = d.cel_at(f).map(|c| c.strokes.clone()).unwrap_or_default();
    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    // the selection and its preview
    let mut inside: Option<Rect> = None;
    if let Some(sel) = app.ui.brush.selection.clone() {
        let mut bounds: Option<(f32, f32, f32, f32)> = None;
        for &i in &sel.strokes {
            let Some(s) = strokes.get(i) else { continue };
            polyline(painter, &map.to_screen, s, (s.size * map.scale).max(1.5), accent.gamma_multiply(0.7));
            if let Some(b) = s.bounds() {
                bounds = Some(bounds.map_or(b, |a| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))));
            }
        }
        if let (Some(b), Some(c)) = (bounds, sel.centre) {
            let t = sel.pending.transform((c[0], c[1]));
            let corners = [(b.0, b.1), (b.2, b.1), (b.2, b.3), (b.0, b.3)];
            let shown: Vec<Pos2> = corners
                .iter()
                .map(|&(x, y)| {
                    let moved = t;
                    let (s, co) = moved.rotation.to_radians().sin_cos();
                    let (px, py) = (f64::from(x) - moved.pivot.0, f64::from(y) - moved.pivot.1);
                    screen(
                        &map.to_screen,
                        moved.pivot.0 + moved.scale * (co * px - s * py) + moved.dx,
                        moved.pivot.1 + moved.scale * (s * px + co * py) + moved.dy,
                    )
                })
                .collect();
            painter.add(egui::Shape::closed_line(shown.clone(), EStroke::new(1.5, if sel.pending.is_identity() { accent } else { result })));
            let mut r = Rect::NOTHING;
            for p in &shown {
                r.extend_with(*p);
            }
            inside = Some(r.expand(6.0));
            if !sel.pending.is_identity() {
                match transform_selection(&strokes, &sel.strokes, &t) {
                    Ok((moved, _)) => {
                        for &i in sel.strokes.iter().chain(&sel.erasers) {
                            if let Some(s) = moved.get(i).filter(|s| s.brush != waltcut_project::drawing::Brush::Eraser) {
                                polyline(painter, &map.to_screen, s, (s.size * map.scale).max(1.5), result);
                            }
                        }
                    }
                    Err(why) => {
                        painter.text(
                            shown[0] + vec2(0.0, -14.0),
                            egui::Align2::LEFT_BOTTOM,
                            format!("Refused: {why}"),
                            egui::FontId::proportional(12.0),
                            Color32::from_rgb(0xff, 0x6b, 0x6b),
                        );
                    }
                }
            } else if let Some(c) = &sel.conflict {
                painter.text(
                    shown[0] + vec2(0.0, -14.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("Ambiguous: {c}"),
                    egui::FontId::proportional(12.0),
                    Color32::from_rgb(0xff, 0x6b, 0x6b),
                );
            }
        }
    }
    // drags: a lasso, or a move of the pending transform when it starts inside the selection's box
    let points_id = resp.id.with("lasso-points");
    let mode_id = resp.id.with("lasso-mode");
    let mut pts: Vec<Pos2> = ui.data(|dd| dd.get_temp(points_id)).unwrap_or_default();
    let mut mode: u8 = ui.data(|dd| dd.get_temp(mode_id)).unwrap_or(0);
    if resp.drag_started_by(egui::PointerButton::Primary) {
        pts.clear();
        let at = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
        mode = if at.is_some_and(|p| inside.is_some_and(|r| r.contains(p))) { 2 } else { 1 };
        if let (1, Some(p)) = (mode, at) {
            pts.push(p);
        }
    }
    if resp.dragged_by(egui::PointerButton::Primary) {
        if mode == 1 {
            if let Some(p) = resp.interact_pointer_pos()
                && pts.last().is_none_or(|l| l.distance(p) >= 2.0)
                && pts.len() < MAX_LASSO_POINTS
            {
                pts.push(p);
            }
        } else if mode == 2 {
            let moved = resp.drag_delta();
            if moved != egui::Vec2::ZERO
                && let Some(here) = resp.interact_pointer_pos()
            {
                let (a, b) = (map.canvas(here - moved), map.canvas(here));
                if let Some(s) = app.ui.brush.selection.as_mut() {
                    s.pending.dx = (s.pending.dx + f64::from(b.0 - a.0)).clamp(-1.0e6, 1.0e6);
                    s.pending.dy = (s.pending.dy + f64::from(b.1 - a.1)).clamp(-1.0e6, 1.0e6);
                }
            }
        }
    }
    if mode == 1 && pts.len() >= 2 {
        let mut shape = pts.clone();
        shape.push(pts[0]);
        painter.add(egui::Shape::line(shape, EStroke::new(1.5, Color32::from_white_alpha(220))));
    }
    // Escape drops the lasso being drawn, else the pending transform, else the selection
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        if mode == 1 && !escape && pts.len() >= 3 {
            let canvas: Vec<Value> = pts
                .iter()
                .map(|p| {
                    let c = map.canvas(*p);
                    json!([c.0, c.1])
                })
                .collect();
            if let Err(e) = select(app, &json!({ "lasso": canvas })) {
                app.ui.status = e;
            }
        }
        pts.clear();
        mode = 0;
    }
    if escape {
        if mode != 0 {
            pts.clear();
            mode = 0;
        } else if app.ui.brush.selection.as_ref().is_some_and(|s| !s.pending.is_identity()) {
            if let Some(s) = app.ui.brush.selection.as_mut() {
                s.pending = Pending::default();
            }
        } else {
            app.ui.brush.selection = None;
        }
    }
    ui.data_mut(|dd| {
        dd.insert_temp(points_id, pts);
        dd.insert_temp(mode_id, mode);
    });
}

/// The Lasso toggle and, with a selection, the transform controls (Brush options bar).
pub fn controls(app: &mut WaltCutApp, ui: &mut Ui) {
    validate(app);
    let mut on = app.ui.brush.lasso;
    let r = ui.selectable_label(on, "Lasso").on_hover_text("Drag around strokes to select them, then move, rotate or scale them (Alt+L)");
    app.auto.add("program.brush.lasso", r.rect, "lasso select");
    if r.clicked() {
        on = !on;
        let _ = route(app, "drawing.lassoMode", &json!({"enabled": on}));
    }
    let Some(sel) = app.ui.brush.selection.clone() else { return };
    let text = format!(
        "{} stroke{}{}",
        sel.strokes.len(),
        if sel.strokes.len() == 1 { "" } else { "s" },
        if sel.erasers.is_empty() { String::new() } else { format!(" + {} eraser", sel.erasers.len()) }
    );
    let l = ui.label(egui::RichText::new(text).strong());
    app.auto.add("program.brush.lasso.status", l.rect, "selection");
    let mut p = sel.pending;
    let mut changed = false;
    let mut drag =
        |ui: &mut Ui, id: &str, label: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, speed: f64, auto: &mut crate::automation::Registry| {
            ui.label(label);
            let r = ui.add(egui::DragValue::new(v).range(range).speed(speed));
            auto.add(&format!("program.brush.lasso.{id}"), r.rect, label);
            changed |= r.changed();
        };
    drag(ui, "dx", "X", &mut p.dx, -100_000.0..=100_000.0, 1.0, &mut app.auto);
    drag(ui, "dy", "Y", &mut p.dy, -100_000.0..=100_000.0, 1.0, &mut app.auto);
    drag(ui, "rotation", "Rotate °", &mut p.rotation, -360.0..=360.0, 0.5, &mut app.auto);
    drag(ui, "scale", "Scale ×", &mut p.scale, 0.01..=100.0, 0.01, &mut app.auto);
    if changed && let Some(s) = app.ui.brush.selection.as_mut() {
        s.pending = p;
    }
    let refused = refusal(app);
    let can_apply = refused.is_none() && !p.is_identity();
    let apply = ui.add_enabled(can_apply, egui::Button::new("Apply"));
    app.auto.add("program.brush.lasso.apply", apply.rect, "apply transform");
    if apply.clicked() {
        match transform(app, &json!({"apply": true})) {
            Ok(_) => app.ui.status = "Transformed the selected strokes".into(),
            Err(e) => app.ui.status = e,
        }
    }
    let cancel = ui.add_enabled(!p.is_identity(), egui::Button::new("Cancel"));
    app.auto.add("program.brush.lasso.cancel", cancel.rect, "cancel transform");
    if cancel.clicked() {
        let _ = transform(app, &json!({"cancel": true}));
    }
    let clear = ui.button("Clear");
    app.auto.add("program.brush.lasso.clear", clear.rect, "clear selection");
    if clear.clicked() {
        app.ui.brush.selection = None;
    }
    if let Some(why) = refused.filter(|_| !p.is_identity() || sel.conflict.is_some()) {
        ui.label(egui::RichText::new(why).color(Color32::from_rgb(0xff, 0x6b, 0x6b)).small());
    }
}
