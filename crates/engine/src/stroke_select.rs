//! Lasso selection and whole-stroke transforms of the cel shown (#66). The rules (hit test, which
//! erasers move with the paint, what a transform may do) are in
//! [`waltcut_project::drawing_select`].
//!
//! * `drawing.lassoSelect {clip?, frame?, lasso:[[x,y],…]}` (read only) names the strokes a lasso
//!   hits in the cel shown on `frame`, the erasers that belong to them, whether the selection is
//!   ambiguous (`conflict`) and its centre. Coordinates are canvas pixels.
//! * `drawing.transformStrokes {clip?, frame?, strokes:[i,…] | lasso:[[x,y],…], dx?, dy?, rotation?,
//!   scale?, pivot?:[x,y], dryRun?}` moves, rotates (degrees, clockwise) and uniformly scales those
//!   strokes (the brush size scales with them) about `pivot` (default: the selection's centre),
//!   then moves by `dx`, `dy`. It is **one undo step** and changes nothing when refused: a locked
//!   drawing track, a missing cel, a bad selection, a transform that is out of range or that
//!   would land paint under an eraser (or an eraser on other paint). `dryRun: true` reports what
//!   it would do and edits nothing. Clip Motion is never touched: positions are in the drawing's
//!   own canvas.

use serde_json::{Value, json};
use waltcut_project::drawing::MAX_CEL_STROKES;
use waltcut_project::drawing_select::{Transform, lasso_hits, plan_selection, selection_centre, transform_selection};

use crate::commands::{CommandSpec, bad};
use crate::drawing::{edit_drawing, has_drawing, target};
use crate::{EngineError, Result, Session};

fn lasso_p(cmd: &str, v: &Value) -> Result<Vec<(f32, f32)>> {
    let a = v.as_array().ok_or_else(|| bad(cmd, "`lasso` must be a list of [x, y] points"))?;
    a.iter()
        .map(|p| {
            let xy = p.as_array().filter(|x| x.len() == 2).and_then(|x| Some((x[0].as_f64()?, x[1].as_f64()?)));
            xy.filter(|(x, y)| x.is_finite() && y.is_finite() && x.abs() <= 1.0e9 && y.abs() <= 1.0e9).map(|(x, y)| (x as f32, y as f32))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| bad(cmd, "`lasso` must be a list of [x, y] points of finite numbers"))
}

fn num(cmd: &str, p: &Value, key: &str, default: f64) -> Result<f64> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_f64().filter(|n| n.is_finite()).ok_or_else(|| bad(cmd, format!("`{key}` must be a number"))),
    }
}

pub(crate) fn check_keys(cmd: &str, p: &Value, known: &[&str]) -> Result<()> {
    match p {
        Value::Null => Ok(()),
        Value::Object(o) => match o.keys().find(|k| !known.contains(&k.as_str())) {
            Some(k) => Err(bad(cmd, format!("unknown parameter `{k}`; expected: {}", known.join(", ")))),
            None => Ok(()),
        },
        _ => Err(bad(cmd, "parameters must be an object")),
    }
}

/// The strokes of the cel shown on the target frame, with the cel's start frame.
fn cel_strokes(s: &Session, cmd: &str, p: &Value) -> Result<(waltcut_project::ClipId, i64, Vec<waltcut_project::drawing::Stroke>)> {
    let (clip, item, frame) = target(s, cmd, p)?;
    let d = s.project.item(item).and_then(|i| i.as_drawing()).ok_or_else(|| bad(cmd, "not a drawing"))?;
    let cel = d.cel_at(frame).ok_or_else(|| bad(cmd, "no cel is shown on this frame"))?;
    Ok((clip, cel.frame, cel.strokes.to_vec()))
}

fn lasso_select(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.lassoSelect";
    check_keys(ID, p, &["clip", "frame", "time", "lasso"])?;
    let lasso = lasso_p(ID, p.get("lasso").ok_or_else(|| bad(ID, "need `lasso`"))?)?;
    let (clip, cel, strokes) = cel_strokes(s, ID, p)?;
    let hits = lasso_hits(&strokes, &lasso).map_err(|e| bad(ID, e))?;
    if hits.is_empty() {
        return Ok(json!({"clip": clip.0, "cel": cel, "strokes": [], "erasers": [], "conflict": null, "centre": null}));
    }
    let sel = plan_selection(&strokes, &hits).map_err(|e| bad(ID, e))?;
    let centre = selection_centre(&strokes, &sel).map(|(x, y)| json!([x, y]));
    Ok(json!({"clip": clip.0, "cel": cel, "strokes": sel.paint, "erasers": sel.erasers, "conflict": sel.conflict, "centre": centre}))
}

