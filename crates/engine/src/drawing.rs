//! Drawing commands: frame-by-frame animation layers drawn over the picture
//! (`waltcut_project::drawing`).
//!
//! A drawing clip is targeted by `clip`, else the selected drawing clip, else the topmost drawing
//! clip under the playhead. Frames are drawing frames (0 = the clip's first source frame) and come
//! from `frame`, else from the timeline time in `time` / `timecode` / `seconds`, else the playhead.
//! Every stroke is one undo step.

use serde_json::{Value, json};
use waltcut_project::drawing::{self, Brush, Cel, Drawing, Stroke, StrokePoint};
use waltcut_project::{ClipId, ItemId, ItemKind, Sequence, TrackItem};
use waltcut_time::Tick;

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, has_seq, str_p, time_p, u64_p};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(id: &'static str, label: &'static str, menu: &'static [&'static str], params: &'static str, enabled: Enabled, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut: None, params, enabled, run, journal: true }
}

/// A command that only moves the view (the playhead). It is journaled all the same: only journaled
/// commands can be bound to a key, and replaying a playhead move after a crash is harmless.
fn view_spec(
    id: &'static str,
    label: &'static str,
    menu: &'static [&'static str],
    shortcut: &'static str,
    params: &'static str,
    enabled: Enabled,
    run: Run,
) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut: Some(shortcut), params, enabled, run, journal: true }
}

/// The drawing behind a track item, if it is a drawing clip.
pub fn drawing_of<'a>(s: &'a Session, seq: &Sequence, c: ClipId) -> Option<(ItemId, &'a Drawing)> {
    let (_, it) = seq.find_item(c)?;
    let pi = s.project.item(it.item)?;
    pi.as_drawing().map(|d| (it.item, d))
}

/// The drawing clip a command works on (see the module docs).
pub fn target_clip(s: &Session, p: &Value) -> Option<ClipId> {
    let seq = s.active_sequence()?;
    if let Some(c) = u64_p(p, "clip") {
        return drawing_of(s, seq, ClipId(c)).map(|_| ClipId(c));
    }
    if let Some(c) = s.state.selection.iter().copied().find(|c| drawing_of(s, seq, *c).is_some()) {
        return Some(c);
    }
    let t = s.playhead();
    seq.video_tracks.iter().rev().filter(|tr| tr.enabled).find_map(|tr| tr.item_at(t).filter(|it| drawing_of(s, seq, it.id).is_some()).map(|it| it.id))
}

/// Target clip, its drawing item and the drawing frame a command works on.
fn target(s: &Session, cmd: &str, p: &Value) -> Result<(ClipId, ItemId, i64)> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let clip = target_clip(s, p).ok_or_else(|| bad(cmd, "no drawing clip: select one or make one with Graphics and Titles ▸ New Layer ▸ New Drawing"))?;
    let (id, d) = drawing_of(s, seq, clip).ok_or_else(|| bad(cmd, "not a drawing clip"))?;
    let frame = match p.get("frame").and_then(Value::as_i64) {
        Some(f) => f,
        None => {
            let (_, it) = seq.find_item(clip).ok_or_else(|| bad(cmd, "no such clip"))?;
            let t = time_p(s, p, "").unwrap_or_else(|| s.playhead());
            // the clip's last frame for a time past its end, its first for one before its start
            let t = if it.duration > Tick::ZERO { t.clamp(it.start, it.end() - Tick(1)) } else { it.start };
            d.frame_at(it.source_time_at(t))
        }
    };
    Ok((clip, id, frame))
}

/// Edit the drawing `id` as one undo step.
fn edit_drawing<R>(s: &mut Session, cmd: &str, label: &str, id: ItemId, f: impl FnOnce(&mut Drawing) -> Result<R>) -> Result<R> {
    s.edit(label, |pr, _| {
        let d = pr.item_mut(id).and_then(|i| i.as_drawing_mut()).ok_or_else(|| bad(cmd, "not a drawing"))?;
        f(d)
    })
}

