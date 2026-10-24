//! Drawing view (#63): turn, mirror and pan the Program monitor's picture to draw comfortably.
//!
//! This is only how the picture is *shown*. The rotation and the mirror live in the Program
//! monitor's view state (`ui.inspect` → `ui.program.canvas`), never in the project: they add no
//! undo step, do not mark the project changed, do not touch clip Motion or any artwork and are
//! not part of any export. They are not loaded back at start-up, so WaltCut always opens
//! unturned. A stroke is mapped back through the view, the monitor zoom and the clip's Motion,
//! so it lands where the pen is whatever is set.
//!
//! * `view.canvasRotate {degrees}` sets the rotation, `view.canvasRotateRight` /
//!   `view.canvasRotateLeft` `{degrees?}` turn it by a step (15° by default). Clockwise is
//!   positive, the value is kept in (-180, 180].
//! * `view.canvasMirror {enabled?}` flips the picture left to right (before the rotation).
//! * `view.canvasReset` returns to the default: 0°, not mirrored, Fit, centred.
//! * `view.canvasPan {active?}` pans a magnified picture with the pen or mouse while it is held:
//!   bound to a key, for as long as the key is down (it ends on Escape, loss of focus and when
//!   another tool is chosen); from a command until `{active: false}`. While it is held a drag
//!   never draws.
//!
//! **Modifier ownership.** `Alt` + a letter is a held drawing aid (`Alt+E` erase, `Alt+H` pan),
//! `Cmd+Alt` + an arrow is pose review (#61). A reference placed under the drawing (#3) is moved
//! with its own controls, never with a plain drag on the picture, so the pan hold and the
//! reference cannot compete for the same gesture.
//!
//! While the picture is turned or mirrored the monitor's other overlays (safe margins, rulers,
//! guides, the Graphics and Transform handles, the eyedropper) are not drawn: they assume an
//! upright picture. Reset the view to use them.

use egui::{Pos2, Rect, vec2};
use serde::Serialize;
use serde_json::{Value, json};
use waltcut_geom::{Affine, Vec2};

use crate::WaltCutApp;
use crate::state::Tool;

/// Largest |degrees| a command accepts (it is wrapped into (-180, 180]).
const MAX_DEGREES: f64 = 1.0e6;
/// The default step of the rotate commands.
pub const DEFAULT_STEP: f32 = 15.0;

/// Why the pan hold is on, if it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum PanHold {
    #[default]
    Off,
    /// The key bound to Pan While Held is down.
    Key,
    /// Switched on by `view.canvasPan {active: true}` until `{active: false}`.
    Command,
}

/// How the Program monitor's picture is turned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct CanvasView {
    /// Degrees, clockwise, in (-180, 180].
    pub rotation: f32,
    /// Flipped left to right, before the rotation.
    pub mirror: bool,
    #[serde(rename = "panHeld")]
    pub pan_held: PanHold,
}

/// `degrees` wrapped into (-180, 180]; non-finite counts as 0.
pub fn wrap_degrees(degrees: f64) -> f32 {
    if !degrees.is_finite() {
        return 0.0;
    }
    let r = degrees.rem_euclid(360.0);
    (if r > 180.0 { r - 360.0 } else { r }) as f32
}

impl CanvasView {
    /// Whether the picture is shown as it is (nothing turned or mirrored).
    pub fn is_upright(&self) -> bool {
        self.turn() == 0.0 && !self.mirror
    }

    /// The rotation as it is used: finite and within (-180, 180], whatever the state holds.
    fn turn(&self) -> f32 {
        wrap_degrees(self.rotation as f64)
    }

    /// The layout → screen transform about `centre`: mirror, then rotate.
    pub fn affine(&self, centre: Pos2) -> Affine {
        let (cx, cy) = (centre.x as f64, centre.y as f64);
        let flip = Affine::scale(if self.mirror { -1.0 } else { 1.0 }, 1.0);
        Affine::translate(cx, cy).then_apply(&Affine::rotate_deg(self.turn() as f64)).then_apply(&flip).then_apply(&Affine::translate(-cx, -cy))
    }

    /// A layout point as it is shown.
    pub fn show(&self, centre: Pos2, p: Pos2) -> Pos2 {
        let q = self.affine(centre).apply(Vec2::new(p.x as f64, p.y as f64));
        Pos2::new(q.x as f32, q.y as f32)
    }

    /// The layout point shown at the screen point `p` (itself if the transform cannot be undone).
    pub fn unshow(&self, centre: Pos2, p: Pos2) -> Pos2 {
        match self.affine(centre).inverse() {
            Some(inv) => {
                let q = inv.apply(Vec2::new(p.x as f64, p.y as f64));
                Pos2::new(q.x as f32, q.y as f32)
            }
            None => p,
        }
    }

