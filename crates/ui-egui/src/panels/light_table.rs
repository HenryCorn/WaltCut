//! Light table (#64): temporarily shift and turn the pinned comparison pose to check proportions
//! against the cel being drawn.
//!
//! This is only how the pinned onion-skin pose is *shown*. The shift lives in the Brush tool's
//! UI state (`ui.inspect` → `ui.brush.light_table`), never in the project: it adds no undo step,
//! does not mark the project changed, does not move the pose's source cel or its exposure, and is
//! not part of any export or AI source frame. It is not loaded back, so WaltCut starts unshifted.
//!
//! * `drawing.lightTable {dx?, dy?, rotation?, reset?}` sets the shift of the pinned pose (canvas
//!   pixels, degrees clockwise about the middle of the canvas); with no parameter it reports.
//! * `drawing.lightTableShift {active?}` (`Alt+T`, **Light table ▸ Shift pose with pen** in the options bar) is the hold:
//!   while it is on, a drag on the picture moves the pose (Shift-drag turns it) and never draws.
//!   Bound to a key it lasts as long as the key is down; as a command, until `{active: false}`.
//! * `drawing.lightTableReset` (`Alt+Shift+T`, **Reset** in the options bar) puts the pose back.
//!
//! **Exit rules.**
//! * Letting go of the key, `{active: false}`, unticking **Shift pose** or choosing another tool
//!   ends the hold and *keeps* the alignment, so the current cel can be drawn against it.
//! * Escape, or the window losing focus, *during* a hold abandons the whole hold: the alignment
//!   goes back to what it was when the hold began.
//! * Reset, unpinning the pose, pinning another cel, a different drawing clip or the pinned cel
//!   being deleted *discard* the alignment. Moving the playhead, drawing, switching tools or
//!   turning Onion skin off keep it (the pose is only hidden meanwhile).
//!
//! A drag is mapped back through the Program monitor's view (rotation, mirror, zoom) and the
//! clip's Motion, exactly as a stroke is, so the pose follows the pen in a turned view.
//!
//! Automation ids: `program.brush.light.menu`, then in the menu `.shift`, `.dx`, `.dy`, `.rotation`,
//! `.reset`; while shifting or shifted `.badge`, `.done`, `.resetBadge`.

use egui::{Color32, Pos2, Stroke as EStroke, Ui, vec2};
use serde::Serialize;
use serde_json::{Value, json};
use waltcut_geom::{Affine, Vec2};
use waltcut_project::drawing::Drawing;

use super::drawing::{Mapping, OnionPin, target};
use crate::WaltCutApp;
use crate::state::Tool;

/// Largest |offset| a command accepts, canvas pixels.
pub const MAX_OFFSET: f64 = 1.0e6;
/// Largest |degrees| a command accepts (it is wrapped into (-180, 180]).
pub const MAX_DEGREES: f64 = 36_000.0;

/// `degrees` wrapped into (-180, 180]; non-finite counts as 0.
fn wrap(degrees: f64) -> f64 {
    if !degrees.is_finite() {
        return 0.0;
    }
    let r = degrees.rem_euclid(360.0);
    if r > 180.0 { r - 360.0 } else { r }
}

/// How far the pinned pose is moved and turned, in canvas pixels and degrees clockwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Shift {
    pub dx: f64,
    pub dy: f64,
    pub rotation: f64,
}

impl Shift {
    pub fn is_identity(&self) -> bool {
        self.dx == 0.0 && self.dy == 0.0 && self.rotation == 0.0
    }

    /// The shift as it is used: every number finite and inside its bounds, whatever the state
    /// holds (a hand-edited UI state cannot send the pose to infinity).
    pub fn sane(self) -> Shift {
        let bound = |v: f64| if v.is_finite() { v.clamp(-MAX_OFFSET, MAX_OFFSET) } else { 0.0 };
        Shift { dx: bound(self.dx), dy: bound(self.dy), rotation: wrap(self.rotation) }
    }

