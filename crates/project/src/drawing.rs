//! Drawings: frame-by-frame 2D animation drawn over the picture.
//!
//! A drawing is an [`ItemKind::Drawing`](crate::ItemKind) project item: a transparent canvas of the
//! sequence frame size holding **cels**. Each cel starts at a drawing frame and is shown until the
//! next cel starts (its *exposure*: a cel every frame animates on ones, every other frame on twos),
//! so a held pose costs nothing. A cel is a list of [`Stroke`]s in paint order (first = back), each
//! a polyline of pressure samples from a mouse or drawing tablet.
//!
//! A drawing clip places the drawing on a video track like any other source, so Motion, Opacity,
//! blend modes and their keyframes apply. Strokes are plain data: the renderer
//! rasterises them (`waltcut_render::drawing`) and the UI paints its own live preview.

use serde::{Deserialize, Serialize};
use waltcut_time::{FrameRate, Tick};

/// The most samples one stroke may hold (a long, slow scribble at 240 Hz is a few thousand).
pub const MAX_STROKE_POINTS: usize = 20_000;
/// The most strokes one cel may hold.
pub const MAX_CEL_STROKES: usize = 5_000;
/// The most cels one drawing may hold.
pub const MAX_CELS: usize = 20_000;
/// Largest brush diameter in canvas pixels.
pub const MAX_BRUSH_SIZE: f32 = 1_000.0;

/// How a stroke lays down paint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Brush {
    /// Hard round tip; pressure sets the width.
    #[default]
    Pen,
    /// Thin, slightly soft line; pressure sets the opacity as well as the width.
    Pencil,
    /// Wide, flat, translucent tip with a constant width.
    Marker,
    /// Removes paint from the strokes under it in the same cel.
    Eraser,
}

impl Brush {
    pub const ALL: [Brush; 4] = [Brush::Pen, Brush::Pencil, Brush::Marker, Brush::Eraser];

    pub fn label(self) -> &'static str {
        match self {
            Brush::Pen => "Pen",
            Brush::Pencil => "Pencil",
            Brush::Marker => "Marker",
            Brush::Eraser => "Eraser",
        }
    }

    pub fn from_name(s: &str) -> Option<Brush> {
        Brush::ALL.into_iter().find(|b| b.label().eq_ignore_ascii_case(s.trim()))
    }

    /// Radius and opacity factors (0..1) at a pressure sample.
    pub fn response(self, pressure: f32) -> (f32, f32) {
        let p = if pressure.is_finite() { pressure.clamp(0.0, 1.0) } else { 1.0 };
        match self {
            // never vanish entirely: a light touch still leaves a hairline
            Brush::Pen => (0.15 + 0.85 * p, 1.0),
            Brush::Pencil => (0.4 + 0.6 * p, 0.25 + 0.75 * p),
            Brush::Marker => (1.0, 0.6),
            Brush::Eraser => (0.3 + 0.7 * p, 1.0),
        }
    }
}

/// One pointer sample, in drawing canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokePoint {
    pub x: f32,
    pub y: f32,
    /// Pen pressure, 0..1 (1 for a mouse).
    #[serde(default = "full_pressure")]
    pub p: f32,
}

fn full_pressure() -> f32 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    #[serde(default)]
    pub brush: Brush,
    /// Linear-light-agnostic sRGB colour with straight alpha, as `graphic` colours are.
    pub color: [f32; 4],
    /// Diameter at full pressure, canvas pixels.
    pub size: f32,
    pub points: Vec<StrokePoint>,
}

impl Stroke {
    /// Clamp a stroke from an untrusted source (command parameters, a project file) to sane values.
    /// Fails when nothing drawable is left.
    pub fn sanitized(mut self) -> Result<Stroke, String> {
        if !self.size.is_finite() || self.size <= 0.0 {
            return Err("stroke size must be a positive number".into());
        }
        self.size = self.size.min(MAX_BRUSH_SIZE);
        for c in &mut self.color {
            *c = if c.is_finite() { c.clamp(0.0, 1.0) } else { 1.0 };
        }
        self.points.retain(|p| p.x.is_finite() && p.y.is_finite());
        if self.points.is_empty() {
            return Err("a stroke needs at least one point".into());
        }
        if self.points.len() > MAX_STROKE_POINTS {
            return Err(format!("a stroke may hold at most {MAX_STROKE_POINTS} points"));
        }
        for p in &mut self.points {
            p.p = if p.p.is_finite() { p.p.clamp(0.0, 1.0) } else { 1.0 };
        }
        Ok(self)
    }

