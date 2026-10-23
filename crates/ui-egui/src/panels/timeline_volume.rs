//! Clip-level audio gain line in the Timeline.

use egui::{Color32, Pos2, Rect, Stroke, pos2};
use serde_json::json;
use waltcut_project::{ClipId, Sequence, TrackItem, TrackKind};

use super::timeline::{Layout, Row};
use super::timeline_hit::EDGE_PX;
use crate::WaltCutApp;

const MIN_DB: f64 = -96.0;
const MAX_DB: f64 = 96.0;

fn band(body: Rect) -> Rect {
    Rect::from_min_max(pos2(body.min.x, body.min.y + 17.0), pos2(body.max.x, body.max.y - 3.0))
}

fn db_to_pos(db: f64) -> f32 {
    ((db.clamp(MIN_DB, MAX_DB) - MIN_DB) / (MAX_DB - MIN_DB)) as f32
}

fn pos_to_db(value: f32) -> f64 {
    MIN_DB + value.clamp(0.0, 1.0) as f64 * (MAX_DB - MIN_DB)
}

fn y_of(body: Rect, db: f64) -> f32 {
    let b = band(body);
    b.max.y - db_to_pos(db) * b.height()
}

fn find<'a>(seq: &'a Sequence, layout: &'a Layout, clip: ClipId) -> Option<(&'a Row, &'a TrackItem)> {
    let (tid, item) = seq.find_item(clip)?;
    Some((layout.rows.iter().find(|row| row.track == tid)?, item))
}

fn body(layout: &Layout, row: &Row, item: &TrackItem) -> Rect {
    Rect::from_min_max(pos2(layout.x_of(item.start), row.rect.min.y + 1.0), pos2(layout.x_of(item.end()), row.rect.max.y - 1.0))
}

pub fn paint(painter: &egui::Painter, body: Rect, item: &TrackItem) {
    let b = band(body);
    if b.height() < 6.0 || b.width() <= 0.0 {
        return;
    }
    let y = y_of(body, item.gain_db);
    painter.line_segment([pos2(body.min.x, y + 1.0), pos2(body.max.x, y + 1.0)], Stroke::new(1.0, Color32::BLACK));
    painter.line_segment([pos2(body.min.x, y), pos2(body.max.x, y)], Stroke::new(1.0, Color32::WHITE));
}

pub fn gain_at(item: &TrackItem, time: waltcut_time::Tick) -> f32 {
    waltcut_render::audio::clip_gain_at(item, time)
}

pub fn on_line(seq: &Sequence, layout: &Layout, clip: ClipId, pos: Pos2) -> bool {
    let Some((row, item)) = find(seq, layout, clip) else { return false };
    if row.kind != TrackKind::Audio || seq.track(row.track).is_none_or(|track| track.locked) {
        return false;
    }
    let rect = body(layout, row, item);
    band(rect).height() >= 6.0 && (pos.y - y_of(rect, item.gain_db)).abs() <= 4.0
}

#[derive(Clone, Debug)]
pub struct LineDrag {
    clip: ClipId,
    from: f64,
    y: f32,
    last: Option<f64>,
}

impl LineDrag {
    pub fn changed(&self) -> bool {
        self.last.is_some()
    }
}

pub fn start(seq: &Sequence, layout: &Layout, clip: ClipId, at: Pos2) -> Option<LineDrag> {
    let (_, item) = find(seq, layout, clip)?;
    Some(LineDrag { clip, from: item.gain_db, y: at.y, last: None })
}

pub fn drag(app: &mut WaltCutApp, ui: &egui::Ui, seq: &Sequence, layout: &Layout, mut drag: LineDrag, y: f32) -> LineDrag {
    let Some((row, item)) = find(seq, layout, drag.clip) else { return drag };
    let height = band(body(layout, row, item)).height().max(1.0);
    let db = pos_to_db(db_to_pos(drag.from) + (drag.y - y) / height);
    let begin = drag.last.is_none();
    if drag.last != Some(db) {
        if let Err(error) = app.session.execute("clip.audioGain", json!({"clips": [drag.clip.0], "mode": "set", "db": db, "merge": true, "begin": begin})) {
            app.ui.status = error.to_string();
        } else {
            drag.last = Some(db);
        }
    }
    let text = format!("{db:+.1} dB");
    egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::new("timeline-gain-tip"), egui::PopupAnchor::Pointer).show(|ui| ui.label(text));
    drag
}

pub fn interact(app: &mut WaltCutApp, seq: &Sequence, layout: &Layout, area: Rect) {
    for row in layout.rows.iter().filter(|row| row.kind == TrackKind::Audio) {
        let Some(track) = seq.track(row.track).filter(|track| !track.locked) else { continue };
        for item in &track.items {
            let rect = body(layout, row, item);
            let visible = rect.intersect(area).intersect(layout.content);
            let edge = EDGE_PX.min(rect.width() / 3.0);
            let (x0, x1) = (visible.min.x.max(rect.min.x + edge), visible.max.x.min(rect.max.x - edge));
            if visible.height() > 0.0 && x1 > x0 {
                let x = (x0 + x1) * 0.5;
                app.auto.add(
                    &format!("timeline.clip.{}.volume", item.id.0),
                    Rect::from_center_size(pos2(x, y_of(rect, item.gain_db)), egui::vec2(12.0, 8.0)),
                    "Clip audio gain",
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_line_represents_the_full_audio_gain_range() {
        for db in [-96.0, -6.0, 0.0, 15.0, 60.0, 96.0] {
            assert!((pos_to_db(db_to_pos(db)) - db).abs() < 1e-5, "{db}");
        }
        assert_eq!(db_to_pos(0.0), 0.5);
    }
}