    /// Canvas → canvas: turn about `pivot`, then move.
    pub fn affine(&self, pivot: (f64, f64)) -> Affine {
        let s = self.sane();
        Affine::translate(pivot.0 + s.dx, pivot.1 + s.dy).then_apply(&Affine::rotate_deg(s.rotation)).then_apply(&Affine::translate(-pivot.0, -pivot.1))
    }

    /// The shift after the pen went from `prev` to `cur` (canvas pixels): moved by the pen's
    /// travel, or, with `turn`, turned about the pose's (moved) pivot by the angle the pen swept.
    pub fn dragged(self, pivot: (f64, f64), prev: (f64, f64), cur: (f64, f64), turn: bool) -> Shift {
        let s = self.sane();
        if !turn {
            return Shift { dx: s.dx + (cur.0 - prev.0), dy: s.dy + (cur.1 - prev.1), ..s }.sane();
        }
        let centre = (pivot.0 + s.dx, pivot.1 + s.dy);
        let (a, b) = ((prev.0 - centre.0, prev.1 - centre.1), (cur.0 - centre.0, cur.1 - centre.1));
        // a pen on the pivot has no direction
        if !(a.0.hypot(a.1) >= 1.0 && b.0.hypot(b.1) >= 1.0) {
            return s;
        }
        let sweep = (b.1.atan2(b.0) - a.1.atan2(a.0)).to_degrees();
        Shift { rotation: s.rotation + wrap(sweep), ..s }.sane()
    }
}

/// Why the shift hold is on, if it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum ShiftHold {
    #[default]
    Off,
    /// The key bound to Shift Pose While Held is down.
    Key,
    /// Switched on by `drawing.lightTableShift {active: true}` until `{active: false}`.
    Command,
}

/// The light table: the shift of the pinned pose and whether a drag adjusts it. View state only.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LightTable {
    pub hold: ShiftHold,
    /// The pinned cel the shift belongs to.
    pub pose: Option<OnionPin>,
    pub shift: Shift,
    /// The shift when the hold began: Escape and loss of focus go back to it.
    #[serde(skip)]
    baseline: Option<Shift>,
}

impl LightTable {
    /// The shift of the pose `pin` (none for any other).
    pub fn shift_of(&self, pin: Option<OnionPin>) -> Shift {
        if self.pose.is_some() && self.pose == pin { self.shift.sane() } else { Shift::default() }
    }

    pub fn is_shifted(&self) -> bool {
        self.pose.is_some() && !self.shift.sane().is_identity()
    }
}

pub fn params_doc(id: &str) -> Option<&'static str> {
    match id {
        "drawing.lightTable" => Some(
            r#"{"dx":px?,"dy":px?,"rotation":deg?,"reset":bool?}  move (canvas pixels) and turn (degrees, clockwise, about the middle of the canvas) the pinned onion-skin pose; view only, nothing is edited or exported. With no parameter, report"#,
        ),
        "drawing.lightTableShift" => Some(
            r#"{"active":bool?}  true (default): a drag on the picture moves the pinned pose (Shift-drag turns it) instead of drawing, until false, Escape, loss of focus (those two also undo the hold's changes) or another tool; bound to a key: while it is held"#,
        ),
        "drawing.lightTableReset" => Some("{}  put the pinned pose back where it was drawn"),
        _ => None,
    }
}

/// The hold needs the Brush tool and a pinned pose; the others are view settings.
pub fn enabled(app: &WaltCutApp, id: &str) -> bool {
    match id {
        "drawing.lightTableShift" => app.ui.tool == Tool::Brush && pinned(app).is_some(),
        _ => params_doc(id).is_some(),
    }
}

/// The pinned pose of the drawing under the playhead, with its cel index, if it exists.
fn pinned(app: &WaltCutApp) -> Option<(OnionPin, usize)> {
    let pin = app.ui.brush.onion_pin?;
    let (clip, _, d) = target(app)?;
    (clip.0 == pin.clip).then_some(())?;
    Some((pin, d.cel_on(pin.frame)?))
}

