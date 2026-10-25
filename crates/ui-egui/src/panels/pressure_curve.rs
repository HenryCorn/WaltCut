//! Pressure response curves for the Brush tool (#56): how a pen's pressure becomes line width and
//! opacity, tuned apart for each, with a live preview and a one-action reset.
//!
//! * The brush's own response (unchanged) is the default, so a stroke made with it is stored and
//!   drawn exactly as before.
//! * A curve maps pressure (0..1) to the pressure the brush then uses, by straight lines between
//!   2–8 control points (`waltcut_project::drawing::Curve`). Presets: `linear`, `soft`, `firm`,
//!   `scurve`. The editor in the Brush options bar's **Feel** menu has five handles per curve.
//! * The curves in use are **stored with each new stroke** (`Stroke.response`, written only when
//!   they differ from the default), so a saved stroke looks the same whatever the settings are
//!   when the project is reopened; changing the settings later never alters a stroke already drawn.
//! * Only a **pen** uses them: a mouse always draws at full pressure, so it stays predictable
//!   whatever the curves are, and with Pressure switched off no curve is stored either.
//! * `drawing.pressureCurve {preset?, width?, opacity?, reset?}` sets them (a bad curve is refused
//!   whole and changes nothing); with no parameter it reports. The setting is saved with the
//!   interface state, adds no undo step and marks nothing changed.
//!
//! Automation ids: `program.brush.pressure.preset.<linear|soft|firm|scurve>`,
//! `program.brush.pressure.<width|opacity>.h0` … `.h4` (the handles) and
//! `program.brush.pressure.reset`.

use egui::{Color32, Pos2, Rect, Sense, Stroke as EStroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use waltcut_geom::Affine;
use waltcut_project::drawing::{Brush, CURVE_PRESETS, Curve, PressureResponse, Stroke, StrokePoint};

use super::drawing::BrushSettings;
use super::drawing_input::Source;
use crate::WaltCutApp;

/// Handles per curve in the editor (at pressure 0, ¼, ½, ¾ and 1).
pub const HANDLES: usize = 5;

pub fn params_doc(id: &str) -> Option<&'static str> {
    match id {
        "drawing.pressureCurve" => Some(
            r#"{"preset":"linear"|"soft"|"firm"|"scurve"?,"width":preset|[[pressure,response],...]|null?,"opacity":preset|[[pressure,response],...]|null?,"reset":bool?}  set how a pen's pressure becomes width and opacity for strokes drawn from now on (a curve has 2-8 points from pressure 0 to 1, increasing, all within 0-1; null is the brush's own response); `preset` sets both; with no parameter, report. A mouse always draws at full pressure"#,
        ),
        _ => None,
    }
}

impl BrushSettings {
    /// The response stored with a stroke made now: None for the brush's own response, for a
    /// mouse and when Pressure is off (a pen queue's own pressures are unaffected by that switch).
    pub fn stroke_response(&self) -> Option<PressureResponse> {
        (self.pressure && self.pen.source == Source::Pen && !self.response.is_default()).then(|| self.response.clone())
    }
}

fn curve_json(c: &Option<Curve>) -> Value {
    c.as_ref().map_or(Value::Null, |c| json!(c.0))
}

fn state(b: &BrushSettings) -> Value {
    json!({"width": curve_json(&b.response.width), "opacity": curve_json(&b.response.opacity), "default": b.response.is_default()})
}

/// `drawing.pressureCurve`. Every parameter is checked before any is applied.
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    params_doc(id)?;
    Some(run(app, params))
}

