//! Region fill commands (#68): `drawing.fill`, `drawing.fillEdit` and `drawing.fillRemove`.
//!
//! A fill is one operation in the cel's paint order (see `docs/animation.md`, Fill a region): the
//! flood fill is `waltcut_render::drawing_fill`, the stored result `waltcut_project::drawing_fill`.
//! Each command is **one undo step** and changes nothing, history included, when refused: a locked
//! track, no cel, a seed outside the canvas, a canvas or region over the limits, a bad parameter, a
//! fill that is cancelled or runs past its time budget.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use waltcut_project::ClipId;
use waltcut_project::drawing::{MAX_CEL_STROKES, Stroke};
use waltcut_project::drawing_fill::{Fill, MAX_CEL_FILL_RUNS, MAX_DRAWING_FILL_RUNS};
use waltcut_render::drawing_fill::{FillError, compute_fill};

use crate::commands::{CommandSpec, bad};
use crate::drawing::{cel_mode, edit_drawing, has_drawing, parse_color, target, target_clip};
use crate::stroke_select::check_keys;
use crate::{EngineError, Result, Session};

/// How long a fill may run before it is stopped.
pub const FILL_BUDGET: Duration = Duration::from_secs(5);
/// The tolerance a fill uses when none is given.
pub const DEFAULT_TOLERANCE: u8 = 32;

/// A stroke as `drawing.inspect {strokes: true}` shows it: a fill's mask is summarised, not listed.
pub(crate) fn stroke_json(s: &Stroke) -> Value {
    match &s.fill {
        Some(f) => json!({
            "brush": "Fill",
            "color": s.color,
            "fill": {"seed": f.seed, "tolerance": f.tolerance, "rect": f.rect, "runs": f.runs.len(), "pixels": f.filled_pixels()},
        }),
        None => serde_json::to_value(s).unwrap_or(Value::Null),
    }
}

/// Refuse a drawing on a locked track.
fn unlocked(s: &Session, cmd: &str, clip: ClipId) -> Result<()> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (track, _) = seq.find_item(clip).ok_or_else(|| bad(cmd, "no such clip"))?;
    if seq.track(track).is_none_or(|t| t.locked) {
        return Err(bad(cmd, "the drawing's track is locked: unlock it first"));
    }
    Ok(())
}

/// The seed: `seed: [x, y]` (whole canvas pixels) or `point: [x, y]` (canvas coordinates, the pixel
/// they fall in). A point that cannot be a pixel of any canvas is refused here, one outside this
/// canvas by the fill.
fn seed_p(cmd: &str, p: &Value) -> Result<[u32; 2]> {
    let pair = |key: &str| -> Result<Option<(f64, f64)>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_array()
                .filter(|a| a.len() == 2)
                .and_then(|a| Some((a[0].as_f64()?, a[1].as_f64()?)))
                .filter(|(x, y)| x.is_finite() && y.is_finite())
                .map(Some)
                .ok_or_else(|| bad(cmd, format!("`{key}` must be [x, y] of finite numbers"))),
        }
    };
    let (x, y) = match (pair("seed")?, pair("point")?) {
        (Some(s), None) if s.0.fract() == 0.0 && s.1.fract() == 0.0 => s,
        (Some(_), None) => return Err(bad(cmd, "`seed` is a whole pixel; use `point` for canvas coordinates")),
        (None, Some((x, y))) => (x.floor(), y.floor()),
        _ => return Err(bad(cmd, "give exactly one of `seed` ([x, y] pixel) and `point` ([x, y] canvas coordinates)")),
    };
    let ok = |v: f64| (0.0..f64::from(u32::MAX)).contains(&v);
    if !ok(x) || !ok(y) {
        return Err(bad(cmd, format!("the point ({x}, {y}) is outside the canvas")));
    }
    Ok([x as u32, y as u32])
}

fn tolerance_p(cmd: &str, p: &Value) -> Result<Option<u8>> {
    match p.get("tolerance") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().and_then(|t| u8::try_from(t).ok()).map(Some).ok_or_else(|| bad(cmd, "`tolerance` is a whole number from 0 to 255")),
    }
}