/// Inclusive drawing-frame bounds visible through this clip's current source-time mapping.
fn drawing_frame_bounds(s: &Session, cmd: &str, clip: ClipId, d: &Drawing) -> Result<(i64, i64)> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = seq.find_item(clip).ok_or_else(|| bad(cmd, "no such clip"))?;
    if it.duration <= Tick::ZERO {
        return Err(bad(cmd, "the drawing clip has no visible frames"));
    }
    let first = d.frame_at(it.source_time_at(it.start));
    let last = d.frame_at(it.source_time_at(it.end() - Tick(1)));
    Ok((first.min(last), first.max(last)))
}

fn editable_frame_bounds(s: &Session, cmd: &str, clip: ClipId, d: &Drawing) -> Result<(i64, i64)> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (track, it) = seq.find_item(clip).ok_or_else(|| bad(cmd, "no such clip"))?;
    if seq.track(track).is_some_and(|t| t.locked) {
        return Err(bad(cmd, "the drawing is on a locked track"));
    }
    if it.frame_hold.is_some() {
        return Err(bad(cmd, "cannot retime cels in a frame-held drawing clip"));
    }
    drawing_frame_bounds(s, cmd, clip, d)
}

fn retime_cel(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.retimeCel";
    let (clip, id, _) = target(s, ID, p)?;
    let from = p.get("from").and_then(Value::as_i64).ok_or_else(|| bad(ID, "need integer `from` frame"))?;
    let to = p.get("to").and_then(Value::as_i64).ok_or_else(|| bad(ID, "need integer `to` frame"))?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, d) = drawing_of(s, seq, clip).ok_or_else(|| bad(ID, "not a drawing clip"))?;
    let (first, last) = editable_frame_bounds(s, ID, clip, d)?;
    if from < first || from > last || to < first || to > last {
        return Err(bad(ID, format!("cel frames must be within the drawing clip's visible range {first}..={last}")));
    }
    if d.cel_on(from).is_none() {
        return Err(bad(ID, format!("no cel starts on frame {from}")));
    }
    if from == to {
        return Ok(json!({"clip": clip.0, "cel": from, "frame": to}));
    }
    edit_drawing(s, ID, "Retime Cel", id, |d| d.retime_cel(from, to).map_err(|e| bad(ID, e)))?;
    Ok(json!({"clip": clip.0, "cel": from, "frame": to}))
}

/// `spacing` (1 to 96 frames) or `on`: "ones" | "twos" | "threes".
fn spacing_param(cmd: &str, p: &Value) -> Result<i64> {
    if let Some(on) = p.get("on").and_then(Value::as_str) {
        return match on {
            "ones" => Ok(1),
            "twos" => Ok(2),
            "threes" => Ok(3),
            _ => Err(bad(cmd, "`on` is ones, twos or threes")),
        };
    }
    p.get("spacing").and_then(Value::as_i64).ok_or_else(|| bad(cmd, "need `on` (ones, twos, threes) or integer `spacing`"))
}

fn retime_exposures(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.retimeExposures";
    let (clip, id, _) = target(s, ID, p)?;
    let spacing = spacing_param(ID, p)?;
    let ripple = p.get("ripple").and_then(Value::as_bool).unwrap_or(true);
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, d) = drawing_of(s, seq, clip).ok_or_else(|| bad(ID, "not a drawing clip"))?;
    let (first, last) = editable_frame_bounds(s, ID, clip, d)?;
    let limit = last.checked_add(1).ok_or_else(|| bad(ID, "drawing clip end is out of range"))?;
    let frames: Vec<i64> = match p.get("frames") {
        Some(v) => {
            let a = v.as_array().ok_or_else(|| bad(ID, "`frames` is a list of cel start frames"))?;
            if a.len() > d.cels.len() {
                return Err(bad(ID, "more frames than the drawing has cels"));
            }
            a.iter().map(|f| f.as_i64().ok_or_else(|| bad(ID, "`frames` holds integers"))).collect::<Result<_>>()?
        }
        None => return Err(bad(ID, "need `frames`: the start frames of the selected cels")),
    };
    if frames.iter().any(|f| *f < first || *f > last) {
        return Err(bad(ID, format!("cel frames must be within the drawing clip's visible range {first}..={last}")));
    }
    // checked on a copy first so a refusal leaves the history alone, and so `dryRun` can show the result
    let mut probe = d.clone();
    let moved = probe.respace_cels(&frames, spacing, ripple, limit).map_err(|e| bad(ID, e))?;
    let planned: Vec<i64> = probe.cels.iter().map(|c| c.frame).collect();
    if p.get("dryRun").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(json!({"clip": clip.0, "moved": moved, "spacing": spacing, "cels": planned, "dryRun": true}));
    }
    if moved == 0 {
        return Ok(json!({"clip": clip.0, "moved": 0}));
    }
    edit_drawing(s, ID, "Retime Exposures", id, |d| d.respace_cels(&frames, spacing, ripple, limit).map_err(|e| bad(ID, e)))?;
    Ok(json!({"clip": clip.0, "moved": moved, "spacing": spacing, "cels": planned}))
}

