//! In-between assist (#72): generate the cels between two chosen cels from an *explicit* pairing of
//! whole strokes.
//!
//! Nothing here finds a correspondence or scores one. A [`Request`] names the pairs; [`analyze`]
//! says whether they can be used and reports everything it noticed (strokes left out, fills, a
//! pair whose ends look swapped); [`generate`] builds the strokes of one in-between. The mapping is
//! only used to generate cels, so it is never stored in the project.
//!
//! Every bound is a constant here: pairs, frames per call, points per stroke and points in total.

use crate::drawing::{Brush, Stroke, morph_stroke};

/// Most pairs one request may name.
pub const MAX_PAIRS: usize = 256;
/// Most in-between frames one call may generate.
pub const MAX_FRAMES: usize = 256;
/// Most generated points (all frames and strokes of one call).
pub const MAX_TOTAL_POINTS: usize = 2_000_000;
/// Largest spacing in drawing frames.
pub const MAX_STEP: i64 = 1_000;
/// Most problems [`analyze`] lists (the first ones).
pub const MAX_PROBLEMS: usize = 64;
/// Points per generated stroke (the resampling of [`morph_stroke`]).
pub const MAX_STROKE_POINTS: usize = 512;

/// One whole stroke of the start cel becoming one whole stroke of the end cel. Both numbers are
/// positions in the cel's stroke list when the request is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub from: usize,
    pub to: usize,
    /// The end stroke is run backwards.
    pub reverse: bool,
}

/// What happens to a stroke that is in no pair.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unmatched {
    /// It is left out of the in-betweens.
    #[default]
    Drop,
    /// It is kept from the nearer cel.
    Nearest,
}

impl Unmatched {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "drop" => Some(Self::Drop),
            "nearest" => Some(Self::Nearest),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Drop => "drop",
            Self::Nearest => "nearest",
        }
    }
}

/// How the position of a frame between the two cels is shaped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Easing {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    pub const ALL: [Easing; 4] = [Easing::Linear, Easing::EaseIn, Easing::EaseOut, Easing::EaseInOut];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.name() == s)
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::EaseIn => "easeIn",
            Self::EaseOut => "easeOut",
            Self::EaseInOut => "easeInOut",
        }
    }

    /// `t` in 0..1 (anything else is clamped; NaN is 0) shaped; 0 and 1 stay put.
    pub fn apply(self, t: f64) -> f64 {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => t * (2.0 - t),
            Self::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// An explicit correspondence and how to generate from it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub pairs: Vec<Pair>,
    pub unmatched: Unmatched,
    pub easing: Easing,
}

/// Which of the two cels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The start cel.
    From,
    /// The end cel.
    To,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::From => "from",
            Side::To => "to",
        }
    }
}

/// Why a request cannot be used. Nothing is generated while there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    NoPairs,
    TooManyPairs {
        count: usize,
    },
    BlankCel {
        side: Side,
    },
    OutOfRange {
        side: Side,
        index: usize,
        count: usize,
    },
    RepeatedStroke {
        side: Side,
        index: usize,
    },
    EmptyStroke {
        side: Side,
        index: usize,
    },
    /// An eraser cuts what is under it in its own cel, which does not survive a morph.
    Eraser {
        side: Side,
        index: usize,
    },
    /// A fill is a stored result and is never morphed.
    FillPaired {
        side: Side,
        index: usize,
    },
}

