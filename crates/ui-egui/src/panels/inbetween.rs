//! In-between assist (#72) in the interface: a window that lists which stroke of the cel shown
//! becomes which stroke of the next cel, previews the result over the picture and accepts it.
//!
//! Everything it does is a command (`drawing.inbetweenPreview`, `drawing.inbetweenAccept`,
//! `drawing.inbetweenCancel`, see `docs/animation.md`): the window only collects the pairs and
//! shows what the command answered. It never pairs anything by itself; **Pair in order** fills the
//! list with 1-to-1 rows that you then see and can change, and the preview says what it did.
//!
//! The table and the last answer are UI state (`ui.inspect` → `ui.brush.inbetween`), never saved
//! or loaded back. The preview is tinted and drawn over the picture only: it is not in the
//! project, the history, a still or a movie.
//!
//! Automation ids: `program.brush.inbetween.open` (options bar), then in the window `.add`,
//! `.inOrder`, `.clear`, `.step`, `.easing`, `.unmatched`, `.preview`, `.accept`, `.cancel`,
//! `.prev`, `.next`, `.status`, and per row `.row.<n>.from`, `.to`, `.reverse`, `.remove`.

use egui::{Color32, Pos2, Stroke as EStroke, Ui};
use serde::Serialize;
use serde_json::{Value, json};
use waltcut_geom::Vec2;
use waltcut_project::drawing::Drawing;
use waltcut_project::inbetween::{Easing, MAX_PAIRS, MAX_STEP};

use super::drawing::Mapping;
use crate::WaltCutApp;

/// The colour of the preview: not a colour a drawing is likely to use, so it is never mistaken for
/// the artwork.
const PREVIEW_TINT: Color32 = Color32::from_rgb(214, 70, 230);

/// One row of the pairing table: stroke `from` of the start cel becomes stroke `to` of the end cel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    pub from: usize,
    pub to: usize,
    pub reverse: bool,
}

/// The assist's UI state. Never saved or loaded back.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Assist {
    pub open: bool,
    pub rows: Vec<Row>,
    /// Spacing in drawing frames.
    pub step: u32,
    /// An [`Easing`] name.
    pub easing: String,
    /// Keep unpaired strokes from the nearer cel (default: leave them out).
    pub keep_unpaired: bool,
    /// Which of the previewed frames is drawn over the picture (an index into the frames).
    pub shown: usize,
    /// The parameters the preview was made with: Accept sends exactly these.
    pub previewed: Option<Value>,
    /// The last answer of preview or accept (the lines under the buttons come from it).
    pub report: Option<Value>,
    /// A refusal, if the last command was refused.
    pub refusal: Option<String>,
}

impl Default for Assist {
    fn default() -> Self {
        Assist { open: false, rows: Vec::new(), step: 2, easing: "linear".into(), keep_unpaired: false, shown: 0, previewed: None, report: None, refusal: None }
    }
}

impl Assist {
    /// The command parameters the table stands for (the cels are given by `from` / `to` frames).
    fn params(&self, from: i64, to: i64) -> Value {
        let pairs: Vec<Value> = self.rows.iter().take(MAX_PAIRS).map(|r| json!({"from": r.from, "to": r.to, "reverse": r.reverse})).collect();
        json!({
            "from": from,
            "to": to,
            "pairs": pairs,
            "step": i64::from(self.step).clamp(1, MAX_STEP),
            "easing": Easing::parse(&self.easing).unwrap_or_default().name(),
            "unmatched": if self.keep_unpaired { "nearest" } else { "drop" },
        })
    }
}

/// The frames of the cel shown and of the next cel, if there is one.
fn cels(d: &Drawing, f: i64) -> Option<(i64, i64, usize, usize)> {
    let i = d.cel_index_at(f)?;
    let (a, b) = (d.cels.get(i)?, d.cels.get(i + 1)?);
    Some((a.frame, b.frame, a.strokes.len(), b.strokes.len()))
}

fn cancel(app: &mut WaltCutApp) {
    let _ = app.session.execute("drawing.inbetweenCancel", json!({}));
    let a = &mut app.ui.brush.inbetween;
    a.previewed = None;
    a.report = None;
    a.refusal = None;
    a.shown = 0;
}

