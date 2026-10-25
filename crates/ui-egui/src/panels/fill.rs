//! Fill mode in the Brush tool (#68). The fill itself is the engine's `drawing.fill` (one undo
//! step, stored in the cel's paint order: `docs/animation.md`, Fill a region); this module is the
//! view state around it: the mode, the tolerance, the click on the picture and the controls.
//!
//! * **Fill** (Brush options bar, `drawing.fillMode`, `Alt+F`): while it is on, a click on the
//!   picture fills the contiguous region under it with the brush colour. A drag draws nothing.
//! * **Tolerance** (`drawing.fillMode {tolerance}`, 0–255, default 32): how different from the
//!   clicked pixel a pixel may be and still be part of the region.
//! * **Recolour last / Remove last**: change or remove the newest fill of the cel shown
//!   (`drawing.fillEdit`, `drawing.fillRemove`); both are one undo step.
//!
//! * **Reference / Gap / Grow / Preview / Apply / Cancel** (#69): fill from another drawing clip's
//!   line art, close gaps up to twice the radius, grow the edge under lines, and look at the region
//!   before it is painted (`drawing.fillPreview` shows it over the picture; Apply runs
//!   `drawing.fill` with the same parameters, Cancel or Escape drops it). The picture shows the
//!   previewed region in the brush colour; nothing is changed until Apply.
//!
//! Fill mode and Lasso mode exclude each other. Nothing here is saved with the project; the mode
//! is not loaded back (a session starts drawing) and the tolerance is saved with the interface state.
//!
//! Automation ids: `program.brush.fill`, `program.brush.fill.tolerance`,
//! `program.brush.fill.recolor`, `program.brush.fill.remove`, `program.brush.fill.reference`,
//! `program.brush.fill.gap`, `program.brush.fill.grow`, `program.brush.fill.preview`,
//! `program.brush.fill.apply`, `program.brush.fill.cancel`. `ui.inspect` reports `ui.brush.fill`,
//! `fill_tolerance`, `fill_gap`, `fill_grow`, `fill_reference`, `fill_preview` and `fill_pending`.

use std::sync::Arc;

use egui::{Color32, Painter, Rect, Stroke as EStroke, Ui};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use waltcut_project::drawing_fill::{Fill, MAX_FILL_GAP, MAX_FILL_GROW};

use super::drawing::Mapping;
use crate::WaltCutApp;
use crate::state::Tool;

/// A previewed fill waiting for Apply or Cancel (#69). Interface state only: never saved or loaded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingFill {
    /// What Apply runs: the `drawing.fill` parameters, pinned to the time of the preview.
    pub params: Value,
    pub pixels: u64,
    pub runs: usize,
    pub rect: [u32; 4],
    /// The session revision the preview was made at; a later change drops it.
    pub revision: u64,
    /// The region, for drawing over the picture.
    #[serde(skip)]
    pub region: Option<Arc<Fill>>,
}

pub fn params_doc(id: &str) -> Option<&'static str> {
    match id {
        "drawing.fillMode" => Some(
            r#"{"enabled":bool?,"tolerance":0-255?,"gap":0-8?,"grow":0-8?,"reference":clip id|null?,"preview":bool?}  clicks on the picture fill the contiguous region under them with the brush colour instead of drawing; toggles when `enabled` is absent; `tolerance` sets how different a pixel may be and still join the region (default 32); `reference` finds the region in another drawing clip's line art (null: the drawing itself); `gap` closes openings up to twice that many pixels; `grow` widens the edge under lines; `preview` shows the region on a click and waits for drawing.fillApply or drawing.fillCancel"#,
        ),
        "drawing.fillApply" => Some("{}  paint the previewed fill: the same region, one undo step"),
        "drawing.fillCancel" => Some("{}  drop the previewed fill; nothing was changed"),
        _ => None,
    }
}

pub fn enabled(app: &WaltCutApp, id: &str) -> bool {
    match id {
        "drawing.fillApply" | "drawing.fillCancel" => app.ui.tool == Tool::Brush && app.ui.brush.fill_pending.is_some(),
        _ => params_doc(id).is_some() && app.ui.tool == Tool::Brush,
    }
}