/// Timeline time at which `clip` first shows the drawing cel `cel` on or after its start, for a
/// frame mapping that only ever moves one way (forward, or reversed): the first tick where the
/// shown frame has reached `frame`, then the first sequence frame boundary at or after it that
/// still shows that cel.
fn time_of_cel(s: &Session, it: &TrackItem, d: &Drawing, frame: i64, cel: usize) -> Option<Tick> {
    let shown = |t: Tick| d.frame_at(it.source_time_at(t));
    let reached = |t: Tick| if it.reverse { shown(t) <= frame } else { shown(t) >= frame };
    let (mut lo, mut hi) = (it.start.0, it.end().0.checked_sub(1)?);
    if lo > hi || !reached(Tick(hi)) {
        return None;
    }
    // first tick in [lo, hi] that has reached `frame`; at most 64 halvings
    for _ in 0..64 {
        if lo >= hi {
            break;
        }
        let mid = lo + (hi - lo) / 2;
        if reached(Tick(mid)) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let exact = Tick(lo);
    if d.cel_index_at(shown(exact)) != Some(cel) {
        return None;
    }
    let rate = s.sequence_rate();
    let mut on_grid = rate.tick_of(rate.frame_at(exact));
    if on_grid < exact {
        on_grid = rate.tick_of(rate.frame_at(exact).checked_add(1)?);
    }
    Some(if on_grid >= it.start && on_grid < it.end() && d.cel_index_at(shown(on_grid)) == Some(cel) { on_grid } else { exact })
}

/// Move the playhead to the previous or next drawing of the target clip (`drawing.previousCel`,
/// `drawing.nextCel`): the cel before or after the one shown, a held pose counting once. "Next"
/// follows the timeline, so on a reversed clip it is the cel with the lower start frame. A cel
/// trimmed off the clip does not count, one whose hold reaches into the clip does. `skipBlank`
/// passes over cels without strokes. View-only: nothing is edited and no undo step is made.
fn step_cel(s: &mut Session, p: &Value, forward: bool) -> Result<Value> {
    let id = if forward { "drawing.nextCel" } else { "drawing.previousCel" };
    let skip_blank = p.get("skipBlank").and_then(Value::as_bool).unwrap_or(false);
    let (clip, _, frame) = target(s, id, p)?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, d) = drawing_of(s, seq, clip).ok_or_else(|| bad(id, "not a drawing clip"))?;
    let (_, it) = seq.find_item(clip).ok_or_else(|| bad(id, "no such clip"))?;
    let (first, last) = drawing_frame_bounds(s, id, clip, d)?;
    if frame < first || frame > last {
        return Err(bad(id, format!("frame {frame} is outside the clip's visible range {first}..={last}")));
    }
    let toward_higher = forward != it.reverse;
    let shown = d.cel_index_at(frame);
    let none = || bad(id, if forward { "no next cel in this clip" } else { "no previous cel in this clip" });
    let mut at = if toward_higher { Some(shown.map_or(0, |i| i + 1)) } else { shown.and_then(|i| i.checked_sub(1)) };
    let found = loop {
        let cel = at.and_then(|i| d.cels.get(i)).ok_or_else(none)?;
        let i = at.ok_or_else(none)?;
        let end = d.cels.get(i + 1).map(|c| c.frame);
        // the clip shows this cel from `cel.frame` (or its first frame) up to `end`
        if toward_higher && cel.frame > last || !toward_higher && end.is_some_and(|e| e <= first) {
            return Err(none());
        }
        let visible = cel.frame <= last && end.is_none_or(|e| e > first);
        if visible && !(skip_blank && cel.strokes.is_empty()) {
            break (i, cel.frame.clamp(first, last), cel.strokes.is_empty());
        }
        at = if toward_higher { i.checked_add(1) } else { i.checked_sub(1) };
    };
    let (index, target_frame, blank) = found;
    let time = time_of_cel(s, it, d, target_frame, index).ok_or_else(|| bad(id, "that cel is not shown by the clip"))?;
    let cel_start = d.cels.get(index).map(|c| c.frame);
    s.set_playhead(time);
    Ok(json!({"clip": clip.0, "cel": cel_start, "frame": target_frame, "time": time.0, "blank": blank}))
}