impl Problem {
    pub fn kind(&self) -> &'static str {
        match self {
            Problem::NoPairs => "noPairs",
            Problem::TooManyPairs { .. } => "tooManyPairs",
            Problem::BlankCel { .. } => "blankCel",
            Problem::OutOfRange { .. } => "outOfRange",
            Problem::RepeatedStroke { .. } => "repeatedStroke",
            Problem::EmptyStroke { .. } => "emptyStroke",
            Problem::Eraser { .. } => "eraser",
            Problem::FillPaired { .. } => "fillPaired",
        }
    }

    pub fn side(&self) -> Option<Side> {
        match self {
            Problem::BlankCel { side }
            | Problem::OutOfRange { side, .. }
            | Problem::RepeatedStroke { side, .. }
            | Problem::EmptyStroke { side, .. }
            | Problem::Eraser { side, .. }
            | Problem::FillPaired { side, .. } => Some(*side),
            Problem::NoPairs | Problem::TooManyPairs { .. } => None,
        }
    }

    pub fn index(&self) -> Option<usize> {
        match self {
            Problem::OutOfRange { index, .. }
            | Problem::RepeatedStroke { index, .. }
            | Problem::EmptyStroke { index, .. }
            | Problem::Eraser { index, .. }
            | Problem::FillPaired { index, .. } => Some(*index),
            _ => None,
        }
    }

    pub fn message(&self) -> String {
        let cel = |s: &Side| match s {
            Side::From => "the start cel",
            Side::To => "the end cel",
        };
        match self {
            Problem::NoPairs => "no stroke pairs: name at least one {from, to} pair".into(),
            Problem::TooManyPairs { count } => format!("{count} pairs; at most {MAX_PAIRS} per call"),
            Problem::BlankCel { side } => format!("{} is blank: there is nothing to pair", cel(side)),
            Problem::OutOfRange { side, index, count } => format!("{} has no stroke {index} (it has {count})", cel(side)),
            Problem::RepeatedStroke { side, index } => format!("stroke {index} of {} is in more than one pair", cel(side)),
            Problem::EmptyStroke { side, index } => format!("stroke {index} of {} has no points", cel(side)),
            Problem::Eraser { side, index } => {
                format!("stroke {index} of {} is an eraser; erasers are not supported (clear or redraw that part first)", cel(side))
            }
            Problem::FillPaired { side, index } => format!("stroke {index} of {} is a fill; fills are never morphed and cannot be paired", cel(side)),
        }
    }
}

/// A usable pair with what was measured about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairInfo {
    pub pair: Pair,
    pub points_from: usize,
    pub points_to: usize,
    /// Joining the strokes the other way round (reversing the pair if it is not, un-reversing it if
    /// it is) would put their ends closer together. A measurement, never acted on.
    pub other_direction_is_closer: bool,
}

/// What [`analyze`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Why the request cannot be used; empty when it can.
    pub problems: Vec<Problem>,
    /// The pairs, as given, with measurements (empty when there are problems).
    pub pairs: Vec<PairInfo>,
    /// Strokes of the start / end cel that are in no pair (fills excluded).
    pub unmatched_from: Vec<usize>,
    pub unmatched_to: Vec<usize>,
    /// Fills of either cel: never paired, shown from the nearer cel.
    pub fills_from: Vec<usize>,
    pub fills_to: Vec<usize>,
}

impl Analysis {
    pub fn usable(&self) -> bool {
        self.problems.is_empty()
    }
}

fn gap(p: Option<&crate::drawing::StrokePoint>, q: Option<&crate::drawing::StrokePoint>) -> f64 {
    match (p, q) {
        (Some(p), Some(q)) => f64::from(p.x - q.x).hypot(f64::from(p.y - q.y)),
        _ => 0.0,
    }
}