    /// The screen rectangle that bounds the shown picture `pic`.
    pub fn bounds(&self, pic: Rect) -> Rect {
        if self.is_upright() {
            return pic;
        }
        let c = pic.center();
        [pic.left_top(), pic.right_top(), pic.right_bottom(), pic.left_bottom()].iter().fold(Rect::NOTHING, |r, p| {
            let mut r = r;
            r.extend_with(self.show(c, *p));
            r
        })
    }

    /// The picture's corners as shown, clockwise from the top-left of the layout.
    pub fn corners(&self, pic: Rect) -> [Pos2; 4] {
        let c = pic.center();
        [pic.left_top(), pic.right_top(), pic.right_bottom(), pic.left_bottom()].map(|p| self.show(c, p))
    }

    /// `pic` made smaller about its centre (never larger) so the turned picture fits in `area`.
    pub fn fitted(&self, pic: Rect, area: Rect) -> Rect {
        if self.is_upright() {
            return pic;
        }
        let b = self.bounds(pic);
        let k = (area.width() / b.width()).min(area.height() / b.height());
        if !(k.is_finite() && k > 0.0) {
            return Rect::from_center_size(pic.center(), vec2(1.0, 1.0));
        }
        if k >= 1.0 { pic } else { Rect::from_center_size(pic.center(), pic.size() * k) }
    }
}

/// Paint `texture` over the layout rectangle `pic` as it is shown under `view`.
pub fn paint_picture(painter: &egui::Painter, texture: egui::TextureId, pic: Rect, view: &CanvasView) {
    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if view.is_upright() {
        painter.image(texture, pic, uv, egui::Color32::WHITE);
        return;
    }
    let mut mesh = egui::Mesh::with_texture(texture);
    let uvs = [uv.left_top(), uv.right_top(), uv.right_bottom(), uv.left_bottom()];
    for (pos, uv) in view.corners(pic).into_iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: egui::Color32::WHITE });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Fill the shown picture with a flat colour.
pub fn fill_picture(painter: &egui::Painter, pic: Rect, view: &CanvasView, color: egui::Color32) {
    if view.is_upright() {
        painter.rect_filled(pic, 0.0, color);
    } else {
        painter.add(egui::Shape::convex_polygon(view.corners(pic).to_vec(), color, egui::Stroke::NONE));
    }
}

pub fn params_doc(id: &str) -> Option<&'static str> {
    match id {
        "view.canvasRotate" => Some(r#"{"degrees":number}  set the view's rotation, clockwise, wrapped into (-180, 180]; view only"#),
        "view.canvasRotateRight" | "view.canvasRotateLeft" => Some(r#"{"degrees":number?}  turn the view by a step (default 15°); view only"#),
        "view.canvasMirror" => Some(r#"{"enabled":bool?}  flip the picture left to right (toggle by default); view only"#),
        "view.canvasReset" => Some("{}  rotation 0°, not mirrored, Fit, centred"),
        "view.canvasPan" => Some(
            r#"{"active":bool?}  pan a magnified picture with a drag, without drawing, until false, the window loses focus or another tool is chosen; bound to a key: while it is held"#,
        ),
        _ => None,
    }
}

/// Menu checkmark of the mirror toggle.
pub fn checked(app: &WaltCutApp, id: &str) -> Option<bool> {
    (id == "view.canvasMirror").then_some(app.ui.program.canvas.mirror)
}

fn number(params: &Value, key: &str) -> Result<Option<f64>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64() {
            Some(n) if n.is_finite() && n.abs() <= MAX_DEGREES => Ok(Some(n)),
            _ => Err(format!("`{key}` must be a number between -{MAX_DEGREES} and {MAX_DEGREES}")),
        },
    }
}

fn state(app: &WaltCutApp) -> Value {
    json!({"rotation": app.ui.program.canvas.rotation, "mirror": app.ui.program.canvas.mirror, "panHeld": app.ui.program.canvas.pan_held != PanHold::Off})
}

/// The `view.canvas*` commands (None for other ids).
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    params_doc(id)?;
    Some(run(app, id, params))
}

fn run(app: &mut WaltCutApp, id: &str, params: &Value) -> Result<Value, String> {
    if !matches!(params, Value::Null | Value::Object(_)) {
        return Err("parameters must be an object".into());
    }
    match id {
        "view.canvasRotate" => {
            let degrees = number(params, "degrees")?.ok_or("`degrees` is required")?;
            app.ui.program.canvas.rotation = wrap_degrees(degrees);
        }
        "view.canvasRotateRight" | "view.canvasRotateLeft" => {
            let step = number(params, "degrees")?.unwrap_or(DEFAULT_STEP as f64);
            let signed = if id == "view.canvasRotateRight" { step } else { -step };
            app.ui.program.canvas.rotation = wrap_degrees(app.ui.program.canvas.rotation as f64 + signed);
        }
        "view.canvasMirror" => {
            let c = &mut app.ui.program.canvas;
            c.mirror = match params.get("enabled") {
                None | Some(Value::Null) => !c.mirror,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err("`enabled` must be true or false".into()),
            };
        }
        "view.canvasReset" => {
            let v = &mut app.ui.program;
            v.canvas = CanvasView::default();
            v.zoom = None;
            v.pan = [0.0, 0.0];
        }
        _ => {
            let active = match params.get("active") {
                None | Some(Value::Null) => true,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err("`active` must be true or false".into()),
            };
            let key = params.get("key").and_then(Value::as_bool).unwrap_or(false);
            app.ui.program.canvas.pan_held = match (active, key) {
                (false, _) => PanHold::Off,
                (true, true) => PanHold::Key,
                (true, false) => PanHold::Command,
            };
        }
    }
    Ok(state(app))
}