fn set_hold(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.setHold";
    let (clip, id, _) = target(s, ID, p)?;
    let frame = p.get("frame").and_then(Value::as_i64).ok_or_else(|| bad(ID, "need integer `frame`"))?;
    let end_frame = p.get("endFrame").and_then(Value::as_i64).ok_or_else(|| bad(ID, "need integer exclusive `endFrame`"))?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, d) = drawing_of(s, seq, clip).ok_or_else(|| bad(ID, "not a drawing clip"))?;
    let (first, last) = editable_frame_bounds(s, ID, clip, d)?;
    if frame < first || frame > last {
        return Err(bad(ID, format!("cel frame must be within the drawing clip's visible range {first}..={last}")));
    }
    let clip_end = last.checked_add(1).ok_or_else(|| bad(ID, "drawing clip end is out of range"))?;
    let cel_index = d.cel_on(frame).ok_or_else(|| bad(ID, format!("no cel starts on frame {frame}")))?;
    let current_end = d.exposure_end(frame, clip_end).ok_or_else(|| bad(ID, "no cel starts on that frame"))?;
    if end_frame <= frame || end_frame > clip_end {
        return Err(bad(ID, format!("hold end must be after frame {frame} and no later than {clip_end}")));
    }
    if end_frame == current_end {
        return Ok(json!({"clip": clip.0, "cel": frame, "endFrame": end_frame, "blank": false}));
    }
    if end_frame > last {
        return Err(bad(ID, format!("hold end must be after frame {frame} and within the drawing clip's visible range {first}..={last}")));
    }
    let next = d.cels.get(cel_index + 1).map(|c| c.frame);
    let next_next = d.cels.get(cel_index + 2).map(|c| c.frame);
    if let Some(next_next) = next_next
        && end_frame >= next_next
    {
        return Err(bad(ID, "changing this hold would cross the following cel"));
    }
    let blank = next.is_none();
    edit_drawing(s, ID, "Set Cel Hold", id, |d| {
        if let Some(next) = next {
            d.retime_cel(next, end_frame).map_err(|e| bad(ID, e))?;
        } else {
            d.ensure_cel(end_frame, false).map_err(|e| bad(ID, e))?;
        }
        Ok(())
    })?;
    Ok(json!({"clip": clip.0, "cel": frame, "endFrame": end_frame, "blank": blank}))
}

/// Parse a stroke: `{brush?, color?: [r,g,b,a] | "#rrggbb", size?, points: [[x, y, pressure?], …]}`
/// (points may also be `{x, y, p?}` objects).
pub fn parse_stroke(cmd: &str, v: &Value) -> Result<Stroke> {
    let brush = match str_p(v, "brush") {
        Some(b) => Brush::from_name(b).ok_or_else(|| bad(cmd, format!("unknown brush `{b}` (Pen, Pencil, Marker or Eraser)")))?,
        None => Brush::Pen,
    };
    let color = match v.get("color") {
        None | Some(Value::Null) => [0.0, 0.0, 0.0, 1.0],
        Some(Value::String(h)) => hex_color(h).ok_or_else(|| bad(cmd, format!("bad colour `{h}`")))?,
        Some(Value::Array(a)) => {
            let c = |i: usize, d: f64| a.get(i).and_then(Value::as_f64).unwrap_or(d) as f32;
            [c(0, 0.0), c(1, 0.0), c(2, 0.0), c(3, 1.0)]
        }
        Some(_) => return Err(bad(cmd, "`color` is [r, g, b, a] or \"#rrggbb\"")),
    };
    let size = f64_p(v, "size").unwrap_or(6.0) as f32;
    let pts = v.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "a stroke needs `points`"))?;
    if pts.len() > drawing::MAX_STROKE_POINTS {
        return Err(bad(cmd, format!("a stroke may hold at most {} points", drawing::MAX_STROKE_POINTS)));
    }
    let points = pts
        .iter()
        .filter_map(|q| match q {
            Value::Array(a) => {
                Some(StrokePoint { x: a.first()?.as_f64()? as f32, y: a.get(1)?.as_f64()? as f32, p: a.get(2).and_then(Value::as_f64).unwrap_or(1.0) as f32 })
            }
            Value::Object(_) => Some(StrokePoint { x: f64_p(q, "x")? as f32, y: f64_p(q, "y")? as f32, p: f64_p(q, "p").unwrap_or(1.0) as f32 }),
            _ => None,
        })
        .collect();
    Stroke { brush, color, size, points }.sanitized().map_err(|e| bad(cmd, e))
}