fn small(params: &Value, key: &str, max: u8) -> Result<Option<u8>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|t| u8::try_from(t).ok())
            .filter(|t| *t <= max)
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a whole number from 0 to {max}")),
    }
}

fn state(app: &WaltCutApp) -> Value {
    let b = &app.ui.brush;
    json!({"fill": b.fill, "tolerance": b.fill_tolerance, "gap": b.fill_gap, "grow": b.fill_grow, "reference": b.fill_reference, "preview": b.fill_preview})
}

/// `drawing.fillMode`, `drawing.fillApply`, `drawing.fillCancel`.
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    params_doc(id)?;
    if app.ui.tool != Tool::Brush {
        return Some(Err(format!("`{id}` is available while the Brush tool is selected")));
    }
    let (Value::Object(_) | Value::Null) = params else { return Some(Err("parameters must be an object".into())) };
    if id != "drawing.fillMode" {
        if let Some(k) = params.as_object().and_then(|o| o.keys().next()) {
            return Some(Err(format!("unknown parameter `{k}`; `{id}` takes none")));
        }
        return Some(if id == "drawing.fillApply" { apply(app) } else { cancel(app).map(|()| json!({"cancelled": true})) });
    }
    const KEYS: [&str; 6] = ["enabled", "tolerance", "gap", "grow", "reference", "preview"];
    if let Some(k) = params.as_object().and_then(|o| o.keys().find(|k| !KEYS.contains(&k.as_str()))) {
        return Some(Err(format!("unknown parameter `{k}`; expected: {}", KEYS.join(", "))));
    }
    let flag = |key: &str| match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("`{key}` must be true or false")),
    };
    let parsed = (|| -> Result<_, String> {
        let enabled = flag("enabled")?;
        let preview = flag("preview")?;
        let tolerance = match params.get("tolerance") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().and_then(|t| u8::try_from(t).ok()).ok_or("`tolerance` must be a whole number from 0 to 255")?),
        };
        let (gap, grow) = (small(params, "gap", MAX_FILL_GAP)?, small(params, "grow", MAX_FILL_GROW)?);
        let reference = match params.get("reference") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(v) => Some(Some(v.as_u64().filter(|c| *c > 0).ok_or("`reference` must be the id of a drawing clip, or null")?)),
        };
        Ok((enabled, preview, tolerance, gap, grow, reference))
    })();
    let (enabled, preview, tolerance, gap, grow, reference) = match parsed {
        Ok(v) => v,
        Err(e) => return Some(Err(e)),
    };
    // all checked: now apply
    let any_option = tolerance.is_some() || gap.is_some() || grow.is_some() || reference.is_some() || preview.is_some();
    if let Some(t) = tolerance {
        app.ui.brush.fill_tolerance = t;
    }
    if let Some(g) = gap {
        app.ui.brush.fill_gap = g;
    }
    if let Some(g) = grow {
        app.ui.brush.fill_grow = g;
    }
    if let Some(r) = reference {
        app.ui.brush.fill_reference = r;
    }
    if let Some(p) = preview {
        app.ui.brush.fill_preview = p;
    }
    let on = enabled.unwrap_or(if any_option { app.ui.brush.fill } else { !app.ui.brush.fill });
    app.ui.brush.fill = on;
    if on {
        app.ui.brush.lasso = false;
        app.ui.brush.selection = None;
    }
    // a preview belongs to the settings it was made with
    if !on || !app.ui.brush.fill_preview || any_option {
        app.ui.brush.fill_pending = None;
    }
    Some(Ok(state(app)))
}

/// Drop the previewed fill.
fn cancel(app: &mut WaltCutApp) -> Result<(), String> {
    if app.ui.brush.fill_pending.take().is_none() {
        return Err("there is no previewed fill".into());
    }
    app.ui.status = "Fill preview cancelled".into();
    Ok(())
}