fn transform_strokes(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "drawing.transformStrokes";
    check_keys(ID, p, &["clip", "frame", "time", "strokes", "lasso", "dx", "dy", "rotation", "scale", "pivot", "dryRun"])?;
    let dry = match p.get("dryRun") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(ID, "`dryRun` must be true or false")),
    };
    let (clip, item, frame) = target(s, ID, p)?;
    // a locked track refuses edits to what is on it
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (tid, _) = seq.find_item(clip).ok_or_else(|| bad(ID, "no such clip"))?;
    if seq.track(tid).is_none_or(|t| t.locked) {
        return Err(bad(ID, "the drawing's track is locked: unlock it first"));
    }
    let (_, cel_frame, strokes) = cel_strokes(s, ID, p)?;
    let selected: Vec<usize> = match (p.get("strokes"), p.get("lasso")) {
        (Some(v), None) => {
            let a = v.as_array().filter(|a| a.len() <= MAX_CEL_STROKES).ok_or_else(|| bad(ID, "`strokes` must be a list of stroke numbers"))?;
            a.iter()
                .map(|x| x.as_u64().and_then(|n| usize::try_from(n).ok()))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| bad(ID, "`strokes` must be whole numbers"))?
        }
        (None, Some(v)) => lasso_hits(&strokes, &lasso_p(ID, v)?).map_err(|e| bad(ID, e))?,
        _ => return Err(bad(ID, "give exactly one of `strokes` (numbers) and `lasso` (points)")),
    };
    let plan = plan_selection(&strokes, &selected).map_err(|e| bad(ID, e))?;
    let centre = selection_centre(&strokes, &plan).ok_or_else(|| bad(ID, "the selection has no drawable stroke"))?;
    let pivot = match p.get("pivot") {
        None | Some(Value::Null) => centre,
        Some(v) => {
            let xy = v.as_array().filter(|a| a.len() == 2).and_then(|a| Some((a[0].as_f64()?, a[1].as_f64()?)));
            xy.filter(|(x, y)| x.is_finite() && y.is_finite()).ok_or_else(|| bad(ID, "`pivot` must be [x, y]"))?
        }
    };
    let t =
        Transform { dx: num(ID, p, "dx", 0.0)?, dy: num(ID, p, "dy", 0.0)?, rotation: num(ID, p, "rotation", 0.0)?, scale: num(ID, p, "scale", 1.0)?, pivot };
    let (moved, sel) = transform_selection(&strokes, &plan.paint, &t).map_err(|e| bad(ID, e))?;
    let report = json!({"clip": clip.0, "cel": cel_frame, "strokes": sel.paint, "erasers": sel.erasers, "pivot": [pivot.0, pivot.1], "applied": !dry && !t.is_identity()});
    if dry || t.is_identity() {
        return Ok(report);
    }
    edit_drawing(s, ID, "Transform Strokes", item, |d| {
        let i = d.cel_index_at(frame).ok_or_else(|| bad(ID, "no cel is shown on this frame"))?;
        let cel = d.cels.get(i).ok_or_else(|| bad(ID, "no such cel"))?;
        // the cel changed since it was read only if another edit slipped in: both ran on this thread
        if cel.frame != cel_frame || cel.strokes.len() != moved.len() {
            return Err(bad(ID, "the cel changed"));
        }
        // a linked cel's peers move with it
        d.edit_strokes(i, |v| *v = moved).ok_or_else(|| bad(ID, "no such cel"))?;
        Ok(())
    })?;
    Ok(report)
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "drawing.lassoSelect",
            label: "Lasso Select Strokes",
            menu: &[],
            shortcut: None,
            params: r#"{"clip":id?,"frame":int?,"lasso":[[x,y],...]}  the strokes a lasso hits in the cel shown, the erasers that belong to them and whether the selection is ambiguous; read only"#,
            enabled: has_drawing,
            run: lasso_select,
            journal: false,
        },
        CommandSpec {
            id: "drawing.transformStrokes",
            label: "Transform Strokes",
            menu: &[],
            shortcut: None,
            params: r#"{"clip":id?,"frame":int?,"strokes":[int]|"lasso":[[x,y],...],"dx":px?,"dy":px?,"rotation":deg?,"scale":f?,"pivot":[x,y]?,"dryRun":bool?}  move, rotate and uniformly scale whole strokes of the cel shown; one undo step, nothing changes when refused"#,
            enabled: has_drawing,
            run: transform_strokes,
            journal: true,
        },
    ]
}