/// The middle of the drawing's canvas: what the pose turns about.
pub fn pivot(d: &Drawing) -> (f64, f64) {
    (f64::from(d.width) * 0.5, f64::from(d.height) * 0.5)
}

/// The cel index and canvas → canvas transform of the shifted pose, if one is shifted.
pub fn placement(app: &WaltCutApp, d: &Drawing) -> Option<(usize, Affine)> {
    let lt = &app.ui.brush.light_table;
    let pin = app.ui.brush.onion_pin;
    let shift = lt.shift_of(pin);
    if shift.is_identity() {
        return None;
    }
    let index = d.cel_on(pin?.frame)?;
    Some((index, shift.affine(pivot(d))))
}

/// Whether the pinned pose is on screen now: Onion skin on, and the pose is a ghost (it is not the
/// cel shown, and not hidden behind a nearer cel with the same strokes).
pub fn showing(app: &WaltCutApp) -> bool {
    let b = &app.ui.brush;
    let (true, Some((pin, index)), Some((_, item, d))) = (b.onion, pinned(app), target(app)) else { return false };
    let t = item.source_time_at(app.session.playhead().clamp(item.start, item.end() - waltcut_time::Tick(1)));
    let look = b.onion_look();
    d.onion_plan(d.frame_at(t), look.before, look.after, Some(pin.frame), b.onion_loop).iter().any(|g| g.index == index)
}

fn hold_name(h: ShiftHold) -> &'static str {
    match h {
        ShiftHold::Off => "Off",
        ShiftHold::Key => "Key",
        ShiftHold::Command => "Command",
    }
}

fn state(app: &WaltCutApp) -> Value {
    let lt = &app.ui.brush.light_table;
    let s = lt.shift_of(app.ui.brush.onion_pin);
    json!({"pinned": pinned(app).is_some(), "pose": lt.pose, "dx": s.dx, "dy": s.dy, "rotation": s.rotation,
        "shifted": lt.is_shifted(), "hold": hold_name(lt.hold), "showing": showing(app)})
}

fn num(params: &Value, key: &str, bound: f64) -> Result<Option<f64>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64() {
            Some(n) if n.is_finite() && n.abs() <= bound => Ok(Some(n)),
            _ => Err(format!("`{key}` must be a number between -{bound} and {bound}")),
        },
    }
}

fn flag(params: &Value, key: &str) -> Result<Option<bool>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("`{key}` must be true or false")),
    }
}

/// The `drawing.lightTable*` commands (None for other ids).
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    params_doc(id)?;
    Some(run(app, id, params))
}

fn run(app: &mut WaltCutApp, id: &str, params: &Value) -> Result<Value, String> {
    if !matches!(params, Value::Null | Value::Object(_)) {
        return Err("parameters must be an object".into());
    }
    validate(app);
    match id {
        "drawing.lightTableReset" => app.ui.brush.light_table = LightTable::default(),
        "drawing.lightTableShift" => hold(app, params)?,
        _ => set(app, params)?,
    }
    Ok(state(app))
}