/// Paint the previewed fill with the parameters it was made with.
fn apply(app: &mut WaltCutApp) -> Result<Value, String> {
    let Some(p) = app.ui.brush.fill_pending.take() else { return Err("there is no previewed fill".into()) };
    if p.revision != app.session.revision {
        app.ui.status = "The drawing changed since the preview: click again".into();
        return Err(app.ui.status.clone());
    }
    match app.session.execute("drawing.fill", p.params) {
        Ok(r) => {
            app.ui.status = format!("Filled {} pixels", r["pixels"].as_u64().unwrap_or(0));
            Ok(r)
        }
        Err(e) => {
            app.ui.status = e.to_string();
            Err(e.to_string())
        }
    }
}

/// The newest fill of the cel shown on the target drawing: its index in the cel.
fn last_fill(app: &WaltCutApp) -> Option<usize> {
    let (_, item, d) = super::drawing::target(app)?;
    let t = item.source_time_at(app.session.playhead().clamp(item.start, item.end() - waltcut_time::Tick(1)));
    let cel = d.cel_at(d.frame_at(t))?;
    cel.strokes.iter().rposition(|s| s.is_fill())
}

/// The `drawing.fill` parameters for a click at canvas point `at` with the brush settings.
fn fill_params(app: &WaltCutApp, clip: u64, at: (f32, f32)) -> Value {
    let b = &app.ui.brush;
    let mut params = json!({
        "clip": clip, "cel": b.cel_mode.param(), "point": [at.0, at.1], "tolerance": b.fill_tolerance, "color": b.color,
        "gap": b.fill_gap, "grow": b.fill_grow,
    });
    if let (Some(r), Value::Object(o)) = (b.fill_reference, &mut params) {
        o.insert("reference".into(), json!(r));
    }
    params
}

/// Fill at canvas point `at` with the brush colour and settings; the outcome goes to the status bar.
/// With Preview on, find the region and show it instead, to be applied or cancelled.
pub fn fill_at(app: &mut WaltCutApp, clip: u64, at: (f32, f32)) -> Result<Value, String> {
    let mut params = fill_params(app, clip, at);
    if app.ui.brush.fill_preview {
        // pinned to this time, so Apply looks where the preview looked
        if let Value::Object(o) = &mut params {
            o.insert("time".into(), json!(app.session.playhead().0));
        }
        app.ui.brush.fill_pending = None;
        return match waltcut_engine::drawing_fill::preview_with(&app.session, &params, &|| false) {
            Ok((r, _, fill)) => {
                let pending = PendingFill {
                    params,
                    pixels: r["pixels"].as_u64().unwrap_or(0),
                    runs: fill.runs.len(),
                    rect: fill.rect,
                    revision: app.session.revision,
                    region: Some(Arc::new(fill)),
                };
                app.ui.status = format!("Preview: {} pixels. Apply or Cancel", pending.pixels);
                app.ui.brush.fill_pending = Some(pending);
                Ok(r)
            }
            Err(e) => {
                app.ui.status = e.to_string();
                Err(e.to_string())
            }
        };
    }
    match app.session.execute("drawing.fill", params) {
        Ok(r) => {
            app.ui.status = format!("Filled {} pixels", r["pixels"].as_u64().unwrap_or(0));
            Ok(r)
        }
        Err(e) => {
            app.ui.status = e.to_string();
            Err(e.to_string())
        }
    }
}

