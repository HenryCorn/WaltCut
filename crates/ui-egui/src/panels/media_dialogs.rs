//! Media management dialogs: Link Media (offline media / relink) and Make Offline. Their state lives
//! in `UiState` (serde), so agents can open and fill them with `ui.set` as well as by clicking; the
//! actions run engine commands (`media.*`).
//!
//! Automation ids:
//! - Link Media: `linkMedia.row.<n>`, `linkMedia.match.<fileName|extension|clipId|duration|mediaStart|metadata>`,
//!   `linkMedia.alignTimecode`, `linkMedia.relinkOthers`, `linkMedia.folder`, `linkMedia.browse`,
//!   `linkMedia.exactName`, `linkMedia.search`, `linkMedia.candidate.<n>`, `linkMedia.preview`,
//!   `linkMedia.link`, `linkMedia.locate`, `linkMedia.offline`, `linkMedia.offlineAll`, `linkMedia.cancel`.
//! - Make Offline: `makeOffline.keep`, `makeOffline.delete`, `makeOffline.ok`, `makeOffline.cancel`.

use egui::{Color32, RichText};
use serde_json::{Value, json};

use crate::native_dialogs::FileDialog;
use crate::state::LinkMediaDraft;
use crate::{RelinkHint, WaltCutApp};

type Elems = Vec<(String, egui::Rect, String)>;

fn push(elems: &mut Elems, id: impl Into<String>, r: &egui::Response, label: impl Into<String>) {
    elems.push((id.into(), r.rect, label.into()));
}

/// Draw whichever media dialogs are open; open Link Media when a project needs it.
pub fn show(app: &mut WaltCutApp, ctx: &egui::Context) {
    if app.session.offline.prompt && app.ui.link_media.is_none() {
        app.ui.link_media = Some(LinkMediaDraft::default());
    }
    if app.ui.link_media.is_some() {
        link_media(app, ctx);
    }
    if app.ui.make_offline.is_some() {
        make_offline(app, ctx);
    }
}

/// Menu / shortcut entry points without params open a dialog. Returns None for other ids.
pub fn route(app: &mut WaltCutApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let empty = params.as_object().is_none_or(|m| m.is_empty());
    if !empty {
        return None;
    }
    let enabled = |app: &WaltCutApp, id: &str| waltcut_engine::find_command(id).map_or(Ok(()), |c| (c.enabled)(&app.session));
    match id {
        "media.linkMedia" => {
            let r = app.session.execute("media.linkMedia", json!({})).map_err(|e| e.to_string());
            if let Ok(v) = &r
                && v["missing"].as_array().is_some_and(|a| a.is_empty())
            {
                app.ui.status = tl!("All media is online.").into();
                return Some(r);
            }
            app.ui.link_media = Some(LinkMediaDraft::default());
            Some(r)
        }
        "media.makeOffline" => Some(enabled(app, id).map(|_| {
            app.ui.make_offline = Some(false);
            json!({"dialog": "makeOffline"})
        })),
        _ => None,
    }
}

fn missing_list(app: &WaltCutApp) -> Vec<(u64, String, String, String, &'static str)> {
    app.session
        .offline
        .missing
        .iter()
        .filter_map(|id| {
            let it = app.session.project.item(*id)?;
            let m = it.as_media()?;
            let waltcut_project::MediaRef::File { path } = &m.media else { return None };
            let fname = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
            Some((id.0, it.name.clone(), fname, path.clone(), if m.offline { tl!("Offline") } else { tl!("Missing") }))
        })
        .collect()
}

fn match_params(d: &LinkMediaDraft) -> Value {
    json!({
        "match": {"fileName": d.file_name, "extension": d.extension, "clipId": d.clip_id, "duration": d.duration, "mediaStart": d.media_start, "metadata": d.metadata},
        "alignTimecode": d.align_timecode,
        "relinkOthers": d.relink_others,
    })
}