/// Check `req` against the strokes of the start cel `a` and the end cel `b`. Never panics, never
/// invents a pair.
pub fn analyze(a: &[Stroke], b: &[Stroke], req: &Request) -> Analysis {
    let mut out = Analysis::default();
    let mut problems: Vec<Problem> = Vec::new();
    let mut note = |p: Problem| {
        if problems.len() < MAX_PROBLEMS {
            problems.push(p);
        }
    };
    if a.is_empty() {
        note(Problem::BlankCel { side: Side::From });
    }
    if b.is_empty() {
        note(Problem::BlankCel { side: Side::To });
    }
    if req.pairs.is_empty() {
        note(Problem::NoPairs);
    }
    if req.pairs.len() > MAX_PAIRS {
        note(Problem::TooManyPairs { count: req.pairs.len() });
    }
    for (side, strokes) in [(Side::From, a), (Side::To, b)] {
        for (index, s) in strokes.iter().enumerate() {
            if s.brush == Brush::Eraser {
                note(Problem::Eraser { side, index });
            }
        }
    }
    let mut used_a = vec![false; a.len()];
    let mut used_b = vec![false; b.len()];
    for pair in req.pairs.iter().take(MAX_PAIRS) {
        for (side, strokes, used, index) in [(Side::From, a, &mut used_a, pair.from), (Side::To, b, &mut used_b, pair.to)] {
            let Some(stroke) = strokes.get(index) else {
                if !strokes.is_empty() {
                    note(Problem::OutOfRange { side, index, count: strokes.len() });
                }
                continue;
            };
            if used.get(index).copied().unwrap_or(false) {
                note(Problem::RepeatedStroke { side, index });
            }
            if let Some(u) = used.get_mut(index) {
                *u = true;
            }
            if stroke.is_fill() {
                note(Problem::FillPaired { side, index });
            } else if stroke.brush != Brush::Eraser && stroke.points.is_empty() {
                note(Problem::EmptyStroke { side, index });
            }
        }
    }
    out.problems = problems;
    for (i, s) in a.iter().enumerate() {
        if s.is_fill() {
            out.fills_from.push(i);
        } else if !used_a.get(i).copied().unwrap_or(false) {
            out.unmatched_from.push(i);
        }
    }
    for (i, s) in b.iter().enumerate() {
        if s.is_fill() {
            out.fills_to.push(i);
        } else if !used_b.get(i).copied().unwrap_or(false) {
            out.unmatched_to.push(i);
        }
    }
    if out.problems.is_empty() {
        for pair in &req.pairs {
            let (Some(sa), Some(sb)) = (a.get(pair.from), b.get(pair.to)) else { continue };
            let same = gap(sa.points.first(), sb.points.first()) + gap(sa.points.last(), sb.points.last());
            let flipped = gap(sa.points.first(), sb.points.last()) + gap(sa.points.last(), sb.points.first());
            let (given, other) = if pair.reverse { (flipped, same) } else { (same, flipped) };
            out.pairs.push(PairInfo { pair: *pair, points_from: sa.points.len(), points_to: sb.points.len(), other_direction_is_closer: other < given });
        }
    }
    out
}

/// The strokes of the in-between at `t` (0..1 between the cels, before easing). Call it only with a
/// usable [`Analysis`] of the same strokes and request: it returns nothing otherwise.
///
/// The order is the nearer cel's (after easing): a pair is placed where its stroke stands in that
/// cel, a fill of that cel stays, and an unpaired stroke of that cel stays only with
/// [`Unmatched::Nearest`].
pub fn generate(a: &[Stroke], b: &[Stroke], req: &Request, analysis: &Analysis, t: f64) -> Vec<Stroke> {
    if !analysis.usable() {
        return Vec::new();
    }
    let u = req.easing.apply(t);
    let from_a = u < 0.5;
    let own: &[Stroke] = if from_a { a } else { b };
    let mut paired: Vec<Option<&Pair>> = vec![None; own.len()];
    for p in &req.pairs {
        let i = if from_a { p.from } else { p.to };
        if let Some(slot) = paired.get_mut(i) {
            *slot = Some(p);
        }
    }
    let mut out = Vec::with_capacity(own.len());
    for (i, s) in own.iter().enumerate() {
        if s.is_fill() {
            out.push(s.clone());
        } else if let Some(p) = paired.get(i).copied().flatten() {
            if let (Some(sa), Some(sb)) = (a.get(p.from), b.get(p.to)) {
                out.push(morph_stroke(sa, sb, p.reverse, u as f32));
            }
        } else if req.unmatched == Unmatched::Nearest {
            out.push(s.clone());
        }
    }
    out
}