    /// Bounding box `(min_x, min_y, max_x, max_y)` including the brush radius.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let r = self.size * 0.5 + 1.0;
        let mut it = self.points.iter();
        let first = it.next()?;
        let mut b = (first.x, first.y, first.x, first.y);
        for p in it {
            b = (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y));
        }
        Some((b.0 - r, b.1 - r, b.2 + r, b.3 + r))
    }
}

/// A drawing shown from `frame` until the next cel starts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cel {
    /// First drawing frame (0 = the drawing's start) this cel is shown on.
    pub frame: i64,
    /// Shared, copy-on-write: a clone of the cel, of the drawing, or an undo snapshot holds the
    /// same strokes until one of them is edited ([`Cel::strokes_mut`]), so adding a stroke copies
    /// the strokes of that one cel, not of the whole drawing.
    #[serde(default)]
    pub strokes: std::sync::Arc<Vec<Stroke>>,
}

impl Cel {
    /// The strokes for editing: copies them first when another clone of the cel still shares them.
    pub fn strokes_mut(&mut self) -> &mut Vec<Stroke> {
        std::sync::Arc::make_mut(&mut self.strokes)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub width: u32,
    pub height: u32,
    pub rate: FrameRate,
    /// Sorted by `frame`, no two on the same frame.
    #[serde(default)]
    pub cels: Vec<Cel>,
}

impl Drawing {
    pub fn new(width: u32, height: u32, rate: FrameRate) -> Self {
        Self { width, height, rate, cels: Vec::new() }
    }

    /// The drawing frame shown at drawing time `t`.
    pub fn frame_at(&self, t: Tick) -> i64 {
        if self.rate.num == 0 || self.rate.den == 0 {
            return 0;
        }
        self.rate.frame_at(t)
    }

    /// Index of the cel shown on drawing frame `f` (the last one starting at or before it).
    pub fn cel_index_at(&self, f: i64) -> Option<usize> {
        self.cels.partition_point(|c| c.frame <= f).checked_sub(1)
    }

    pub fn cel_at(&self, f: i64) -> Option<&Cel> {
        self.cel_index_at(f).and_then(|i| self.cels.get(i))
    }

    /// The cel starting exactly on frame `f`.
    pub fn cel_on(&self, f: i64) -> Option<usize> {
        self.cels.binary_search_by_key(&f, |c| c.frame).ok()
    }

    /// Exclusive end of the exposure starting at `f`, if a cel starts there.
    pub fn exposure_end(&self, f: i64, drawing_end: i64) -> Option<i64> {
        self.cel_on(f).map(|i| self.cels.get(i + 1).map_or(drawing_end, |c| c.frame))
    }

    /// Move the cel starting at `from` to `to`, preserving its strokes.
    pub fn retime_cel(&mut self, from: i64, to: i64) -> Result<(), String> {
        if to < 0 {
            return Err("cel frame must not be negative".into());
        }
        let i = self.cel_on(from).ok_or_else(|| format!("no cel starts on frame {from}"))?;
        if from == to {
            return Ok(());
        }
        if self.cel_on(to).is_some() {
            return Err(format!("frame {to} already starts a cel"));
        }
        let cel = self.cels.get_mut(i).ok_or_else(|| "no such cel".to_string())?;
        cel.frame = to;
        self.cels.sort_by_key(|c| c.frame);
        Ok(())
    }

    /// The cel starting on frame `f`, created (empty, or as a copy of the cel shown there when
    /// `copy_held`) if there is none. Returns its index.
    pub fn ensure_cel(&mut self, f: i64, copy_held: bool) -> Result<usize, String> {
        if let Some(i) = self.cel_on(f) {
            return Ok(i);
        }
        if self.cels.len() >= MAX_CELS {
            return Err(format!("a drawing may hold at most {MAX_CELS} cels"));
        }
        let strokes = if copy_held { self.cel_at(f).map(|c| c.strokes.clone()).unwrap_or_default() } else { Default::default() };
        let i = self.cels.partition_point(|c| c.frame < f);
        self.cels.insert(i, Cel { frame: f, strokes });
        Ok(i)
    }

    /// The start frames of the cels before and after the one shown on frame `f`, for onion skins.
    pub fn neighbours(&self, f: i64) -> (Option<&Cel>, Option<&Cel>) {
        match self.cel_index_at(f) {
            Some(i) => (i.checked_sub(1).and_then(|j| self.cels.get(j)), self.cels.get(i + 1)),
            None => (None, self.cels.first()),
        }
    }

