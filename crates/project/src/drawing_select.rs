//! Lasso selection and whole-stroke transforms of a cel (#66).
//!
//! A cel is a list of strokes in paint order; an **eraser** stroke removes the paint of the strokes
//! *before* it in the list where it passes. These rules decide what a selection and a transform may
//! do so that moving paint never silently reveals or removes anything:
//!
//! * **Hit test.** A lasso is a polygon in canvas pixels (3 to [`MAX_LASSO_POINTS`] points, closed
//!   automatically, even-odd rule). A paint stroke is hit when **more than half of its points** are
//!   inside it. Eraser strokes are never hit directly.
//! * **Ownership.** An eraser that comes after a selected stroke and whose bounds overlap it
//!   *belongs* to the selection and is transformed with it, so the erased gap goes with the paint.
//!   When such an eraser's bounds also overlap an unselected paint stroke before it, it crosses
//!   both: the selection is **ambiguous** and a transform is refused (`conflict`) until those strokes
//!   are selected too. A fill (#68) is never hit, never selectable and never moves: it is a saved
//!   result. For these rules it is unselected paint with the bounds of its region. Erasers that
//!   touch no selected stroke stay where they are.
//! * **Landing.** A transform is refused when, after it, a moved eraser would overlap an
//!   unselected paint stroke before it (it would erase that stroke), or moved paint would land under
//!   an unselected eraser after it (it would be erased). Order in the cel never changes.
//! * **Numbers.** Every coordinate and brush size after the transform must be finite and within
//!   ±[`MAX_COORD`] and the brush limits, else nothing changes. The scale is uniform and scales the
//!   brush size too; colour, brush and pressure are untouched.
//!
//! Overlap tests use stroke bounds including the brush radius: conservative, so a refusal may be
//! stricter than the pixels strictly need, never looser.

use crate::drawing::{Brush, MAX_BRUSH_SIZE, MAX_CEL_STROKES, Stroke};

/// Most vertices a lasso may have.
pub const MAX_LASSO_POINTS: usize = 10_000;
/// Largest |coordinate| a transformed point may have, in canvas pixels.
pub const MAX_COORD: f32 = 1.0e6;
/// Smallest brush size a scaled stroke may end up with.
pub const MIN_BRUSH_SIZE: f32 = 0.05;
/// Largest uniform scale (and its reciprocal as the smallest) a transform accepts.
pub const MAX_SCALE: f64 = 100.0;

/// A similarity transform of canvas pixels: scale and rotate about `pivot`, then move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub dx: f64,
    pub dy: f64,
    /// Degrees, clockwise on screen (y down).
    pub rotation: f64,
    pub scale: f64,
    pub pivot: (f64, f64),
}

impl Transform {
    pub fn identity() -> Self {
        Self { dx: 0.0, dy: 0.0, rotation: 0.0, scale: 1.0, pivot: (0.0, 0.0) }
    }

    pub fn is_identity(&self) -> bool {
        self.dx == 0.0 && self.dy == 0.0 && self.rotation == 0.0 && self.scale == 1.0
    }

    /// Reject numbers that cannot make a sane transform.
    pub fn check(&self) -> Result<(), String> {
        let finite = [self.dx, self.dy, self.rotation, self.scale, self.pivot.0, self.pivot.1].iter().all(|v| v.is_finite());
        if !finite {
            return Err("the transform must be made of finite numbers".into());
        }
        if !(1.0 / MAX_SCALE..=MAX_SCALE).contains(&self.scale) {
            return Err(format!("scale must be between {} and {MAX_SCALE}", 1.0 / MAX_SCALE));
        }
        if self.rotation.abs() > 36_000.0 || self.dx.abs() > f64::from(MAX_COORD) || self.dy.abs() > f64::from(MAX_COORD) {
            return Err("the move or rotation is out of range".into());
        }
        Ok(())
    }

