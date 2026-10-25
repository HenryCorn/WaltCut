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

pub use crate::drawing_fill::Fill;
use crate::drawing_fill::{MAX_CEL_FILL_RUNS, MAX_DRAWING_FILL_RUNS};

/// The most samples one stroke may hold (a long, slow scribble at 240 Hz is a few thousand).
pub const MAX_STROKE_POINTS: usize = 20_000;
/// The most strokes one cel may hold.
pub const MAX_CEL_STROKES: usize = 5_000;
/// The most cels one drawing may hold.
pub const MAX_CELS: usize = 20_000;
/// The widest spacing `respace_cels` accepts, in frames.
pub const MAX_SPACING: i64 = 96;
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
    /// A stored region fill (#68): carries a [`Fill`] and no points. Not a pen you draw with, so it
    /// is not in [`Brush::ALL`] and `drawing.addStroke` does not take it.
    Fill,
}

impl Brush {
    /// The four brushes a stroke is drawn with (a fill is made by `drawing.fill`).
    pub const ALL: [Brush; 4] = [Brush::Pen, Brush::Pencil, Brush::Marker, Brush::Eraser];

    pub fn label(self) -> &'static str {
        match self {
            Brush::Pen => "Pen",
            Brush::Pencil => "Pencil",
            Brush::Marker => "Marker",
            Brush::Eraser => "Eraser",
            Brush::Fill => "Fill",
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
            Brush::Fill => (1.0, 1.0),
        }
    }
}

/// The most control points a response curve may have.
pub const MAX_CURVE_POINTS: usize = 8;

/// A pressure response curve (#56): pressure (x, 0..1) to the pressure the brush then uses (y,
/// 0..1), straight lines between control points. The first point is at x = 0 and the last at
/// x = 1, x strictly increases, and there are 2..=[`MAX_CURVE_POINTS`] of them, so a curve is
/// total and cheap to evaluate. `[[0,0],[1,1]]` is the identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Curve(pub Vec<[f32; 2]>);

/// The named response presets: `linear` (the identity), `soft` (a light touch already leaves a
/// good line), `firm` (needs a harder press) and `scurve` (soft at both ends).
pub const CURVE_PRESETS: [&str; 4] = ["linear", "soft", "firm", "scurve"];

impl Curve {
    pub fn linear() -> Curve {
        Curve(vec![[0.0, 0.0], [1.0, 1.0]])
    }

    pub fn preset(name: &str) -> Option<Curve> {
        let pts: &[[f32; 2]] = match name.trim().to_ascii_lowercase().as_str() {
            "linear" => return Some(Curve::linear()),
            "soft" => &[[0.0, 0.0], [0.25, 0.5], [0.6, 0.85], [1.0, 1.0]],
            "firm" => &[[0.0, 0.0], [0.5, 0.25], [0.8, 0.6], [1.0, 1.0]],
            "scurve" => &[[0.0, 0.0], [0.25, 0.1], [0.5, 0.5], [0.75, 0.9], [1.0, 1.0]],
            _ => return None,
        };
        Some(Curve(pts.to_vec()))
    }

    /// The reason this is not a usable curve, if it is not one.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.0.len();
        if !(2..=MAX_CURVE_POINTS).contains(&n) {
            return Err(format!("a pressure curve has 2 to {MAX_CURVE_POINTS} points, not {n}"));
        }
        if self.0.iter().any(|p| !p[0].is_finite() || !p[1].is_finite() || !(0.0..=1.0).contains(&p[0]) || !(0.0..=1.0).contains(&p[1])) {
            return Err("every curve point is a pair of numbers from 0 to 1".into());
        }
        if self.0.first().map(|p| p[0]) != Some(0.0) || self.0.last().map(|p| p[0]) != Some(1.0) {
            return Err("a pressure curve starts at pressure 0 and ends at pressure 1".into());
        }
        if self.0.windows(2).any(|w| w[1][0] <= w[0][0]) {
            return Err("curve points must be in increasing pressure order".into());
        }
        Ok(())
    }

    /// Whether this is the identity (a straight line from the corner to the corner).
    pub fn is_identity(&self) -> bool {
        self.0.as_slice() == [[0.0, 0.0], [1.0, 1.0]] || (self.validate().is_ok() && self.0.iter().all(|p| p[0] == p[1]))
    }

    /// The curve's value at `pressure`. Total: a pressure outside 0..1 or not finite is brought
    /// into range (a mouse's 1), and a curve that fails [`Curve::validate`] is the identity.
    pub fn at(&self, pressure: f32) -> f32 {
        let x = if pressure.is_finite() { pressure.clamp(0.0, 1.0) } else { 1.0 };
        if self.validate().is_err() {
            return x;
        }
        for w in self.0.windows(2) {
            if x <= w[1][0] {
                let span = w[1][0] - w[0][0];
                let t = if span > 0.0 { (x - w[0][0]) / span } else { 1.0 };
                return (w[0][1] + (w[1][1] - w[0][1]) * t).clamp(0.0, 1.0);
            }
        }
        self.0.last().map_or(x, |p| p[1])
    }
}

