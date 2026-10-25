//! Reference images (#3): place a picture under the drawing to trace it, and adjust it without
//! leaving the drawing workflow.
//!
//! A **reference** is an ordinary video clip on a video track named `Reference` that sits
//! directly below the drawing it serves and lasts as long as the drawing clip does. It is normal
//! artwork: it is included in previews and exports, hiding it (`hidden`) excludes it, and the
//! guide policy of #65 (`guide: true`) keeps it in the preview but out of delivery renders.
//!
//! * `reference.place {path?|item?, clip?, opacity?, guide?}` imports `path` (or uses the project
//!   item `item`, which must have a picture), puts it on a `Reference` track directly below the
//!   target drawing for the drawing's duration at `opacity` percent (50 by default), and leaves
//!   the playhead and selection alone. The track below is reused when it is a free, unlocked
//!   `Reference` track; otherwise a new one is added there. It is **one undo step** and all or
//!   nothing: a locked drawing track or reference track, a missing target, a file that cannot be
//!   imported or a clip that cannot be placed change nothing, and no import is left behind.
//! * `reference.set {clip?, opacity?, hidden?, locked?, guide?, x?, y?, dx?, dy?, scale?, rotation?,
//!   merge?}` adjusts a reference: opacity (0-100 %), hide, lock (the `Reference` track), guide,
//!   and Motion position (`x`, `y`, or relative `dx`, `dy`, in sequence pixels), scale (%) and
//!   rotation (degrees). All values are checked before any is applied. A drag is one undo step:
//!   send `merge: "start"` with its first call and `merge: true` with the following ones; the
//!   next drag starts again with `"start"`.
//! * `reference.list {clip?}` names the references of the target drawing: identity, track and
//!   current settings, for scripts and the UI.

use serde_json::{Value, json};
use waltcut_geom::Vec2;
use waltcut_project::{ClipId, ItemId, ItemKind, ParamValue, Sequence, TrackId, TrackKind};
use waltcut_time::{Tick, TimeRange};

use crate::commands::{CommandSpec, always, bad, has_seq, u64_p};
use crate::{EngineError, Result, Session};

/// Name of the track a reference lives on.
pub const TRACK_NAME: &str = "Reference";
/// Opacity a placed reference starts at, in percent.
pub const DEFAULT_OPACITY: f64 = 50.0;
/// Largest |position| (sequence pixels), scale (%) and |rotation| (degrees) accepted.
const MAX_POSITION: f64 = 1.0e6;
const MAX_SCALE: f64 = 10_000.0;
const MAX_ROTATION: f64 = 36_000.0;

fn is_reference_track(name: &str) -> bool {
    name.starts_with(TRACK_NAME)
}

/// The drawing clip a reference command works on: `clip`, else the selected or visible one.
fn target(s: &Session, cmd: &str, p: &Value) -> Result<ClipId> {
    let probe = match u64_p(p, "target") {
        Some(c) => json!({ "clip": c }),
        None => Value::Null,
    };
    crate::drawing::target_clip(s, &probe).ok_or_else(|| bad(cmd, "no drawing clip: select one, or pass `target` (a drawing clip id)"))
}

/// The references of drawing clip `drawing`: `(track index, clip)` for clips on a `Reference`
/// track below it that overlap its duration, nearest track first.
fn references(seq: &Sequence, drawing: ClipId) -> Vec<(usize, ClipId)> {
    let Some((tid, d)) = seq.find_item(drawing) else { return Vec::new() };
    let Some(di) = seq.video_tracks.iter().position(|t| t.id == tid) else { return Vec::new() };
    let mut out = Vec::new();
    for ti in (0..di).rev() {
        let Some(tr) = seq.video_tracks.get(ti) else { continue };
        if !is_reference_track(&tr.name) {
            continue;
        }
        for it in &tr.items {
            if it.start < d.end() && it.end() > d.start {
                out.push((ti, it.id));
            }
        }
    }
    out
}

fn number(cmd: &str, p: &Value, key: &str, lo: f64, hi: f64) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64() {
            Some(n) if n.is_finite() && (lo..=hi).contains(&n) => Ok(Some(n)),
            _ => Err(bad(cmd, format!("`{key}` must be a number from {lo} to {hi}"))),
        },
    }
}

fn flag(cmd: &str, p: &Value, key: &str) -> Result<Option<bool>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad(cmd, format!("`{key}` must be true or false"))),
    }
}