/// End a pan hold that no longer applies: its key came up, Escape or loss of focus, or another
/// tool was chosen. Called every frame before shortcuts are handled.
pub fn release_stale_pan(app: &mut WaltCutApp, ctx: &egui::Context) {
    let hold = app.ui.program.canvas.pan_held;
    if hold == PanHold::Off {
        return;
    }
    // the loss itself, not the state: a window driven over the control channel is never focused
    let (focus_lost, escape) = ctx.input(|i| (i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))), i.key_pressed(egui::Key::Escape)));
    let key_up = hold == PanHold::Key && !app.bindings.iter().any(|(_, k, id, _)| id == "view.canvasPan" && ctx.input(|i| i.key_down(*k)));
    if app.ui.tool != Tool::Brush || focus_lost || escape || key_up {
        app.ui.program.canvas.pan_held = PanHold::Off;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Pos2, b: Pos2) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn rotation_is_clockwise_and_mirror_comes_first() {
        let c = Pos2::new(100.0, 50.0);
        let right = CanvasView { rotation: 90.0, ..Default::default() };
        // a point to the right of the centre goes below it (clockwise on a y-down screen)
        assert!(close(right.show(c, Pos2::new(110.0, 50.0)), Pos2::new(100.0, 60.0)));
        let mirror = CanvasView { mirror: true, ..Default::default() };
        assert!(close(mirror.show(c, Pos2::new(110.0, 50.0)), Pos2::new(90.0, 50.0)));
        // mirrored, then turned: right → left → above
        let both = CanvasView { rotation: 90.0, mirror: true, ..Default::default() };
        assert!(close(both.show(c, Pos2::new(110.0, 50.0)), Pos2::new(100.0, 40.0)));
    }

    #[test]
    fn unshow_inverts_show_for_every_turn_and_mirror() {
        let c = Pos2::new(320.0, 180.0);
        for mirror in [false, true] {
            for r in [-179.0, -90.0, -13.5, 0.0, 1.0, 45.0, 90.0, 180.0] {
                let v = CanvasView { rotation: r, mirror, ..Default::default() };
                for p in [Pos2::new(0.0, 0.0), Pos2::new(500.0, 30.0), Pos2::new(-40.0, 900.0)] {
                    assert!(close(v.unshow(c, v.show(c, p)), p), "{r} {mirror} {p:?}");
                }
            }
        }
    }

    #[test]
    fn degrees_wrap_into_half_open_range_and_hostile_values_are_zero() {
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-190.0), 170.0);
        assert_eq!(wrap_degrees(180.0), 180.0);
        assert_eq!(wrap_degrees(-180.0), 180.0);
        assert_eq!(wrap_degrees(720.0), 0.0);
        assert_eq!(wrap_degrees(f64::NAN), 0.0);
        assert_eq!(wrap_degrees(f64::INFINITY), 0.0);
    }

    #[test]
    fn a_turned_picture_is_fitted_into_its_area_and_bounds_hold_it() {
        let area = Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(400.0, 300.0));
        let pic = Rect::from_center_size(area.center(), vec2(400.0, 225.0));
        let v = CanvasView { rotation: 90.0, ..Default::default() };
        let fitted = v.fitted(pic, area);
        let b = v.bounds(fitted);
        assert!(b.width() <= area.width() + 1e-3 && b.height() <= area.height() + 1e-3, "{b:?}");
        assert!((b.height() - 300.0).abs() < 1e-3, "the turned height fills the area: {b:?}");
        assert!(close(fitted.center(), pic.center()));
        // upright: untouched; never enlarged
        assert_eq!(CanvasView::default().fitted(pic, area), pic);
        let small = Rect::from_center_size(area.center(), vec2(40.0, 20.0));
        assert_eq!(v.fitted(small, area), small);
        // an empty area is not a division by zero
        let _ = v.fitted(pic, Rect::from_min_size(Pos2::ZERO, vec2(0.0, 0.0)));
        let _ = v.fitted(Rect::NOTHING, area);
    }

    #[test]
    fn a_singular_transform_cannot_panic() {
        let v = CanvasView { rotation: f32::NAN, mirror: true, ..Default::default() };
        let c = Pos2::new(1.0, 1.0);
        let _ = v.unshow(c, Pos2::new(5.0, 5.0));
        let _ = v.bounds(Rect::from_min_size(c, vec2(10.0, 10.0)));
    }
}
