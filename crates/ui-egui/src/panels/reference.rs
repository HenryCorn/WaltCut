//! Reference images in the drawing workflow (#3): the **Reference** menu of the Brush options bar
//! and the commands behind it (`reference.place`, `reference.set`, `reference.list`).
//!
//! * **Place Image…** asks for a picture (the host's open dialog) and runs `reference.place` with
//!   it; **Paste Image** does the same with the picture on the system clipboard, saved the way
//!   Paste Media saves it. Both are one undo step and leave the playhead and selection alone.
//! * The controls under them act on the target drawing's reference: opacity, Hide, Lock, "Guide
//!   only" (kept out of exports, #65), Scale, Rotation and four Nudge buttons, each a
//!   `reference.set`. **Move with pen** turns the Program monitor's drag into a move of the
//!   reference (the Brush tool does not draw while it is on); one drag is one undo step.
//!
//! Automation ids: `program.brush.reference` (the menu), `program.brush.reference.<place|paste|
//! opacity|hide|lock|guide|scale|rotation|move|nudge.left|right|up|down>`, and
//! `program.brush.reference.list` (the reference's name). `ui.inspect` reports
//! `ui.brush.reference_move` and `references`.

use egui::Ui;
use serde_json::{Value, json};

use crate::WaltCutApp;
use crate::native_dialogs::FileDialog;

/// Pixels one Nudge button moves the reference by (sequence pixels).
const NUDGE: f64 = 10.0;

fn run(app: &mut WaltCutApp, id: &str, params: Value) {
    let r = match route(app, id, &params) {
        Some(r) => r,
        None => app.session.execute(id, params).map_err(|e| e.to_string()),
    };
    if let Err(e) = r {
        app.ui.status = e;
    }
}

/// `reference.place` without a picture asks for one; `reference.pasteImage` places the picture on
/// the clipboard. Everything else is the engine's (None).
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    match id {
        "reference.place" if params.get("path").is_none() && params.get("item").is_none() => {
            let exts: Vec<&str> = waltcut_media::STILL_EXTENSIONS.to_vec();
            let mut rest = params.clone();
            if !rest.is_object() {
                rest = json!({});
            }
            Some(app.pick(FileDialog::open_file("Reference image", &exts), move |app, paths| {
                let Some(path) = paths.into_iter().next() else { return Ok(Value::Null) };
                rest["path"] = json!(path);
                app.session.execute("reference.place", rest).map_err(|e| e.to_string())
            }))
        }
        "reference.pasteImage" => Some(paste(app)),
        _ => None,
    }
}

fn paste(app: &mut WaltCutApp) -> Result<Value, String> {
    let media = app.hooks.clipboard_media.as_mut().and_then(|f| f()).ok_or("there is no picture on the clipboard")?;
    let path = match media {
        crate::ClipboardMedia::Files(paths) => paths.into_iter().next().ok_or("there is no picture on the clipboard")?,
        crate::ClipboardMedia::Image { width, height, rgba } => {
            let png = crate::menus::encode_png(width, height, rgba)?;
            waltcut_engine::paste_media::save_pasted_image(&mut app.session, &png).map_err(|e| e.to_string())?
        }
    };
    app.session.execute("reference.place", json!({"path": path})).map_err(|e| e.to_string())
}

fn f(v: &Value, k: &str, default: f64) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(default)
}