fn index_p(cmd: &str, p: &Value) -> Result<usize> {
    p.get("index")
        .and_then(Value::as_u64)
        .and_then(|i| usize::try_from(i).ok())
        .ok_or_else(|| bad(cmd, "need `index`: the fill's position in the cel (see drawing.inspect {strokes: true})"))
}

/// A fill colour: a visible colour, clamped to 0..1.
fn fill_color(cmd: &str, v: Option<&Value>) -> Result<[f32; 4]> {
    let mut c = parse_color(cmd, v)?;
    if c.iter().any(|x| !x.is_finite()) {
        return Err(bad(cmd, "`color` must be made of finite numbers"));
    }
    for x in &mut c {
        *x = x.clamp(0.0, 1.0);
    }
    if c[3] <= 0.0 {
        return Err(bad(cmd, "a fully transparent fill changes nothing: use a visible colour (or the eraser)"));
    }
    Ok(c)
}

fn refusal(cmd: &str, e: FillError) -> EngineError {
    match e {
        FillError::Cancelled => bad(cmd, "the fill was cancelled or took too long; nothing was changed"),
        FillError::Refused(why) => bad(cmd, why),
    }
}

fn runs_of(strokes: &[Stroke]) -> usize {
    strokes.iter().filter_map(|s| s.fill.as_deref()).map(|f| f.runs.len()).sum()
}

fn report(clip: ClipId, cel: i64, index: usize, color: [f32; 4], fill: &Fill, changed: bool) -> Value {
    let [x, y, w, h] = fill.rect;
    json!({
        "clip": clip.0, "cel": cel, "index": index, "changed": changed, "color": color, "seed": fill.seed, "tolerance": fill.tolerance,
        "rect": {"x": x, "y": y, "w": w, "h": h}, "pixels": fill.filled_pixels(), "runs": fill.runs.len(),
    })
}

/// `drawing.fill`, with a callback that can stop it (a flag in a caller that has one; the time
/// budget always applies).
pub(crate) fn fill_with(s: &mut Session, p: &Value, cancel: &dyn Fn() -> bool) -> Result<Value> {
    const ID: &str = "drawing.fill";
    check_keys(ID, p, &["clip", "frame", "time", "point", "seed", "tolerance", "color", "cel"])?;
    let (clip, item, frame) = target(s, ID, p)?;
    unlocked(s, ID, clip)?;
    let seed = seed_p(ID, p)?;
    let tolerance = tolerance_p(ID, p)?.unwrap_or(DEFAULT_TOLERANCE);
    let color = fill_color(ID, p.get("color"))?;
    let (new_cel, copy) = cel_mode(ID, p)?;
    let d = s.project.item(item).and_then(|i| i.as_drawing()).ok_or_else(|| bad(ID, "not a drawing"))?;
    // the paint the fill goes after: the cel it will be added to, which is the one shown, or on a
    // new cel the one already starting on this frame; failing that a copy of the cel held, or nothing
    let existing = if new_cel { d.cel_on(frame) } else { d.cel_index_at(frame) }.and_then(|i| d.cels.get(i));
    let base: Arc<Vec<Stroke>> = match (existing, copy) {
        (Some(c), _) => c.strokes.clone(),
        (None, true) => d.cel_at(frame).map(|c| c.strokes.clone()).unwrap_or_default(),
        (None, false) => Arc::default(),
    };
    let creating = existing.is_none();
    if base.len() >= MAX_CEL_STROKES {
        return Err(bad(ID, format!("a cel may hold at most {MAX_CEL_STROKES} strokes")));
    }
    let started = Instant::now();
    let stop = || cancel() || started.elapsed() > FILL_BUDGET;
    let (fill, _) = compute_fill(&base, d.width, d.height, seed, tolerance, &stop).map_err(|e| refusal(ID, e))?;
    let runs = fill.runs.len();
    let cel_runs = runs_of(&base);
    if cel_runs + runs > MAX_CEL_FILL_RUNS {
        return Err(bad(ID, format!("this cel's fills would hold {} runs and a cel holds at most {MAX_CEL_FILL_RUNS}", cel_runs + runs)));
    }
    let drawing_runs = d.fill_runs() + if creating && copy { cel_runs } else { 0 };
    if drawing_runs + runs > MAX_DRAWING_FILL_RUNS {
        return Err(bad(ID, format!("this drawing's fills would hold {} runs and a drawing holds at most {MAX_DRAWING_FILL_RUNS}", drawing_runs + runs)));
    }
    let shown = fill.clone();
    let (cel_frame, index) = edit_drawing(s, ID, "Fill Region", item, |d| {
        let i = match d.cel_index_at(frame) {
            Some(i) if !new_cel => i,
            _ => d.ensure_cel(frame, copy).map_err(|e| bad(ID, e))?,
        };
        let cel = d.cels.get_mut(i).ok_or_else(|| bad(ID, "no such cel"))?;
        if cel.strokes.len() >= MAX_CEL_STROKES {
            return Err(bad(ID, format!("a cel may hold at most {MAX_CEL_STROKES} strokes")));
        }
        cel.strokes_mut().push(Stroke::fill_op(color, fill));
        Ok((cel.frame, cel.strokes.len() - 1))
    })?;
    Ok(report(clip, cel_frame, index, color, &shown, true))
}

