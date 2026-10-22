//! Responsive layout controls and the Replace Fonts dialog for graphic clips.

use crate::widgets::CenteredDialog as _;
use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use waltcut_project::graphic::{layer_display_name, layer_indices};
use waltcut_project::graphic_design::layer_uid;
use waltcut_project::{ClipId, RollMode, TrackItem};
use waltcut_time::Tick;

use crate::FilmcraftApp;
use crate::theme::Tokens;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplaceDraft {
    pub from: String,
    pub to: String,
    pub error: String,
}

type Elems = Vec<(String, Rect, String)>;

fn row_label(ui: &mut egui::Ui, label: &str, t: &Tokens) -> egui::Ui {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    ui.painter().text(pos2(r.min.x + 18.0, r.center().y), Align2::LEFT_CENTER, label, Tokens::ui(12.0), t.text_dim);
    ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(r.min.x + 130.0, r.min.y + 3.0), pos2(r.max.x - 6.0, r.max.y - 3.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    )
}

/// Responsive Design – Time (intro / outro and roll / crawl), shown with no layer selected.
pub fn responsive_time(app: &FilmcraftApp, ui: &mut egui::Ui, clip: ClipId, it: &TrackItem, autos: &mut Elems, actions: &mut Vec<(String, Value)>) {
    let t = app.tokens;
    let rate = app.session.sequence_rate();
    let m = it.graphic.as_deref().cloned().unwrap_or_default();
    let frames = |x: Tick| rate.frame_at(rate.snap_nearest(x)) as f64;
    for (key, label, v) in [("intro", "Intro Duration", m.intro), ("outro", "Outro Duration", m.outro)] {
        let mut vui = row_label(ui, label, &t);
        let mut f = frames(v);
        let r = vui.add(crate::widgets::number_drag(&mut f).speed(0.2).range(0.0..=100_000.0).suffix(" fr"));
        autos.push((format!("graphics.time.{key}"), r.rect, label.into()));
        if r.changed() {
            actions.push(("graphics.setResponsiveTime".into(), json!({"clip": clip.0, format!("{key}Frames"): f.round()})));
        }
    }
    let mut vui = row_label(ui, "Roll", &t);
    let mut mode = m.roll.mode;
    let r = egui::ComboBox::from_id_salt(("gfx-roll", clip.0)).selected_text(mode.label()).width(120.0).show_ui(&mut vui, |ui| {
        for x in RollMode::ALL {
            ui.selectable_value(&mut mode, x, x.label());
        }
    });
    autos.push(("graphics.roll.mode".into(), r.response.rect, "Roll".into()));
    if mode != m.roll.mode {
        let name = match mode {
            RollMode::Off => "off",
            RollMode::Roll => "roll",
            RollMode::CrawlLeft => "crawlLeft",
            RollMode::CrawlRight => "crawlRight",
        };
        actions.push(("graphics.setRoll".into(), json!({"clip": clip.0, "mode": name})));
    }
    if m.roll.mode == RollMode::Off {
        return;
    }
    let mut vui = row_label(ui, "", &t);
    for (key, label, v) in [("startOffScreen", "Start Off Screen", m.roll.start_off_screen), ("endOffScreen", "End Off Screen", m.roll.end_off_screen)] {
        let mut b = v;
        let r = vui.checkbox(&mut b, label);
        autos.push((format!("graphics.roll.{key}"), r.rect, label.into()));
        if r.changed() {
            actions.push(("graphics.setRoll".into(), json!({"clip": clip.0, key: b})));
        }
    }
    for (key, label, v) in [
        ("preroll", "Preroll", m.roll.preroll),
        ("easeIn", "Ease In", m.roll.ease_in),
        ("easeOut", "Ease Out", m.roll.ease_out),
        ("postroll", "Postroll", m.roll.postroll),
    ] {
        let mut vui = row_label(ui, label, &t);
        let mut f = frames(v);
        let r = vui.add(crate::widgets::number_drag(&mut f).speed(0.2).range(0.0..=100_000.0).suffix(" fr"));
        autos.push((format!("graphics.roll.{key}"), r.rect, label.into()));
        if r.changed() {
            actions.push(("graphics.setRoll".into(), json!({"clip": clip.0, format!("{key}Frames"): f.round()})));
        }
    }
}

