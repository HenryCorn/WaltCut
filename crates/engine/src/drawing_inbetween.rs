//! In-between assist commands (#72): `drawing.inbetweenPreview`, `drawing.inbetweenAccept` and
//! `drawing.inbetweenCancel`. The rules are in `docs/animation.md`.
//!
//! The pairing of strokes is explicit and only generates cels, so it is never stored in the
//! project. A *preview* is kept in the session (`Session::inbetween`), outside the project and the
//! history: it adds no undo step, does not change the project's revision and is dropped as soon as
//! either of its cels changes. *Accept* is the only command here that edits the drawing, and it
//! is one undo step.

use std::sync::Arc;

use serde_json::{Value, json};
use waltcut_project::drawing::{Cel, Drawing, Stroke};
use waltcut_project::inbetween::{self, Analysis, Easing, MAX_PAIRS, MAX_TOTAL_POINTS, Pair, Problem, Request, Unmatched};
use waltcut_project::{ClipId, ItemId};

use crate::commands::{CommandSpec, always, bad};
use crate::drawing::{drawing_frame_bounds, drawing_of, edit_drawing, has_drawing, target, unlocked_clip};
use crate::{EngineError, Result, Session};

const PREVIEW: &str = "drawing.inbetweenPreview";
const ACCEPT: &str = "drawing.inbetweenAccept";
const CANCEL: &str = "drawing.inbetweenCancel";

/// Why a generated frame was not made.
const CEL_STARTS_HERE: &str = "cel starts here";
const OUTSIDE_CLIP: &str = "outside the clip";
const TOO_MANY_CELS: &str = "the drawing has no room for more cels";

/// What a call asks for, as given.
#[derive(Clone, Debug, PartialEq)]
struct Params {
    from: Option<i64>,
    to: Option<i64>,
    request: Request,
    step: Option<i64>,
    frames: Option<Vec<i64>>,
}

/// What would be made, worked out against the project as it is now.
struct Plan {
    clip: ClipId,
    item: ItemId,
    from: i64,
    to: i64,
    params: Params,
    a: Arc<Vec<Stroke>>,
    b: Arc<Vec<Stroke>>,
    analysis: Analysis,
    /// Frames that will be made.
    frames: Vec<i64>,
    skipped: Vec<(i64, &'static str)>,
}

/// The kept preview: the plan that was shown, and the cels it was worked out from.
#[derive(Clone)]
pub struct Preview {
    pub clip: ClipId,
    item: ItemId,
    /// First and last cel (the frames they start on).
    pub from: i64,
    pub to: i64,
    params: Params,
    a: Arc<Vec<Stroke>>,
    b: Arc<Vec<Stroke>>,
    analysis: Analysis,
    /// The frames that would be made.
    pub frames: Vec<i64>,
}

impl Preview {
    pub fn request(&self) -> &Request {
        &self.params.request
    }

    /// The strokes the in-between on `frame` would have: the same code that accept runs.
    pub fn strokes_at(&self, frame: i64) -> Vec<Stroke> {
        inbetween::generate(&self.a, &self.b, &self.params.request, &self.analysis, inbetween::position(self.from, self.to, frame))
    }
}

impl Session {
    /// The preview, if there is one and its cels are as they were.
    pub fn inbetween_preview(&self) -> Option<&Preview> {
        self.inbetween.as_ref().filter(|p| self.preview_is_current(p))
    }

    fn preview_is_current(&self, pv: &Preview) -> bool {
        let Some(d) = self.project.item(pv.item).and_then(|i| i.as_drawing()) else { return false };
        let same = |frame: i64, strokes: &Arc<Vec<Stroke>>| {
            d.cel_on(frame).and_then(|i| d.cels.get(i)).is_some_and(|c| Arc::ptr_eq(&c.strokes, strokes) || *c.strokes == **strokes)
        };
        same(pv.from, &pv.a) && same(pv.to, &pv.b)
    }

    /// Throw the preview away if either of its cels changed or is gone. Cheap; called whenever the
    /// project changes and before the preview is used.
    pub fn prune_inbetween(&mut self) {
        if self.inbetween.as_ref().is_some_and(|p| !self.preview_is_current(p)) {
            self.inbetween = None;
        }
    }
}

fn int(cmd: &str, v: &Value, what: &str) -> Result<i64> {
    v.as_i64().ok_or_else(|| bad(cmd, format!("`{what}` must be a whole number")))
}

fn opt_int(cmd: &str, p: &Value, key: &str) -> Result<Option<i64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => int(cmd, v, key).map(Some),
    }
}

fn only_keys(cmd: &str, p: &Value, allowed: &[&str]) -> Result<()> {
    match p {
        Value::Null => Ok(()),
        Value::Object(o) => match o.keys().find(|k| !allowed.contains(&k.as_str())) {
            Some(k) => Err(bad(cmd, format!("unknown parameter `{k}`; expected: {}", allowed.join(", ")))),
            None => Ok(()),
        },
        _ => Err(bad(cmd, "parameters must be an object")),
    }
}