/// Draw a previewed region over the picture: the filled cells in the brush colour, the edge paler.
fn paint_region(painter: &Painter, map: &Mapping, fill: &Fill, color: [f32; 4]) {
    let [rx, ry, rw, _] = fill.rect;
    if rw == 0 {
        return;
    }
    let tint = |a: u8| {
        Color32::from_rgba_unmultiplied(
            (color[0].clamp(0.0, 1.0) * 255.0) as u8,
            (color[1].clamp(0.0, 1.0) * 255.0) as u8,
            (color[2].clamp(0.0, 1.0) * 255.0) as u8,
            a,
        )
    };
    let (filled, edge) = (tint(130), tint(70));
    let mut mesh = egui::Mesh::default();
    let mut at = 0u64;
    for &[class, len] in &fill.runs {
        let end = at + u64::from(len);
        if class != 0 {
            let mut cell = at;
            while cell < end {
                let (row, col) = (cell / u64::from(rw), cell % u64::from(rw));
                let n = (end - cell).min(u64::from(rw) - col);
                let (x, y) = ((u64::from(rx) + col) as f64, (u64::from(ry) + row) as f64);
                let base = mesh.vertices.len() as u32;
                for (dx, dy) in [(0.0, 0.0), (n as f64, 0.0), (n as f64, 1.0), (0.0, 1.0)] {
                    let q = map.to_screen.apply(waltcut_geom::Vec2::new(x + dx, y + dy));
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: egui::pos2(q.x as f32, q.y as f32),
                        uv: egui::epaint::WHITE_UV,
                        color: if class == 1 { filled } else { edge },
                    });
                }
                mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
                cell += n;
            }
        }
        at = end;
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// The picture while Fill is on: a cursor and a click that fills. `bar` is the options bar, which
/// a click must not reach through.
pub(super) fn overlay(app: &mut WaltCutApp, ui: &mut Ui, painter: &Painter, resp: &egui::Response, map: &Mapping, bar: Rect, clip: u64) {
    ui.ctx().set_cursor_icon(egui::CursorIcon::Cell);
    if app.ui.brush.fill_pending.as_ref().is_some_and(|p| p.revision != app.session.revision) {
        app.ui.brush.fill_pending = None;
    }
    if let Some(region) = app.ui.brush.fill_pending.as_ref().and_then(|p| p.region.clone()) {
        paint_region(painter, map, &region, app.ui.brush.color);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            let _ = cancel(app);
        }
    }
    if let Some(p) = resp.hover_pos().filter(|p| !bar.contains(*p)) {
        painter.circle_stroke(p, 5.0, EStroke::new(1.5, Color32::from_white_alpha(220)));
        painter.circle_stroke(p, 6.5, EStroke::new(1.0, Color32::from_black_alpha(160)));
    }
    if resp.clicked_by(egui::PointerButton::Primary)
        && let Some(p) = resp.interact_pointer_pos().filter(|p| !bar.contains(*p))
    {
        let _ = fill_at(app, clip, map.canvas(p));
    }
}

/// The other drawing clips of the sequence the region can be found in: `(clip id, label)`.
fn reference_choices(app: &WaltCutApp) -> Vec<(u64, String)> {
    let Some(seq) = app.session.active_sequence() else { return Vec::new() };
    let own = waltcut_engine::drawing::target_clip(&app.session, &Value::Null).map(|c| c.0);
    seq.video_tracks
        .iter()
        .flat_map(|t| t.items.iter())
        .filter(|it| Some(it.id.0) != own && waltcut_engine::drawing::drawing_of(&app.session, seq, it.id).is_some())
        .map(|it| (it.id.0, format!("{} (clip {})", app.session.project.item(it.item).map_or("Drawing", |i| i.name.as_str()), it.id.0)))
        .collect()
}