fn hex_color(h: &str) -> Option<[f32; 4]> {
    let h = h.trim().trim_start_matches('#');
    if h.len() != 6 && h.len() != 8 {
        return None;
    }
    let byte = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).map(|b| b as f32 / 255.0);
    Some([byte(0)?, byte(2)?, byte(4)?, if h.len() == 8 { byte(6)? } else { 1.0 }])
}

fn new_drawing(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.new";
    let seconds = f64_p(p, "seconds").unwrap_or(5.0).max(0.01);
    let mut placement = p.clone();
    if let Some(o) = placement.as_object_mut() {
        // `seconds` is the duration here, not the placement time
        o.remove("seconds");
    }
    let name = str_p(p, "name").unwrap_or("Drawing").to_string();
    let item_name = name.clone();
    let clip = crate::graphics::place_video_clip(s, ID, &name, &placement, "New Drawing", Vec::new(), move |pr, (w, h, rate)| {
        let src = pr.add_item(&item_name, waltcut_project::Label::Mango, ItemKind::Drawing(std::sync::Arc::new(Drawing::new(w, h, rate))), None);
        (src, rate.snap_nearest(Tick::from_seconds_f64(seconds)).max(rate.frame_duration()))
    })?;
    let item = s.active_sequence().and_then(|q| q.find_item(clip)).map(|(_, it)| it.item.0);
    Ok(json!({"clip": clip.0, "item": item}))
}

/// How a stroke picks its cel: draw into the cel shown (`hold`), or start a cel on this frame,
/// empty (`new`) or as a copy of the one shown (`copy`).
fn cel_mode(cmd: &str, p: &Value) -> Result<(bool, bool)> {
    Ok(match str_p(p, "cel").unwrap_or("hold") {
        "hold" | "current" => (false, false),
        "new" => (true, false),
        "copy" => (true, true),
        other => return Err(bad(cmd, format!("`cel` is hold, new or copy, not `{other}`"))),
    })
}

fn add_stroke(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.addStroke";
    let (clip, id, frame) = target(s, ID, p)?;
    let stroke = parse_stroke(ID, p.get("stroke").unwrap_or(p))?;
    let (new_cel, copy) = cel_mode(ID, p)?;
    let label = if stroke.brush == Brush::Eraser { "Erase" } else { "Draw Stroke" };
    let (cel_frame, n) = edit_drawing(s, ID, label, id, |d| {
        // before the first cel there is nothing shown: the stroke starts one here
        let i = match d.cel_index_at(frame) {
            Some(i) if !new_cel => i,
            _ => d.ensure_cel(frame, copy).map_err(|e| bad(ID, e))?,
        };
        let cel = d.cels.get_mut(i).ok_or_else(|| bad(ID, "no such cel"))?;
        if cel.strokes.len() >= drawing::MAX_CEL_STROKES {
            return Err(bad(ID, format!("a cel may hold at most {} strokes", drawing::MAX_CEL_STROKES)));
        }
        cel.strokes_mut().push(stroke);
        Ok((cel.frame, cel.strokes.len()))
    })?;
    Ok(json!({"clip": clip.0, "cel": cel_frame, "strokes": n}))
}