fn fill_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    fill_with(s, p, &|| false)
}

/// `drawing.fillEdit {index, color?, tolerance?}`.
pub(crate) fn fill_edit_with(s: &mut Session, p: &Value, cancel: &dyn Fn() -> bool) -> Result<Value> {
    const ID: &str = "drawing.fillEdit";
    check_keys(ID, p, &["clip", "frame", "time", "index", "color", "tolerance"])?;
    let (clip, item, frame) = target(s, ID, p)?;
    unlocked(s, ID, clip)?;
    let index = index_p(ID, p)?;
    let new_tolerance = tolerance_p(ID, p)?;
    let has_color = !matches!(p.get("color"), None | Some(Value::Null));
    if new_tolerance.is_none() && !has_color {
        return Err(bad(ID, "give `color`, `tolerance` or both"));
    }
    let d = s.project.item(item).and_then(|i| i.as_drawing()).ok_or_else(|| bad(ID, "not a drawing"))?;
    let ci = d.cel_index_at(frame).ok_or_else(|| bad(ID, "no cel is shown on this frame"))?;
    let cel = d.cels.get(ci).ok_or_else(|| bad(ID, "no such cel"))?;
    let (old, old_fill) =
        cel.strokes.get(index).and_then(|st| st.fill.clone().map(|f| (st, f))).ok_or_else(|| bad(ID, format!("stroke {index} of this cel is not a fill")))?;
    let color = if has_color { fill_color(ID, p.get("color"))? } else { old.color };
    let tolerance = new_tolerance.unwrap_or(old_fill.tolerance);
    if color == old.color && tolerance == old_fill.tolerance {
        return Ok(report(clip, cel.frame, index, color, &old_fill, false));
    }
    let fill = if tolerance == old_fill.tolerance {
        old_fill.clone()
    } else {
        // the region again, from the same seed, against the paint before this fill as it is now
        let before = cel.strokes.get(..index).ok_or_else(|| bad(ID, "no such stroke"))?;
        let started = Instant::now();
        let stop = || cancel() || started.elapsed() > FILL_BUDGET;
        Arc::new(compute_fill(before, d.width, d.height, old_fill.seed, tolerance, &stop).map_err(|e| refusal(ID, e))?.0)
    };
    let cel_runs = (runs_of(&cel.strokes).saturating_sub(old_fill.runs.len())) + fill.runs.len();
    if cel_runs > MAX_CEL_FILL_RUNS {
        return Err(bad(ID, format!("this cel's fills would hold {cel_runs} runs and a cel holds at most {MAX_CEL_FILL_RUNS}")));
    }
    let drawing_runs = d.fill_runs().saturating_sub(old_fill.runs.len()) + fill.runs.len();
    if drawing_runs > MAX_DRAWING_FILL_RUNS {
        return Err(bad(ID, format!("this drawing's fills would hold {drawing_runs} runs and a drawing holds at most {MAX_DRAWING_FILL_RUNS}")));
    }
    let (cel_frame, shown) = (cel.frame, fill.clone());
    edit_drawing(s, ID, "Edit Fill", item, |d| {
        let cel = d.cels.get_mut(ci).ok_or_else(|| bad(ID, "no such cel"))?;
        let slot = cel.strokes_mut().get_mut(index).ok_or_else(|| bad(ID, "no such stroke"))?;
        slot.color = color;
        slot.fill = Some(fill);
        Ok(())
    })?;
    Ok(report(clip, cel_frame, index, color, &shown, true))
}