    /// Restore the invariants after loading untrusted data: sorted unique cels, sane strokes.
    pub fn sanitize(&mut self) {
        self.cels.truncate(MAX_CELS);
        self.cels.sort_by_key(|c| c.frame);
        self.cels.dedup_by_key(|c| c.frame);
        for c in &mut self.cels {
            let strokes = c.strokes_mut();
            strokes.truncate(MAX_CEL_STROKES);
            *strokes = std::mem::take(strokes).into_iter().filter_map(|s| s.sanitized().ok()).collect();
        }
    }
}

/// Straight in-betweens from cel `a` to cel `b`: stroke `n` of `a` morphs into stroke `n` of `b`
/// (strokes beyond the shorter cel are kept from `a` for the first half, from `b` for the second).
/// `t` in 0..1. Each pair of strokes is resampled to the same number of points by arc length, so
/// strokes drawn with a different number of samples still line up.
pub fn tween(a: &Cel, b: &Cel, t: f32) -> Vec<Stroke> {
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
    let n = a.strokes.len().max(b.strokes.len());
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        match (a.strokes.get(i), b.strokes.get(i)) {
            (Some(sa), Some(sb)) => {
                let k = sa.points.len().max(sb.points.len()).clamp(2, 512);
                let pa = resample(&sa.points, k);
                let pb = resample(&sb.points, k);
                let lerp = |x: f32, y: f32| x + (y - x) * t;
                let points = pa.iter().zip(&pb).map(|(p, q)| StrokePoint { x: lerp(p.x, q.x), y: lerp(p.y, q.y), p: lerp(p.p, q.p) }).collect();
                let mut color = sa.color;
                for (c, d) in color.iter_mut().zip(sb.color) {
                    *c = lerp(*c, d);
                }
                out.push(Stroke { brush: if t < 0.5 { sa.brush } else { sb.brush }, color, size: lerp(sa.size, sb.size), points });
            }
            (Some(s), None) if t < 0.5 => out.push(s.clone()),
            (None, Some(s)) if t >= 0.5 => out.push(s.clone()),
            _ => {}
        }
    }
    out
}