fn preview(app: &mut WaltCutApp, params: Value) {
    let r = app.session.execute("drawing.inbetweenPreview", params.clone());
    let a = &mut app.ui.brush.inbetween;
    a.shown = 0;
    match r {
        Ok(report) => {
            a.previewed = (report["status"] == "ok").then_some(params);
            a.report = Some(report);
            a.refusal = None;
        }
        Err(e) => {
            a.previewed = None;
            a.report = None;
            a.refusal = Some(e.to_string());
        }
    }
}

fn accept(app: &mut WaltCutApp) {
    // exactly what was previewed, as explicit parameters
    let Some(params) = app.ui.brush.inbetween.previewed.clone() else { return };
    match app.session.execute("drawing.inbetweenAccept", params) {
        Ok(report) => {
            let n = report["cels"].as_array().map_or(0, Vec::len);
            app.ui.status = format!("Made {n} in-between cel{}", if n == 1 { "" } else { "s" });
            let a = &mut app.ui.brush.inbetween;
            a.rows.clear();
            a.previewed = None;
            a.report = None;
            a.refusal = None;
            a.shown = 0;
        }
        Err(e) => app.ui.brush.inbetween.refusal = Some(e.to_string()),
    }
}

/// What the last answer says, one line each: `(warning, text)`.
fn lines(report: &Value) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let list = |v: &Value| v.as_array().map(|a| a.iter().filter_map(Value::as_i64).map(|n| n.to_string()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    if report["status"] != "ok" {
        out.push((true, "Not supported, nothing was made:".into()));
        for p in report["problems"].as_array().into_iter().flatten() {
            out.push((true, format!("• {}", p["message"].as_str().unwrap_or("?"))));
        }
        return out;
    }
    let frames = list(&report["frames"]);
    out.push((false, format!("Would make cels on frames {frames}")));
    for s in report["skipped"].as_array().into_iter().flatten() {
        out.push((true, format!("Frame {} skipped: {}", s["frame"], s["reason"].as_str().unwrap_or("?"))));
    }
    for (side, name) in [("from", "start"), ("to", "end")] {
        let u = list(&report["unmatched"][side]);
        if !u.is_empty() {
            let what = if report["unmatched"]["policy"] == "nearest" { "kept from the nearer cel" } else { "left out" };
            out.push((true, format!("Unpaired strokes of the {name} cel, {what}: {u}")));
        }
        let f = list(&report["fills"][side]);
        if !f.is_empty() {
            out.push((false, format!("Fills of the {name} cel ({f}) are not morphed: the nearer cel's fill is shown")));
        }
    }
    for p in report["pairs"].as_array().into_iter().flatten() {
        if p["otherDirectionIsCloser"] == true {
            let dir = if p["reverse"] == true { "not reversed" } else { "reversed" };
            out.push((true, format!("Pair {} → {}: the ends line up better {dir} (not changed)", p["from"], p["to"])));
        }
    }
    out
}

fn status_of(app: &WaltCutApp) -> Option<&Value> {
    app.ui.brush.inbetween.report.as_ref()
}

/// The options-bar button that opens the window.
pub fn controls(app: &mut WaltCutApp, ui: &mut Ui) {
    let open = app.ui.brush.inbetween.open;
    let r = ui.selectable_label(open, "Assist…").on_hover_text("Choose which stroke becomes which, preview the in-betweens, then accept them");
    app.auto.add("program.brush.inbetween.open", r.rect, "in-between assist");
    if r.clicked() {
        if open {
            cancel(app);
        }
        app.ui.brush.inbetween.open = !open;
    }
}

/// The window. `f` is the drawing frame under the playhead.
pub fn window(app: &mut WaltCutApp, ctx: &egui::Context, clip: u64, d: &Drawing, f: i64) {
    // another drawing clip, or cels that changed under the preview, end it
    app.session.prune_inbetween();
    if app.session.inbetween.as_ref().is_some_and(|p| p.clip.0 != clip) {
        cancel(app);
    }
    if app.ui.brush.inbetween.previewed.is_some() && app.session.inbetween_preview().is_none() {
        cancel(app);
        app.ui.brush.inbetween.refusal = Some("The preview ended because a cel it used changed.".into());
    }
    if !app.ui.brush.inbetween.open {
        return;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        if app.ui.brush.inbetween.previewed.is_some() {
            cancel(app);
        } else {
            app.ui.brush.inbetween.open = false;
        }
        return;
    }
    let mut open = true;
    egui::Window::new("In-between assist").id(egui::Id::new("inbetween-assist")).open(&mut open).resizable(false).default_width(360.0).show(ctx, |ui| {
        body(app, ui, d, f);
    });
    if !open {
        cancel(app);
        app.ui.brush.inbetween.open = false;
    }
}

fn body(app: &mut WaltCutApp, ui: &mut Ui, d: &Drawing, f: i64) {
    let Some((from, to, na, nb)) = cels(d, f) else {
        ui.label("Put the playhead on a cel that has a later cel to move towards.");
        return;
    };
    ui.label(format!("Start cel: frame {from} ({na} strokes) → end cel: frame {to} ({nb} strokes)"));
    ui.label(egui::RichText::new("Stroke numbers count from 0 in the order they were drawn.").small());
    let mut changed = false;
    let mut remove = None;
    let rows = app.ui.brush.inbetween.rows.clone();
    let mut edited = rows.clone();
    for (n, row) in edited.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let r = ui.add(egui::DragValue::new(&mut row.from).range(0..=na.saturating_sub(1)).prefix("start "));
            app.auto.add(&format!("program.brush.inbetween.row.{n}.from"), r.rect, "start stroke");
            changed |= r.changed();
            ui.label("→");
            let r = ui.add(egui::DragValue::new(&mut row.to).range(0..=nb.saturating_sub(1)).prefix("end "));
            app.auto.add(&format!("program.brush.inbetween.row.{n}.to"), r.rect, "end stroke");
            changed |= r.changed();
            let r = ui.checkbox(&mut row.reverse, "Reverse");
            app.auto.add(&format!("program.brush.inbetween.row.{n}.reverse"), r.rect, "reverse end stroke");
            changed |= r.changed();
            let r = ui.small_button("Remove").on_hover_text("Remove this pair");
            app.auto.add(&format!("program.brush.inbetween.row.{n}.remove"), r.rect, "remove pair");
            if r.clicked() {
                remove = Some(n);
            }
        });
    }
    if let Some(n) = remove {
        edited.remove(n);
        changed = true;
    }
    ui.horizontal(|ui| {
        let r = ui.add_enabled(edited.len() < MAX_PAIRS && na > 0 && nb > 0, egui::Button::new("Add pair"));
        app.auto.add("program.brush.inbetween.add", r.rect, "add pair");
        if r.clicked() {
            edited.push(Row { from: 0, to: 0, reverse: false });
            changed = true;
        }
        let r = ui
            .add_enabled(na.min(nb) > 0, egui::Button::new("Pair in order"))
            .on_hover_text("Fill the list with start stroke n → end stroke n; you can change any row before previewing");
        app.auto.add("program.brush.inbetween.inOrder", r.rect, "pair in order");
        if r.clicked() {
            edited = (0..na.min(nb).min(MAX_PAIRS)).map(|i| Row { from: i, to: i, reverse: false }).collect();
            changed = true;
        }
        let r = ui.add_enabled(!edited.is_empty(), egui::Button::new("Clear"));
        app.auto.add("program.brush.inbetween.clear", r.rect, "clear pairs");
        if r.clicked() {
            edited.clear();
            changed = true;
        }
    });
    let a = &mut app.ui.brush.inbetween;
    if changed {
        a.rows = edited;
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Every");
        let mut step = a.step;
        let r = ui.add(egui::DragValue::new(&mut step).range(1..=MAX_STEP as u32).suffix(" frames"));
        app.auto.add("program.brush.inbetween.step", r.rect, "spacing");
        a.step = step;
        let cur = Easing::parse(&a.easing).unwrap_or_default();
        let combo = egui::ComboBox::from_id_salt("inbetween-easing").selected_text(cur.name()).show_ui(ui, |ui| {
            for e in Easing::ALL {
                if ui.selectable_label(cur == e, e.name()).clicked() {
                    a.easing = e.name().into();
                }
            }
        });
        app.auto.add("program.brush.inbetween.easing", combo.response.rect, "easing");
    });
    let r = ui.checkbox(&mut a.keep_unpaired, "Keep unpaired strokes from the nearer cel");
    app.auto.add("program.brush.inbetween.unmatched", r.rect, "keep unpaired strokes");
    ui.separator();
    let current = a.params(from, to);
    let stale = a.previewed.as_ref().is_some_and(|p| *p != current);
    let has_rows = !a.rows.is_empty();
    ui.horizontal(|ui| {
        let r = ui.add_enabled(has_rows, egui::Button::new("Preview"));
        app.auto.add("program.brush.inbetween.preview", r.rect, "preview in-betweens");
        if r.clicked() {
            preview(app, current.clone());
        }
        let can = app.ui.brush.inbetween.previewed.is_some() && !stale;
        let n = app.session.inbetween_preview().map_or(0, |p| p.frames.len());
        let r = ui.add_enabled(can, egui::Button::new(format!("Accept {n} cel{}", if n == 1 { "" } else { "s" })));
        app.auto.add("program.brush.inbetween.accept", r.rect, "accept in-betweens");
        if r.clicked() {
            accept(app);
        }
        let r = ui.add_enabled(app.ui.brush.inbetween.previewed.is_some(), egui::Button::new("Cancel"));
        app.auto.add("program.brush.inbetween.cancel", r.rect, "cancel preview");
        if r.clicked() {
            cancel(app);
        }
    });
    if stale {
        ui.colored_label(Color32::from_rgb(230, 170, 50), "The pairs changed: preview again before accepting.");
    }
    // which previewed frame is drawn over the picture
    let frames: Vec<i64> = app.session.inbetween_preview().map(|p| p.frames.clone()).unwrap_or_default();
    if !frames.is_empty() {
        let last = frames.len() - 1;
        let a = &mut app.ui.brush.inbetween;
        a.shown = a.shown.min(last);
        ui.horizontal(|ui| {
            let r = ui.add_enabled(a.shown > 0, egui::Button::new("◀"));
            app.auto.add("program.brush.inbetween.prev", r.rect, "previous previewed frame");
            if r.clicked() {
                a.shown -= 1;
            }
            ui.label(format!("Showing frame {} ({} of {})", frames.get(a.shown).copied().unwrap_or(0), a.shown + 1, frames.len()));
            let r = ui.add_enabled(a.shown < last, egui::Button::new("▶"));
            app.auto.add("program.brush.inbetween.next", r.rect, "next previewed frame");
            if r.clicked() {
                a.shown += 1;
            }
        });
    }
    let text = match (&app.ui.brush.inbetween.refusal, status_of(app)) {
        (Some(e), _) => vec![(true, e.clone())],
        (None, Some(r)) => lines(r),
        _ => Vec::new(),
    };
    let sr = ui.vertical(|ui| {
        for (warn, t) in &text {
            let rt = egui::RichText::new(t).small();
            ui.label(if *warn { rt.color(Color32::from_rgb(230, 170, 50)) } else { rt });
        }
    });
    app.auto.add("program.brush.inbetween.status", sr.response.rect, "in-between status");
}