/// `drawing.lightTable`. Every parameter is checked before any is applied.
fn set(app: &mut WaltCutApp, params: &Value) -> Result<(), String> {
    if let Some(o) = params.as_object()
        && let Some(k) = o.keys().find(|k| !["dx", "dy", "rotation", "reset"].contains(&k.as_str()))
    {
        return Err(format!("unknown parameter `{k}`; expected: dx, dy, rotation, reset"));
    }
    let (dx, dy, rotation) = (num(params, "dx", MAX_OFFSET)?, num(params, "dy", MAX_OFFSET)?, num(params, "rotation", MAX_DEGREES)?);
    let reset = flag(params, "reset")?.unwrap_or(false);
    if reset && (dx.is_some() || dy.is_some() || rotation.is_some()) {
        return Err("`reset` cannot be combined with dx, dy or rotation".into());
    }
    if dx.is_none() && dy.is_none() && rotation.is_none() && !reset {
        return Ok(());
    }
    if reset {
        app.ui.brush.light_table = LightTable::default();
        return Ok(());
    }
    let (pin, _) = pinned(app).ok_or("pin a pose first (Pin Pose for Onion Skin): the light table shifts the pinned pose")?;
    let cur = app.ui.brush.light_table.shift_of(Some(pin));
    let next = Shift { dx: dx.unwrap_or(cur.dx), dy: dy.unwrap_or(cur.dy), rotation: rotation.map_or(cur.rotation, wrap) }.sane();
    let lt = &mut app.ui.brush.light_table;
    lt.pose = Some(pin);
    lt.shift = next;
    Ok(())
}

/// `drawing.lightTableShift`.
fn hold(app: &mut WaltCutApp, params: &Value) -> Result<(), String> {
    let active = flag(params, "active")?.unwrap_or(true);
    let key = flag(params, "key")?.unwrap_or(false);
    if !active {
        let lt = &mut app.ui.brush.light_table;
        lt.hold = ShiftHold::Off;
        lt.baseline = None;
        settle(lt);
        return Ok(());
    }
    if app.ui.tool != Tool::Brush {
        return Err("`drawing.lightTableShift` is available while the Brush tool is selected".into());
    }
    let (pin, _) = pinned(app).ok_or("pin a pose first (Pin Pose for Onion Skin): the light table shifts the pinned pose")?;
    if !app.ui.brush.onion {
        return Err("turn Onion skin on: the pinned pose is hidden without it".into());
    }
    let lt = &mut app.ui.brush.light_table;
    if lt.pose != Some(pin) {
        *lt = LightTable::default();
        lt.pose = Some(pin);
    }
    if lt.baseline.is_none() {
        lt.baseline = Some(lt.shift.sane());
    }
    lt.hold = if key { ShiftHold::Key } else { ShiftHold::Command };
    Ok(())
}

/// A table that holds nothing (no hold, no shift) is the default.
fn settle(lt: &mut LightTable) {
    if lt.hold == ShiftHold::Off && lt.shift.sane().is_identity() {
        *lt = LightTable::default();
    }
}

/// Discard the alignment when its target is gone: the pin was cleared or moved to another cel,
/// the drawing clip changed or the pinned cel was deleted. Playing past the end of the clip, or
/// into a gap, is not a change of target.
pub fn validate(app: &mut WaltCutApp) {
    let lt = &app.ui.brush.light_table;
    let Some(pose) = lt.pose else {
        if lt.hold != ShiftHold::Off {
            app.ui.brush.light_table = LightTable::default();
        }
        return;
    };
    let keep = app.ui.brush.onion_pin == Some(pose)
        && match target(app) {
            Some((clip, _, d)) => clip.0 != pose.clip || d.cel_on(pose.frame).is_some(),
            None => true,
        };
    if !keep {
        app.ui.brush.light_table = LightTable::default();
    }
}

/// End a hold that no longer applies: its key came up, Escape or loss of focus (those also undo
/// what the hold changed), or another tool was chosen. Called every frame before shortcuts are
/// handled.
pub fn release_stale_shift(app: &mut WaltCutApp, ctx: &egui::Context) {
    validate(app);
    let hold = app.ui.brush.light_table.hold;
    if hold == ShiftHold::Off {
        return;
    }
    // the loss itself, not the state: a window driven over the control channel is never focused
    let (focus_lost, escape) = ctx.input(|i| (i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))), i.key_pressed(egui::Key::Escape)));
    let key_up = hold == ShiftHold::Key && !app.bindings.iter().any(|(_, k, id, _)| id == "drawing.lightTableShift" && ctx.input(|i| i.key_down(*k)));
    let lt = &mut app.ui.brush.light_table;
    if focus_lost || escape {
        lt.shift = lt.baseline.take().unwrap_or_default();
        lt.hold = ShiftHold::Off;
        settle(lt);
    } else if app.ui.tool != Tool::Brush || key_up {
        lt.hold = ShiftHold::Off;
        lt.baseline = None;
        settle(lt);
    }
}