fn stroke_number(cmd: &str, v: Option<&Value>, what: &str) -> Result<usize> {
    v.and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| bad(cmd, format!("each pair needs a whole-number `{what}` (a stroke position, counted from 0)")))
}

fn parse_pairs(cmd: &str, v: &Value) -> Result<Vec<Pair>> {
    let list = v.as_array().ok_or_else(|| bad(cmd, "`pairs` must be a list of {from, to, reverse?}"))?;
    if list.len() > MAX_PAIRS {
        return Err(bad(cmd, format!("{} pairs; at most {MAX_PAIRS} per call", list.len())));
    }
    list.iter()
        .map(|item| {
            only_keys(cmd, item, &["from", "to", "reverse"]).and_then(|()| {
                let reverse = match item.get("reverse") {
                    None | Some(Value::Null) => false,
                    Some(Value::Bool(b)) => *b,
                    Some(_) => return Err(bad(cmd, "`reverse` must be true or false")),
                };
                Ok(Pair { from: stroke_number(cmd, item.get("from"), "from")?, to: stroke_number(cmd, item.get("to"), "to")?, reverse })
            })
        })
        .collect()
}

fn parse_params(cmd: &str, p: &Value, extra: &[&str]) -> Result<Params> {
    let mut allowed = vec!["clip", "frame", "from", "to", "pairs", "step", "frames", "easing", "unmatched"];
    allowed.extend_from_slice(extra);
    only_keys(cmd, p, &allowed)?;
    let pairs = parse_pairs(
        cmd,
        p.get("pairs").ok_or_else(|| bad(cmd, "need `pairs`: [{from, to, reverse?}, ...], the strokes to join (see drawing.inspect {strokes: true})"))?,
    )?;
    let easing = match p.get("easing") {
        None | Some(Value::Null) => Easing::Linear,
        Some(v) => v.as_str().and_then(Easing::parse).ok_or_else(|| bad(cmd, "`easing` must be linear, easeIn, easeOut or easeInOut"))?,
    };
    let unmatched = match p.get("unmatched") {
        None | Some(Value::Null) => Unmatched::Drop,
        Some(v) => v.as_str().and_then(Unmatched::parse).ok_or_else(|| bad(cmd, "`unmatched` must be drop or nearest"))?,
    };
    let frames = match p.get("frames") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let list = v.as_array().ok_or_else(|| bad(cmd, "`frames` must be a list of drawing frames"))?;
            if list.len() > inbetween::MAX_FRAMES {
                return Err(bad(cmd, format!("{} frames; at most {} per call", list.len(), inbetween::MAX_FRAMES)));
            }
            Some(list.iter().map(|f| int(cmd, f, "frames")).collect::<Result<Vec<_>>>()?)
        }
    };
    let step = opt_int(cmd, p, "step")?;
    if step.is_some() && frames.is_some() {
        return Err(bad(cmd, "give `step` or `frames`, not both"));
    }
    Ok(Params { from: opt_int(cmd, p, "from")?, to: opt_int(cmd, p, "to")?, request: Request { pairs, unmatched, easing }, step, frames })
}

/// Work out what `params` would make on the drawing clip, without changing anything.
fn make_plan(s: &Session, cmd: &str, p: &Value, params: Params) -> Result<Plan> {
    let (clip, item, frame) = target(s, cmd, p)?;
    plan_for(s, cmd, clip, item, frame, params)
}