/// Draw the shown preview frame over the picture, tinted, with a note. Not part of any export.
pub(super) fn paint_overlay(app: &WaltCutApp, painter: &egui::Painter, map: &Mapping, note_at: Pos2) {
    let Some(pv) = app.session.inbetween_preview() else { return };
    let a = &app.ui.brush.inbetween;
    let Some(&frame) = pv.frames.get(a.shown.min(pv.frames.len().saturating_sub(1))) else { return };
    for s in pv.strokes_at(frame) {
        if s.is_fill() {
            continue;
        }
        let pts: Vec<Pos2> = s
            .points
            .iter()
            .map(|p| {
                let q = map.to_screen.apply(Vec2::new(f64::from(p.x), f64::from(p.y)));
                Pos2::new(q.x as f32, q.y as f32)
            })
            .collect();
        let width = (s.size * map.scale).clamp(1.0, 200.0);
        match pts.as_slice() {
            [] => {}
            [p] => {
                painter.circle_filled(*p, width * 0.5, PREVIEW_TINT);
            }
            _ => {
                painter.add(egui::Shape::line(pts, EStroke::new(width, PREVIEW_TINT.gamma_multiply(0.8))));
            }
        }
    }
    painter.text(
        note_at,
        egui::Align2::LEFT_TOP,
        format!("In-between preview · frame {frame} · not applied"),
        egui::FontId::proportional(12.0),
        PREVIEW_TINT.gamma_multiply(1.3),
    );
}