/// How a stroke's pressure becomes width and opacity, stored with the stroke so it looks the same
/// whatever the preferences are when the project is reopened (#56). A missing curve is the brush's
/// own response; a stroke with no `response` is exactly what earlier builds drew.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PressureResponse {
    /// Applied to the pressure that sets the width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<Curve>,
    /// Applied to the pressure that sets the opacity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Curve>,
}

impl PressureResponse {
    pub fn validate(&self) -> Result<(), String> {
        for (name, c) in [("width", &self.width), ("opacity", &self.opacity)] {
            if let Some(c) = c {
                c.validate().map_err(|e| format!("`{name}`: {e}"))?;
            }
        }
        Ok(())
    }

    /// Whether both curves are the brush's own response.
    pub fn is_default(&self) -> bool {
        self.width.as_ref().is_none_or(Curve::is_identity) && self.opacity.as_ref().is_none_or(Curve::is_identity)
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
    /// How pressure becomes width and opacity; absent for the brush's own response (and written
    /// only when set, so a stroke without one is stored as it always was).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<PressureResponse>,
    /// The stored result of a fill: present exactly when `brush` is [`Brush::Fill`] (written only
    /// then). Shared, so cloning a cel's strokes never copies a mask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<std::sync::Arc<Fill>>,
}

impl Stroke {
    /// A fill operation painting `fill` in `color` (sRGB, straight alpha).
    pub fn fill_op(color: [f32; 4], fill: Fill) -> Stroke {
        Stroke { brush: Brush::Fill, color, size: 1.0, points: Vec::new(), response: None, fill: Some(std::sync::Arc::new(fill)) }
    }

    /// Whether this operation is a fill.
    pub fn is_fill(&self) -> bool {
        self.brush == Brush::Fill
    }

    /// Clamp a fill from an untrusted source to sane values, for a canvas of `w × h`. Fails when
    /// it is not a usable fill.
    pub fn sanitized_fill(mut self, w: u32, h: u32) -> Result<Stroke, String> {
        let fill = self.fill.as_deref().filter(|_| self.brush == Brush::Fill).ok_or("a fill needs its region")?;
        fill.validate(w, h)?;
        if self.color.iter().any(|c| !c.is_finite()) || self.color[3] <= 0.0 {
            return Err("a fill needs a visible colour".into());
        }
        for c in &mut self.color {
            *c = c.clamp(0.0, 1.0);
        }
        self.size = 1.0;
        self.points.clear();
        self.response = None;
        Ok(self)
    }

    /// Clamp a stroke from an untrusted source (command parameters, a project file) to sane values.
    /// Fails when nothing drawable is left.
    pub fn sanitized(mut self) -> Result<Stroke, String> {
        if self.brush == Brush::Fill || self.fill.is_some() {
            return Err("a fill is made with the fill command, not drawn".into());
        }
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
        if let Some(r) = &self.response {
            r.validate()?;
            // the brush's own response is stored as none
            if r.is_default() {
                self.response = None;
            }
        }
        Ok(self)
    }

    /// The radius and opacity factors (0..1) at a pressure sample: the brush's response applied to
    /// the pressure after this stroke's own curves, if it has any.
    pub fn response_at(&self, pressure: f32) -> (f32, f32) {
        let through = |c: Option<&Curve>| c.map_or(pressure, |c| c.at(pressure));
        let (width_p, opacity_p) = match &self.response {
            Some(r) => (through(r.width.as_ref()), through(r.opacity.as_ref())),
            None => (pressure, pressure),
        };
        (self.brush.response(width_p).0, self.brush.response(opacity_p).1)
    }

    /// Bounding box `(min_x, min_y, max_x, max_y)` including the brush radius.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        if let Some(f) = &self.fill {
            return Some(f.bounds());
        }
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
    /// How many fill runs this cel's fills hold.
    pub fn fill_runs(&self) -> usize {
        self.strokes.iter().filter_map(|s| s.fill.as_deref()).map(|f| f.runs.len()).sum()
    }

    /// The strokes for editing: copies them first when another clone of the cel still shares them.
    pub fn strokes_mut(&mut self) -> &mut Vec<Stroke> {
        std::sync::Arc::make_mut(&mut self.strokes)
    }
}

/// The most distinct cels the onion skin shows on each side of the one shown.
pub const MAX_ONION_CELS: usize = 4;

/// Why a cel is ghosted by the onion skin. `Before(1)` / `After(1)` are the nearest cels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhostRole {
    Before(usize),
    After(usize),
    /// The pose pinned for comparison.
    Pin,
    /// The first cel of the drawing, to compare with a loop's end.
    LoopStart,
}