fn plan_for(s: &Session, cmd: &str, clip: ClipId, item: ItemId, frame: i64, mut params: Params) -> Result<Plan> {
    unlocked_clip(s, cmd, clip)?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, d) = drawing_of(s, seq, clip).ok_or_else(|| bad(cmd, "not a drawing clip"))?;
    let from = match params.from {
        Some(f) => f,
        None => d.cel_index_at(frame).and_then(|i| d.cels.get(i)).map(|c| c.frame).ok_or_else(|| bad(cmd, "no cel is shown here: give `from`"))?,
    };
    let to = match params.to {
        Some(f) => f,
        None => {
            let i = d.cel_on(from).ok_or_else(|| bad(cmd, format!("no cel starts on frame {from}")))?;
            d.cels.get(i + 1).map(|c| c.frame).ok_or_else(|| bad(cmd, "there is no later cel to move towards: give `to`"))?
        }
    };
    let cel = |f: i64, what: &str| -> Result<&Cel> {
        d.cel_on(f).and_then(|i| d.cels.get(i)).ok_or_else(|| bad(cmd, format!("no cel starts on frame {f} (the {what} cel)")))
    };
    let (a, b) = (cel(from, "start")?.strokes.clone(), cel(to, "end")?.strokes.clone());
    if to <= from {
        return Err(bad(cmd, "the end cel must start after the start cel"));
    }
    params.from = Some(from);
    params.to = Some(to);
    let analysis = inbetween::analyze(&a, &b, &params.request);
    let mut plan = Plan { clip, item, from, to, params, a, b, analysis, frames: Vec::new(), skipped: Vec::new() };
    if !plan.analysis.usable() {
        return Ok(plan);
    }
    let scheduled = inbetween::schedule(from, to, plan.params.step, plan.params.frames.as_deref()).map_err(|e| bad(cmd, e))?;
    let (first, last) = drawing_frame_bounds(s, cmd, clip, d)?;
    let mut room = waltcut_project::drawing::MAX_CELS.saturating_sub(d.cels.len());
    for f in scheduled {
        if d.cel_on(f).is_some() {
            plan.skipped.push((f, CEL_STARTS_HERE));
        } else if f < first || f > last {
            plan.skipped.push((f, OUTSIDE_CLIP));
        } else if room == 0 {
            plan.skipped.push((f, TOO_MANY_CELS));
        } else {
            room -= 1;
            plan.frames.push(f);
        }
    }
    if plan.frames.is_empty() {
        let why = plan.skipped.iter().map(|(f, r)| format!("{f}: {r}")).collect::<Vec<_>>().join(", ");
        return Err(bad(cmd, format!("every in-between frame was skipped ({why}); nothing to make")));
    }
    let per_frame = inbetween::points_per_frame(&plan.a, &plan.b, &plan.params.request, &plan.analysis);
    if per_frame.saturating_mul(plan.frames.len()) > MAX_TOTAL_POINTS {
        return Err(bad(
            cmd,
            format!(
                "{} frames of {per_frame} points is over the limit of {MAX_TOTAL_POINTS} generated points: raise the spacing or pair fewer strokes",
                plan.frames.len()
            ),
        ));
    }
    Ok(plan)
}

fn problem_json(p: &Problem) -> Value {
    let mut v = json!({"kind": p.kind(), "message": p.message()});
    if let Some(o) = v.as_object_mut() {
        if let Some(side) = p.side() {
            o.insert("side".into(), json!(side.name()));
        }
        if let Some(i) = p.index() {
            o.insert("index".into(), json!(i));
        }
    }
    v
}

/// The answer to a preview or an accept: the mapping as used and everything that was noticed.
fn report(plan: &Plan, geometry: bool) -> Value {
    let a = &plan.analysis;
    let req = &plan.params.request;
    let mut v = json!({
        "clip": plan.clip.0,
        "from": plan.from,
        "to": plan.to,
        "status": if a.usable() { "ok" } else { "unsupported" },
        "problems": a.problems.iter().map(problem_json).collect::<Vec<_>>(),
        "pairs": a.pairs.iter().map(|i| json!({
            "from": i.pair.from, "to": i.pair.to, "reverse": i.pair.reverse,
            "pointsFrom": i.points_from, "pointsTo": i.points_to,
            "otherDirectionIsCloser": i.other_direction_is_closer,
        })).collect::<Vec<_>>(),
        "unmatched": {"policy": req.unmatched.name(), "from": a.unmatched_from, "to": a.unmatched_to},
        "fills": {"from": a.fills_from, "to": a.fills_to},
        "easing": req.easing.name(),
        "frames": plan.frames,
        "skipped": plan.skipped.iter().map(|(f, why)| json!({"frame": f, "reason": why})).collect::<Vec<_>>(),
    });
    if geometry && let Some(o) = v.as_object_mut() {
        let cels: Vec<Value> = plan
            .frames
            .iter()
            .map(|&f| {
                let strokes = inbetween::generate(&plan.a, &plan.b, req, a, inbetween::position(plan.from, plan.to, f));
                json!({"frame": f, "strokes": strokes.iter().map(crate::drawing_fill::stroke_json).collect::<Vec<_>>()})
            })
            .collect();
        o.insert("cels".into(), Value::Array(cels));
    }
    v
}

fn keep(plan: &Plan) -> Preview {
    Preview {
        clip: plan.clip,
        item: plan.item,
        from: plan.from,
        to: plan.to,
        params: plan.params.clone(),
        a: plan.a.clone(),
        b: plan.b.clone(),
        analysis: plan.analysis.clone(),
        frames: plan.frames.clone(),
    }
}

/// `drawing.inbetweenPreview`: what the pairing would make. Changes nothing in the project.
fn preview(s: &mut Session, p: &Value) -> Result<Value> {
    s.prune_inbetween();
    let params = parse_params(PREVIEW, p, &["geometry"])?;
    let geometry = match p.get("geometry") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(PREVIEW, "`geometry` must be true or false")),
    };
    let plan = make_plan(s, PREVIEW, p, params)?;
    // an unsupported pairing replaces nothing: the previous preview, if any, is dropped so that the
    // answer and the state agree
    s.inbetween = plan.analysis.usable().then(|| keep(&plan));
    Ok(report(&plan, geometry))
}