/// The points one generated frame holds, the larger of what either cel's order would give.
pub fn points_per_frame(a: &[Stroke], b: &[Stroke], req: &Request, analysis: &Analysis) -> usize {
    let morphed: usize = analysis.pairs.iter().map(|p| p.points_from.max(p.points_to).clamp(2, MAX_STROKE_POINTS)).sum();
    let kept = |strokes: &[Stroke], unmatched: &[usize], fills: &[usize]| -> usize {
        let pick = |idx: &[usize]| idx.iter().filter_map(|&i| strokes.get(i)).map(|s| s.points.len().max(1)).fold(0usize, usize::saturating_add);
        let extra = if req.unmatched == Unmatched::Nearest { pick(unmatched) } else { 0 };
        extra.saturating_add(pick(fills))
    };
    let side_a = kept(a, &analysis.unmatched_from, &analysis.fills_from);
    let side_b = kept(b, &analysis.unmatched_to, &analysis.fills_to);
    morphed.saturating_add(side_a.max(side_b))
}

/// The drawing frames to generate strictly between `start` and `end`: one every `step`
/// (1 to [`MAX_STEP`], default 2) after `start`, or exactly `frames` (sorted, without repeats).
pub fn schedule(start: i64, end: i64, step: Option<i64>, frames: Option<&[i64]>) -> Result<Vec<i64>, String> {
    if end <= start {
        return Err("the end cel must start after the start cel".into());
    }
    if let Some(list) = frames {
        let mut v: Vec<i64> = list.to_vec();
        if let Some(f) = v.iter().find(|&&f| f <= start || f >= end) {
            return Err(format!("frame {f} is not between the cels (frames {start} and {end})"));
        }
        v.sort_unstable();
        v.dedup();
        if v.is_empty() {
            return Err("no frames to generate".into());
        }
        if v.len() > MAX_FRAMES {
            return Err(format!("{} frames; at most {MAX_FRAMES} per call", v.len()));
        }
        return Ok(v);
    }
    let step = step.unwrap_or(2);
    if !(1..=MAX_STEP).contains(&step) {
        return Err(format!("step must be between 1 and {MAX_STEP} frames"));
    }
    // widened so a hostile pair of frames cannot overflow
    let span = i128::from(end) - i128::from(start);
    let count = (span - 1) / i128::from(step);
    if count > MAX_FRAMES as i128 {
        return Err(format!("{count} in-betweens on {step}s between frames {start} and {end}; at most {MAX_FRAMES} per call: raise the spacing"));
    }
    if count == 0 {
        return Err(format!("no room for in-betweens on {step}s between frames {start} and {end}"));
    }
    Ok((1..=count as i64).map(|n| start + n * step).collect())
}

