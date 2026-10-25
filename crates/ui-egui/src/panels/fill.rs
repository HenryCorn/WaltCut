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
//! Fill mode and Lasso mode exclude each other. Nothing here is saved with the project; the mode
//! is not loaded back (a session starts drawing) and the tolerance is saved with the interface state.
//!
//! Automation ids: `program.brush.fill`, `program.brush.fill.tolerance`,
//! `program.brush.fill.recolor`, `program.brush.fill.remove`. `ui.inspect` reports `ui.brush.fill`
//! and `ui.brush.fill_tolerance`.

use egui::{Color32, Painter, Rect, Stroke as EStroke, Ui};
use serde_json::{Value, json};

use super::drawing::Mapping;
use crate::WaltCutApp;
use crate::state::Tool;

pub fn params_doc(id: &str) -> Option<&'static str> {
    match id {
        "drawing.fillMode" => Some(
            r#"{"enabled":bool?,"tolerance":0-255?}  clicks on the picture fill the contiguous region under them with the brush colour instead of drawing; toggles when `enabled` is absent; `tolerance` sets how different a pixel may be and still join the region (default 32)"#,
        ),
        _ => None,
    }
}

pub fn enabled(app: &WaltCutApp, id: &str) -> bool {
    params_doc(id).is_some() && app.ui.tool == Tool::Brush
}

/// `drawing.fillMode`.
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    params_doc(id)?;
    if app.ui.tool != Tool::Brush {
        return Some(Err(format!("`{id}` is available while the Brush tool is selected")));
    }
    let (Value::Object(_) | Value::Null) = params else { return Some(Err("parameters must be an object".into())) };
    if let Some(k) = params.as_object().and_then(|o| o.keys().find(|k| !["enabled", "tolerance"].contains(&k.as_str()))) {
        return Some(Err(format!("unknown parameter `{k}`; expected: enabled, tolerance")));
    }
    let enabled = match params.get("enabled") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Some(Err("`enabled` must be true or false".into())),
    };
    let tolerance = match params.get("tolerance") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_u64().and_then(|t| u8::try_from(t).ok()) {
            Some(t) => Some(t),
            None => return Some(Err("`tolerance` must be a whole number from 0 to 255".into())),
        },
    };
    // all checked: now apply
    if let Some(t) = tolerance {
        app.ui.brush.fill_tolerance = t;
    }
    let on = enabled.unwrap_or(if tolerance.is_some() { app.ui.brush.fill } else { !app.ui.brush.fill });
    app.ui.brush.fill = on;
    if on {
        app.ui.brush.lasso = false;
        app.ui.brush.selection = None;
    }
    Some(Ok(json!({"fill": on, "tolerance": app.ui.brush.fill_tolerance})))
}

/// The newest fill of the cel shown on the target drawing: its index in the cel.
fn last_fill(app: &WaltCutApp) -> Option<usize> {
    let (_, item, d) = super::drawing::target(app)?;
    let t = item.source_time_at(app.session.playhead().clamp(item.start, item.end() - waltcut_time::Tick(1)));
    let cel = d.cel_at(d.frame_at(t))?;
    cel.strokes.iter().rposition(|s| s.is_fill())
}

/// Fill at canvas point `at` with the brush colour and settings; the outcome goes to the status bar.
pub fn fill_at(app: &mut WaltCutApp, clip: u64, at: (f32, f32)) -> Result<Value, String> {
    let b = &app.ui.brush;
    let params = json!({"clip": clip, "cel": b.cel_mode.param(), "point": [at.0, at.1], "tolerance": b.fill_tolerance, "color": b.color});
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

/// The picture while Fill is on: a cursor and a click that fills. `bar` is the options bar, which
/// a click must not reach through.
pub(super) fn overlay(app: &mut WaltCutApp, ui: &mut Ui, painter: &Painter, resp: &egui::Response, map: &Mapping, bar: Rect, clip: u64) {
    ui.ctx().set_cursor_icon(egui::CursorIcon::Cell);
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