/// `drawing.inbetweenAccept`: make the cels. With `pairs` the call is self-contained; without, it
/// accepts the current preview (all its frames, or the `frames` listed).
fn accept(s: &mut Session, p: &Value) -> Result<Value> {
    s.prune_inbetween();
    let inline = p.get("pairs").is_some();
    let (plan, chosen) = if inline {
        let params = parse_params(ACCEPT, p, &[])?;
        (make_plan(s, ACCEPT, p, params)?, None)
    } else {
        only_keys(ACCEPT, p, &["frames"])?;
        let pv = s.inbetween.clone().ok_or_else(|| {
            bad(ACCEPT, "there is no in-between preview (it is dropped when either cel changes): run drawing.inbetweenPreview, or give `pairs`")
        })?;
        let chosen = match p.get("frames") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let list = v.as_array().ok_or_else(|| bad(ACCEPT, "`frames` must be a list of drawing frames"))?;
                Some(list.iter().map(|f| int(ACCEPT, f, "frames")).collect::<Result<Vec<_>>>()?)
            }
        };
        let (clip, item) = (pv.clip, pv.item);
        let plan = plan_for(s, ACCEPT, clip, item, pv.from, pv.params.clone())?;
        (plan, chosen)
    };
    if !plan.analysis.usable() {
        let why = plan.analysis.problems.iter().map(Problem::message).collect::<Vec<_>>().join("; ");
        return Err(bad(ACCEPT, format!("this pairing is not supported: {why}")));
    }
    let frames: Vec<i64> = match chosen {
        None => plan.frames.clone(),
        Some(list) => {
            if list.is_empty() {
                return Err(bad(ACCEPT, "`frames` is empty: nothing to accept"));
            }
            if let Some(f) = list.iter().find(|f| !plan.frames.contains(f)) {
                return Err(bad(ACCEPT, format!("frame {f} is not one of the in-betweens that can be made ({:?})", plan.frames)));
            }
            plan.frames.iter().copied().filter(|f| list.contains(f)).collect()
        }
    };
    let (item, clip) = (plan.item, plan.clip);
    let made = edit_drawing(s, ACCEPT, "Accept In-betweens", item, |d: &mut Drawing| {
        for &f in &frames {
            let strokes = inbetween::generate(&plan.a, &plan.b, &plan.params.request, &plan.analysis, inbetween::position(plan.from, plan.to, f));
            let k = d.ensure_cel(f, false).map_err(|e| bad(ACCEPT, e))?;
            if let Some(c) = d.cels.get_mut(k) {
                *c = Cel { frame: f, strokes: Arc::new(strokes), link: None };
            }
        }
        Ok(frames.clone())
    })?;
    s.inbetween = None;
    let mut out = report(&plan, false);
    if let Some(o) = out.as_object_mut() {
        o.insert("cels".into(), json!(made));
        o.insert("clip".into(), json!(clip.0));
    }
    Ok(out)
}

/// `drawing.inbetweenCancel`: drop the preview. Nothing else changes.
fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    only_keys(CANCEL, p, &[])?;
    Ok(json!({"cancelled": s.inbetween.take().is_some()}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: PREVIEW,
            label: "Preview In-betweens",
            menu: &["Graphics and Titles", "Drawing"],
            shortcut: None,
            params: r#"{"clip":id?,"frame":int?,"from":int? (start cel's frame, default the cel shown),"to":int? (end cel's frame, default the next cel),"pairs":[{"from":stroke,"to":stroke,"reverse":bool?},...],"step":int? | "frames":[int,...],"easing":"linear|easeIn|easeOut|easeInOut"?,"unmatched":"drop|nearest"?,"geometry":bool?}  show what joining those strokes would make, changing nothing in the project; an unsupported pairing says why"#,
            enabled: has_drawing,
            run: preview,
            journal: false,
        },
        CommandSpec {
            id: ACCEPT,
            label: "Accept In-betweens",
            menu: &["Graphics and Titles", "Drawing"],
            shortcut: None,
            params: r#"{"frames":[int,...]?}  make the previewed in-betweens (or only those frames) as new unlinked cels; or, with the preview's own parameters ("pairs" and the rest), make them without a preview; one undo step"#,
            enabled: has_drawing,
            run: accept,
            journal: true,
        },
        CommandSpec {
            id: CANCEL,
            label: "Cancel In-between Preview",
            menu: &["Graphics and Titles", "Drawing"],
            shortcut: None,
            params: "{}  drop the preview; nothing else changes",
            enabled: always,
            run: cancel,
            journal: false,
        },
    ]
}