/// Responsive Design – Position for the selected layer: Pin To and the pinned edges.
pub fn responsive_position(
    app: &FilmcraftApp,
    ui: &mut egui::Ui,
    clip: ClipId,
    it: &TrackItem,
    layer: usize,
    autos: &mut Elems,
    actions: &mut Vec<(String, Value)>,
) {
    let t = app.tokens;
    let idx = layer_indices(&it.effects);
    let Some(&ei) = idx.get(layer) else { return };
    let pin = it.effects[ei].layer.as_ref().and_then(|x| x.pin.clone());
    let target_name = match pin.as_ref().map(|p| p.to) {
        None => "None".to_string(),
        Some(waltcut_project::PinTarget::Frame) => "Video Frame".to_string(),
        Some(waltcut_project::PinTarget::Layer(uid)) => {
            (0..idx.len()).find(|&i| layer_uid(&it.effects[idx[i]]) == uid).map(|i| layer_display_name(&it.effects[idx[i]], i)).unwrap_or_default()
        }
    };
    let mut vui = row_label(ui, "Pin To", &t);
    let r = egui::ComboBox::from_id_salt(("gfx-pin", clip.0, layer)).selected_text(&target_name).width(150.0).show_ui(&mut vui, |ui| {
        if ui.selectable_label(pin.is_none(), "None").clicked() {
            actions.push(("graphics.pin".into(), json!({"clip": clip.0, "layer": layer, "to": "none"})));
        }
        let on_frame = matches!(pin.as_ref().map(|p| p.to), Some(waltcut_project::PinTarget::Frame));
        if ui.selectable_label(on_frame, "Video Frame").clicked() {
            actions.push(("graphics.pin".into(), json!({"clip": clip.0, "layer": layer, "to": "frame"})));
        }
        for (i, &e) in idx.iter().enumerate() {
            if i != layer {
                let n = layer_display_name(&it.effects[e], i);
                if ui.selectable_label(n == target_name, &n).clicked() {
                    actions.push(("graphics.pin".into(), json!({"clip": clip.0, "layer": layer, "to": i})));
                }
            }
        }
    });
    autos.push(("graphics.pin.to".into(), r.response.rect, "Pin To".into()));
    let Some(p) = pin else { return };
    let mut vui = row_label(ui, "Pinned Edges", &t);
    let mut edges = [p.left, p.top, p.right, p.bottom];
    let to = match p.to {
        waltcut_project::PinTarget::Frame => json!("frame"),
        waltcut_project::PinTarget::Layer(uid) => {
            json!((0..idx.len()).find(|&i| layer_uid(&it.effects[idx[i]]) == uid).unwrap_or(0))
        }
    };
    let mut changed = false;
    for (k, name) in ["left", "top", "right", "bottom"].iter().enumerate() {
        let r = vui.toggle_value(&mut edges[k], name[..1].to_ascii_uppercase());
        autos.push((format!("graphics.pin.{name}"), r.rect, format!("Pin {name} edge")));
        changed |= r.changed();
    }
    if changed {
        let list: Vec<&str> = ["left", "top", "right", "bottom"].iter().zip(edges).filter(|(_, on)| *on).map(|(n, _)| *n).collect();
        if list.is_empty() {
            actions.push(("graphics.pin".into(), json!({"clip": clip.0, "layer": layer, "to": "none"})));
        } else {
            actions.push(("graphics.pin".into(), json!({"clip": clip.0, "layer": layer, "to": to, "edges": list})));
        }
    }
}

/// Menu / shortcut entry points of this module (None for other ids).
pub fn route(app: &mut FilmcraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let empty = params.as_object().is_none_or(|m| m.is_empty());
    match id {
        "file.replaceFonts" if empty => {
            let used = waltcut_engine::graphic_design::fonts_used(&app.session);
            let from = used.first().map(|f| f.0.clone()).unwrap_or_default();
            app.ui.replace_fonts = Some(ReplaceDraft { from, to: waltcut_text::fonts::DEFAULT_FAMILY.into(), error: String::new() });
            Some(Ok(json!({"dialog": "replaceFonts"})))
        }
        _ => None,
    }
}

pub fn replace_dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.replace_fonts.clone() else { return };
    let used = waltcut_engine::graphic_design::fonts_used(&app.session);
    let mut elems: Elems = Vec::new();
    let mut action: Option<bool> = None;
    egui::Window::new("Replace Fonts in Projects")
        .id(egui::Id::new("Replace Fonts in Projects"))
        .collapsible(false)
        .resizable(false)
        .default_width(440.0)
        .centered(ctx)
        .show(ctx, |ui| {
            ui.label("Fonts used in the project:");
            egui::Grid::new("gfx-fonts-used").striped(true).num_columns(3).show(ui, |ui| {
                for (f, st, n) in &used {
                    let missing = waltcut_text::resolve(f, st).missing;
                    ui.label(if missing {
                        egui::RichText::new(tlf!("{f} (missing)", f)).color(Color32::from_rgb(0xff, 0xa0, 0x60))
                    } else {
                        egui::RichText::new(f)
                    });
                    ui.label(st);
                    ui.label(format!("{n}"));
                    ui.end_row();
                }
            });
            ui.separator();
            let mut fams: Vec<String> = used.iter().map(|u| u.0.clone()).collect();
            fams.dedup();
            ui.horizontal(|ui| {
                ui.label("Replace:");
                let r = egui::ComboBox::from_id_salt("gfx-rf-from").selected_text(&d.from).width(160.0).show_ui(ui, |ui| {
                    for f in &fams {
                        ui.selectable_value(&mut d.from, f.clone(), f);
                    }
                });
                elems.push(("replaceFonts.from".into(), r.response.rect, "Replace".into()));
                ui.label("with:");
                let r = egui::ComboBox::from_id_salt("gfx-rf-to").selected_text(&d.to).width(160.0).height(360.0).show_ui(ui, |ui| {
                    for (f, _) in waltcut_text::families() {
                        ui.selectable_value(&mut d.to, f.clone(), &f);
                    }
                });
                elems.push(("replaceFonts.to".into(), r.response.rect, "With".into()));
            });
            if !d.error.is_empty() {
                ui.colored_label(Color32::from_rgb(0xff, 0x80, 0x80), &d.error);
            }
            ui.horizontal(|ui| {
                let ok = ui.button("OK");
                elems.push(("replaceFonts.ok".into(), ok.rect, "OK".into()));
                let cancel = ui.button("Cancel");
                elems.push(("replaceFonts.cancel".into(), cancel.rect, "Cancel".into()));
                if ok.clicked() {
                    action = Some(true);
                }
                if cancel.clicked() {
                    action = Some(false);
                }
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    match action {
        Some(true) => match app.session.execute("file.replaceFonts", json!({"from": d.from, "to": d.to})) {
            Ok(v) => {
                app.ui.replace_fonts = None;
                app.ui.status = tlf!("Replaced {n} font uses", n = v["replaced"]);
            }
            Err(e) => {
                d.error = e.to_string();
                app.ui.replace_fonts = Some(d);
            }
        },
        Some(false) => app.ui.replace_fonts = None,
        None => app.ui.replace_fonts = Some(d),
    }
}
