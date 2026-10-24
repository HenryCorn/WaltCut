//! Destination, retained output format, exact range and one cancellable export job.

use egui::{Rect, pos2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use waltcut_engine::export::Format;

use crate::WaltCutApp;
use crate::native_dialogs::FileDialog;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportUi {
    pub path: String,
    pub format: Format,
    pub range: String,
    pub start_frame: i64,
    pub end_frame: i64,
    #[serde(skip)]
    pub for_sequence: Option<u64>,
    pub quick_open: bool,
    /// Draw guide clips in this export (#65). Asked for each time: neither saved nor loaded.
    #[serde(skip)]
    pub include_guides: bool,
    #[serde(skip)]
    replace_path: Option<String>,
}

impl Default for ExportUi {
    fn default() -> Self {
        Self {
            path: String::new(),
            format: Format::H264,
            range: "inOut".into(),
            start_frame: 0,
            end_frame: 24,
            for_sequence: None,
            quick_open: false,
            include_guides: false,
            replace_path: None,
        }
    }
}

impl ExportUi {
    pub fn validate(&self) -> Result<(), String> {
        if self.path.len() > 4096 || self.path.contains('\0') {
            return Err("export path must be at most 4096 bytes and contain no NUL".into());
        }
        if !["entire", "inOut", "workArea", "custom"].contains(&self.range.as_str()) {
            return Err("export range must be entire, inOut, workArea or custom".into());
        }
        if self.start_frame < 0 || self.end_frame > 1_000_000_000 || self.end_frame <= self.start_frame {
            return Err("export frames must increase within 0..1000000000".into());
        }
        if self.range == "custom" && self.end_frame.checked_sub(self.start_frame).is_none_or(|n| n > 1_000_000) {
            return Err("custom export must contain at most 1000000 frames".into());
        }
        Ok(())
    }
}

pub fn export_params(app: &WaltCutApp) -> Value {
    let ex = &app.ui.export;
    let mut p = json!({"path": ex.path, "format": ex.format.id(), "range": ex.range});
    if ex.range == "custom" {
        p["startFrame"] = json!(ex.start_frame);
        p["endFrame"] = json!(ex.end_frame);
    }
    if ex.include_guides {
        p["includeGuides"] = json!(true);
    }
    p
}

fn sync_sequence(app: &mut WaltCutApp) {
    let Some(seq) = app.session.state.active_sequence else { return };
    if app.ui.export.for_sequence != Some(seq.0) {
        app.ui.export.for_sequence = Some(seq.0);
        app.ui.export.replace_path = None;
    }
    if app.ui.export.path.is_empty() {
        let directory = waltcut_engine::export_tools::default_export_dir(&app.session);
        app.ui.export.path = directory.join(format!("Export.{}", app.ui.export.format.extension())).to_string_lossy().into_owned();
    }
}

fn controls(app: &mut WaltCutApp, ui: &mut egui::Ui, quick: bool) {
    sync_sequence(app);
    if app.session.state.active_sequence.is_none() {
        ui.label("Open a sequence to export.");
        return;
    }
    let prefix = if quick { "quickExport" } else { "export" };
    let mut ex = app.ui.export.clone();
    let before = (ex.path.clone(), ex.format, ex.range.clone(), ex.start_frame, ex.end_frame);
    ui.label("Destination");
    let response = ui.add(egui::TextEdit::singleline(&mut ex.path).desired_width(440.0));
    app.auto.add(&format!("{prefix}.path"), response.rect, "Destination");
    let browse = ui.button("Browse…");
    app.auto.add(&format!("{prefix}.browse"), browse.rect, "Choose destination");
    ui.add_space(8.0);
    ui.label("Format");
    let old_format = ex.format;
    let response = egui::ComboBox::from_id_salt(format!("{prefix}.format")).selected_text(ex.format.label()).show_ui(ui, |ui| {
        for format in Format::ALL {
            let response = ui.selectable_value(&mut ex.format, format, format.label());
            app.auto.add(&format!("{prefix}.format.{}", format.id()), response.rect, format.label());
        }
    });
    app.auto.add(&format!("{prefix}.format"), response.response.rect, "Output format");
    if ex.format != old_format {
        let mut path = std::path::PathBuf::from(&ex.path);
        path.set_extension(ex.format.extension());
        ex.path = path.to_string_lossy().into_owned();
    }
    ui.add_space(8.0);
    ui.label("Range");
    let response = egui::ComboBox::from_id_salt(format!("{prefix}.range")).selected_text(&ex.range).show_ui(ui, |ui| {
        for (mode, label) in
            [("entire", "Entire sequence"), ("inOut", "Sequence In/Out"), ("workArea", "Work area"), ("custom", "Custom frames (end exclusive)")]
        {
            let response = ui.selectable_value(&mut ex.range, mode.into(), label);
            app.auto.add(&format!("{prefix}.range.{mode}"), response.rect, label);
        }
    });
    app.auto.add(&format!("{prefix}.range"), response.response.rect, "Range");
    if ex.range == "custom" {
        ui.horizontal(|ui| {
            ui.label("Start frame");
            let response = ui.add(egui::DragValue::new(&mut ex.start_frame).range(0..=1_000_000_000));
            app.auto.add(&format!("{prefix}.range.start"), response.rect, "Start frame");
            ui.label("End frame");
            let response = ui.add(egui::DragValue::new(&mut ex.end_frame).range(1..=1_000_000_000));
            app.auto.add(&format!("{prefix}.range.end"), response.rect, "End frame (exclusive)");
        });
    }
    if before != (ex.path.clone(), ex.format, ex.range.clone(), ex.start_frame, ex.end_frame) {
        ex.replace_path = None;
    }
    let response =
        ui.checkbox(&mut ex.include_guides, "Include guide clips").on_hover_text("Guide clips (Clip ▸ Use as Guide) are left out of exports unless this is on");
    app.auto.add(&format!("{prefix}.includeGuides"), response.rect, "Include guide clips");
    let active =
        app.session.jobs.iter().find(|j| app.session.export_offline.contains_key(&j.id) && j.result.lock().unwrap_or_else(|e| e.into_inner()).is_none());
    let cancel = if let Some(job) = active {
        let response = ui.add(egui::ProgressBar::new(job.progress.fraction()).show_percentage());
        app.auto.add(&format!("{prefix}.progress"), response.rect, "Export progress");
        let response = ui.label(job.progress.status.lock().unwrap_or_else(|e| e.into_inner()).clone());
        app.auto.add(&format!("{prefix}.progress.status"), response.rect, "Export status");
        let response = ui.button("Cancel export");
        app.auto.add(&format!("{prefix}.cancel"), response.rect, "Cancel export");
        response.clicked().then_some(job.id)
    } else {
        None
    };
    let busy = active.is_some();
    let mut start = false;
    let mut overwrite = false;
    if ex.replace_path.as_deref() == Some(ex.path.as_str()) {
        ui.label("The destination already exists. Replace it only after the export succeeds?");
        ui.horizontal(|ui| {
            let response = ui.add_enabled(!busy, egui::Button::new("Replace"));
            app.auto.add(&format!("{prefix}.replace"), response.rect, "Replace existing movie");
            start = response.clicked();
            overwrite = start;
            let response = ui.button("Keep existing file");
            app.auto.add(&format!("{prefix}.keep"), response.rect, "Keep existing file");
            if response.clicked() {
                ex.replace_path = None;
            }
        });
    } else {
        let response = ui.add_enabled(!busy && !ex.path.trim().is_empty() && ex.validate().is_ok(), egui::Button::new("Export"));
        app.auto.add(&format!("{prefix}.button"), response.rect, "Start export");
        start = response.clicked();
        if start && !ex.format.is_image_sequence() && std::path::Path::new(&ex.path).exists() {
            ex.replace_path = Some(ex.path.clone());
            start = false;
        }
    }
    if let Some(job) = app.session.jobs.iter().rev().find(|j| app.session.export_offline.contains_key(&j.id))
        && let Some(result) = job.result.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
    {
        let message = match result {
            Ok(report) => {
                let offline = app.session.export_offline.get(&job.id).map_or(0, |items| items.lock().unwrap_or_else(|e| e.into_inner()).len());
                let warning = if offline == 0 { String::new() } else { format!("; warning: {offline} offline item(s) rendered as placeholders") };
                format!("Exported {} frames to {}{warning}", report.frames, report.path)
            }
            Err(error) => format!("Export failed: {error}"),
        };
        let response = ui.label(&message);
        app.auto.add(&format!("{prefix}.status"), response.rect, &message);
    }
    app.ui.export = ex;
    if browse.clicked() {
        let format = app.ui.export.format;
        let sequence = app.ui.export.for_sequence;
        app.pick_ui(FileDialog::save_as(format.label(), &[format.extension()], &app.ui.export.path), move |app, paths| {
            if app.ui.export.for_sequence == sequence
                && (!quick || app.ui.export.quick_open)
                && let Some(path) = paths.into_iter().next()
            {
                app.ui.export.path = path;
                app.ui.export.replace_path = None;
            }
        });
    }
    if let Some(job) = cancel
        && let Err(error) = app.session.execute("jobs.cancel", json!({"job": job}))
    {
        app.ui.status = error.to_string();
    }
    if start {
        let mut params = export_params(app);
        params["overwrite"] = json!(overwrite);
        match app.session.execute(if quick { "export.quick" } else { "file.exportMedia" }, params) {
            Ok(_) => {
                app.ui.export.replace_path = None;
                app.ui.status = "Export started; use Cancel to stop it.".into();
            }
            Err(error) => app.ui.status = error.to_string(),
        }
    }
}

pub fn show(app: &mut WaltCutApp, ui: &mut egui::Ui, rect: Rect) {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(20.0)), |ui| {
        ui.heading("Export");
        controls(app, ui, false);
    });
}

pub fn quick_export(app: &mut WaltCutApp, ctx: &egui::Context, position: egui::Pos2) {
    if !app.ui.export.quick_open {
        return;
    }
    let mut close = false;
    egui::Window::new("Quick Export")
        .id(egui::Id::new("quick-export"))
        .default_pos(pos2(position.x.max(0.0), position.y))
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            controls(app, ui, true);
            let response = ui.button("Close");
            app.auto.add("quickExport.close", response.rect, "Close Quick Export");
            close = response.clicked();
        });
    if close || crate::widgets::escape_closes(ctx) {
        app.ui.export.quick_open = false;
    }
}