fn check_keys(cmd: &str, p: &Value, known: &[&str]) -> Result<()> {
    match p {
        Value::Null => Ok(()),
        Value::Object(o) => match o.keys().find(|k| !known.contains(&k.as_str())) {
            Some(k) => Err(bad(cmd, format!("unknown parameter `{k}`; expected: {}", known.join(", ")))),
            None => Ok(()),
        },
        _ => Err(bad(cmd, "parameters must be an object")),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Merge {
    No,
    Start,
    Join,
}

fn place(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "reference.place";
    check_keys(ID, p, &["path", "item", "target", "opacity", "guide", "bin"])?;
    let opacity = number(ID, p, "opacity", 0.0, 100.0)?.unwrap_or(DEFAULT_OPACITY);
    let guide = flag(ID, p, "guide")?.unwrap_or(false);
    let path = match p.get("path") {
        None | Some(Value::Null) => None,
        Some(v) => {
            Some(v.as_str().filter(|x| !x.trim().is_empty() && x.len() <= 4096 && !x.contains('\0')).ok_or_else(|| bad(ID, "`path` must be a file path"))?)
        }
    };
    let existing = match p.get("item") {
        None | Some(Value::Null) => None,
        Some(v) => Some(ItemId(v.as_u64().ok_or_else(|| bad(ID, "`item` must be a project item id"))?)),
    };
    if path.is_some() == existing.is_some() {
        return Err(bad(ID, "pass exactly one of `path` (an image to import) and `item` (one already in the project)"));
    }
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let drawing = target(s, ID, p)?;
    let (dtrack, d) = seq.find_item(drawing).ok_or_else(|| bad(ID, "no such drawing clip"))?;
    let di = seq.video_tracks.iter().position(|t| t.id == dtrack).ok_or_else(|| bad(ID, "no such track"))?;
    if seq.video_tracks.get(di).is_none_or(|t| t.locked) {
        return Err(bad(ID, "the drawing's track is locked: unlock it first"));
    }
    let (start, duration) = (d.start, d.duration);
    if duration <= Tick::ZERO {
        return Err(bad(ID, "the drawing clip has no duration"));
    }
    if let Some(item) = existing {
        let pi = s.project.item(item).ok_or_else(|| bad(ID, format!("no project item {}", item.0)))?;
        if !matches!(pi.kind, ItemKind::Media(_)) || !pi.has_video() {
            return Err(bad(ID, "a reference must be a picture or video"));
        }
    }
    // a free, unlocked Reference track right below, else a new one there
    let below = di.checked_sub(1).and_then(|i| seq.video_tracks.get(i));
    let reuse = match below {
        Some(t) if is_reference_track(&t.name) => {
            if t.locked {
                return Err(bad(ID, "the Reference track below the drawing is locked: unlock it first"));
            }
            (!t.items.iter().any(|i| i.start < start + duration && i.end() > start)).then_some(t.id)
        }
        _ => None,
    };
    let bin = u64_p(p, "bin");
    let path = path.map(str::to_string);
    let label = "Place Reference Image";
    // placing selects the new clip and may move the playhead; a reference should not disturb them
    let (playhead, selection) = (s.playhead(), s.state.selection.clone());
    s.edit_group(label, |s| {
        let item = match (&path, existing) {
            (Some(path), _) => {
                let mut import = json!({ "paths": [path] });
                if let Some(b) = bin {
                    import["bin"] = json!(b);
                }
                let imported = s.execute("file.import", import)?;
                let first = imported["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).map(ItemId);
                let item = first.ok_or_else(|| {
                    let why = imported["errors"].as_array().and_then(|e| e.first()).map(|e| e.to_string()).unwrap_or_default();
                    bad(ID, format!("{path} could not be imported {why}"))
                })?;
                let pi = s.project.item(item).ok_or_else(|| bad(ID, "the import produced no item"))?;
                if !pi.has_video() {
                    return Err(bad(ID, format!("{path} has no picture")));
                }
                item
            }
            (None, Some(item)) => item,
            (None, None) => return Err(bad(ID, "no source")),
        };
        let track = match reuse {
            Some(t) => t,
            None => {
                let added = s.execute("sequence.addTracks", json!({"video": 1, "videoAfter": di}))?;
                let id = added["video"][0].as_u64().map(TrackId).ok_or_else(|| bad(ID, "no track was added"))?;
                s.edit_sequence(label, |q, _, _| {
                    if let Some(t) = q.video_tracks.iter_mut().find(|t| t.id == id) {
                        t.name = TRACK_NAME.into();
                    }
                    Ok(())
                })?;
                id
            }
        };
        let range = {
            let pi = s.project.item(item).ok_or_else(|| bad(ID, "no such item"))?;
            // a picture is as long as the drawing; a video no longer than it has footage
            let available = crate::commands::full_duration(s, pi);
            let still = matches!(&pi.kind, ItemKind::Media(m) if matches!(m.info.kind, waltcut_media::MediaKind::Still));
            let len = if still { duration } else { duration.min(available) };
            TimeRange::new(Tick::ZERO, len)
        };
        let clips = crate::commands::place_item(s, item, range, start, Some(track), None, false, label, None)?;
        let clip = clips.first().copied().ok_or_else(|| bad(ID, "nothing was placed"))?;
        s.edit_sequence(label, |q, _, _| {
            let (_, it) = q.find_item_mut(clip).ok_or_else(|| bad(ID, "the placed clip vanished"))?;
            if let Some(o) = it.effect_mut("opacity").and_then(|e| e.param_mut("opacity")) {
                o.value = ParamValue::Float(opacity);
            }
            it.guide = guide;
            Ok(())
        })?;
        s.set_playhead(playhead);
        s.state.selection = selection;
        Ok(json!({"clip": clip.0, "item": item.0, "track": track.0, "drawing": drawing.0, "opacity": opacity, "newTrack": reuse.is_none()}))
    })
}

fn describe(seq: &Sequence, ti: usize, clip: ClipId) -> Option<Value> {
    let tr = seq.video_tracks.get(ti)?;
    let it = tr.items.iter().find(|i| i.id == clip)?;
    let mt = it.source_time_at(it.start);
    let motion = |name: &str| it.effect("motion").and_then(|e| e.param(name)).map(|p| p.value_at(mt));
    let (x, y) = match motion("position") {
        Some(ParamValue::Vec2(v)) => (v.x, v.y),
        _ => (f64::NAN, f64::NAN),
    };
    let float = |v: Option<ParamValue>| match v {
        Some(ParamValue::Float(f)) => json!(f),
        _ => Value::Null,
    };
    let opacity = it.effect("opacity").and_then(|e| e.param("opacity")).map(|p| p.value_at(mt));
    Some(json!({
        "clip": clip.0, "item": it.item.0, "name": it.name, "track": tr.id.0, "trackIndex": ti,
        "hidden": !it.enabled, "locked": tr.locked, "guide": it.guide,
        "opacity": float(opacity), "x": if x.is_finite() { json!(x) } else { Value::Null }, "y": if y.is_finite() { json!(y) } else { Value::Null },
        "scale": float(motion("scale")), "rotation": float(motion("rotation")),
        "start": it.start.0, "duration": it.duration.0,
    }))
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "reference.list";
    check_keys(ID, p, &["target"])?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let drawing = target(s, ID, p)?;
    let refs: Vec<Value> = references(seq, drawing).into_iter().filter_map(|(ti, c)| describe(seq, ti, c)).collect();
    Ok(json!({"drawing": drawing.0, "references": refs}))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "reference.set";
    check_keys(ID, p, &["clip", "target", "opacity", "hidden", "locked", "guide", "x", "y", "dx", "dy", "scale", "rotation", "merge"])?;
    let opacity = number(ID, p, "opacity", 0.0, 100.0)?;
    let (hidden, locked, guide) = (flag(ID, p, "hidden")?, flag(ID, p, "locked")?, flag(ID, p, "guide")?);
    // false: a step of its own; "start": a step that the following `true` calls join; true: join
    let merge = match p.get("merge") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Merge::No,
        Some(Value::Bool(true)) => Merge::Join,
        Some(Value::String(v)) if v == "start" => Merge::Start,
        Some(_) => return Err(bad(ID, "`merge` must be true, false or \"start\"")),
    };
    let (x, y) = (number(ID, p, "x", -MAX_POSITION, MAX_POSITION)?, number(ID, p, "y", -MAX_POSITION, MAX_POSITION)?);
    let (dx, dy) = (number(ID, p, "dx", -MAX_POSITION, MAX_POSITION)?, number(ID, p, "dy", -MAX_POSITION, MAX_POSITION)?);
    let scale = number(ID, p, "scale", 0.01, MAX_SCALE)?;
    let rotation = number(ID, p, "rotation", -MAX_ROTATION, MAX_ROTATION)?;
    if (x.is_some() && dx.is_some()) || (y.is_some() && dy.is_some()) {
        return Err(bad(ID, "give a position (`x`, `y`) or a move (`dx`, `dy`) for an axis, not both"));
    }
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let clip = match u64_p(p, "clip") {
        Some(c) => ClipId(c),
        None => {
            let drawing = target(s, ID, p)?;
            references(seq, drawing).first().map(|(_, c)| *c).ok_or_else(|| bad(ID, "this drawing has no reference: place one first"))?
        }
    };
    let (tid, it) = seq.find_item(clip).ok_or_else(|| bad(ID, format!("no clip {}", clip.0)))?;
    let tr = seq.track(tid).ok_or(EngineError::NoSequence)?;
    if tr.kind != TrackKind::Video || !is_reference_track(&tr.name) {
        return Err(bad(ID, format!("clip {} is not a reference (it is not on a `{TRACK_NAME}` track)", clip.0)));
    }
    let edits_clip = opacity.is_some()
        || hidden.is_some()
        || guide.is_some()
        || x.is_some()
        || y.is_some()
        || dx.is_some()
        || dy.is_some()
        || scale.is_some()
        || rotation.is_some();
    if tr.locked && (edits_clip || locked != Some(false)) {
        return Err(bad(ID, "the reference is locked: unlock it first (`locked: false`)"));
    }
    let _ = it;
    let label = if x.is_some() || y.is_some() || dx.is_some() || dy.is_some() { "Move Reference" } else { "Adjust Reference" };
    let apply = |q: &mut Sequence| -> Result<()> {
        let tid = q.find_item(clip).map(|(t, _)| t).ok_or_else(|| bad(ID, "no such clip"))?;
        if let Some(l) = locked
            && let Some(t) = q.video_tracks.iter_mut().find(|t| t.id == tid)
        {
            t.locked = l;
        }
        let (_, it) = q.find_item_mut(clip).ok_or_else(|| bad(ID, "no such clip"))?;
        let mt = it.source_time_at(it.start);
        if let Some(h) = hidden {
            it.enabled = !h;
        }
        if let Some(g) = guide {
            it.guide = g;
        }
        if let Some(o) = opacity
            && let Some(par) = it.effect_mut("opacity").and_then(|e| e.param_mut("opacity"))
        {
            par.set_at(mt, ParamValue::Float(o));
        }
        if let Some(m) = it.effect_mut("motion") {
            if let Some(par) = m.param_mut("position")
                && (x.is_some() || y.is_some() || dx.is_some() || dy.is_some())
            {
                let ParamValue::Vec2(cur) = par.value_at(mt) else { return Err(bad(ID, "the reference has no position")) };
                // an "auto" (NaN) coordinate is the frame centre until it is moved
                let base = |v: f64| if v.is_finite() { v } else { 0.0 };
                let nx = x.unwrap_or_else(|| base(cur.x) + dx.unwrap_or(0.0));
                let ny = y.unwrap_or_else(|| base(cur.y) + dy.unwrap_or(0.0));
                par.set_at(mt, ParamValue::Vec2(Vec2::new(nx, ny)));
            }
            if let Some(v) = scale
                && let Some(par) = m.param_mut("scale")
            {
                par.set_at(mt, ParamValue::Float(v));
            }
            if let Some(v) = rotation
                && let Some(par) = m.param_mut("rotation")
            {
                par.set_at(mt, ParamValue::Float(v));
            }
        }
        Ok(())
    };
    match merge {
        Merge::Join => {
            s.edit_sequence_as(label, Some("reference.set"), |q, _, _| apply(q))?;
        }
        Merge::Start => {
            s.edit_sequence(label, |q, _, _| apply(q))?;
            s.history.merge_key = Some("reference.set".into());
        }
        Merge::No => {
            s.edit_sequence(label, |q, _, _| apply(q))?;
        }
    }
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let ti = seq.video_tracks.iter().position(|t| t.items.iter().any(|i| i.id == clip)).ok_or(EngineError::NoSequence)?;
    describe(seq, ti, clip).ok_or(EngineError::NoSequence)
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "reference.place",
            label: "Place Reference Image",
            menu: &["Graphics and Titles", "Drawing"],
            shortcut: None,
            params: r#"{"path":str|"item":id,"target":drawingClipId?,"opacity":0-100=50,"guide":bool=false,"bin":binId?}  put a picture on a Reference track right below the drawing, for its duration; one undo step"#,
            enabled: has_seq,
            run: place,
            journal: true,
        },
        CommandSpec {
            id: "reference.set",
            label: "Adjust Reference",
            menu: &[],
            shortcut: None,
            params: r#"{"clip":id?,"target":id?,"opacity":0-100?,"hidden":bool?,"locked":bool?,"guide":bool?,"x":px?,"y":px?,"dx":px?,"dy":px?,"scale":%?,"rotation":deg?,"merge":"start"|bool?}"#,
            enabled: has_seq,
            run: set,
            journal: true,
        },
        CommandSpec {
            id: "reference.list",
            label: "List References",
            menu: &[],
            shortcut: None,
            params: r#"{"target":drawingClipId?}  the references of the drawing: identity, track and settings"#,
            enabled: always,
            run: list,
            journal: false,
        },
    ]
}

/// The references of the drawing the Brush tool works on now (what `reference.list` reports),
/// read without running a command: the UI asks every frame.
pub fn current(s: &Session) -> Vec<Value> {
    let Some(seq) = s.active_sequence() else { return Vec::new() };
    let Some(drawing) = crate::drawing::target_clip(s, &Value::Null) else { return Vec::new() };
    references(seq, drawing).into_iter().filter_map(|(ti, c)| describe(seq, ti, c)).collect()
}