fn run(app: &mut WaltCutApp, params: &Value) -> Result<Value, String> {
    let obj = match params {
        Value::Null => None,
        Value::Object(o) => Some(o),
        _ => return Err("parameters must be an object".into()),
    };
    if let Some(o) = obj
        && let Some(k) = o.keys().find(|k| !["preset", "width", "opacity", "reset"].contains(&k.as_str()))
    {
        return Err(format!("unknown parameter `{k}`; expected: preset, width, opacity, reset"));
    }
    let get = |k: &str| obj.and_then(|o| o.get(k));
    let reset = match get("reset") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("`reset` must be true or false".into()),
    };
    let given = ["preset", "width", "opacity"].iter().any(|k| get(k).is_some());
    if reset && given {
        return Err("`reset` cannot be combined with preset, width or opacity".into());
    }
    // one parse for everything, so a bad value changes nothing
    let mut next = if reset { PressureResponse::default() } else { app.ui.brush.response.clone() };
    if let Some(p) = get("preset") {
        let name = p.as_str().ok_or("`preset` is a name: linear, soft, firm or scurve")?;
        let c = Curve::preset(name).ok_or_else(|| format!("unknown preset `{name}` ({})", CURVE_PRESETS.join(", ")))?;
        next = PressureResponse { width: Some(c.clone()), opacity: Some(c) };
    }
    for key in ["width", "opacity"] {
        let Some(v) = get(key) else { continue };
        let parsed = waltcut_engine::drawing::parse_response("drawing.pressureCurve", &json!({ key: v })).map_err(|e| e.to_string())?;
        let c = if key == "width" { parsed.width } else { parsed.opacity };
        if key == "width" {
            next.width = c;
        } else {
            next.opacity = c;
        }
    }
    // the brush's own response is stored as nothing
    for c in [&mut next.width, &mut next.opacity] {
        if c.as_ref().is_some_and(Curve::is_identity) {
            *c = None;
        }
    }
    app.ui.brush.response = next;
    Ok(state(&app.ui.brush))
}

/// The five handle heights of a curve (None is the identity).
pub fn handles(c: &Option<Curve>) -> [f32; HANDLES] {
    std::array::from_fn(|i| {
        let x = i as f32 / (HANDLES - 1) as f32;
        c.as_ref().map_or(x, |c| c.at(x))
    })
}

/// The curve through five handle heights (None when they are the identity).
pub fn from_handles(h: &[f32; HANDLES]) -> Option<Curve> {
    let pts: Vec<[f32; 2]> =
        h.iter().enumerate().map(|(i, y)| [i as f32 / (HANDLES - 1) as f32, if y.is_finite() { y.clamp(0.0, 1.0) } else { 0.0 }]).collect();
    let c = Curve(pts);
    (!c.is_identity()).then_some(c)
}

/// One curve's editor: a small graph (pressure across, response up) with five draggable handles.
fn editor(ui: &mut Ui, which: &str, label: &str, curve: &mut Option<Curve>, auto: &mut crate::automation::Registry) {
    ui.label(egui::RichText::new(label).small());
    let (rect, _) = ui.allocate_exact_size(vec2(132.0, 84.0), Sense::hover());
    let inner = rect.shrink(8.0);
    let at = |x: f32, y: f32| pos2(inner.min.x + x * inner.width(), inner.max.y - y * inner.height());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    painter.line_segment([at(0.0, 0.0), at(1.0, 1.0)], EStroke::new(1.0, Color32::from_white_alpha(40)));
    let mut h = handles(curve);
    let mut changed = false;
    for (i, y) in h.iter_mut().enumerate() {
        let x = i as f32 / (HANDLES - 1) as f32;
        let hit = Rect::from_center_size(at(x, *y), vec2(14.0, 14.0));
        let r = ui.interact(hit, ui.id().with((which, i)), Sense::drag());
        auto.add(&format!("program.brush.pressure.{which}.h{i}"), hit, &format!("{label} handle {i}"));
        if r.dragged()
            && let Some(p) = r.interact_pointer_pos()
        {
            *y = ((inner.max.y - p.y) / inner.height().max(1.0)).clamp(0.0, 1.0);
            changed = true;
        }
    }
    let shown = if changed { from_handles(&h) } else { curve.clone() };
    let line: Vec<Pos2> = (0..=32)
        .map(|k| {
            let x = k as f32 / 32.0;
            at(x, shown.as_ref().map_or(x, |c| c.at(x)))
        })
        .collect();
    painter.add(egui::Shape::line(line, EStroke::new(1.5, Color32::from_rgb(70, 130, 230))));
    for (i, y) in h.iter().enumerate() {
        painter.circle_filled(at(i as f32 / (HANDLES - 1) as f32, *y), 3.5, Color32::WHITE);
    }
    if changed {
        *curve = shown;
    }
}