/// The Reference menu of the Brush options bar.
pub fn menu(app: &mut WaltCutApp, ui: &mut Ui) {
    let refs = waltcut_engine::reference::current(&app.session);
    // stays open while the controls are used (a nudge, a slider, a checkbox); a click outside closes it
    let config = egui::containers::menu::MenuConfig::default().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let (button, _) = egui::containers::menu::MenuButton::new("Reference").config(config).ui(ui, |ui| {
        let place = ui.button("Place Image…");
        app.auto.add("program.brush.reference.place", place.rect, "place reference image");
        if place.clicked() {
            run(app, "reference.place", json!({}));
            ui.close();
        }
        let paste = ui.button("Paste Image");
        app.auto.add("program.brush.reference.paste", paste.rect, "paste reference image");
        if paste.clicked() {
            run(app, "reference.pasteImage", json!({}));
            ui.close();
        }
        let Some(first) = refs.first() else { return };
        ui.separator();
        let name = first.get("name").and_then(Value::as_str).unwrap_or("reference");
        let l = ui.label(egui::RichText::new(name).strong());
        app.auto.add("program.brush.reference.list", l.rect, name);
        let clip = first.get("clip").and_then(Value::as_u64).unwrap_or(0);
        let locked = first.get("locked").and_then(Value::as_bool).unwrap_or(false);
        // a slider is one undo step per drag: the first change starts it, the rest join it
        let slide = |app: &mut WaltCutApp, ui: &mut Ui, id: &str, key: &str, value: f64, range: std::ops::RangeInclusive<f64>, text: &str| {
            let mut v = value;
            let resp = ui.add_enabled(!locked, egui::Slider::new(&mut v, range).text(text));
            app.auto.add(&format!("program.brush.reference.{id}"), resp.rect, text);
            if resp.changed() {
                let merge = if resp.drag_started() { json!("start") } else { json!(resp.dragged()) };
                run(app, "reference.set", json!({"clip": clip, key: v, "merge": merge}));
            }
        };
        slide(app, ui, "opacity", "opacity", f(first, "opacity", 50.0), 0.0..=100.0, "Opacity %");
        slide(app, ui, "scale", "scale", f(first, "scale", 100.0), 1.0..=400.0, "Scale %");
        slide(app, ui, "rotation", "rotation", f(first, "rotation", 0.0), -180.0..=180.0, "Rotation °");
        let toggle = |app: &mut WaltCutApp, ui: &mut Ui, id: &str, key: &str, now: bool, text: &str, enabled: bool| {
            let mut v = now;
            let resp = ui.add_enabled(enabled, egui::Checkbox::new(&mut v, text));
            app.auto.add(&format!("program.brush.reference.{id}"), resp.rect, text);
            if resp.changed() {
                run(app, "reference.set", json!({"clip": clip, key: v}));
            }
        };
        toggle(app, ui, "hide", "hidden", first.get("hidden").and_then(Value::as_bool).unwrap_or(false), "Hide", !locked);
        toggle(app, ui, "lock", "locked", locked, "Lock", true);
        toggle(app, ui, "guide", "guide", first.get("guide").and_then(Value::as_bool).unwrap_or(false), "Guide only (not exported)", !locked);
        ui.horizontal(|ui| {
            for (id, label, dx, dy) in [("left", "Left", -NUDGE, 0.0), ("right", "Right", NUDGE, 0.0), ("up", "Up", 0.0, -NUDGE), ("down", "Down", 0.0, NUDGE)]
            {
                let b = ui.add_enabled(!locked, egui::Button::new(label));
                app.auto.add(&format!("program.brush.reference.nudge.{id}"), b.rect, &format!("nudge {id}"));
                if b.clicked() {
                    run(app, "reference.set", json!({"clip": clip, "dx": dx, "dy": dy}));
                }
            }
        });
        let mut moving = app.ui.brush.reference_move;
        let mv = ui.add_enabled(!locked, egui::Checkbox::new(&mut moving, "Move with pen (drag the picture)"));
        app.auto.add("program.brush.reference.move", mv.rect, "move reference with pen");
        if mv.changed() {
            app.ui.brush.reference_move = moving;
        }
    });
    app.auto.add("program.brush.reference", button.rect, "reference image");
}

/// Whether a drag on the picture moves the reference now (Move with pen is on and the target
/// drawing has an unlocked reference).
pub fn moving(app: &WaltCutApp) -> bool {
    app.ui.brush.reference_move
        && waltcut_engine::reference::current(&app.session).first().is_some_and(|r| r.get("locked").and_then(Value::as_bool) != Some(true))
}