/// `points` resampled to `k` samples evenly spaced along its length.
fn resample(points: &[StrokePoint], k: usize) -> Vec<StrokePoint> {
    let Some(&first) = points.first() else { return Vec::new() };
    if points.len() == 1 || k < 2 {
        return vec![first; k.max(1)];
    }
    let mut cum = Vec::with_capacity(points.len());
    let mut total = 0.0f32;
    cum.push(0.0);
    for w in points.windows(2) {
        if let [p, q] = w {
            total += ((q.x - p.x).powi(2) + (q.y - p.y).powi(2)).sqrt();
        }
        cum.push(total);
    }
    if total <= f32::EPSILON {
        return vec![first; k];
    }
    let mut out = Vec::with_capacity(k);
    let mut j = 0usize;
    for i in 0..k {
        let d = total * i as f32 / (k - 1) as f32;
        while j + 2 < cum.len() && cum.get(j + 1).is_some_and(|&c| c < d) {
            j += 1;
        }
        let (Some(&p), Some(&q), Some(&c0), Some(&c1)) = (points.get(j), points.get(j + 1), cum.get(j), cum.get(j + 1)) else {
            break;
        };
        let f = if c1 - c0 > f32::EPSILON { ((d - c0) / (c1 - c0)).clamp(0.0, 1.0) } else { 0.0 };
        out.push(StrokePoint { x: p.x + (q.x - p.x) * f, y: p.y + (q.y - p.y) * f, p: p.p + (q.p - p.p) * f });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(x0: f32, x1: f32, n: usize) -> Stroke {
        let points = (0..n).map(|i| StrokePoint { x: x0 + (x1 - x0) * i as f32 / (n - 1).max(1) as f32, y: 10.0, p: 1.0 }).collect();
        Stroke { brush: Brush::Pen, color: [1.0, 0.0, 0.0, 1.0], size: 4.0, points }
    }

    #[test]
    fn cels_hold_until_the_next_one() {
        let mut d = Drawing::new(100, 100, FrameRate::new(24, 1));
        assert!(d.cel_at(0).is_none());
        d.ensure_cel(2, false).unwrap();
        d.ensure_cel(6, false).unwrap();
        d.ensure_cel(4, false).unwrap();
        assert_eq!(d.cels.iter().map(|c| c.frame).collect::<Vec<_>>(), vec![2, 4, 6]);
        assert!(d.cel_at(1).is_none());
        assert_eq!(d.cel_at(3).map(|c| c.frame), Some(2));
        assert_eq!(d.cel_at(100).map(|c| c.frame), Some(6));
        let (prev, next) = d.neighbours(4);
        assert_eq!((prev.map(|c| c.frame), next.map(|c| c.frame)), (Some(2), Some(6)));
    }

    #[test]
    fn ensure_cel_can_copy_the_held_drawing() {
        let mut d = Drawing::new(100, 100, FrameRate::new(24, 1));
        let i = d.ensure_cel(0, false).unwrap();
        d.cels[i].strokes_mut().push(line(0.0, 10.0, 3));
        let j = d.ensure_cel(5, true).unwrap();
        assert_eq!(d.cels[j].strokes.len(), 1);
        assert_eq!(d.ensure_cel(5, true).unwrap(), j);
    }

    #[test]
    fn retiming_preserves_content_and_recalculates_exposure() {
        let mut d = Drawing::new(100, 100, FrameRate::new(24, 1));
        let first = d.ensure_cel(2, false).unwrap();
        d.cels[first].strokes.push(line(0.0, 10.0, 3));
        d.ensure_cel(6, false).unwrap();
        assert_eq!(d.exposure_end(2, 12), Some(6));

        d.retime_cel(6, 8).unwrap();

        assert_eq!(d.cels.iter().map(|c| c.frame).collect::<Vec<_>>(), vec![2, 8]);
        assert_eq!(d.exposure_end(2, 12), Some(8));
        assert_eq!(d.cel_at(8).map(|c| c.frame), Some(8));
        assert_eq!(d.cels.first().map(|c| c.strokes.len()), Some(1));
    }

    #[test]
    fn retiming_rejects_missing_colliding_and_negative_frames_without_mutation() {
        let mut d = Drawing::new(100, 100, FrameRate::new(24, 1));
        d.ensure_cel(2, false).unwrap();
        d.ensure_cel(6, false).unwrap();
        let before = d.clone();
        for (from, to) in [(5, 8), (2, 6), (2, -1)] {
            assert!(d.retime_cel(from, to).is_err());
            assert_eq!(d, before);
        }
    }

    #[test]
    fn retimed_frames_round_trip_at_fractional_rates() {
        let mut drawing = Drawing::new(100, 100, FrameRate::new(24_000, 1_001));
        drawing.ensure_cel(3, false).unwrap();
        drawing.ensure_cel(9, false).unwrap();
        drawing.retime_cel(9, 11).unwrap();

        let bytes = serde_json::to_vec(&drawing).unwrap();
        let loaded: Drawing = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(loaded, drawing);
        assert_eq!(loaded.exposure_end(3, 24), Some(11));
        assert_eq!(loaded.rate, FrameRate::new(24_000, 1_001));
    }

    #[test]
    fn hostile_strokes_are_refused_or_clamped() {
        let mut s = line(0.0, 1.0, 2);
        s.size = f32::NAN;
        assert!(s.clone().sanitized().is_err());
        s.size = 1e9;
        s.color = [f32::INFINITY, -3.0, 0.5, 2.0];
        s.points.push(StrokePoint { x: f32::NAN, y: 0.0, p: 7.0 });
        s.points[0].p = f32::NAN;
        let s = s.sanitized().unwrap();
        assert_eq!(s.size, MAX_BRUSH_SIZE);
        assert_eq!(s.color, [1.0, 0.0, 0.5, 1.0]);
        assert_eq!(s.points.len(), 2);
        assert_eq!(s.points[0].p, 1.0);
        let empty = Stroke { points: vec![], ..line(0.0, 1.0, 2) };
        assert!(empty.sanitized().is_err());
    }

    #[test]
    fn sanitize_restores_order_and_uniqueness() {
        let mut d = Drawing::new(10, 10, FrameRate::new(24, 1));
        d.cels = vec![Cel { frame: 5, strokes: vec![].into() }, Cel { frame: 1, strokes: vec![].into() }, Cel { frame: 5, strokes: vec![].into() }];
        d.sanitize();
        assert_eq!(d.cels.iter().map(|c| c.frame).collect::<Vec<_>>(), vec![1, 5]);
    }

    #[test]
    fn tween_halfway_between_two_lines() {
        let a = Cel { frame: 0, strokes: vec![line(0.0, 10.0, 3)].into() };
        let b = Cel { frame: 4, strokes: vec![line(20.0, 30.0, 7)].into() };
        let mid = tween(&a, &b, 0.5);
        assert_eq!(mid.len(), 1);
        let pts = &mid[0].points;
        assert_eq!(pts.len(), 7);
        assert!((pts[0].x - 10.0).abs() < 1e-4);
        assert!((pts[6].x - 20.0).abs() < 1e-4);
        // degenerate input never panics
        assert!(tween(&Cel::default(), &Cel::default(), f32::NAN).is_empty());
        let dot = Cel { frame: 0, strokes: vec![line(5.0, 5.0, 1)].into() };
        assert_eq!(tween(&dot, &b, 0.25).len(), 1);
    }
}