    fn point(&self, x: f32, y: f32) -> (f64, f64) {
        let (s, c) = self.rotation.to_radians().sin_cos();
        let (px, py) = (f64::from(x) - self.pivot.0, f64::from(y) - self.pivot.1);
        (self.pivot.0 + self.scale * (c * px - s * py) + self.dx, self.pivot.1 + self.scale * (s * px + c * py) + self.dy)
    }
}

/// What a selection covers.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Selection {
    /// The selected paint strokes, ascending.
    pub paint: Vec<usize>,
    /// The erasers that belong to them and move with them, ascending.
    pub erasers: Vec<usize>,
    /// Why a transform would be refused (an eraser crosses selected and unselected strokes).
    pub conflict: Option<String>,
}

type Bounds = (f32, f32, f32, f32);

fn overlap(a: Bounds, b: Bounds) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

fn is_eraser(s: &Stroke) -> bool {
    s.brush == Brush::Eraser
}

/// Even-odd point-in-polygon.
fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut hit = false;
    let mut j = poly.len().saturating_sub(1);
    for (i, a) in poly.iter().enumerate() {
        let b = poly.get(j).copied().unwrap_or(*a);
        if (a.1 > y) != (b.1 > y) {
            let t = (y - a.1) / (b.1 - a.1);
            if x < a.0 + t * (b.0 - a.0) {
                hit = !hit;
            }
        }
        j = i;
    }
    hit
}

/// The paint strokes a lasso hits (more than half of a stroke's points inside), ascending.
pub fn lasso_hits(strokes: &[Stroke], lasso: &[(f32, f32)]) -> Result<Vec<usize>, String> {
    if lasso.len() < 3 {
        return Err("a lasso needs at least 3 points".into());
    }
    if lasso.len() > MAX_LASSO_POINTS {
        return Err(format!("a lasso may have at most {MAX_LASSO_POINTS} points"));
    }
    if lasso.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
        return Err("the lasso must be made of finite points".into());
    }
    let mut out = Vec::new();
    for (i, s) in strokes.iter().enumerate().take(MAX_CEL_STROKES) {
        if is_eraser(s) || s.points.is_empty() {
            continue;
        }
        let n = s.points.iter().filter(|p| inside(lasso, p.x, p.y)).count();
        if n * 2 > s.points.len() {
            out.push(i);
        }
    }
    Ok(out)
}

/// The selection for the paint strokes `selected` (any order, no repeats): the erasers that belong
/// to it and whether it is ambiguous. Erasers cannot be selected.
pub fn plan_selection(strokes: &[Stroke], selected: &[usize]) -> Result<Selection, String> {
    let mut paint = selected.to_vec();
    paint.sort_unstable();
    if paint.windows(2).any(|w| w[0] == w[1]) {
        return Err("a stroke is selected twice".into());
    }
    if paint.is_empty() {
        return Err("select at least one stroke".into());
    }
    for &i in &paint {
        let s = strokes.get(i).ok_or_else(|| format!("no stroke {i} in this cel"))?;
        if is_eraser(s) {
            return Err(format!("stroke {i} is an eraser: erasers move with the paint they erase and cannot be selected"));
        }
        if s.is_fill() {
            return Err(format!("stroke {i} is a fill: a fill is a saved result and is not moved with strokes (redo it with drawing.fillEdit, or remove it)"));
        }
    }
    let bounds: Vec<Option<Bounds>> = strokes.iter().map(Stroke::bounds).collect();
    let mut erasers = Vec::new();
    let mut conflict = None;
    for (e, s) in strokes.iter().enumerate() {
        let (true, Some(eb)) = (is_eraser(s), bounds.get(e).copied().flatten()) else { continue };
        let touches = paint.iter().filter(|&&p| p < e).any(|&p| bounds.get(p).copied().flatten().is_some_and(|pb| overlap(eb, pb)));
        if !touches {
            continue;
        }
        let shared = strokes
            .iter()
            .enumerate()
            .take(e)
            .find(|(u, o)| !is_eraser(o) && paint.binary_search(u).is_err() && bounds.get(*u).copied().flatten().is_some_and(|ub| overlap(eb, ub)));
        if let Some((u, _)) = shared
            && conflict.is_none()
        {
            let fix = if strokes.get(u).is_some_and(Stroke::is_fill) { "remove that fill first (drawing.fillRemove)" } else { "select it too" };
            conflict = Some(format!("eraser stroke {e} crosses the selection and stroke {u}: {fix}, or erase again"));
        }
        erasers.push(e);
    }
    Ok(Selection { paint, erasers, conflict })
}