/// A sample stroke: a wave with the pressure rising from nothing to full and back, drawn with the
/// brush and the curves as they are now.
fn preview(ui: &mut Ui, b: &BrushSettings) {
    let (rect, _) = ui.allocate_exact_size(vec2(272.0, 40.0), Sense::hover());
    ui.painter().rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let n = 48usize;
    let points = (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            StrokePoint { x: 10.0 + t * 252.0, y: 20.0 + (t * std::f32::consts::TAU).sin() * 9.0, p: 1.0 - (2.0 * t - 1.0).abs() }
        })
        .collect();
    let stroke = Stroke {
        brush: if b.brush == Brush::Eraser { Brush::Pen } else { b.brush },
        // light on the menu's dark ground: the preview shows width and opacity, not the colour
        color: [0.92, 0.92, 0.92, 1.0],
        size: b.size.clamp(2.0, 18.0),
        points,
        response: (!b.response.is_default()).then(|| b.response.clone()),
    };
    let to_screen = Affine::translate(f64::from(rect.min.x), f64::from(rect.min.y));
    super::drawing_raster::paint_stroke_preview(&ui.painter_at(rect), "brush-response-preview", &stroke, &to_screen, rect);
}

/// The pressure-response part of the **Feel** menu.
pub fn section(ui: &mut Ui, b: &mut BrushSettings, auto: &mut crate::automation::Registry) {
    ui.separator();
    ui.label(egui::RichText::new("Pressure response").strong());
    ui.horizontal(|ui| {
        for name in CURVE_PRESETS {
            let r = ui.button(match name {
                "scurve" => "S-curve",
                "linear" => "Linear",
                "soft" => "Soft",
                _ => "Firm",
            });
            auto.add(&format!("program.brush.pressure.preset.{name}"), r.rect, name);
            if r.clicked()
                && let Some(c) = Curve::preset(name)
            {
                let c = (!c.is_identity()).then_some(c);
                b.response = PressureResponse { width: c.clone(), opacity: c };
            }
        }
    });
    ui.horizontal(|ui| {
        editor(ui, "width", "Width", &mut b.response.width, auto);
        editor(ui, "opacity", "Opacity", &mut b.response.opacity, auto);
    });
    preview(ui, b);
    ui.horizontal(|ui| {
        let r = ui.add_enabled(!b.response.is_default(), egui::Button::new("Reset"));
        auto.add("program.brush.pressure.reset", r.rect, "reset pressure response");
        if r.clicked() {
            b.response = PressureResponse::default();
        }
        ui.label(egui::RichText::new("A pen only: a mouse draws at full pressure.").small());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_and_curves_round_trip_and_the_identity_is_none() {
        assert_eq!(from_handles(&[0.0, 0.25, 0.5, 0.75, 1.0]), None);
        assert_eq!(handles(&None), [0.0, 0.25, 0.5, 0.75, 1.0]);
        let h = [0.0, 0.5, 0.8, 0.95, 1.0];
        let c = from_handles(&h);
        assert_eq!(handles(&c), h);
        assert!(c.as_ref().is_some_and(|c| c.validate().is_ok()));
        // hostile heights are brought into range
        let c = from_handles(&[f32::NAN, 9.0, -3.0, f32::INFINITY, 1.0]);
        assert!(c.is_some_and(|c| c.validate().is_ok()));
    }

    #[test]
    fn a_mouse_and_pressure_off_store_no_response() {
        let mut b = BrushSettings { response: PressureResponse { width: Curve::preset("soft"), opacity: None }, ..BrushSettings::default() };
        assert_eq!(b.stroke_response(), None, "no pen seen yet");
        b.pen.source = Source::Pen;
        assert!(b.stroke_response().is_some());
        b.pressure = false;
        assert_eq!(b.stroke_response(), None);
        b.pressure = true;
        b.pen.source = Source::Pointer;
        assert_eq!(b.stroke_response(), None);
        b.pen.source = Source::Pen;
        b.response = PressureResponse::default();
        assert_eq!(b.stroke_response(), None, "the default is never stored");
    }
}