/// Where `frame` stands between the cels, 0..1, from exact frame numbers.
pub fn position(start: i64, end: i64, frame: i64) -> f64 {
    let span = i128::from(end) - i128::from(start);
    if span <= 0 {
        return 0.0;
    }
    let at = (i128::from(frame) - i128::from(start)).clamp(0, span);
    at as f64 / span as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::StrokePoint;

    fn stroke(brush: Brush, pts: &[(f32, f32)]) -> Stroke {
        Stroke {
            brush,
            color: [1.0, 0.0, 0.0, 1.0],
            size: 4.0,
            points: pts.iter().map(|&(x, y)| StrokePoint { x, y, p: 1.0 }).collect(),
            response: None,
            fill: None,
        }
    }
    fn line(x0: f32, x1: f32, y: f32) -> Stroke {
        stroke(Brush::Pen, &[(x0, y), (x1, y)])
    }
    fn pair(from: usize, to: usize, reverse: bool) -> Pair {
        Pair { from, to, reverse }
    }
    fn req(pairs: Vec<Pair>) -> Request {
        Request { pairs, ..Default::default() }
    }

    #[test]
    fn a_reversed_pair_untwists_the_stroke() {
        let a = [line(0.0, 10.0, 0.0)];
        let b = [stroke(Brush::Pen, &[(10.0, 20.0), (0.0, 20.0)])];
        let r = req(vec![pair(0, 0, false)]);
        let an = analyze(&a, &b, &r);
        assert!(an.usable());
        // as given the ends are far apart and flipping would bring them closer: reported, not applied
        assert!(an.pairs[0].other_direction_is_closer);
        let mid = generate(&a, &b, &r, &an, 0.5);
        assert_eq!(mid[0].points.first().map(|p| p.x), Some(5.0), "the twisted morph pulls the start towards the far end");
        let fixed = req(vec![pair(0, 0, true)]);
        let an2 = analyze(&a, &b, &fixed);
        assert!(!an2.pairs[0].other_direction_is_closer);
        let mid = generate(&a, &b, &fixed, &an2, 0.5);
        let xs: Vec<f32> = mid[0].points.iter().map(|p| p.x).collect();
        assert!(xs[0].abs() < 1e-4 && (xs[xs.len() - 1] - 10.0).abs() < 1e-4, "{xs:?}");
    }

    #[test]
    fn the_pairing_is_the_users_not_the_list_order() {
        let a = [line(0.0, 10.0, 0.0), line(0.0, 10.0, 50.0)];
        let b = [line(0.0, 10.0, 60.0), line(0.0, 10.0, 10.0)];
        // a0 -> b1 and a1 -> b0, the opposite of by position
        let r = req(vec![pair(0, 1, false), pair(1, 0, false)]);
        let an = analyze(&a, &b, &r);
        assert!(an.usable() && an.unmatched_from.is_empty() && an.unmatched_to.is_empty());
        let mid = generate(&a, &b, &r, &an, 0.25);
        // nearer the start cel: its order, a0 stays near y 0 -> 10 (a quarter of the way)
        assert!((mid[0].points[0].y - 2.5).abs() < 1e-4 && (mid[1].points[0].y - 52.5).abs() < 1e-4);
        let late = generate(&a, &b, &r, &an, 0.75);
        // nearer the end cel: its order (b0 first), b0 is the end of a1
        assert!((late[0].points[0].y - (50.0 + 0.75 * 10.0)).abs() < 1e-4, "{:?}", late[0].points[0]);
    }

    #[test]
    fn missing_strokes_are_reported_and_dropped_or_kept_as_asked() {
        let a = [line(0.0, 10.0, 0.0), line(0.0, 10.0, 20.0)];
        let b = [line(0.0, 10.0, 5.0), line(0.0, 10.0, 99.0), line(0.0, 10.0, 98.0)];
        let mut r = req(vec![pair(0, 0, false)]);
        let an = analyze(&a, &b, &r);
        assert_eq!((an.unmatched_from.clone(), an.unmatched_to.clone()), (vec![1], vec![1, 2]));
        assert_eq!(generate(&a, &b, &r, &an, 0.25).len(), 1, "unmatched strokes are dropped by default");
        assert_eq!(generate(&a, &b, &r, &an, 0.75).len(), 1);
        r.unmatched = Unmatched::Nearest;
        assert_eq!(generate(&a, &b, &r, &an, 0.25).len(), 2, "the start cel's extra stroke stays before the middle");
        assert_eq!(generate(&a, &b, &r, &an, 0.75).len(), 3, "the end cel's extra strokes from the middle on");
    }

    #[test]
    fn unsupported_and_bad_requests_are_explained_not_guessed() {
        let a = [line(0.0, 10.0, 0.0), stroke(Brush::Eraser, &[(1.0, 1.0), (2.0, 2.0)])];
        let b = [line(0.0, 10.0, 5.0)];
        let an = analyze(&a, &b, &req(vec![pair(0, 0, false)]));
        assert!(!an.usable() && an.problems == vec![Problem::Eraser { side: Side::From, index: 1 }]);
        assert!(an.pairs.is_empty() && generate(&a, &b, &req(vec![pair(0, 0, false)]), &an, 0.5).is_empty());

        let a = [line(0.0, 1.0, 0.0)];
        let kinds = |r: Request, a: &[Stroke], b: &[Stroke]| analyze(a, b, &r).problems.iter().map(|p| p.kind()).collect::<Vec<_>>();
        assert_eq!(kinds(req(vec![]), &a, &a), ["noPairs"]);
        assert_eq!(kinds(req(vec![pair(0, 0, false)]), &a, &[]), ["blankCel"]);
        assert_eq!(kinds(req(vec![pair(0, 7, false)]), &a, &a), ["outOfRange"]);
        assert_eq!(kinds(req(vec![pair(0, 0, false), pair(0, 0, true)]), &a, &a), ["repeatedStroke", "repeatedStroke"]);
        let mut empty = line(0.0, 1.0, 0.0);
        empty.points.clear();
        assert_eq!(kinds(req(vec![pair(0, 0, false)]), &[empty], &a), ["emptyStroke"]);
        let fill = Stroke { brush: Brush::Fill, ..line(0.0, 1.0, 0.0) };
        let an = analyze(&[fill.clone(), line(0.0, 1.0, 0.0)], &a, &req(vec![pair(0, 0, false)]));
        assert_eq!(an.problems, vec![Problem::FillPaired { side: Side::From, index: 0 }]);
        // fills that are not paired are listed and follow the nearer cel
        let r = req(vec![pair(1, 0, false)]);
        let an = analyze(&[fill, line(0.0, 1.0, 0.0)], &a, &r);
        assert!(an.usable() && an.fills_from == vec![0]);
        let out = generate(&[Stroke { brush: Brush::Fill, ..line(0.0, 1.0, 0.0) }, line(0.0, 1.0, 0.0)], &a, &r, &an, 0.25);
        assert!(out[0].is_fill() && !out[1].is_fill());
    }

    #[test]
    fn hostile_requests_are_bounded() {
        let a = [line(0.0, 1.0, 0.0)];
        let many = req((0..MAX_PAIRS + 5).map(|i| pair(i, i, false)).collect());
        let an = analyze(&a, &a, &many);
        assert!(an.problems.iter().any(|p| p.kind() == "tooManyPairs") && an.problems.len() <= MAX_PROBLEMS);
        let huge = req(vec![pair(usize::MAX, usize::MAX, false)]);
        assert_eq!(analyze(&a, &a, &huge).problems.len(), 2);
        // NaN positions and easing never escape 0..1
        for e in Easing::ALL {
            for t in [f64::NAN, -1.0, 0.0, 0.3, 1.0, 9.0, f64::INFINITY] {
                let v = e.apply(t);
                assert!((0.0..=1.0).contains(&v), "{e:?} {t} {v}");
            }
            assert_eq!((e.apply(0.0), e.apply(1.0)), (0.0, 1.0));
        }
        assert_eq!(position(0, i64::MAX, i64::MIN), 0.0);
        assert!((position(i64::MIN, i64::MAX, 0) - 0.5).abs() < 1e-9);
        assert_eq!(position(5, 5, 5), 0.0);
    }

    #[test]
    fn frames_are_exact_and_bounded() {
        assert_eq!(schedule(0, 8, Some(2), None).unwrap(), [2, 4, 6]);
        assert_eq!(schedule(0, 8, None, None).unwrap(), [2, 4, 6]);
        assert_eq!(schedule(0, 1, Some(1), None), Err("no room for in-betweens on 1s between frames 0 and 1".into()));
        assert_eq!(schedule(3, 9, Some(1), None).unwrap(), [4, 5, 6, 7, 8]);
        assert_eq!(schedule(0, 10, Some(4), None).unwrap(), [4, 8]);
        assert_eq!(schedule(0, 10, None, Some(&[7, 3, 3])).unwrap(), [3, 7]);
        assert!(schedule(0, 10, None, Some(&[0])).is_err() && schedule(0, 10, None, Some(&[10])).is_err() && schedule(0, 10, None, Some(&[])).is_err());
        assert!(schedule(10, 0, Some(1), None).is_err() && schedule(0, 10, Some(0), None).is_err() && schedule(0, 10, Some(-3), None).is_err());
        assert!(schedule(0, 10, Some(MAX_STEP + 1), None).is_err());
        assert!(schedule(0, MAX_FRAMES as i64 + 1, Some(1), None).is_ok());
        assert!(schedule(0, MAX_FRAMES as i64 + 2, Some(1), None).is_err());
        assert!(schedule(i64::MIN, i64::MAX, Some(1), None).is_err());
        assert!(schedule(i64::MIN, i64::MAX, Some(MAX_STEP), None).is_err());
        let big: Vec<i64> = (1..=MAX_FRAMES as i64 + 1).collect();
        assert!(schedule(0, 10_000, None, Some(&big)).is_err());
    }
}