fn set_strokes(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.setStrokes";
    let (clip, id, frame) = target(s, ID, p)?;
    let list = p.get("strokes").and_then(Value::as_array).ok_or_else(|| bad(ID, "need `strokes`: a list of strokes"))?;
    if list.len() > drawing::MAX_CEL_STROKES {
        return Err(bad(ID, format!("a cel may hold at most {} strokes", drawing::MAX_CEL_STROKES)));
    }
    let strokes = list.iter().map(|v| parse_stroke(ID, v)).collect::<Result<Vec<_>>>()?;
    let n = strokes.len();
    edit_drawing(s, ID, "Set Drawing", id, |d| {
        let i = d.ensure_cel(frame, false).map_err(|e| bad(ID, e))?;
        if let Some(c) = d.cels.get_mut(i) {
            c.strokes = strokes.into();
        }
        Ok(())
    })?;
    Ok(json!({"clip": clip.0, "cel": frame, "strokes": n}))
}

fn new_cel(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.newCel";
    let (clip, id, frame) = target(s, ID, p)?;
    let copy = bool_p(p, "copy").unwrap_or(false);
    edit_drawing(s, ID, if copy { "Duplicate Cel" } else { "New Cel" }, id, |d| {
        if d.cel_on(frame).is_some() {
            return Err(bad(ID, format!("frame {frame} already starts a cel")));
        }
        d.ensure_cel(frame, copy).map_err(|e| bad(ID, e))
    })?;
    Ok(json!({"clip": clip.0, "cel": frame}))
}

fn clear_cel(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.clearCel";
    let (clip, id, frame) = target(s, ID, p)?;
    let cel = edit_drawing(s, ID, "Clear Cel", id, |d| {
        let i = d.cel_index_at(frame).ok_or_else(|| bad(ID, "no cel is shown here"))?;
        let c = d.cels.get_mut(i).ok_or_else(|| bad(ID, "no such cel"))?;
        c.strokes_mut().clear();
        Ok(c.frame)
    })?;
    Ok(json!({"clip": clip.0, "cel": cel}))
}

fn delete_cel(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.deleteCel";
    let (clip, id, frame) = target(s, ID, p)?;
    let cel = edit_drawing(s, ID, "Delete Cel", id, |d| {
        let i = d.cel_index_at(frame).ok_or_else(|| bad(ID, "no cel is shown here"))?;
        Ok(d.cels.remove(i).frame)
    })?;
    Ok(json!({"clip": clip.0, "deleted": cel}))
}

/// Fill the frames between the cel shown and the next one with in-betweens every `step` frames.
fn tween_cels(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.tween";
    let (clip, id, frame) = target(s, ID, p)?;
    let step = u64_p(p, "step").unwrap_or(2).clamp(1, 1000) as i64;
    let made = edit_drawing(s, ID, "In-between Cels", id, |d| {
        let i = d.cel_index_at(frame).ok_or_else(|| bad(ID, "no cel is shown here"))?;
        let (Some(a), Some(b)) = (d.cels.get(i).cloned(), d.cels.get(i + 1).cloned()) else {
            return Err(bad(ID, "in-betweens need a later cel to move towards"));
        };
        let span = b.frame.saturating_sub(a.frame);
        let mut made = Vec::new();
        let mut f = a.frame.saturating_add(step);
        while f < b.frame {
            let t = (f - a.frame) as f32 / span.max(1) as f32;
            let strokes = drawing::tween(&a, &b, t);
            let k = d.ensure_cel(f, false).map_err(|e| bad(ID, e))?;
            if let Some(c) = d.cels.get_mut(k) {
                *c = Cel { frame: f, strokes: strokes.into() };
            }
            made.push(f);
            f = f.saturating_add(step);
        }
        if made.is_empty() {
            return Err(bad(ID, format!("no room for in-betweens on {step}s between frames {} and {}", a.frame, b.frame)));
        }
        Ok(made)
    })?;
    Ok(json!({"clip": clip.0, "cels": made}))
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.inspect";
    let (clip, _, frame) = target(s, ID, p)?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, d) = drawing_of(s, seq, clip).ok_or_else(|| bad(ID, "not a drawing clip"))?;
    let full = bool_p(p, "strokes").unwrap_or(false);
    let cels: Vec<Value> = d
        .cels
        .iter()
        .map(|c| if full { json!({"frame": c.frame, "strokes": c.strokes}) } else { json!({"frame": c.frame, "strokes": c.strokes.len()}) })
        .collect();
    let (_, last) = drawing_frame_bounds(s, ID, clip, d)?;
    let clip_end = last.checked_add(1).ok_or_else(|| bad(ID, "drawing clip end is out of range"))?;
    let exposures: Vec<Value> = d.cels.iter().map(|c| json!({"frame": c.frame, "endFrame": d.exposure_end(c.frame, clip_end)})).collect();
    Ok(json!({
        "clip": clip.0,
        "width": d.width,
        "height": d.height,
        "frame": frame,
        "shown": d.cel_at(frame).map(|c| c.frame),
        "cels": cels,
        "exposures": exposures,
    }))
}