/// Reference, gap, grow, preview, apply and cancel (#69).
fn reference_controls(app: &mut WaltCutApp, ui: &mut Ui) {
    let choices = reference_choices(app);
    let current = app.ui.brush.fill_reference;
    let label = match current {
        None => "This drawing".to_string(),
        Some(c) => choices.iter().find(|(id, _)| *id == c).map_or_else(|| format!("clip {c} (missing)"), |(_, l)| l.clone()),
    };
    let mut picked = current;
    let combo = egui::ComboBox::from_id_salt("fill-reference").selected_text(label).width(150.0).show_ui(ui, |ui| {
        ui.selectable_value(&mut picked, None, "This drawing");
        for (id, name) in &choices {
            ui.selectable_value(&mut picked, Some(*id), name.as_str());
        }
    });
    app.auto.add("program.brush.fill.reference", combo.response.rect, "fill reference drawing");
    combo.response.on_hover_text("Find the region in another drawing clip's line art; the colour is painted on this drawing");
    if picked != current {
        let _ = route(app, "drawing.fillMode", &json!({"reference": picked}));
    }
    let mut gap = app.ui.brush.fill_gap;
    let r = ui
        .add(egui::DragValue::new(&mut gap).range(0..=MAX_FILL_GAP).prefix("Gap "))
        .on_hover_text("Close openings in the lines up to twice this many pixels wide (canvas pixels, whatever the zoom)");
    app.auto.add("program.brush.fill.gap", r.rect, "fill gap closing radius");
    if r.changed() {
        let _ = route(app, "drawing.fillMode", &json!({"gap": gap}));
    }
    let mut grow = app.ui.brush.fill_grow;
    let r = ui
        .add(egui::DragValue::new(&mut grow).range(0..=MAX_FILL_GROW).prefix("Grow "))
        .on_hover_text("Widen the colour under the lines by this many pixels, so no halo shows between colour and line");
    app.auto.add("program.brush.fill.grow", r.rect, "fill edge expansion");
    if r.changed() {
        let _ = route(app, "drawing.fillMode", &json!({"grow": grow}));
    }
    let preview = app.ui.brush.fill_preview;
    let r = ui.selectable_label(preview, "Preview").on_hover_text("A click shows the region first; Apply paints it, Cancel (Esc) drops it");
    app.auto.add("program.brush.fill.preview", r.rect, "fill preview");
    if r.clicked() {
        let _ = route(app, "drawing.fillMode", &json!({"preview": !preview}));
    }
    if preview {
        let pending = app.ui.brush.fill_pending.is_some();
        let r = ui.add_enabled(pending, egui::Button::new("Apply"));
        app.auto.add("program.brush.fill.apply", r.rect, "apply fill preview");
        if r.clicked() {
            let _ = route(app, "drawing.fillApply", &json!({}));
        }
        let r = ui.add_enabled(pending, egui::Button::new("Cancel"));
        app.auto.add("program.brush.fill.cancel", r.rect, "cancel fill preview");
        if r.clicked() {
            let _ = route(app, "drawing.fillCancel", &json!({}));
        }
    }
}

/// The Fill toggle and, while it is on, the tolerance and the edits of the newest fill.
pub fn controls(app: &mut WaltCutApp, ui: &mut Ui) {
    let on = app.ui.brush.fill;
    let r = ui.selectable_label(on, "Fill").on_hover_text("Click inside a closed shape to colour it with the brush colour (Alt+F)");
    app.auto.add("program.brush.fill", r.rect, "fill region");
    if r.clicked() {
        let _ = route(app, "drawing.fillMode", &json!({"enabled": !on}));
    }
    if !app.ui.brush.fill {
        return;
    }
    let mut t = app.ui.brush.fill_tolerance;
    let r = ui
        .add(egui::Slider::new(&mut t, 0..=255).text("Tolerance"))
        .on_hover_text("How different from the clicked pixel a pixel may be and still be filled: 0 is an exact match");
    app.auto.add("program.brush.fill.tolerance", r.rect, "fill tolerance");
    if r.changed() {
        app.ui.brush.fill_tolerance = t;
    }
    reference_controls(app, ui);
    let last = last_fill(app);
    let recolor = ui.add_enabled(last.is_some(), egui::Button::new("Recolour last"));
    app.auto.add("program.brush.fill.recolor", recolor.rect, "recolour last fill");
    if recolor.clicked()
        && let Some(index) = last
    {
        let color = app.ui.brush.color;
        match app.session.execute("drawing.fillEdit", json!({"index": index, "color": color})) {
            Ok(_) => app.ui.status = "Recoloured the last fill".into(),
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    let remove = ui.add_enabled(last.is_some(), egui::Button::new("Remove last"));
    app.auto.add("program.brush.fill.remove", remove.rect, "remove last fill");
    if remove.clicked()
        && let Some(index) = last
    {
        match app.session.execute("drawing.fillRemove", json!({"index": index})) {
            Ok(_) => app.ui.status = "Removed the last fill".into(),
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}