/// The centre of the selected paint's bounds: the default pivot.
pub fn selection_centre(strokes: &[Stroke], sel: &Selection) -> Option<(f64, f64)> {
    let mut b: Option<Bounds> = None;
    for &i in &sel.paint {
        let Some(sb) = strokes.get(i).and_then(Stroke::bounds) else { continue };
        b = Some(match b {
            None => sb,
            Some(a) => (a.0.min(sb.0), a.1.min(sb.1), a.2.max(sb.2), a.3.max(sb.3)),
        });
    }
    b.map(|b| (f64::from(b.0 + b.2) / 2.0, f64::from(b.1 + b.3) / 2.0))
}

/// `strokes` with the selection transformed: all or nothing. `selected` are paint strokes
/// (see [`plan_selection`]); their owned erasers move too. Returns the new strokes and the plan.
pub fn transform_selection(strokes: &[Stroke], selected: &[usize], t: &Transform) -> Result<(Vec<Stroke>, Selection), String> {
    t.check()?;
    let sel = plan_selection(strokes, selected)?;
    if let Some(c) = &sel.conflict {
        return Err(c.clone());
    }
    let mut moved: Vec<usize> = sel.paint.iter().chain(&sel.erasers).copied().collect();
    moved.sort_unstable();
    let mut out = strokes.to_vec();
    for &i in &moved {
        let s = out.get_mut(i).ok_or("no such stroke")?;
        let size = f64::from(s.size) * t.scale;
        if !(f64::from(MIN_BRUSH_SIZE)..=f64::from(MAX_BRUSH_SIZE)).contains(&size) {
            return Err(format!("the brush size of stroke {i} would become {size:.2}: it must stay between {MIN_BRUSH_SIZE} and {MAX_BRUSH_SIZE}"));
        }
        s.size = size as f32;
        for p in &mut s.points {
            let (x, y) = t.point(p.x, p.y);
            if !(x.is_finite() && y.is_finite()) || x.abs() > f64::from(MAX_COORD) || y.abs() > f64::from(MAX_COORD) {
                return Err(format!("stroke {i} would leave the allowed range (±{MAX_COORD} pixels)"));
            }
            p.x = x as f32;
            p.y = y as f32;
        }
    }
    // where it lands: a moved eraser must not reach unselected paint before it, and moved paint
    // must not end up under an unselected eraser after it
    let nb: Vec<Option<Bounds>> = out.iter().map(Stroke::bounds).collect();
    let is_moved = |i: usize| moved.binary_search(&i).is_ok();
    for &e in &sel.erasers {
        let Some(eb) = nb.get(e).copied().flatten() else { continue };
        for (u, o) in out.iter().enumerate().take(e) {
            if !is_eraser(o) && !is_moved(u) && nb.get(u).copied().flatten().is_some_and(|ub| overlap(eb, ub)) {
                return Err(format!("moving there would make the eraser stroke {e} erase stroke {u}"));
            }
        }
    }
    for &p in &sel.paint {
        let Some(pb) = nb.get(p).copied().flatten() else { continue };
        for (e, o) in out.iter().enumerate().skip(p + 1) {
            if is_eraser(o) && !is_moved(e) && nb.get(e).copied().flatten().is_some_and(|eb| overlap(pb, eb)) {
                return Err(format!("moving there would put stroke {p} under the eraser stroke {e}"));
            }
        }
    }
    Ok((out, sel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::StrokePoint;

    fn stroke(brush: Brush, pts: &[(f32, f32)], size: f32) -> Stroke {
        Stroke {
            brush,
            color: [0.0, 0.0, 0.0, 1.0],
            size,
            points: pts.iter().map(|&(x, y)| StrokePoint { x, y, p: 1.0 }).collect(),
            response: None,
            fill: None,
        }
    }
    fn pen(pts: &[(f32, f32)]) -> Stroke {
        stroke(Brush::Pen, pts, 4.0)
    }
    fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(f32, f32)> {
        vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    }

    #[test]
    fn a_stroke_is_hit_when_more_than_half_its_points_are_inside() {
        let strokes = vec![
            pen(&[(10.0, 10.0), (20.0, 10.0), (30.0, 10.0)]),                 // inside
            pen(&[(10.0, 10.0), (200.0, 10.0), (300.0, 10.0)]),               // 1 of 3 inside
            pen(&[(10.0, 10.0), (20.0, 10.0), (300.0, 10.0), (400.0, 10.0)]), // exactly half: not hit
            stroke(Brush::Eraser, &[(10.0, 10.0), (20.0, 10.0)], 8.0),        // never hit
            pen(&[]),
        ];
        let lasso = square(0.0, 0.0, 100.0, 50.0);
        assert_eq!(lasso_hits(&strokes, &lasso).unwrap(), vec![0]);
        // the lasso may run either way round and is closed automatically
        let mut rev = lasso.clone();
        rev.reverse();
        assert_eq!(lasso_hits(&strokes, &rev).unwrap(), vec![0]);
        // a concave lasso respects its notch
        let c = vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (50.0, 20.0), (0.0, 100.0)];
        let s = vec![pen(&[(50.0, 60.0), (50.0, 70.0)]), pen(&[(10.0, 10.0), (90.0, 10.0)])];
        assert_eq!(lasso_hits(&s, &c).unwrap(), vec![1], "the point in the notch is outside");
    }

    #[test]
    fn bad_lassos_are_refused() {
        let s = vec![pen(&[(1.0, 1.0)])];
        assert!(lasso_hits(&s, &[(0.0, 0.0), (1.0, 1.0)]).is_err());
        assert!(lasso_hits(&s, &[(0.0, 0.0), (f32::NAN, 1.0), (1.0, 5.0)]).is_err());
        assert!(lasso_hits(&s, &vec![(0.0, 0.0); MAX_LASSO_POINTS + 1]).is_err());
        // a degenerate (collinear) lasso hits nothing
        assert_eq!(lasso_hits(&s, &[(0.0, 0.0), (5.0, 5.0), (10.0, 10.0)]).unwrap(), Vec::<usize>::new());
    }

    #[test]
    fn an_eraser_over_only_the_selection_belongs_to_it() {
        let strokes = vec![
            pen(&[(10.0, 10.0), (90.0, 10.0)]),                          // 0 selected
            pen(&[(10.0, 200.0), (90.0, 200.0)]),                        // 1 far away, unselected
            stroke(Brush::Eraser, &[(50.0, 0.0), (50.0, 20.0)], 6.0),    // 2 erases a gap in 0
            stroke(Brush::Eraser, &[(50.0, 190.0), (50.0, 210.0)], 6.0), // 3 erases 1
        ];
        let sel = plan_selection(&strokes, &[0]).unwrap();
        assert_eq!((sel.paint, sel.erasers, sel.conflict), (vec![0], vec![2], None));
        // an eraser before the stroke does not touch it
        let before = vec![stroke(Brush::Eraser, &[(50.0, 0.0), (50.0, 20.0)], 6.0), pen(&[(10.0, 10.0), (90.0, 10.0)])];
        assert!(plan_selection(&before, &[1]).unwrap().erasers.is_empty());
    }

    #[test]
    fn an_eraser_across_selected_and_unselected_paint_is_ambiguous() {
        let strokes = vec![pen(&[(10.0, 10.0), (90.0, 10.0)]), pen(&[(10.0, 20.0), (90.0, 20.0)]), stroke(Brush::Eraser, &[(50.0, 0.0), (50.0, 30.0)], 6.0)];
        let sel = plan_selection(&strokes, &[0]).unwrap();
        assert_eq!(sel.erasers, vec![2]);
        assert!(sel.conflict.as_deref().is_some_and(|c| c.contains("crosses") && c.contains('1')), "{:?}", sel.conflict);
        assert!(transform_selection(&strokes, &[0], &Transform { dx: 5.0, ..Transform::identity() }).is_err());
        // selecting both is fine
        let both = plan_selection(&strokes, &[0, 1]).unwrap();
        assert_eq!((both.erasers, both.conflict), (vec![2], None));
    }

    #[test]
    fn selections_refuse_erasers_repeats_and_missing_strokes() {
        let strokes = vec![pen(&[(0.0, 0.0)]), stroke(Brush::Eraser, &[(0.0, 0.0)], 4.0)];
        for bad in [vec![], vec![1], vec![0, 0], vec![5]] {
            assert!(plan_selection(&strokes, &bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_transform_moves_rotates_and_scales_paint_and_its_eraser_together() {
        let strokes = vec![pen(&[(10.0, 10.0), (30.0, 10.0)]), stroke(Brush::Eraser, &[(20.0, 5.0), (20.0, 15.0)], 4.0)];
        // rotate 90° clockwise about (20, 10), scale 2, then move by (100, 0)
        let t = Transform { dx: 100.0, dy: 0.0, rotation: 90.0, scale: 2.0, pivot: (20.0, 10.0) };
        let (out, sel) = transform_selection(&strokes, &[0], &t).unwrap();
        assert_eq!(sel.erasers, vec![1]);
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        // (10,10) is 10 left of the pivot → 20 above it after the quarter turn… clockwise on screen: left → up
        assert!(close(out[0].points[0].x, 120.0) && close(out[0].points[0].y, -10.0), "{:?}", out[0].points[0]);
        assert!(close(out[0].points[1].x, 120.0) && close(out[0].points[1].y, 30.0), "{:?}", out[0].points[1]);
        assert_eq!((out[0].size, out[1].size), (8.0, 8.0), "the brush scales with the strokes");
        // colour, brush and pressure are untouched; the input is not modified
        assert_eq!((out[0].color, out[0].brush, out[0].points[0].p), (strokes[0].color, Brush::Pen, 1.0));
        assert_eq!(strokes[0].points[0].x, 10.0);
        // the identity changes nothing
        let (same, _) = transform_selection(&strokes, &[0], &Transform { pivot: (20.0, 10.0), ..Transform::identity() }).unwrap();
        assert_eq!(same, strokes);
    }

    #[test]
    fn landing_on_other_paint_or_under_other_erasers_is_refused() {
        // the eraser moves with stroke 0 and would land on stroke 1
        let strokes = vec![pen(&[(10.0, 10.0), (90.0, 10.0)]), pen(&[(10.0, 100.0), (90.0, 100.0)]), stroke(Brush::Eraser, &[(50.0, 0.0), (50.0, 20.0)], 6.0)];
        let down = Transform { dy: 90.0, ..Transform::identity() };
        let e = transform_selection(&strokes, &[0], &down).unwrap_err();
        assert!(e.contains("erase stroke 1"), "{e}");
        // moved paint must not land under an unselected eraser that comes later
        let strokes = vec![
            pen(&[(10.0, 10.0), (90.0, 10.0)]),
            stroke(Brush::Eraser, &[(50.0, 195.0), (50.0, 215.0)], 6.0), // erases nothing now
        ];
        let e = transform_selection(&strokes, &[0], &Transform { dy: 195.0, ..Transform::identity() }).unwrap_err();
        assert!(e.contains("under the eraser"), "{e}");
        // somewhere free is fine
        assert!(transform_selection(&strokes, &[0], &Transform { dx: 300.0, ..Transform::identity() }).is_ok());
    }

    #[test]
    fn extreme_and_hostile_transforms_change_nothing() {
        let strokes = vec![pen(&[(10.0, 10.0), (30.0, 10.0)])];
        let id = Transform::identity();
        for t in [
            Transform { scale: 0.0, ..id },
            Transform { scale: -1.0, ..id },
            Transform { scale: 1e9, ..id },
            Transform { scale: f64::NAN, ..id },
            Transform { rotation: f64::INFINITY, ..id },
            Transform { dx: 1e12, ..id },
            Transform { dx: f64::NAN, ..id },
            Transform { pivot: (f64::NAN, 0.0), ..id },
            Transform { scale: 50.0, dx: 999_000.0, ..id }, // leaves ±MAX_COORD
        ] {
            assert!(transform_selection(&strokes, &[0], &t).is_err(), "{t:?}");
        }
        // a brush that would get too large or too small
        let big = vec![stroke(Brush::Pen, &[(0.0, 0.0)], 900.0)];
        assert!(transform_selection(&big, &[0], &Transform { scale: 2.0, ..id }).is_err());
        let small = vec![stroke(Brush::Pen, &[(0.0, 0.0)], 1.0)];
        assert!(transform_selection(&small, &[0], &Transform { scale: 0.01, ..id }).is_err());
    }

    #[test]
    fn the_centre_is_the_middle_of_the_selected_bounds() {
        let strokes = vec![pen(&[(0.0, 0.0), (10.0, 10.0)]), pen(&[(100.0, 0.0)]), pen(&[(500.0, 500.0)])];
        let sel = plan_selection(&strokes, &[0, 1]).unwrap();
        let (cx, cy) = selection_centre(&strokes, &sel).unwrap();
        assert!((cx - 50.0).abs() < 1e-3 && (cy - 5.0).abs() < 1e-3, "{cx} {cy}");
    }

    #[test]
    fn a_fill_is_never_selected_or_moved_and_counts_as_unselected_paint_for_erasers() {
        use crate::drawing_fill::Fill;
        let fill = Stroke::fill_op([1.0, 0.0, 0.0, 1.0], Fill::encode([40, 5], 0, [30, 0, 40, 20], &[1u8; 800]).unwrap());
        let strokes = vec![pen(&[(10.0, 10.0), (90.0, 10.0)]), fill, stroke(Brush::Eraser, &[(50.0, 0.0), (50.0, 20.0)], 6.0)];
        // a lasso around everything hits only the pen stroke
        let around = [(0.0, 0.0), (200.0, 0.0), (200.0, 200.0), (0.0, 200.0)];
        assert_eq!(lasso_hits(&strokes, &around).unwrap(), vec![0]);
        // asking for the fill by number is refused
        let e = plan_selection(&strokes, &[1]).unwrap_err();
        assert!(e.contains("fill"), "{e}");
        // the eraser crosses the selected pen stroke and the fill's region: ambiguous, and the way out names the fill
        let sel = plan_selection(&strokes, &[0]).unwrap();
        let why = sel.conflict.unwrap();
        assert!(why.contains("stroke 1") && why.contains("remove that fill"), "{why}");
        assert!(transform_selection(&strokes, &[0], &Transform { dx: 5.0, ..Transform::identity() }).is_err());
        // with no eraser in the way the pen stroke moves and the fill stays exactly as it was
        let calm = vec![strokes[0].clone(), strokes[1].clone()];
        let (moved, _) = transform_selection(&calm, &[0], &Transform { dx: 5.0, ..Transform::identity() }).unwrap();
        assert_eq!(moved[1], calm[1]);
        assert_eq!(moved[0].points[0].x, 15.0);
    }
}