fn fill_edit_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    fill_edit_with(s, p, &|| false)
}

fn fill_remove(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.fillRemove";
    check_keys(ID, p, &["clip", "frame", "time", "index"])?;
    let (clip, item, frame) = target(s, ID, p)?;
    unlocked(s, ID, clip)?;
    let index = index_p(ID, p)?;
    let d = s.project.item(item).and_then(|i| i.as_drawing()).ok_or_else(|| bad(ID, "not a drawing"))?;
    let ci = d.cel_index_at(frame).ok_or_else(|| bad(ID, "no cel is shown on this frame"))?;
    let cel = d.cels.get(ci).ok_or_else(|| bad(ID, "no such cel"))?;
    if !cel.strokes.get(index).is_some_and(Stroke::is_fill) {
        return Err(bad(ID, format!("stroke {index} of this cel is not a fill")));
    }
    let cel_frame = cel.frame;
    edit_drawing(s, ID, "Remove Fill", item, |d| {
        let cel = d.cels.get_mut(ci).ok_or_else(|| bad(ID, "no such cel"))?;
        let strokes = cel.strokes_mut();
        if index >= strokes.len() {
            return Err(bad(ID, "no such stroke"));
        }
        strokes.remove(index);
        Ok(())
    })?;
    Ok(json!({"clip": clip.0, "cel": cel_frame, "removed": index}))
}

/// A drawing that can be edited: one is targeted and its track is not locked.
fn can_edit_drawing(s: &Session) -> std::result::Result<(), String> {
    has_drawing(s)?;
    let seq = s.active_sequence().ok_or("no sequence")?;
    let clip = target_clip(s, &Value::Null).ok_or("no drawing clip")?;
    let (track, _) = seq.find_item(clip).ok_or("no such clip")?;
    if seq.track(track).is_none_or(|t| t.locked) {
        return Err("the drawing's track is locked".into());
    }
    Ok(())
}

fn spec(id: &'static str, label: &'static str, params: &'static str, run: fn(&mut Session, &Value) -> Result<Value>) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: can_edit_drawing, run, journal: true }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec(
            "drawing.fill",
            "Fill Region",
            r##"{"clip":id?,"frame":int?,"point":[x,y]|"seed":[px,py],"tolerance":0-255?,"color":"#rrggbb"|[r,g,b,a]?,"cel":"hold|new|copy"?}  fill the contiguous region around the point on the cel shown (canvas pixels); one undo step"##,
            fill_cmd,
        ),
        spec(
            "drawing.fillEdit",
            "Edit Fill",
            r##"{"clip":id?,"frame":int?,"index":int,"color":"#rrggbb"|[r,g,b,a]?,"tolerance":0-255?}  recolour a fill of the cel shown, or find its region again from the same seed against the paint before it"##,
            fill_edit_cmd,
        ),
        spec("drawing.fillRemove", "Remove Fill", r#"{"clip":id?,"frame":int?,"index":int}  remove a fill from the cel shown"#, fill_remove),
    ]
}
