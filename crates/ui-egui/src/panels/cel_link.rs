//! Linked exposures in the interface (#67). The operations are the engine's `drawing.linkCel` and
//! `drawing.makeIndependent` (see `docs/animation.md`, "Reuse a cel"); this module is what shows
//! and starts them:
//!
//! * the **exposure lane** marks every exposure of a link group with the group's colour and a chain
//!   glyph, so shared artwork is visible at a glance (an unlinked cel has neither, and a duplicate is
//!   never marked: it is its own cel);
//! * the Brush **options bar** says, while the cel shown is linked, that an edit changes all of its
//!   exposures (`Linked ×3`), and offers **Link** (an exposure on this frame that shares the picked
//!   exposure, else the cel shown) and **Make Independent**;
//! * the exposure's right-click menu offers the same two.
//!
//! Nothing here is stored: the state is in the project's cels. Automation ids:
//! `program.brush.link`, `program.brush.linked`, `program.brush.makeIndependent`,
//! `timeline.celMenu.link`, `timeline.celMenu.independent`; a linked exposure's lane entry
//! `timeline.cel.<clip>.<frame>` says so in its label.

use egui::{Color32, Painter, Rect, Stroke as EStroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use waltcut_project::ClipId;
use waltcut_project::drawing::{Drawing, LinkId};

use crate::WaltCutApp;

/// The colour of a link group: the golden-ratio walk around the hue circle, so neighbouring ids
/// differ clearly.
pub fn tint(link: LinkId) -> Color32 {
    let hue = (f64::from(link) * 0.618_033_988_75).fract() as f32;
    egui::ecolor::Hsva::new(hue, 0.6, 0.95, 1.0).into()
}

/// Mark the exposure `rect` as part of group `link`: a stripe in the group colour along its top
/// and, when it is wide enough, a chain (two interlocked rings) at its start.
pub fn paint_badge(p: &Painter, rect: Rect, link: LinkId) {
    let colour = tint(link);
    p.line_segment([rect.left_top() + vec2(0.0, 1.0), rect.right_top() + vec2(0.0, 1.0)], EStroke::new(2.0, colour));
    if rect.width() >= 18.0 && rect.height() >= 9.0 {
        let c = pos2(rect.min.x + 9.0, rect.center().y + 1.0);
        let r = (rect.height() * 0.28).clamp(2.0, 4.0);
        let ring = EStroke::new(1.3, Color32::WHITE);
        p.circle_stroke(c - vec2(r * 0.6, 0.0), r, ring);
        p.circle_stroke(c + vec2(r * 0.6, 0.0), r, ring);
    }
}

/// What the lane's automation entry says about an exposure.
pub fn lane_label(base: &str, link: Option<LinkId>, shared: usize) -> String {
    match link {
        Some(id) if shared > 1 => format!("{base} (linked: {shared} exposures share this artwork, group {id}; editing one edits all)"),
        _ => base.to_string(),
    }
}

/// The picked exposure of `clip` to link from, when exactly one is picked.
fn picked_frame(app: &WaltCutApp, clip: ClipId, d: &Drawing) -> Option<i64> {
    let (c, frames) = app.tl.cel_sel.as_ref()?;
    match frames.as_slice() {
        [f] if *c == clip && d.cel_on(*f).is_some() => Some(*f),
        _ => None,
    }
}

/// The playhead's drawing frame in `clip`'s drawing.
fn playhead_frame(app: &WaltCutApp, item: &waltcut_project::TrackItem, d: &Drawing) -> i64 {
    let t = item.source_time_at(app.session.playhead().clamp(item.start, item.end() - waltcut_time::Tick(1)));
    d.frame_at(t)
}

fn report(app: &mut WaltCutApp, command: &str, params: Value, done: impl FnOnce(&Value) -> String) {
    match app.session.execute(command, params) {
        Ok(r) => app.ui.status = done(&r),
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// The options-bar controls: **Link**, and for a linked cel its scope and **Make Independent**.
pub fn controls(app: &mut WaltCutApp, ui: &mut Ui) {
    let Some((clip, item, d)) = super::drawing::target(app) else { return };
    let f = playhead_frame(app, &item, &d);
    let shown = d.cel_index_at(f);
    let source = picked_frame(app, clip, &d).or_else(|| shown.and_then(|i| d.cels.get(i)).map(|c| c.frame));
    let free = d.cel_on(f).is_none();
    let on = source.is_some() && free;
    let hint = match (source, free) {
        (None, _) => "Nothing to link to: draw a cel first".to_string(),
        (_, false) => format!("A cel already starts on frame {f}: move the playhead to a frame without one"),
        (Some(s), true) => format!(
            "Start an exposure on this frame that IS the artwork of the cel from frame {s}: editing either edits both. Duplicate makes an independent copy instead"
        ),
    };
    let r = ui.add_enabled(on, egui::Button::new("Link")).on_hover_text(hint);
    app.auto.add_response("program.brush.link", &r, "link cel: an exposure sharing another cel's artwork");
    if r.clicked()
        && let Some(from) = source
    {
        report(app, "drawing.linkCel", json!({"clip": clip.0, "from": from, "frame": f}), |v| {
            format!("Linked frame {f} to frame {from}: {} exposures now share this artwork", v["linked"].as_u64().unwrap_or(2))
        });
    }
    let Some((i, cel)) = shown.and_then(|i| d.cels.get(i).map(|c| (i, c))) else { return };
    let (Some(link), shared) = (cel.link, d.exposures_sharing(i)) else { return };
    if shared < 2 {
        return;
    }
    let frames: Vec<String> = d.link_members(link).filter_map(|j| d.cels.get(j)).map(|c| c.frame.to_string()).collect();
    let badge = ui.label(egui::RichText::new(format!("Linked ×{shared}")).strong().color(tint(link))).on_hover_text(format!(
        "This cel is linked: drawing on it, erasing, filling or transforming it changes all {shared} exposures (frames {}). Make Independent detaches this one",
        frames.join(", ")
    ));
    app.auto.add("program.brush.linked", badge.rect, &format!("linked exposure: {shared} exposures share this artwork, edits change all of them"));
    let r = ui.button("Make Independent").on_hover_text("Detach this exposure: it keeps what it shows, and edits stop reaching the others");
    app.auto.add_response("program.brush.makeIndependent", &r, "make this exposure independent of its link group");
    if r.clicked() {
        report(app, "drawing.makeIndependent", json!({"clip": clip.0, "frame": f}), |_| "This exposure is independent now".into());
    }
}

/// What the exposure menu can do with the picked exposure: whether Link and Make Independent apply.
pub struct MenuState {
    pub clip: ClipId,
    pub from: i64,
    pub linked: bool,
    pub frame: i64,
    pub free: bool,
}

pub fn menu_state(app: &WaltCutApp) -> Option<MenuState> {
    let (clip, frames) = app.tl.cel_sel.as_ref()?;
    let [from] = frames.as_slice() else { return None };
    let seq = app.session.active_sequence()?;
    let (_, item) = seq.find_item(*clip)?;
    let d = app.session.project.item(item.item)?.as_drawing()?;
    let i = d.cel_on(*from)?;
    let frame = playhead_frame(app, item, d);
    Some(MenuState { clip: *clip, from: *from, linked: d.exposures_sharing(i) > 1, frame, free: d.cel_on(frame).is_none() })
}

/// The two exposure-menu entries. Returns whether the menu should close.
pub fn menu_items(app: &mut WaltCutApp, ui: &mut Ui) -> bool {
    let state = menu_state(app);
    ui.separator();
    let can_link = state.as_ref().is_some_and(|s| s.free);
    let r = ui
        .add_enabled(can_link, egui::Button::new("Link at Playhead"))
        .on_hover_text("Start an exposure at the playhead that shares this exposure's artwork (editing either edits both)");
    app.auto.add_response("timeline.celMenu.link", &r, "link at playhead");
    let mut close = false;
    if r.clicked()
        && let Some(s) = &state
    {
        report(app, "drawing.linkCel", json!({"clip": s.clip.0, "from": s.from, "frame": s.frame}), |v| {
            format!("Linked frame {} to frame {}: {} exposures share this artwork", s.frame, s.from, v["linked"].as_u64().unwrap_or(2))
        });
        close = true;
    }
    let can_detach = state.as_ref().is_some_and(|s| s.linked);
    let r =
        ui.add_enabled(can_detach, egui::Button::new("Make Independent")).on_hover_text("Detach this exposure from its linked group; it keeps what it shows");
    app.auto.add_response("timeline.celMenu.independent", &r, "make independent");
    if r.clicked()
        && let Some(s) = &state
    {
        report(app, "drawing.makeIndependent", json!({"clip": s.clip.0, "frame": s.from}), |_| "This exposure is independent now".into());
        close = true;
    }
    close
}