fn has_drawing(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    target_clip(s, &Value::Null).map(|_| ()).ok_or_else(|| "no drawing clip is selected or under the playhead".into())
}

pub fn commands() -> Vec<CommandSpec> {
    const STROKE: &str = r##"{"clip":id?,"frame":int?,"time":ticks?,"cel":"hold|new|copy"?,"stroke":{"brush":"Pen|Pencil|Marker|Eraser"?,"color":"#rrggbb"|[r,g,b,a]?,"size":px?,"points":[[x,y,pressure?],...]}}"##;
    vec![
        spec(
            "drawing.new",
            "New Drawing",
            &["Graphics and Titles", "New Layer"],
            r#"{"seconds":f64?,"time":ticks?,"track":int?,"name":str?}"#,
            has_seq,
            new_drawing,
        ),
        spec("drawing.addStroke", "Draw Stroke", &[], STROKE, has_drawing, add_stroke),
        spec("drawing.setStrokes", "Set Drawing", &[], r#"{"clip":id?,"frame":int?,"strokes":[stroke,...]}"#, has_drawing, set_strokes),
        spec("drawing.newCel", "New Cel", &["Graphics and Titles", "Drawing"], r#"{"clip":id?,"frame":int?,"copy":bool?}"#, has_drawing, new_cel),
        spec("drawing.retimeCel", "Retime Cel", &["Graphics and Titles", "Drawing"], r#"{"clip":id?,"from":int,"to":int}"#, has_drawing, retime_cel),
        spec("drawing.setHold", "Set Cel Hold", &["Graphics and Titles", "Drawing"], r#"{"clip":id?,"frame":int,"endFrame":int}"#, has_drawing, set_hold),
        spec(
            "drawing.retimeExposures",
            "Retime Exposures",
            &["Graphics and Titles", "Drawing"],
            r#"{"clip":id?,"frames":[int,...],"on":"ones"|"twos"|"threes","spacing":int?,"ripple":bool?,"dryRun":bool?}"#,
            has_drawing,
            retime_exposures,
        ),
        view_spec(
            "drawing.previousCel",
            "Previous Cel",
            &["Graphics and Titles", "Drawing"],
            "Cmd+Alt+,",
            r#"{"clip":id?,"time":ticks?,"skipBlank":bool?}"#,
            has_drawing,
            |s, p| step_cel(s, p, false),
        ),
        view_spec(
            "drawing.nextCel",
            "Next Cel",
            &["Graphics and Titles", "Drawing"],
            "Cmd+Alt+.",
            r#"{"clip":id?,"time":ticks?,"skipBlank":bool?}"#,
            has_drawing,
            |s, p| step_cel(s, p, true),
        ),
        spec("drawing.clearCel", "Clear Cel", &["Graphics and Titles", "Drawing"], r#"{"clip":id?,"frame":int?}"#, has_drawing, clear_cel),
        spec("drawing.deleteCel", "Delete Cel", &["Graphics and Titles", "Drawing"], r#"{"clip":id?,"frame":int?}"#, has_drawing, delete_cel),
        spec(
            "drawing.tween",
            "In-between to Next Cel",
            &["Graphics and Titles", "Drawing"],
            r#"{"clip":id?,"frame":int?,"step":int?}"#,
            has_drawing,
            tween_cels,
        ),
        CommandSpec {
            id: "drawing.inspect",
            label: "Inspect Drawing",
            menu: &[],
            shortcut: None,
            params: r#"{"clip":id?,"frame":int?,"strokes":bool?}"#,
            enabled: always,
            run: inspect,
            journal: false,
        },
    ]
}
