//! Searchable presets for retained Motion, Opacity and graphic controls.

use egui::{Align2, Rect, Sense, pos2, vec2};

use crate::FilmcraftApp;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let sr = Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(rect.width() - 16.0, 22.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sr));
    let mut q = app.ui.effects_search.clone();
    crate::widgets::search_field(&mut child, &mut q, "Search presets", sr.width(), &t);
    app.ui.effects_search = q.clone();
    let body = Rect::from_min_max(pos2(rect.min.x, sr.max.y + 6.0), rect.max);
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("fx-body"));
    bui.set_clip_rect(body);
    let filter = q.to_lowercase();
    let mut action = None;
    egui::ScrollArea::vertical().id_salt("fx-scroll").auto_shrink([false, false]).show(&mut bui, |ui| {
        let any = app.session.presets.all().iter().any(|p| p.name.to_lowercase().contains(&filter));
        if any && folder_row(app, ui, &filter) {
            action = crate::panels::presets::folder_rows(app, ui, &filter);
        }
    });
    if let Some((cmd, name)) = action {
        crate::panels::presets::run(app, &cmd, &name);
    }
}

pub(crate) fn tree_x(depth: usize) -> f32 {
    8.0 + depth as f32 * 16.0
}

fn folder_row(app: &mut FilmcraftApp, ui: &mut egui::Ui, filter: &str) -> bool {
    let t = app.tokens;
    let key = "Presets";
    let open = !filter.is_empty() || app.ui.expanded_fx.iter().any(|k| k == key);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, t.hover);
    }
    let x = r.min.x + tree_x(0);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(x + 4.0, r.center().y), vec2(10.0, 10.0)),
        if open { Icon::ChevronDown } else { Icon::ChevronRight },
        t.text_dim,
    );
    icons::paint(ui.painter(), Rect::from_center_size(pos2(x + 18.0, r.center().y), vec2(13.0, 13.0)), Icon::Folder, t.text_dim);
    ui.painter().text(pos2(x + 30.0, r.center().y), Align2::LEFT_CENTER, key, Tokens::ui(12.0), t.text);
    app.auto.add("effects.folder.Presets", r, key);
    if resp.clicked() {
        if open {
            app.ui.expanded_fx.retain(|k| k != key);
        } else {
            app.ui.expanded_fx.push(key.to_string());
        }
    }
    open
}