/// A drag on the picture while the hold is on: move the pose, or with Shift held turn it.
pub(super) fn drag(app: &mut WaltCutApp, ui: &Ui, resp: &egui::Response, map: &Mapping, d: &Drawing) {
    let grabbing = ui.input(|i| i.pointer.primary_down());
    ui.ctx().set_cursor_icon(if grabbing { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    let moved = resp.drag_delta();
    if !resp.dragged_by(egui::PointerButton::Primary) || moved == egui::Vec2::ZERO {
        return;
    }
    let (Some(here), Some((pin, _))) = (resp.interact_pointer_pos(), pinned(app)) else { return };
    let turn = ui.input(|i| i.modifiers.shift);
    // egui starts a drag only after the pen has travelled a few points: count from where it was
    // pressed, so the pose stays under the pen from the first touch
    let prev = if resp.drag_started_by(egui::PointerButton::Primary) { ui.input(|i| i.pointer.press_origin()).unwrap_or(here - moved) } else { here - moved };
    let (a, b) = (map.canvas(prev), map.canvas(here));
    let lt = &mut app.ui.brush.light_table;
    let cur = lt.shift_of(Some(pin));
    lt.pose = Some(pin);
    lt.shift = cur.dragged(pivot(d), (f64::from(a.0), f64::from(a.1)), (f64::from(b.0), f64::from(b.1)), turn);
}

/// The pose's outline as it is shown now, canvas pixels: the bounds of its strokes.
fn pose_bounds(d: &Drawing, index: usize) -> Option<(f32, f32, f32, f32)> {
    let cel = d.cels.get(index)?;
    cel.strokes.iter().filter_map(|s| s.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

/// Mark the shifted pose on the picture: a dashed outline of where it is now and a note saying how
/// far it is from where it was drawn.
pub(super) fn paint_indicator(app: &WaltCutApp, painter: &egui::Painter, map: &Mapping, d: &Drawing, bar_bottom: Pos2) {
    let lt = &app.ui.brush.light_table;
    if !lt.is_shifted() && lt.hold == ShiftHold::Off {
        return;
    }
    let blue = Color32::from_rgb(70, 130, 230);
    let s = lt.shift_of(app.ui.brush.onion_pin);
    let shown = showing(app);
    if shown
        && let (Some(pin), Some(_)) = (app.ui.brush.onion_pin, pinned(app))
        && let Some(index) = d.cel_on(pin.frame)
        && let Some((x0, y0, x1, y1)) = pose_bounds(d, index)
    {
        let m = s.affine(pivot(d));
        let to_screen = |x: f32, y: f32| {
            let q = map.to_screen.apply(m.apply(Vec2::new(f64::from(x), f64::from(y))));
            Pos2::new(q.x as f32, q.y as f32)
        };
        let mut outline = vec![to_screen(x0, y0), to_screen(x1, y0), to_screen(x1, y1), to_screen(x0, y1)];
        outline.push(outline[0]);
        painter.extend(egui::Shape::dashed_line(&outline, EStroke::new(1.5, blue), 6.0, 4.0));
    }
    let note = if !shown {
        "Pinned pose not showing: it is the cel shown, hidden behind a nearer copy, or Onion skin is off".to_string()
    } else if s.is_identity() {
        "Shift pose: drag the pinned pose (Shift-drag turns it)".to_string()
    } else {
        format!("Pose shifted · X {:+.0} · Y {:+.0} · {:+.1}°", s.dx, s.dy, s.rotation)
    };
    painter.text(bar_bottom + vec2(0.0, 6.0), egui::Align2::LEFT_TOP, note, egui::FontId::proportional(12.0), blue.gamma_multiply(1.4));
}

/// The light-table controls of the Brush options bar: one menu (the hold, the numbers, Reset) and,
/// while the pose is being shifted or is shifted, the badge that says so with a way out that needs
/// no menu. The bar stays one button wider, so it keeps its height.
pub fn controls(app: &mut WaltCutApp, ui: &mut Ui) {
    validate(app);
    let can = pinned(app).is_some();
    let held = app.ui.brush.light_table.hold != ShiftHold::Off;
    let s = app.ui.brush.light_table.shift_of(app.ui.brush.onion_pin);
    let menu = ui.menu_button("Light table", |ui| {
        if !can {
            ui.label("Pin a pose first (Pin Pose for Onion Skin).");
            return;
        }
        let mut on = held;
        let r = ui
            .checkbox(&mut on, "Shift pose with pen")
            .on_hover_text("Drag the pinned pose to line it up with the drawing; Shift-drag turns it (Alt+T). Escape undoes it");
        app.auto.add("program.brush.light.shift", r.rect, "shift pinned pose");
        if r.changed() {
            run_ui(app, "drawing.lightTableShift", json!({"active": on}));
        }
        let mut next = s;
        let mut changed = false;
        let mut field =
            |ui: &mut Ui, id: &str, label: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, speed: f64, auto: &mut crate::automation::Registry| {
                ui.horizontal(|ui| {
                    ui.label(label);
                    let r = ui.add(egui::DragValue::new(v).range(range).speed(speed));
                    auto.add(&format!("program.brush.light.{id}"), r.rect, label);
                    changed |= r.changed();
                });
            };
        field(ui, "dx", "X", &mut next.dx, -100_000.0..=100_000.0, 1.0, &mut app.auto);
        field(ui, "dy", "Y", &mut next.dy, -100_000.0..=100_000.0, 1.0, &mut app.auto);
        field(ui, "rotation", "Rotate °", &mut next.rotation, -180.0..=180.0, 0.5, &mut app.auto);
        if changed {
            run_ui(app, "drawing.lightTable", json!({"dx": next.dx, "dy": next.dy, "rotation": next.rotation}));
        }
        let r = ui.add_enabled(app.ui.brush.light_table.is_shifted(), egui::Button::new("Reset"));
        app.auto.add("program.brush.light.reset", r.rect, "reset pose shift");
        if r.clicked() {
            run_ui(app, "drawing.lightTableReset", json!({}));
        }
    });
    app.auto.add("program.brush.light.menu", menu.response.rect, "light table");
    let shifted = app.ui.brush.light_table.is_shifted();
    if shifted || held {
        // an obvious sign that the ghost is not where it was drawn, with a way back that needs no menu
        let blue = Color32::from_rgb(70, 130, 230);
        let text = if held { "Shifting pose" } else { "Pose shifted" };
        let l = ui.add(egui::Label::new(egui::RichText::new(text).strong().color(blue)).extend());
        app.auto.add("program.brush.light.badge", l.rect, "pose shifted");
        if held {
            let r = ui.button("Done");
            app.auto.add("program.brush.light.done", r.rect, "stop shifting the pose");
            if r.clicked() {
                run_ui(app, "drawing.lightTableShift", json!({"active": false}));
            }
        }
        if shifted {
            let r = ui.button("Reset pose");
            app.auto.add("program.brush.light.resetBadge", r.rect, "reset pose shift");
            if r.clicked() {
                run_ui(app, "drawing.lightTableReset", json!({}));
            }
        }
    }
}

fn run_ui(app: &mut WaltCutApp, id: &str, params: Value) {
    if let Some(Err(e)) = route(app, id, &params) {
        app.ui.status = e;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec2, b: Vec2) -> bool {
        (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6
    }

    #[test]
    fn the_shift_turns_about_the_pivot_clockwise_then_moves() {
        let pivot = (160.0, 90.0);
        let s = Shift { dx: 10.0, dy: -5.0, rotation: 90.0 };
        let m = s.affine(pivot);
        // the pivot only moves
        assert!(close(m.apply(Vec2::new(160.0, 90.0)), Vec2::new(170.0, 85.0)));
        // a point to the right of the pivot goes below it (clockwise on a y-down canvas), then moves
        assert!(close(m.apply(Vec2::new(170.0, 90.0)), Vec2::new(170.0, 95.0)));
        assert!(Shift::default().affine(pivot).is_identity());
    }

    #[test]
    fn a_drag_moves_by_the_pens_travel_and_with_shift_turns_by_the_swept_angle() {
        let pivot = (100.0, 100.0);
        let moved = Shift::default().dragged(pivot, (10.0, 10.0), (25.0, 4.0), false);
        assert_eq!((moved.dx, moved.dy, moved.rotation), (15.0, -6.0, 0.0));
        // from the right of the pivot to below it is a quarter turn clockwise
        let turned = Shift::default().dragged(pivot, (150.0, 100.0), (100.0, 150.0), true);
        assert!((turned.rotation - 90.0).abs() < 1e-9 && turned.dx == 0.0, "{turned:?}");
        // the pivot moves with the pose: sweeping about the moved pivot
        let off = Shift { dx: 50.0, dy: 0.0, rotation: 0.0 };
        let turned = off.dragged(pivot, (200.0, 100.0), (150.0, 150.0), true);
        assert!((turned.rotation - 90.0).abs() < 1e-9 && turned.dx == 50.0, "{turned:?}");
        // a pen on the pivot turns nothing; wrap-around stays in (-180, 180]
        assert_eq!(Shift::default().dragged(pivot, (100.0, 100.0), (130.0, 100.0), true), Shift::default());
        let past = Shift { rotation: 170.0, ..Default::default() }.dragged(pivot, (150.0, 100.0), (100.0, 150.0), true);
        assert!((past.rotation + 100.0).abs() < 1e-9, "{past:?}");
    }

    #[test]
    fn hostile_numbers_are_brought_back_in_range() {
        let wild = Shift { dx: f64::NAN, dy: f64::INFINITY, rotation: f64::NEG_INFINITY }.sane();
        assert_eq!(wild, Shift::default());
        let big = Shift { dx: 1e300, dy: -1e300, rotation: 1e300 }.sane();
        assert_eq!((big.dx, big.dy), (MAX_OFFSET, -MAX_OFFSET));
        assert!(big.rotation.is_finite() && big.rotation.abs() <= 180.0);
        for (prev, cur) in [((f64::NAN, 0.0), (1.0, 1.0)), ((0.0, 0.0), (f64::INFINITY, 1.0)), ((1e300, 1e300), (-1e300, 1e300))] {
            for turn in [false, true] {
                let s = Shift::default().dragged((10.0, 10.0), prev, cur, turn);
                assert!(s.dx.is_finite() && s.dy.is_finite() && s.rotation.is_finite() && s.dx.abs() <= MAX_OFFSET && s.dy.abs() <= MAX_OFFSET, "{s:?}");
            }
        }
        // a singular transform cannot panic
        let _ = Shift { dx: f64::NAN, ..Default::default() }.affine((f64::NAN, f64::INFINITY));
    }

    #[test]
    fn a_shift_belongs_to_its_pose() {
        let pin = OnionPin { clip: 1, frame: 4 };
        let lt = LightTable { pose: Some(pin), shift: Shift { dx: 3.0, dy: 0.0, rotation: 0.0 }, ..Default::default() };
        assert_eq!(lt.shift_of(Some(pin)).dx, 3.0);
        assert_eq!(lt.shift_of(Some(OnionPin { clip: 1, frame: 5 })), Shift::default());
        assert_eq!(lt.shift_of(None), Shift::default());
        assert!(lt.is_shifted() && !LightTable::default().is_shifted());
    }
}