/// One onion-skin ghost: the cel at `index` in [`Drawing::cels`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ghost {
    pub role: GhostRole,
    pub index: usize,
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

    /// Re-space the run of cels starting on the `selected` frames so each starts `spacing` frames
    /// after the one before ("ones", "twos", "threes"), the first staying put and every cel keeping
    /// its strokes. The cels must be consecutive in the drawing. The last one keeps its own hold:
    /// the cels after the run move by as far as it moved when `ripple`, else the run may not reach
    /// the next cel. Every start stays in `0..limit`. Nothing changes on an error. Returns how many
    /// cels moved.
    pub fn respace_cels(&mut self, selected: &[i64], spacing: i64, ripple: bool, limit: i64) -> Result<usize, String> {
        if !(1..=MAX_SPACING).contains(&spacing) {
            return Err(format!("spacing must be 1 to {MAX_SPACING} frames"));
        }
        let mut idx = selected.iter().map(|f| self.cel_on(*f).ok_or_else(|| format!("no cel starts on frame {f}"))).collect::<Result<Vec<_>, _>>()?;
        idx.sort_unstable();
        idx.dedup();
        let (Some(&first), Some(&last)) = (idx.first(), idx.last()) else { return Err("select at least one cel".into()) };
        if last - first + 1 != idx.len() {
            return Err("the selected cels must follow each other".into());
        }
        let anchor = self.cels.get(first).map(|c| c.frame).ok_or("no such cel")?;
        let old_last = self.cels.get(last).map(|c| c.frame).ok_or("no such cel")?;
        let steps = i64::try_from(idx.len() - 1).map_err(|_| "too many cels".to_string())?;
        let new_last = steps.checked_mul(spacing).and_then(|n| n.checked_add(anchor)).ok_or("frame out of range")?;
        let mut frames: Vec<(usize, i64)> = Vec::with_capacity(self.cels.len().saturating_sub(first));
        for (k, i) in idx.iter().enumerate() {
            frames.push((*i, anchor + k as i64 * spacing));
        }
        if ripple {
            let delta = new_last - old_last;
            for i in last + 1..self.cels.len() {
                let f = self.cels.get(i).map(|c| c.frame).ok_or("no such cel")?;
                frames.push((i, f.checked_add(delta).ok_or("frame out of range")?));
            }
        } else if let Some(next) = self.cels.get(last + 1)
            && new_last >= next.frame
        {
            return Err(format!("the cels would run into the one on frame {}", next.frame));
        }
        if frames.iter().any(|(_, f)| *f < 0 || *f >= limit) {
            return Err(format!("every cel must start before frame {limit}"));
        }
        let mut moved = 0;
        for (i, f) in frames {
            if let Some(c) = self.cels.get_mut(i)
                && c.frame != f
            {
                c.frame = f;
                moved += 1;
            }
        }
        Ok(moved)
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

    /// The onion-skin ghosts for the playhead on drawing frame `f`, in paint order (farthest
    /// first, so the nearest ends on top).
    ///
    /// Up to `before` / `after` (each at most [`MAX_ONION_CELS`]) cels either side of the cel
    /// shown, one ghost per *cel*: a held cel is one cel however long it is held, so it never
    /// repeats. A blank cel is a pose and takes its place (it shows nothing but stops an older
    /// drawing from standing in for it). A cel with the same strokes as the one shown, or as a
    /// nearer ghost, is passed over, since it would only sit on top of it. `pin` adds the cel
    /// starting on that frame and `loop_start` the first cel of the drawing, unless they are the
    /// cel shown or already a ghost.
    pub fn onion_plan(&self, f: i64, before: usize, after: usize, pin: Option<i64>, loop_start: bool) -> Vec<Ghost> {
        let shown = self.cel_index_at(f);
        let mut chosen: Vec<Ghost> = Vec::new();
        let repeats = |chosen: &[Ghost], i: usize| -> bool {
            let Some(c) = self.cels.get(i) else { return true };
            if c.strokes.is_empty() {
                return false;
            }
            let same = |j: usize| self.cels.get(j).is_some_and(|o| std::sync::Arc::ptr_eq(&o.strokes, &c.strokes) || o.strokes == c.strokes);
            shown.is_some_and(same) || chosen.iter().any(|g| same(g.index))
        };
        let take = |chosen: &mut Vec<Ghost>, order: &mut dyn Iterator<Item = usize>, limit: usize, role: fn(usize) -> GhostRole| {
            let mut n = 0;
            for i in order {
                if n >= limit.min(MAX_ONION_CELS) {
                    break;
                }
                if !repeats(chosen, i) {
                    n += 1;
                    chosen.push(Ghost { role: role(n), index: i });
                }
            }
        };
        if let Some(s) = shown {
            take(&mut chosen, &mut (0..s).rev(), before, GhostRole::Before);
        }
        take(&mut chosen, &mut (shown.map_or(0, |s| s + 1)..self.cels.len()), after, GhostRole::After);
        let extra = |chosen: &[Ghost], i: usize| Some(i) != shown && !chosen.iter().any(|g| g.index == i) && !repeats(chosen, i);
        let mut out = Vec::with_capacity(chosen.len() + 2);
        if loop_start && !self.cels.is_empty() && extra(&chosen, 0) {
            out.push(Ghost { role: GhostRole::LoopStart, index: 0 });
        }
        if let Some(i) = pin.and_then(|p| self.cel_on(p))
            && extra(&chosen, i)
            && !out.iter().any(|g| g.index == i)
        {
            out.push(Ghost { role: GhostRole::Pin, index: i });
        }
        let rank = |g: &Ghost| match g.role {
            GhostRole::Before(n) | GhostRole::After(n) => n,
            _ => 0,
        };
        chosen.sort_by_key(|g| std::cmp::Reverse(rank(g)));
        out.extend(chosen);
        out
    }

    /// How many fill runs all the cels hold.
    pub fn fill_runs(&self) -> usize {
        self.cels.iter().map(Cel::fill_runs).sum()
    }

    /// Drop the fills that are not usable on this canvas, or that would pass a cel's or the
    /// drawing's run budget (the first ones in order are kept): a fill from an untrusted file never
    /// reaches the renderer or the editor. Returns how many were dropped. Strokes are untouched.
    pub fn repair_fills(&mut self) -> usize {
        let (w, h) = (self.width, self.height);
        let (mut dropped, mut drawing_runs) = (0usize, 0usize);
        for c in &mut self.cels {
            if !c.strokes.iter().any(|s| s.brush == Brush::Fill || s.fill.is_some()) {
                continue;
            }
            let mut cel_runs = 0usize;
            let keep: Vec<bool> = c
                .strokes
                .iter()
                .map(|s| {
                    if s.brush != Brush::Fill && s.fill.is_none() {
                        return true;
                    }
                    let runs = s.fill.as_deref().map_or(0, |f| f.runs.len());
                    let ok = cel_runs + runs <= MAX_CEL_FILL_RUNS && drawing_runs + runs <= MAX_DRAWING_FILL_RUNS && s.clone().sanitized_fill(w, h).is_ok();
                    if ok {
                        cel_runs += runs;
                        drawing_runs += runs;
                    }
                    ok
                })
                .collect();
            let mut i = 0;
            c.strokes_mut().retain(|_| {
                let k = keep.get(i).copied().unwrap_or(false);
                i += 1;
                dropped += usize::from(!k);
                k
            });
        }
        dropped
    }

    /// Restore the invariants after loading untrusted data: sorted unique cels, sane strokes and
    /// fills (see [`Drawing::repair_fills`]); what cannot be made sane is dropped.
    pub fn sanitize(&mut self) {
        self.cels.truncate(MAX_CELS);
        self.cels.sort_by_key(|c| c.frame);
        self.cels.dedup_by_key(|c| c.frame);
        self.repair_fills();
        let (w, h) = (self.width, self.height);
        for c in &mut self.cels {
            let strokes = c.strokes_mut();
            strokes.truncate(MAX_CEL_STROKES);
            *strokes = std::mem::take(strokes)
                .into_iter()
                .filter_map(|mut s| {
                    if s.brush == Brush::Fill {
                        return s.sanitized_fill(w, h).ok();
                    }
                    // a damaged curve costs the stroke its response, not the stroke
                    if s.response.as_ref().is_some_and(|r| r.validate().is_err()) {
                        s.response = None;
                    }
                    s.sanitized().ok()
                })
                .collect();
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
            // a fill is never morphed: the nearer cel's operation stands in
            (Some(sa), Some(sb)) if sa.is_fill() || sb.is_fill() => out.push(if t < 0.5 { sa.clone() } else { sb.clone() }),
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
                out.push(Stroke {
                    brush: if t < 0.5 { sa.brush } else { sb.brush },
                    color,
                    size: lerp(sa.size, sb.size),
                    points,
                    response: if t < 0.5 { sa.response.clone() } else { sb.response.clone() },
                    fill: None,
                });
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
        Stroke { brush: Brush::Pen, color: [1.0, 0.0, 0.0, 1.0], size: 4.0, points, response: None, fill: None }
    }

    fn cel_of(d: &mut Drawing, frame: i64, strokes: &[Stroke]) {
        let i = d.ensure_cel(frame, false).unwrap();
        d.cels[i].strokes_mut().extend_from_slice(strokes);
    }
    fn plan(d: &Drawing, f: i64, before: usize, after: usize, pin: Option<i64>, lp: bool) -> Vec<(GhostRole, i64)> {
        d.onion_plan(f, before, after, pin, lp).iter().map(|g| (g.role, d.cels[g.index].frame)).collect()
    }

    #[test]
    fn onion_plan_has_one_ghost_per_distinct_cel_and_never_repeats_a_hold() {
        let mut d = Drawing::new(100, 100, FrameRate::new(24, 1));
        for (f, x) in [(0, 10.0), (4, 20.0), (8, 30.0), (12, 40.0), (16, 50.0)] {
            cel_of(&mut d, f, &[line(x, x + 5.0, 3)]);
        }
        // default: one either side; held frames 8..12 all give the same answer
        for f in 8..12 {
            assert_eq!(plan(&d, f, 1, 1, None, false), vec![(GhostRole::Before(1), 4), (GhostRole::After(1), 12)]);
        }
        // two either side, farthest painted first
        assert_eq!(
            plan(&d, 9, 2, 2, None, false),
            vec![(GhostRole::Before(2), 0), (GhostRole::After(2), 16), (GhostRole::Before(1), 4), (GhostRole::After(1), 12)]
        );
        // nothing before the first cel; before the first cel starts only the next shows
        assert_eq!(plan(&d, 0, 3, 0, None, false), vec![]);
        let mut late = d.clone();
        late.cels.remove(0);
        assert_eq!(plan(&late, 1, 2, 1, None, false), vec![(GhostRole::After(1), 4)]);
        // the count is bounded
        assert_eq!(d.onion_plan(8, usize::MAX, usize::MAX, None, false).len(), 4);
        assert!(d.onion_plan(8, 0, 0, None, false).is_empty());
    }

    #[test]
    fn onion_plan_keeps_blank_cels_and_passes_over_copies() {
        let mut d = Drawing::new(100, 100, FrameRate::new(24, 1));
        cel_of(&mut d, 0, &[line(10.0, 20.0, 3)]);
        cel_of(&mut d, 4, &[]); // blank pose
        cel_of(&mut d, 8, &[line(30.0, 40.0, 3)]);
        // the blank cel is the previous pose: it takes the place (and shows nothing) instead of
        // frame 0 standing in for it
        assert_eq!(plan(&d, 8, 1, 0, None, false), vec![(GhostRole::Before(1), 4)]);
        // a duplicate of the cel shown only sits on top of it; a duplicate of a nearer ghost too
        let mut e = Drawing::new(100, 100, FrameRate::new(24, 1));
        let same = [line(10.0, 20.0, 3)];
        cel_of(&mut e, 0, &[line(1.0, 2.0, 3)]);
        cel_of(&mut e, 4, &same);
        cel_of(&mut e, 8, &same);
        cel_of(&mut e, 12, &same);
        assert_eq!(plan(&e, 12, 2, 0, None, false), vec![(GhostRole::Before(1), 0)]);
        assert_eq!(plan(&e, 0, 0, 3, None, false), vec![(GhostRole::After(1), 4)]);
    }

    #[test]
    fn onion_plan_pin_and_loop_start_do_not_duplicate_other_ghosts() {
        let mut d = Drawing::new(100, 100, FrameRate::new(24, 1));
        for (f, x) in [(0, 10.0), (4, 20.0), (8, 30.0), (12, 40.0)] {
            cel_of(&mut d, f, &[line(x, x + 5.0, 3)]);
        }
        assert_eq!(plan(&d, 12, 1, 0, Some(4), true), vec![(GhostRole::LoopStart, 0), (GhostRole::Pin, 4), (GhostRole::Before(1), 8)]);
        // a pin on a neighbour, on the cel shown, on no cel at all, or the loop start being the pin
        assert_eq!(plan(&d, 12, 1, 0, Some(8), false), vec![(GhostRole::Before(1), 8)]);
        assert_eq!(plan(&d, 12, 0, 0, Some(12), false), vec![]);
        assert_eq!(plan(&d, 12, 0, 0, Some(5), false), vec![]);
        assert_eq!(plan(&d, 12, 0, 0, Some(0), true), vec![(GhostRole::LoopStart, 0)]);
        // the loop start is the cel shown: nothing to compare
        assert_eq!(plan(&d, 2, 0, 0, None, true), vec![]);
        // the drawing playing no cel yet, an empty drawing
        assert!(Drawing::new(10, 10, FrameRate::new(24, 1)).onion_plan(-5, 3, 3, Some(0), true).is_empty());
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
        d.cels[first].strokes_mut().push(line(0.0, 10.0, 3));
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

    fn on_ones(frames: &[i64]) -> Drawing {
        let mut d = Drawing::new(100, 100, FrameRate::new(24, 1));
        for f in frames {
            let i = d.ensure_cel(*f, false).unwrap();
            d.cels[i].strokes_mut().push(line(*f as f32, 10.0, 2));
        }
        d
    }

    fn starts(d: &Drawing) -> Vec<i64> {
        d.cels.iter().map(|c| c.frame).collect()
    }

    #[test]
    fn ones_become_twos_and_threes_keeping_strokes_and_rippling_the_rest() {
        let mut d = on_ones(&[0, 1, 2, 3, 10]);
        assert_eq!(d.respace_cels(&[0, 1, 2, 3], 2, true, 48).unwrap(), 4);
        // the last pose of the run keeps its 7-frame hold: the later pose moves with it
        assert_eq!(starts(&d), vec![0, 2, 4, 6, 13]);
        assert_eq!(d.cels[2].strokes.first().map(|s| s.points[0].x), Some(2.0));
        d.respace_cels(&[0, 2, 4, 6], 3, true, 48).unwrap();
        assert_eq!(starts(&d), vec![0, 3, 6, 9, 16]);
        d.respace_cels(&[0, 3, 6, 9], 1, true, 48).unwrap();
        assert_eq!(starts(&d), vec![0, 1, 2, 3, 10]);
    }

    #[test]
    fn respacing_without_ripple_stops_at_the_next_cel_and_a_subrange_anchors_at_its_first() {
        let mut d = on_ones(&[0, 1, 2, 3, 6]);
        assert!(d.respace_cels(&[0, 1, 2, 3], 2, false, 48).is_err());
        assert_eq!(starts(&d), vec![0, 1, 2, 3, 6]);
        let mut d = on_ones(&[0, 1, 2, 6]);
        assert_eq!(d.respace_cels(&[1, 2], 2, false, 48).unwrap(), 1);
        assert_eq!(starts(&d), vec![0, 1, 3, 6]);
    }

    #[test]
    fn respacing_refuses_bad_selections_and_overflow_without_mutation() {
        let mut d = on_ones(&[0, 1, 2, 5]);
        let before = d.clone();
        for (sel, spacing, ripple, limit) in [
            (vec![0, 2], 2, true, 48), // not consecutive
            (vec![7], 2, true, 48),    // no such cel
            (vec![], 2, true, 48),     // empty
            (vec![0, 1], 0, true, 48), // no spacing
            (vec![0, 1], MAX_SPACING + 1, true, 48),
            (vec![0, 1, 2], 3, true, 8),      // would start past the end
            (vec![1, 2], i64::MAX, true, 48), // overflow
            (vec![0, 1], 2, true, 3),         // a later cel would land past the end
        ] {
            assert!(d.respace_cels(&sel, spacing, ripple, limit).is_err(), "{sel:?} x{spacing}");
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

#[cfg(test)]
mod response_tests {
    use super::*;

    fn pen(points: &[(f32, f32, f32)]) -> Stroke {
        Stroke {
            brush: Brush::Pencil,
            color: [0.0, 0.0, 0.0, 1.0],
            size: 10.0,
            points: points.iter().map(|&(x, y, p)| StrokePoint { x, y, p }).collect(),
            response: None,
            fill: None,
        }
    }

    #[test]
    fn curves_hit_their_endpoints_interpolate_and_are_total() {
        for name in CURVE_PRESETS {
            let c = Curve::preset(name).unwrap();
            assert!(c.validate().is_ok(), "{name}");
            assert_eq!((c.at(0.0), c.at(1.0)), (0.0, 1.0), "{name} keeps both endpoints");
        }
        let soft = Curve::preset("soft").unwrap();
        // straight lines between control points
        assert!((soft.at(0.125) - 0.25).abs() < 1e-6 && (soft.at(0.25) - 0.5).abs() < 1e-6);
        assert!(soft.at(0.1) > 0.1 && Curve::preset("firm").unwrap().at(0.1) < 0.1, "soft lifts a light touch, firm needs a harder press");
        // pressures outside 0..1, NaN and infinity are brought into range; a mouse's 1 is the end
        assert_eq!((soft.at(-3.0), soft.at(7.0), soft.at(f32::NAN), soft.at(f32::INFINITY)), (0.0, 1.0, 1.0, 1.0));
        // a curve that fails validation is the identity rather than a panic
        let bad = Curve(vec![[0.5, 0.5], [0.2, 0.9]]);
        assert!(bad.validate().is_err());
        assert_eq!((bad.at(0.3), Curve(vec![]).at(0.3), Curve(vec![[f32::NAN, 0.0]; 3]).at(0.3)), (0.3, 0.3, 0.3));
        assert!(Curve::linear().is_identity() && Curve(vec![[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]).is_identity());
        assert!(!soft.is_identity());
        assert!(Curve::preset("Soft ").is_some() && Curve::preset("nope").is_none());
    }

    #[test]
    fn malformed_curves_are_refused_with_a_reason() {
        let ok = |pts: &[[f32; 2]]| Curve(pts.to_vec()).validate();
        assert!(ok(&[[0.0, 0.2], [1.0, 0.9]]).is_ok());
        for (pts, why) in [
            (vec![[0.0, 0.0]], "points"),
            (vec![[0.0, 0.0]; 9], "points"),
            (vec![[0.0, 0.0], [0.5, 2.0], [1.0, 1.0]], "0 to 1"),
            (vec![[0.0, 0.0], [0.5, f32::NAN], [1.0, 1.0]], "0 to 1"),
            (vec![[0.0, 0.0], [f32::INFINITY, 0.5], [1.0, 1.0]], "0 to 1"),
            (vec![[0.1, 0.0], [1.0, 1.0]], "starts at pressure 0"),
            (vec![[0.0, 0.0], [0.9, 1.0]], "ends at pressure 1"),
            (vec![[0.0, 0.0], [0.5, 0.5], [0.5, 0.6], [1.0, 1.0]], "increasing"),
            (vec![[0.0, 0.0], [0.7, 0.5], [0.4, 0.6], [1.0, 1.0]], "increasing"),
        ] {
            let e = ok(&pts).unwrap_err();
            assert!(e.contains(why), "{pts:?}: {e}");
        }
    }

    #[test]
    fn without_a_response_a_stroke_is_drawn_exactly_as_before() {
        let s = pen(&[(0.0, 0.0, 0.0), (5.0, 0.0, 0.3), (10.0, 0.0, 1.0)]);
        for p in [0.0, 0.1, 0.5, 1.0, f32::NAN] {
            assert_eq!(s.response_at(p), s.brush.response(p), "{p}");
        }
        // the identity, however it is written, is stored as nothing and changes nothing
        let mut id = s.clone();
        id.response = Some(PressureResponse { width: Some(Curve::linear()), opacity: Some(Curve(vec![[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]])) });
        assert_eq!(id.sanitized().unwrap().response, None);
    }

    #[test]
    fn width_and_opacity_follow_their_own_curves() {
        let mut s = pen(&[(0.0, 0.0, 0.5), (9.0, 0.0, 0.5)]);
        s.response = Some(PressureResponse { width: Some(Curve::preset("soft").unwrap()), opacity: None });
        let (w, a) = s.response_at(0.25);
        // the width saw pressure 0.5, the opacity the pen's 0.25
        assert_eq!(w, Brush::Pencil.response(0.5).0);
        assert_eq!(a, Brush::Pencil.response(0.25).1);
        s.response = Some(PressureResponse { width: None, opacity: Some(Curve::preset("firm").unwrap()) });
        let (w, a) = s.response_at(0.5);
        assert_eq!(w, Brush::Pencil.response(0.5).0);
        assert_eq!(a, Brush::Pencil.response(0.25).1);
        // a mouse's full pressure reaches the end of any curve
        assert_eq!(s.response_at(1.0), Brush::Pencil.response(1.0));
    }

    #[test]
    fn the_response_is_stored_only_when_set_and_survives_a_round_trip() {
        let plain = serde_json::to_string(&pen(&[(0.0, 0.0, 1.0)])).unwrap();
        assert!(!plain.contains("response"), "{plain}");
        let mut s = pen(&[(0.0, 0.0, 0.2), (4.0, 0.0, 0.9)]);
        s.response = Some(PressureResponse { width: Curve::preset("scurve"), opacity: Curve::preset("firm") });
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"response\""));
        let back: Stroke = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        // and what was saved before the field existed still loads, drawn the same
        let old: Stroke = serde_json::from_str(&plain).unwrap();
        assert_eq!(old.response, None);
        // an unknown key in a response is refused
        assert!(serde_json::from_str::<Stroke>(r#"{"color":[0,0,0,1],"size":3,"points":[],"response":{"width":[[0,0],[1,1]],"hue":1}}"#).is_err());
    }

    #[test]
    fn a_damaged_curve_in_a_loaded_project_costs_the_response_not_the_stroke() {
        let mut d = Drawing::new(64, 32, waltcut_time::FrameRate::FPS_24);
        d.ensure_cel(0, false).unwrap();
        let mut bad = pen(&[(1.0, 1.0, 0.5), (4.0, 4.0, 0.7)]);
        bad.response = Some(PressureResponse { width: Some(Curve(vec![[0.0, 0.0], [0.5, f32::NAN], [1.0, 1.0]])), opacity: None });
        let mut good = pen(&[(2.0, 2.0, 0.5)]);
        good.response = Some(PressureResponse { width: Curve::preset("soft"), opacity: None });
        if let Some(c) = d.cels.first_mut() {
            c.strokes_mut().extend([bad, good.clone()]);
        }
        d.sanitize();
        let strokes = &d.cels[0].strokes;
        assert_eq!(strokes.len(), 2);
        assert_eq!(strokes[0].response, None);
        assert_eq!(strokes[1].response, good.response);
        // a command's stroke with a damaged curve is refused instead
        let mut cmd = pen(&[(1.0, 1.0, 0.5)]);
        cmd.response = Some(PressureResponse { width: Some(Curve(vec![[0.0, 0.0]])), opacity: None });
        assert!(cmd.sanitized().unwrap_err().contains("`width`"));
    }
}

#[cfg(test)]
mod fill_tests {
    use super::*;
    use crate::drawing_fill::{MAX_CEL_FILL_RUNS, MAX_FILL_PIXELS};

    fn fill_stroke(rect: [u32; 4]) -> Stroke {
        let cells = vec![1u8; (rect[2] * rect[3]) as usize];
        Stroke::fill_op([1.0, 0.0, 0.0, 1.0], Fill::encode([rect[0], rect[1]], 32, rect, &cells).unwrap())
    }

    fn pen() -> Stroke {
        Stroke { brush: Brush::Pen, color: [0.0, 0.0, 0.0, 1.0], size: 4.0, points: vec![StrokePoint { x: 1.0, y: 1.0, p: 1.0 }], response: None, fill: None }
    }

    #[test]
    fn a_fill_is_an_operation_with_bounds_and_no_points() {
        let f = fill_stroke([10, 20, 30, 40]);
        assert!(f.is_fill() && f.points.is_empty());
        assert_eq!(f.bounds(), Some((10.0, 20.0, 40.0, 60.0)));
        assert_eq!(f.clone().sanitized_fill(100, 100).unwrap(), f);
        // a drawn stroke has no fill key; a fill's brush reads back as Fill
        let json = serde_json::to_string(&pen()).unwrap();
        assert!(!json.contains("fill"), "{json}");
        let back: Stroke = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
        assert_eq!(Brush::from_name("Fill"), None, "a fill is not a brush to draw with");
    }

    #[test]
    fn a_fill_is_never_a_drawn_stroke_and_a_stroke_is_never_a_fill() {
        assert!(fill_stroke([0, 0, 4, 4]).sanitized().is_err());
        assert!(Stroke { fill: fill_stroke([0, 0, 4, 4]).fill, ..pen() }.sanitized().is_err());
        assert!(Stroke { brush: Brush::Fill, ..pen() }.sanitized_fill(10, 10).is_err());
        assert!(pen().sanitized_fill(10, 10).is_err());
        let mut clear = fill_stroke([0, 0, 4, 4]);
        clear.color[3] = 0.0;
        assert!(clear.sanitized_fill(10, 10).is_err(), "a fully transparent fill changes nothing");
        let mut nan = fill_stroke([0, 0, 4, 4]);
        nan.color[0] = f32::NAN;
        assert!(nan.sanitized_fill(10, 10).is_err());
        let mut bright = fill_stroke([0, 0, 4, 4]);
        bright.color = [4.0, -1.0, 0.5, 9.0];
        assert_eq!(bright.sanitized_fill(10, 10).unwrap().color, [1.0, 0.0, 0.5, 1.0]);
    }

    #[test]
    fn loading_keeps_good_fills_and_drops_bad_ones_and_those_over_budget() {
        let mut d = Drawing::new(64, 64, FrameRate::new(24, 1));
        let outside = fill_stroke([60, 60, 8, 8]);
        let ok = fill_stroke([0, 0, 8, 8]);
        d.cels = vec![Cel { frame: 0, strokes: vec![pen(), outside, ok.clone(), pen()].into() }];
        d.sanitize();
        assert_eq!(d.cels[0].strokes.len(), 3, "the fill outside the canvas is dropped, the rest is kept in order");
        assert_eq!(d.cels[0].strokes[1], ok);
        // too large a canvas holds no fills
        let mut big = Drawing::new(8192, 8192, FrameRate::new(24, 1));
        assert!(u64::from(big.width) * u64::from(big.height) > MAX_FILL_PIXELS);
        big.cels = vec![Cel { frame: 0, strokes: vec![fill_stroke([0, 0, 8, 8]), pen()].into() }];
        big.sanitize();
        assert_eq!(big.cels[0].strokes.len(), 1);
        // a cel's run budget
        let many = |n: usize| {
            let runs: Vec<[u32; 2]> = (0..n).map(|i| [u32::from(i % 2 == 0), 1]).collect();
            Stroke::fill_op([1.0, 0.0, 0.0, 1.0], Fill { seed: [0, 0], tolerance: 0, rect: [0, 0, n as u32, 1], runs })
        };
        let mut e = Drawing::new(MAX_CEL_FILL_RUNS as u32, 1, FrameRate::new(24, 1));
        e.cels = vec![Cel { frame: 0, strokes: (0..5).map(|_| many(60_000)).collect::<Vec<_>>().into() }];
        e.sanitize();
        assert_eq!(e.cels[0].strokes.len(), 4, "the fifth 60 000-run fill passes the cel's 262 144-run budget");
        assert_eq!(e.fill_runs(), 240_000);
    }

    #[test]
    fn an_in_between_never_morphs_a_fill() {
        let a = Cel { frame: 0, strokes: vec![fill_stroke([0, 0, 4, 4])].into() };
        let b = Cel { frame: 4, strokes: vec![pen()].into() };
        assert!(tween(&a, &b, 0.25)[0].is_fill());
        assert!(!tween(&a, &b, 0.75)[0].is_fill());
        assert!(tween(&b, &a, 0.75)[0].is_fill());
    }
}