fn preview_texture(app: &WaltCutApp, ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    let key = egui::Id::new(("link-preview", path));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(key)) {
        return t;
    }
    let tex = app.session.media.open_file(path, &*app.session.services).ok().and_then(|src| {
        let t = waltcut_time::Tick(src.info().duration.0 / 3);
        let f = src.video_frame(waltcut_media::FrameRequest { time: t, scale: 0.25 }).ok()?;
        let img = egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.to_rgba8());
        Some(ctx.load_texture(format!("link-preview-{path}"), img, egui::TextureOptions::LINEAR))
    });
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    tex
}

fn link_media(app: &mut WaltCutApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.link_media.clone() else { return };
    let rows: Vec<_> = missing_list(app).into_iter().filter(|r| !d.skipped.contains(&r.0)).collect();
    if rows.is_empty() {
        app.ui.link_media = None;
        app.session.offline.prompt = false;
        return;
    }
    d.row = d.row.min(rows.len() - 1);
    let mut elems: Elems = Vec::new();
    let mut keep = true;
    let mut action: Option<&str> = None;
    let accent = app.tokens.accent;
    let sel_path = d.candidate.and_then(|c| d.candidates.get(c)).map(|c| c.0.clone());
    let preview = sel_path.as_deref().and_then(|p| preview_texture(app, ctx, p));
    let rows_max_h = (ctx.content_rect().height() * 0.3).clamp(90.0, 320.0);
    let width = (ctx.content_rect().width() - 40.0).clamp(1.0, 760.0);
    egui::Window::new(tl!("Link Media"))
        .id(egui::Id::new("Link Media"))
        .collapsible(false)
        .resizable(false)
        .default_width(width)
        .max_width(width)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(crate::widgets::dialog_center(ctx))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("link-media-body-scroll").max_height((ctx.content_rect().height() - 140.0).max(1.0)).show(ui, |ui| {
                ui.label(tlf!("{n} clip(s) can't find their media. Locate them, search a folder, or leave them offline.", n = rows.len()));
                ui.add_space(6.0);
                egui::ScrollArea::both().id_salt("link-media-rows-scroll").max_height(rows_max_h).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("link-media-rows").num_columns(4).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                        for h in [tl!("Clip Name"), tl!("File Name"), tl!("File Path"), tl!("Status")] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for (k, (_, name, fname, path, status)) in rows.iter().enumerate() {
                            let r = ui.selectable_label(d.row == k, name);
                            push(&mut elems, format!("linkMedia.row.{k}"), &r, name.clone());
                            if r.clicked() && d.row != k {
                                d.row = k;
                                d.candidates.clear();
                                d.candidate = None;
                            }
                            ui.label(fname);
                            ui.label(RichText::new(path).small());
                            ui.label(RichText::new(*status).color(Color32::from_rgb(0xe0, 0x5a, 0x5a)));
                            ui.end_row();
                        }
                    });
                });
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(tl!("Match file properties:"));
                    for (id, label, v) in [
                        ("fileName", tl!("File Name"), &mut d.file_name),
                        ("extension", tl!("File Extension"), &mut d.extension),
                        ("clipId", tl!("Clip ID (fingerprint)"), &mut d.clip_id),
                        ("duration", tl!("Duration"), &mut d.duration),
                        ("mediaStart", tl!("Media Start"), &mut d.media_start),
                        ("metadata", tl!("Frame Size / Rate"), &mut d.metadata),
                    ] {
                        let r = ui.checkbox(v, label);
                        push(&mut elems, format!("linkMedia.match.{id}"), &r, label);
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let r = ui.checkbox(&mut d.align_timecode, tl!("Align Timecode"));
                    push(&mut elems, "linkMedia.alignTimecode", &r, "Align Timecode");
                    let r = ui.checkbox(&mut d.relink_others, tl!("Relink others automatically"));
                    push(&mut elems, "linkMedia.relinkOthers", &r, "Relink others automatically");
                });
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(tl!("Search in:"));
                    let r = ui.add(egui::TextEdit::singleline(&mut d.folder).desired_width((ui.available_width() * 0.5).min(360.0)).hint_text(tl!("Folder")));
                    push(&mut elems, "linkMedia.folder", &r, "folder");
                    let r = ui.button(tl!("Browse…"));
                    push(&mut elems, "linkMedia.browse", &r, "Browse…");
                    if r.clicked() {
                        action = Some("browse");
                    }
                    let r = ui.checkbox(&mut d.exact_name, tl!("Exact name matches only"));
                    push(&mut elems, "linkMedia.exactName", &r, "Exact name matches only");
                    let r = ui.button(tl!("Search"));
                    push(&mut elems, "linkMedia.search", &r, "Search");
                    if r.clicked() {
                        action = Some("search");
                    }
                });
                ui.horizontal(|ui| {
                    let candidates_width = (ui.available_width() - 192.0 - ui.spacing().item_spacing.x).max(1.0);
                    ui.vertical(|ui| {
                        ui.set_width(candidates_width);
                        if d.candidates.is_empty() {
                            ui.label(RichText::new(tl!("No candidates yet — Search a folder or Locate the file.")).weak());
                        }
                        egui::ScrollArea::vertical().id_salt("link-media-candidates-scroll").max_height(108.0).auto_shrink([false, true]).show(ui, |ui| {
                            for (k, (path, ok, idm, problems)) in d.candidates.iter().enumerate() {
                                let mark = match (ok, idm) {
                                    (true, Some(true)) => tl!("✔ same file"),
                                    (true, _) => tl!("✔ matches"),
                                    (false, Some(false)) => tl!("✖ different file"),
                                    _ => "✖",
                                };
                                let text = format!("{mark}  {path}");
                                let r = ui.selectable_label(
                                    d.candidate == Some(k),
                                    RichText::new(&text).color(if *ok { Color32::LIGHT_GREEN } else { Color32::from_rgb(0xe0, 0x8a, 0x6a) }),
                                );
                                let r = if problems.is_empty() { r } else { r.on_hover_text(problems) };
                                push(&mut elems, format!("linkMedia.candidate.{k}"), &r, text);
                                if r.clicked() {
                                    d.candidate = Some(k);
                                }
                            }
                        });
                    });
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(192.0, 108.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(12, 12, 12));
                    match &preview {
                        Some(t) => {
                            let fitted = crate::panels::monitor::fit(rect, t.size()[0] as f32, t.size()[1] as f32);
                            ui.painter().image(t.id(), fitted, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        None => {
                            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, tl!("Preview"), crate::theme::Tokens::ui(11.0), Color32::GRAY);
                        }
                    }
                    elems.push(("linkMedia.preview".into(), rect, "preview".into()));
                });
                if !d.message.is_empty() {
                    ui.colored_label(Color32::from_rgb(0xe0, 0x8a, 0x6a), &d.message);
                }
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for (id, label) in [("offlineAll", tl!("Offline All")), ("offline", tl!("Offline")), ("cancel", tl!("Cancel")), ("locate", tl!("Locate…"))] {
                    let r = ui.button(label);
                    push(&mut elems, format!("linkMedia.{id}"), &r, label);
                    if r.clicked() {
                        action = Some(id);
                    }
                }
                let r = ui.add_enabled(d.candidate.is_some(), egui::Button::new(RichText::new(tl!("Link")).color(Color32::WHITE)).fill(accent));
                push(&mut elems, "linkMedia.link", &r, "Link");
                if r.clicked() {
                    action = Some("link");
                }
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if crate::widgets::escape_closes(ctx) {
        action = Some("cancel");
    }
    let (item, ..) = rows[d.row].clone();
    let relink = |app: &mut WaltCutApp, d: &mut LinkMediaDraft, path: String| relink_to(app, d, &json!(item), path);
    match action {
        Some("browse") => app.pick_ui(FileDialog::Folder { at: None }, |app, paths| {
            if let (Some(f), Some(d)) = (paths.into_iter().next(), app.ui.link_media.as_mut()) {
                d.folder = f;
            }
        }),
        Some("search") => {
            let mut p = match_params(&d);
            p["folder"] = json!(d.folder);
            p["item"] = json!(item);
            p["exactName"] = json!(d.exact_name);
            match app.session.execute("media.search", p) {
                Ok(v) => {
                    d.candidates = v["results"][0]["candidates"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|c| {
                                    let probs =
                                        c["problems"].as_array().map(|p| p.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).unwrap_or_default();
                                    (c["path"].as_str().unwrap_or_default().to_string(), c["ok"] == json!(true), c["identityMatch"].as_bool(), probs)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    d.candidate = d.candidates.iter().position(|c| c.1);
                    d.message = if d.candidates.is_empty() { tl!("Nothing found with that name.").into() } else { String::new() };
                }
                Err(e) => d.message = e.to_string(),
            }
        }
        Some("locate") => {
            let exts: Vec<&str> =
                waltcut_media::VIDEO_EXTENSIONS.iter().chain(waltcut_media::AUDIO_EXTENSIONS).chain(waltcut_media::STILL_EXTENSIONS).copied().collect();
            let mut params = match_params(&d);
            params["item"] = json!(item);
            let hint = RelinkHint { command: "media.relink".into(), params };
            let item = json!(item);
            app.pick_ui(FileDialog::Relink { exts: exts.iter().map(|e| e.to_string()).collect(), hint }, move |app, paths| {
                let (Some(path), Some(mut d)) = (paths.into_iter().next(), app.ui.link_media.clone()) else { return };
                relink_to(app, &mut d, &item, path);
                app.ui.link_media = Some(d);
            });
        }
        Some("link") => {
            if let Some(path) = d.candidate.and_then(|c| d.candidates.get(c)).map(|c| c.0.clone()) {
                relink(app, &mut d, path);
            }
        }
        Some("offline") => {
            d.skipped.push(item);
            d.candidates.clear();
            d.candidate = None;
        }
        Some("offlineAll") | Some("cancel") => {
            let _ = app.session.execute("media.offlineAll", json!({}));
            keep = false;
        }
        _ => {}
    }
    app.ui.link_media = keep.then_some(d);
}

fn make_offline(app: &mut WaltCutApp, ctx: &egui::Context) {
    let Some(mut delete) = app.ui.make_offline else { return };
    let mut elems: Elems = Vec::new();
    let mut keep = true;
    let mut ok = false;
    let n = app.session.state.project_selection.len();
    egui::Window::new(tl!("Make Offline"))
        .id(egui::Id::new("Make Offline"))
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(crate::widgets::dialog_center(ctx))
        .show(ctx, |ui| {
            ui.label(tlf!("Make {n} selected clip(s) offline. Their clips show the offline slate until you link them again.", n));
            let r = ui.radio(!delete, tl!("Media files remain on disk"));
            push(&mut elems, "makeOffline.keep", &r, "Media files remain on disk");
            if r.clicked() {
                delete = false;
            }
            let r = ui.radio(delete, tl!("Media files are deleted"));
            push(&mut elems, "makeOffline.delete", &r, "Media files are deleted");
            if r.clicked() {
                delete = true;
            }
            ui.horizontal(|ui| {
                let r = ui.button(tl!("Cancel"));
                push(&mut elems, "makeOffline.cancel", &r, "Cancel");
                keep &= !r.clicked();
                let r = ui.button(tl!("OK"));
                push(&mut elems, "makeOffline.ok", &r, "OK");
                ok = r.clicked();
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if ok {
        if let Err(e) = app.session.execute("media.makeOffline", json!({"deleteFiles": delete})) {
            app.ui.status = e.to_string();
        }
        keep = false;
    }
    app.ui.make_offline = (keep && !crate::widgets::escape_closes(ctx)).then_some(delete);
}

/// Link Media: relink `item` to `path` with the dialog's matching options; the result goes into `d`.
fn relink_to(app: &mut WaltCutApp, d: &mut LinkMediaDraft, item: &Value, path: String) {
    let mut p = match_params(d);
    p["item"] = item.clone();
    p["path"] = json!(path);
    match app.session.execute("media.relink", p) {
        Ok(v) => {
            d.message.clear();
            d.candidates.clear();
            d.candidate = None;
            app.ui.status = tlf!("Linked {n} clip(s)", n = v["relinked"].as_array().map_or(0, Vec::len));
        }
        Err(e) => d.message = e.to_string(),
    }
}
