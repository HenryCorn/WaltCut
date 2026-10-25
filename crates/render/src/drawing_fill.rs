//! Region fill for drawing cels (#68): finding the region, and painting a stored one.
//!
//! [`compute_fill`] rasterises the paint a cel has *before* the fill (the same CPU code the Program
//! monitor and export use), then floods the 4-connected set of pixels whose premultiplied linear
//! RGBA, quantised to 8 bits, is within a tolerance of the pixel under the seed. Everything after
//! the rasteriser is integer maths, so a cel gives the same region every time. The result is a
//! [`Fill`] (`waltcut_project::drawing_fill`): the region, plus a one-pixel shell of *edge* cells
//! around it that [`paint_fill`] lays behind existing paint so the colour reaches a line's soft edge
//! without covering the line.
//!
//! Work and memory are bounded by the canvas (at most [`MAX_FILL_PIXELS`]); the flood checks a
//! `cancel` callback as it goes, so a caller can stop it with a flag or a time budget.

use std::fmt;

use waltcut_geom::{Affine, Vec2};
use waltcut_project::drawing::Stroke;
use waltcut_project::drawing_fill::{CLASS_EDGE, CLASS_FILLED, CLASS_OUTSIDE, Fill, MAX_FILL_GAP, MAX_FILL_GROW, MAX_FILL_PIXELS};

use crate::drawing::render_strokes;
use crate::graphics::premul_linear;
use crate::image::Image;

/// How many pixels the flood fills between two looks at the cancel callback.
const CANCEL_EVERY: usize = 1 << 16;

/// Why a fill was not made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FillError {
    /// The fill cannot be made as asked (the reason is for the person).
    Refused(String),
    /// The cancel callback asked to stop.
    Cancelled,
}

impl fmt::Display for FillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FillError::Refused(why) => f.write_str(why),
            FillError::Cancelled => f.write_str("the fill was cancelled"),
        }
    }
}

impl std::error::Error for FillError {}

fn refused(why: impl Into<String>) -> FillError {
    FillError::Refused(why.into())
}

/// One premultiplied linear channel as 0..=255 (a value that is not a number counts as 0).
fn quantise(v: f32) -> u32 {
    if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32 } else { 0 }
}

/// Whether every one of the four 8-bit channels packed in `a` is within `tol` of `b`'s.
fn similar(a: u32, b: u32, tol: u32) -> bool {
    (0..4).all(|c| {
        let (x, y) = ((a >> (8 * c)) & 255, (b >> (8 * c)) & 255);
        x.abs_diff(y) <= tol
    })
}

/// How the region is found beyond the tolerance (#69). Both are in canvas pixels, so the result
/// does not depend on any view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillOptions {
    /// Closing radius: pixels within this many pixels of a line count as line while the region is
    /// searched, so openings up to twice as wide are closed. At most [`MAX_FILL_GAP`].
    pub gap: u8,
    /// Extra edge pixels beyond the one-pixel shell. At most [`MAX_FILL_GROW`].
    pub grow: u8,
}

/// An inclusive rectangle of canvas cells.
#[derive(Clone, Copy)]
struct Win {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Win {
    fn full(w: usize, h: usize) -> Win {
        Win { x0: 0, y0: 0, x1: w.saturating_sub(1), y1: h.saturating_sub(1) }
    }

    /// This rectangle `r` cells larger on every side, kept inside a `w × h` canvas.
    fn expand(self, r: usize, w: usize, h: usize) -> Win {
        Win {
            x0: self.x0.saturating_sub(r),
            y0: self.y0.saturating_sub(r),
            x1: self.x1.saturating_add(r).min(w.saturating_sub(1)),
            y1: self.y1.saturating_add(r).min(h.saturating_sub(1)),
        }
    }

    fn width(&self) -> usize {
        self.x1.saturating_sub(self.x0) + 1
    }

    fn height(&self) -> usize {
        self.y1.saturating_sub(self.y0) + 1
    }
}

/// For every cell of `win` (row-major over the window), whether a cell of the window with `src`
/// true is within `r` cells of it: `dx² + dy² <= r²`. Exact integer maths in two passes (the
/// nearest source in the row, capped, then a scan of `2r + 1` rows), so it is deterministic and
/// costs `O(cells × (2r + 1))`; `cancel` is looked at once per row.
fn within(src: &dyn Fn(usize) -> bool, w: usize, win: Win, r: usize, cancel: &dyn Fn() -> bool) -> Result<Vec<u8>, FillError> {
    let (ww, wh) = (win.width(), win.height());
    let cap = u8::try_from(r + 1).map_err(|_| refused("the radius is too large"))?;
    let mut near = vec![cap; ww * wh];
    for (j, row) in near.chunks_mut(ww).enumerate() {
        if cancel() {
            return Err(FillError::Cancelled);
        }
        let base = (win.y0 + j) * w + win.x0;
        let mut d = cap;
        for (x, v) in row.iter_mut().enumerate() {
            d = if src(base + x) { 0 } else { d.saturating_add(1).min(cap) };
            *v = d;
        }
        d = cap;
        for (x, v) in row.iter_mut().enumerate().rev() {
            d = if src(base + x) { 0 } else { d.saturating_add(1).min(cap) };
            *v = (*v).min(d);
        }
    }
    // the farthest horizontal distance still within r at each row offset
    let lim: Vec<u8> = (0..=r).map(|dy| (0..=r).rev().find(|dx| dx * dx + dy * dy <= r * r).map_or(0, |dx| dx as u8)).collect();
    let mut out = vec![0u8; ww * wh];
    for (y, orow) in out.chunks_mut(ww).enumerate() {
        if cancel() {
            return Err(FillError::Cancelled);
        }
        for dy in 0..=2 * r {
            let Some(yy) = (y + dy).checked_sub(r).filter(|v| *v < wh) else { continue };
            let Some(limit) = lim.get(dy.abs_diff(r)).copied() else { continue };
            let Some(nrow) = near.get(yy * ww..(yy + 1) * ww) else { continue };
            for (o, n) in orow.iter_mut().zip(nrow) {
                if *n <= limit {
                    *o = 1;
                }
            }
        }
    }
    Ok(out)
}

/// The region around `seed` in the paint of `strokes` on a `width × height` canvas, as a [`Fill`]
/// (plus how many pixels it covers). `strokes` are the paint the region is searched in: the cel's
/// own operations before the fill, or a reference drawing's cel (#69).
///
/// With `opts.gap`, the search treats everything within that radius of a line as line, then grows
/// the region back over the pixels it skipped (never over a line), and a seed too close to a line
/// starts from the nearest pixel that is not. The stored seed is the pixel actually used. With
/// `opts.grow` the edge shell is that many pixels wider. The returned fill records both options.
///
/// Refuses a canvas over [`MAX_FILL_PIXELS`] (or without area), a seed outside the canvas and a
/// region too ragged to store; returns [`FillError::Cancelled`] when `cancel` says so. Nothing is
/// kept on failure.
pub fn compute_fill(
    strokes: &[Stroke],
    width: u32,
    height: u32,
    seed: [u32; 2],
    tolerance: u8,
    opts: FillOptions,
    cancel: &dyn Fn() -> bool,
) -> Result<(Fill, u64), FillError> {
    let cells = u64::from(width) * u64::from(height);
    if cells == 0 {
        return Err(refused("the drawing has no canvas to fill"));
    }
    if cells > MAX_FILL_PIXELS {
        return Err(refused(format!("a canvas of {width} × {height} is too large to fill (at most {MAX_FILL_PIXELS} pixels)")));
    }
    if seed[0] >= width || seed[1] >= height {
        return Err(refused(format!("the point ({}, {}) is outside the {width} × {height} canvas", seed[0], seed[1])));
    }
    if opts.gap > MAX_FILL_GAP || opts.grow > MAX_FILL_GROW {
        return Err(refused(format!("`gap` and `grow` are at most {MAX_FILL_GAP} and {MAX_FILL_GROW} pixels")));
    }
    let (gap, edge) = (usize::from(opts.gap), usize::from(opts.grow) + 1);
    let (w, h) = (width as usize, height as usize);
    // the paint to search, 8 bits per channel; the float raster is dropped before the flood
    let pix: Vec<u32> = {
        let mut img = Image::new(w, h);
        for chunk in strokes.chunks(32) {
            if cancel() {
                return Err(FillError::Cancelled);
            }
            render_strokes(chunk, &Affine::IDENTITY, &mut img);
        }
        img.px.as_chunks::<4>().0.iter().map(|p| p.iter().enumerate().fold(0u32, |acc, (c, v)| acc | (quantise(*v) << (8 * c)))).collect()
    };
    let at = |x: usize, y: usize| y * w + x;
    let target = pix.get(at(seed[0] as usize, seed[1] as usize)).copied().ok_or_else(|| refused("the point is outside the canvas"))?;
    let tol = u32::from(tolerance);
    // similar to the target: what the region may be made of
    let free: Vec<u8> = pix.iter().map(|p| u8::from(similar(*p, target, tol))).collect();
    drop(pix);
    // what the flood may enter: with a closing radius, only what is further from a line than that
    let closed: Vec<u8>;
    let open: &[u8] = if gap > 0 {
        let near_line = within(&|i| free.get(i) == Some(&0), w, Win::full(w, h), gap, cancel)?;
        closed = free.iter().zip(&near_line).map(|(f, n)| u8::from(*f == 1 && *n == 0)).collect();
        &closed
    } else {
        &free
    };
    let is_open = |i: usize| open.get(i) == Some(&1);
    let mut seed = seed;
    if !is_open(at(seed[0] as usize, seed[1] as usize)) {
        // too close to a line to start from: the nearest open pixel (distance, then row, then
        // column) among the similar pixels connected to the seed within a bounded window, so the
        // search never hops over a line into another region
        let reach = 2 * gap + 1;
        let (sx, sy) = (seed[0] as usize, seed[1] as usize);
        let (wx0, wy0) = (sx.saturating_sub(reach), sy.saturating_sub(reach));
        let (wx1, wy1) = ((sx + reach).min(w - 1), (sy + reach).min(h - 1));
        let ww = wx1 - wx0 + 1;
        let mut seen = vec![false; ww * (wy1 - wy0 + 1)];
        let mut queue = std::collections::VecDeque::from([(sx, sy)]);
        if let Some(v) = seen.get_mut((sy - wy0) * ww + (sx - wx0)) {
            *v = true;
        }
        let mut best: Option<(usize, usize, usize)> = None;
        while let Some((x, y)) = queue.pop_front() {
            let d2 = x.abs_diff(sx).pow(2) + y.abs_diff(sy).pow(2);
            if d2 > reach * reach {
                continue;
            }
            if is_open(at(x, y)) {
                best = Some(best.map_or((d2, y, x), |b| b.min((d2, y, x))));
            }
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < wx0 || nx > wx1 || ny < wy0 || ny > wy1 || free.get(at(nx, ny)) != Some(&1) {
                    continue;
                }
                if let Some(v) = seen.get_mut((ny - wy0) * ww + (nx - wx0))
                    && !*v
                {
                    *v = true;
                    queue.push_back((nx, ny));
                }
            }
        }
        let Some((_, y, x)) = best else {
            return Err(refused(if gap > 0 {
                "the point is too close to a line to close gaps there: click further from it, or lower the gap"
            } else {
                "the point is not part of any region"
            }));
        };
        seed = [x as u32, y as u32];
    }
    let mut plane = vec![CLASS_OUTSIDE; w * h];
    let free_at = |plane: &[u8], i: usize| plane.get(i) == Some(&CLASS_OUTSIDE) && is_open(i);
    let mut stack: Vec<(u32, u32)> = vec![(seed[0], seed[1])];
    let (mut work, mut next_check) = (0usize, CANCEL_EVERY);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0usize, 0usize);
    let mut filled = 0u64;
    while let Some((sx, sy)) = stack.pop() {
        let (x, y) = (sx as usize, sy as usize);
        if !free_at(&plane, at(x, y)) {
            continue;
        }
        let (mut l, mut r) = (x, x);
        while l > 0 && free_at(&plane, at(l - 1, y)) {
            l -= 1;
        }
        while r + 1 < w && free_at(&plane, at(r + 1, y)) {
            r += 1;
        }
        if let Some(span) = plane.get_mut(at(l, y)..=at(r, y)) {
            span.fill(CLASS_FILLED);
        }
        let n = r - l + 1;
        filled += n as u64;
        (min_x, min_y, max_x, max_y) = (min_x.min(l), min_y.min(y), max_x.max(r), max_y.max(y));
        work += n;
        if work >= next_check {
            if cancel() {
                return Err(FillError::Cancelled);
            }
            next_check = work + CANCEL_EVERY;
        }
        for ny in [y.checked_sub(1), y.checked_add(1).filter(|v| *v < h)].into_iter().flatten() {
            let mut inside = false;
            for nx in l..=r {
                let ok = free_at(&plane, at(nx, ny));
                if ok && !inside {
                    stack.push((nx as u32, ny as u32));
                }
                inside = ok;
            }
        }
    }
    if filled == 0 {
        return Err(refused("the point is not part of any region"));
    }
    let mut bounds = Win { x0: min_x, y0: min_y, x1: max_x, y1: max_y };
    if gap > 0 {
        // grow back over what the radius made the search skip: similar pixels within the radius of
        // the region (never a line, and never further than a closed gap lets it reach)
        let win = bounds.expand(gap, w, h);
        let grown = within(&|i| plane.get(i) == Some(&CLASS_FILLED), w, win, gap, cancel)?;
        let ww = win.width();
        for y in win.y0..=win.y1 {
            for x in win.x0..=win.x1 {
                let i = at(x, y);
                let near = grown.get((y - win.y0) * ww + (x - win.x0)) == Some(&1);
                if near && free.get(i) == Some(&1) && plane.get(i) == Some(&CLASS_OUTSIDE) {
                    if let Some(c) = plane.get_mut(i) {
                        *c = CLASS_FILLED;
                    }
                    filled += 1;
                    bounds = Win { x0: bounds.x0.min(x), y0: bounds.y0.min(y), x1: bounds.x1.max(x), y1: bounds.y1.max(y) };
                }
            }
        }
    }
    // the edge: pixels the region does not include but is within reach of (the soft edge of a line,
    // and under a line when grown), painted behind what is there
    let win = bounds.expand(edge, w, h);
    let reach = within(&|i| plane.get(i) == Some(&CLASS_FILLED), w, win, edge, cancel)?;
    let ww = win.width();
    for y in win.y0..=win.y1 {
        for x in win.x0..=win.x1 {
            let i = at(x, y);
            if reach.get((y - win.y0) * ww + (x - win.x0)) == Some(&1)
                && let Some(c) = plane.get_mut(i).filter(|c| **c == CLASS_OUTSIDE)
            {
                *c = CLASS_EDGE;
            }
        }
    }
    let (rw, rh) = (win.width(), win.height());
    let mut cropped = Vec::with_capacity(rw * rh);
    for y in win.y0..=win.y1 {
        cropped.extend_from_slice(plane.get(at(win.x0, y)..=at(win.x1, y)).ok_or_else(|| refused("the fill is outside the canvas"))?);
    }
    let rect = [win.x0 as u32, win.y0 as u32, rw as u32, rh as u32];
    let fill = Fill::encode(seed, tolerance, rect, &cropped).map_err(FillError::Refused)?.with_options(opts.gap, opts.grow, None);
    Ok((fill, filled))
}

/// Coverage of one cell class at (`i`, `j`) of the decoded mask, 0 outside it.
fn cell(plane: &[u8], w: i64, h: i64, i: i64, j: i64, class: u8) -> f32 {
    if i < 0 || j < 0 || i >= w || j >= h {
        return 0.0;
    }
    if usize::try_from(j * w + i).ok().and_then(|k| plane.get(k)) == Some(&class) { 1.0 } else { 0.0 }
}

/// Paint a fill operation onto `img`, mapping canvas pixels to `img` pixels with `m`.
///
/// Each output pixel centre is taken back to the canvas and the mask is sampled there with bilinear
/// coverage, so at a whole-pixel placement (the drawing's own size, any whole-pixel move) every
/// pixel is exactly in or out, and under Motion scale or rotation the edge is one canvas pixel
/// wide. Filled cells go *over* what is there with the stroke's colour and alpha; edge cells go
/// *behind* it.
pub(crate) fn paint_fill(s: &Stroke, fill: &Fill, m: &Affine, img: &mut Image) {
    if img.w == 0 || img.h == 0 {
        return;
    }
    let (Ok(plane), Some(inv)) = (fill.decode(), m.inverse()) else { return };
    let [rx, ry, rw, rh] = fill.rect;
    let (w, h) = (i64::from(rw), i64::from(rh));
    // the output pixels the rectangle (one cell wider all round) can reach
    let corners = [(-1.0, -1.0), (f64::from(rw) + 1.0, -1.0), (-1.0, f64::from(rh) + 1.0), (f64::from(rw) + 1.0, f64::from(rh) + 1.0)]
        .map(|(cx, cy)| m.apply(Vec2::new(f64::from(rx) + cx, f64::from(ry) + cy)));
    let (mut lo_x, mut lo_y, mut hi_x, mut hi_y) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for c in corners {
        if !c.x.is_finite() || !c.y.is_finite() {
            return;
        }
        (lo_x, lo_y, hi_x, hi_y) = (lo_x.min(c.x), lo_y.min(c.y), hi_x.max(c.x), hi_y.max(c.y));
    }
    let clamp = |v: f64, max: usize| v.clamp(0.0, max as f64) as usize;
    let (x0, y0, x1, y1) = (clamp(lo_x.floor(), img.w), clamp(lo_y.floor(), img.h), clamp(hi_x.ceil(), img.w), clamp(hi_y.ceil(), img.h));
    let colour = premul_linear(s.color);
    if colour[3] <= 0.0 {
        return;
    }
    for oy in y0..y1 {
        for ox in x0..x1 {
            let c = inv.apply(Vec2::new(ox as f64 + 0.5, oy as f64 + 0.5));
            // cell centres sit at whole + 0.5, so the cell to the upper left of the sample is at u - 0.5
            let (u, v) = (c.x - 0.5 - f64::from(rx), c.y - 0.5 - f64::from(ry));
            let (fu, fv) = (u.floor(), v.floor());
            if !fu.is_finite() || !fv.is_finite() || fu < -2.0 || fv < -2.0 || fu > f64::from(rw) + 1.0 || fv > f64::from(rh) + 1.0 {
                continue;
            }
            let (i, j) = (fu as i64, fv as i64);
            let (tx, ty) = ((u - fu) as f32, (v - fv) as f32);
            let coverage = |class: u8| {
                let at = |di: i64, dj: i64| cell(&plane, w, h, i + di, j + dj, class);
                let top = at(0, 0) * (1.0 - tx) + at(1, 0) * tx;
                let bottom = at(0, 1) * (1.0 - tx) + at(1, 1) * tx;
                top * (1.0 - ty) + bottom * ty
            };
            let (over, behind) = (coverage(CLASS_FILLED), coverage(CLASS_EDGE));
            if over <= 0.0 && behind <= 0.0 {
                continue;
            }
            let Some(px) = img.px.get_mut((oy * img.w + ox) * 4..(oy * img.w + ox) * 4 + 4) else { continue };
            if over > 0.0 {
                let inv_a = 1.0 - over * colour[3];
                for (p, src) in px.iter_mut().zip(colour) {
                    *p = src * over + *p * inv_a;
                }
            }
            if behind > 0.0 {
                // destination-over: the colour shows only through what is not yet covered
                let room = (1.0 - px.get(3).copied().unwrap_or(1.0)).clamp(0.0, 1.0);
                for (p, src) in px.iter_mut().zip(colour) {
                    *p += src * behind * room;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use waltcut_project::drawing::{Brush, StrokePoint};
    use waltcut_project::drawing_fill::{MAX_FILL_GAP, MAX_FILL_GROW};

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn stroke(brush: Brush, color: [f32; 4], size: f32, pts: &[(f32, f32)]) -> Stroke {
        Stroke { brush, color, size, points: pts.iter().map(|&(x, y)| StrokePoint { x, y, p: 1.0 }).collect(), response: None, fill: None }
    }

    fn pen(size: f32, pts: &[(f32, f32)]) -> Stroke {
        stroke(Brush::Pen, BLACK, size, pts)
    }

    /// A closed outline through `pts` (the last point joins the first).
    fn outline(size: f32, pts: &[(f32, f32)]) -> Stroke {
        let mut closed = pts.to_vec();
        closed.extend(pts.first().copied());
        pen(size, &closed)
    }

    fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> Stroke {
        outline(4.0, &[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
    }

    fn never() -> bool {
        false
    }

    fn fill_of(strokes: &[Stroke], w: u32, h: u32, seed: [u32; 2], tol: u8) -> Result<Fill, FillError> {
        compute_fill(strokes, w, h, seed, tol, FillOptions::default(), &never).map(|(f, _)| f)
    }

    fn render(strokes: &[Stroke], w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        render_strokes(strokes, &Affine::IDENTITY, &mut img);
        img
    }

    fn with_fill(mut strokes: Vec<Stroke>, fill: Fill, color: [f32; 4]) -> Vec<Stroke> {
        strokes.push(Stroke::fill_op(color, fill));
        strokes
    }

    fn rgba(img: &Image, x: usize, y: usize) -> [f32; 4] {
        img.get(x, y)
    }

    fn is(px: [f32; 4], want: [f32; 4]) -> bool {
        px.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4)
    }

    #[test]
    fn a_closed_shape_fills_inside_and_only_inside() {
        let cel = vec![square(16.0, 16.0, 48.0, 48.0)];
        let fill = fill_of(&cel, 64, 64, [32, 32], 0).unwrap();
        fill.validate(64, 64).unwrap();
        let img = render(&with_fill(cel, fill, RED), 64, 64);
        assert!(is(rgba(&img, 32, 32), [1.0, 0.0, 0.0, 1.0]), "inside is red");
        assert!(is(rgba(&img, 20, 20), RED), "right up to the line");
        assert_eq!(rgba(&img, 4, 4), [0.0; 4], "outside stays empty");
        assert!(is(rgba(&img, 16, 32), [0.0, 0.0, 0.0, 1.0]), "the line is still black, not covered");
    }

    #[test]
    fn the_fill_meets_the_soft_edge_of_a_line_without_a_gap() {
        // lines that do not sit on whole pixels have soft, partly covered edge pixels
        let cel = vec![square(16.37, 16.37, 47.61, 47.61)];
        let fill = fill_of(&cel, 64, 64, [32, 32], 0).unwrap();
        let seams = |fill: Fill| {
            let img = render(&with_fill(cel.clone(), fill, RED), 64, 64);
            // every pixel from the line's solid core to the middle of the shape is opaque (the line's
            // outer soft edge, at x = 14, is outside the shape and stays soft)
            (15..32).filter(|x| rgba(&img, *x, 32)[3] < 0.999).count()
        };
        // the control: the same region without its edge shell leaves see-through pixels beside the line
        let mut bare = fill.decode().unwrap();
        bare.iter_mut().filter(|c| **c == CLASS_EDGE).for_each(|c| *c = CLASS_OUTSIDE);
        let control = Fill::encode(fill.seed, fill.tolerance, fill.rect, &bare).unwrap();
        assert!(seams(control) > 0, "without the shell the soft edge is a see-through seam");
        assert_eq!(seams(fill), 0, "with it, the colour reaches the line");
    }

    #[test]
    fn a_concave_shape_is_filled_including_its_notch_but_not_the_outside() {
        // an L: 20..60 wide at the bottom, a tall arm on the left
        let l = outline(3.0, &[(10.0, 10.0), (30.0, 10.0), (30.0, 40.0), (60.0, 40.0), (60.0, 60.0), (10.0, 60.0)]);
        let cel = vec![l];
        let fill = fill_of(&cel, 80, 80, [15, 15], 0).unwrap();
        let img = render(&with_fill(cel, fill, BLUE), 80, 80);
        assert!(is(rgba(&img, 15, 15), BLUE), "the arm");
        assert!(is(rgba(&img, 50, 50), BLUE), "the far end of the foot, around the corner");
        assert_eq!(rgba(&img, 45, 20), [0.0; 4], "the notch is outside the shape");
        assert_eq!(rgba(&img, 70, 70), [0.0; 4]);
    }

    #[test]
    fn a_hole_is_not_filled_and_filling_the_hole_leaves_the_ring() {
        let cel = vec![square(8.0, 8.0, 56.0, 56.0), square(24.0, 24.0, 40.0, 40.0)];
        let ring = fill_of(&cel, 64, 64, [12, 32], 0).unwrap();
        let out = render(&with_fill(cel.clone(), ring, RED), 64, 64);
        assert!(is(rgba(&out, 12, 32), RED), "the ring between the outlines");
        assert_eq!(rgba(&out, 32, 32), [0.0; 4], "the hole stays empty");
        let hole = fill_of(&cel, 64, 64, [32, 32], 0).unwrap();
        let inner = render(&with_fill(cel, hole, BLUE), 64, 64);
        assert!(is(rgba(&inner, 32, 32), BLUE));
        assert_eq!(rgba(&inner, 12, 32), [0.0; 4], "the ring is not touched");
    }

    #[test]
    fn tolerance_decides_whether_a_translucent_line_holds_the_region() {
        let marker = stroke(Brush::Marker, BLACK, 4.0, &[(32.0, 0.0), (32.0, 63.0)]);
        let cel = vec![marker];
        let strict = fill_of(&cel, 64, 64, [5, 32], 0).unwrap();
        let leaky = fill_of(&cel, 64, 64, [5, 32], 200).unwrap();
        let (a, b) = (strict.filled_pixels(), leaky.filled_pixels());
        assert!(a < 64 * 64 / 2 + 64, "tolerance 0 stops at the marker: {a}");
        assert_eq!(b, 64 * 64, "a tolerance above the marker's alpha crosses it: {b}");
    }

    #[test]
    fn a_seed_on_paint_fills_that_paint_and_one_on_a_fill_recolours_it() {
        let cel = vec![square(16.0, 16.0, 48.0, 48.0)];
        let first = fill_of(&cel, 64, 64, [32, 32], 0).unwrap();
        let red = with_fill(cel, first, RED);
        // a second fill seeded inside the red one finds exactly the red region
        let again = fill_of(&red, 64, 64, [32, 32], 0).unwrap();
        let blue = render(&with_fill(red, again, BLUE), 64, 64);
        assert!(is(rgba(&blue, 32, 32), BLUE) && is(rgba(&blue, 20, 20), BLUE));
        // seeded on the line: the line itself is the region
        let on_line = fill_of(&[square(16.0, 16.0, 48.0, 48.0)], 64, 64, [32, 16], 0).unwrap();
        assert!(on_line.filled_pixels() < 64 * 4 * 6, "only the outline: {}", on_line.filled_pixels());
    }

    #[test]
    fn corner_seeds_and_an_empty_canvas() {
        let none = fill_of(&[], 64, 48, [0, 0], 0).unwrap();
        assert_eq!(none.rect, [0, 0, 64, 48]);
        assert_eq!(none.runs, vec![[1, 64 * 48]]);
        let cel = vec![square(16.0, 16.0, 48.0, 40.0)];
        for seed in [[0, 0], [63, 0], [0, 47], [63, 47]] {
            let f = fill_of(&cel, 64, 48, seed, 0).unwrap();
            f.validate(64, 48).unwrap();
            let img = render(&with_fill(cel.clone(), f, RED), 64, 48);
            assert!(is(rgba(&img, seed[0] as usize, seed[1] as usize), RED), "corner {seed:?}");
            assert_eq!(rgba(&img, 32, 28), [0.0; 4], "the inside of the shape was not reached from outside");
        }
    }

    #[test]
    fn hostile_sizes_and_seeds_are_refused_before_anything_is_allocated() {
        for (w, h) in [(0, 10), (10, 0), (0, 0), (8192, 8192), (u32::MAX, u32::MAX), (4097, 2161)] {
            let e = fill_of(&[], w, h, [0, 0], 0).unwrap_err();
            assert!(matches!(e, FillError::Refused(_)), "{w}x{h}");
        }
        for seed in [[64, 0], [0, 64], [u32::MAX, u32::MAX]] {
            assert!(matches!(fill_of(&[], 64, 64, seed, 0), Err(FillError::Refused(_))), "{seed:?}");
        }
        // the largest accepted canvas is still fine (one run)
        let big = fill_of(&[], 4096, 2160, [4095, 2159], 0).unwrap();
        assert_eq!(big.runs, vec![[1, 4096 * 2160]]);
        // damaged strokes in the cel never panic the rasteriser feeding the flood
        let wild = stroke(Brush::Pencil, BLACK, 1e6, &[(f32::NAN, 0.0), (-1e30, 1e30), (8.0, 8.0)]);
        fill_of(&[wild], 16, 16, [1, 1], 255).unwrap();
    }

    #[test]
    fn paint_fill_erase_in_order() {
        let line = pen(6.0, &[(0.0, 32.0), (63.0, 32.0)]);
        let bg = fill_of(&[], 64, 64, [1, 1], 0).unwrap();
        // fill, then a pen stroke over it, then an eraser over both
        let cel = vec![Stroke::fill_op(RED, bg), line.clone(), stroke(Brush::Eraser, BLACK, 8.0, &[(32.0, 0.0), (32.0, 63.0)])];
        let img = render(&cel, 64, 64);
        assert!(is(rgba(&img, 10, 10), RED), "fill shows");
        assert!(is(rgba(&img, 10, 32), BLACK), "a later stroke covers the fill");
        assert_eq!(rgba(&img, 32, 10), [0.0; 4], "a later eraser removes the fill");
        assert_eq!(rgba(&img, 32, 32), [0.0; 4], "and the stroke");
        // an eraser *before* the fill does not touch it
        let early = vec![stroke(Brush::Eraser, BLACK, 8.0, &[(32.0, 0.0), (32.0, 63.0)]), cel[0].clone()];
        assert!(is(rgba(&render(&early, 64, 64), 32, 10), RED));
        // a translucent fill lets what is under it through
        let half = Stroke::fill_op([1.0, 0.0, 0.0, 0.5], fill_of(&[], 64, 64, [1, 1], 0).unwrap());
        let mixed = render(&[line, half], 64, 64);
        assert!(rgba(&mixed, 10, 32)[3] > 0.99 && rgba(&mixed, 10, 10)[3] > 0.49 && rgba(&mixed, 10, 10)[3] < 0.51);
    }

    #[test]
    fn the_same_cel_gives_the_same_fill_and_the_same_pixels() {
        let cel = vec![square(16.0, 16.0, 48.0, 48.0), stroke(Brush::Pencil, BLACK, 5.0, &[(10.0, 10.0), (50.0, 30.0)])];
        let a = fill_of(&cel, 64, 64, [30, 40], 24).unwrap();
        assert_eq!(a, fill_of(&cel, 64, 64, [30, 40], 24).unwrap());
        let one = render(&with_fill(cel.clone(), a.clone(), RED), 64, 64);
        let two = render(&with_fill(cel, a, RED), 64, 64);
        assert_eq!(one.px, two.px);
    }

    #[test]
    fn a_region_too_ragged_to_store_is_refused() {
        let dots: Vec<Stroke> = (0..210).flat_map(|j| (0..210).map(move |i| pen(1.0, &[(1.5 + i as f32 * 2.0, 1.5 + j as f32 * 2.0)]))).collect();
        let e = fill_of(&dots, 420, 420, [0, 0], 0).unwrap_err();
        match e {
            FillError::Refused(why) => assert!(why.contains("too ragged"), "{why}"),
            FillError::Cancelled => panic!("not cancelled"),
        }
    }

    #[test]
    fn cancelling_stops_the_work_and_stores_nothing() {
        // a callback that says "stop" from its n-th look onwards
        let after = |n: u32| {
            let asked = Cell::new(0);
            move || {
                asked.set(asked.get() + 1);
                asked.get() > n
            }
        };
        assert_eq!(compute_fill(&[], 1000, 1000, [0, 0], 0, FillOptions::default(), &|| true).unwrap_err(), FillError::Cancelled);
        let cel = vec![square(100.0, 100.0, 900.0, 900.0)];
        // cancelled while rasterising (before the flood), in the flood, and while closing the shell
        assert_eq!(compute_fill(&cel, 1000, 1000, [500, 500], 0, FillOptions::default(), &|| true).unwrap_err(), FillError::Cancelled);
        assert_eq!(compute_fill(&cel, 1000, 1000, [500, 500], 0, FillOptions::default(), &after(1)).unwrap_err(), FillError::Cancelled);
        assert_eq!(compute_fill(&cel, 1000, 1000, [500, 500], 0, FillOptions::default(), &after(3)).unwrap_err(), FillError::Cancelled);
        // never cancelled: it finishes
        assert!(compute_fill(&cel, 1000, 1000, [500, 500], 0, FillOptions::default(), &never).is_ok());
    }

    #[test]
    fn a_whole_pixel_move_is_exact_and_scaling_and_rotation_keep_the_fill_in_place() {
        let cel = vec![square(16.0, 16.0, 48.0, 48.0)];
        let fill = fill_of(&cel, 64, 64, [32, 32], 0).unwrap();
        let op = Stroke::fill_op(RED, fill);
        let mut at = Image::new(80, 80);
        render_strokes(std::slice::from_ref(&op), &Affine::translate(7.0, 3.0), &mut at);
        let mut plain = Image::new(64, 64);
        render_strokes(std::slice::from_ref(&op), &Affine::IDENTITY, &mut plain);
        for y in 0..64 {
            for x in 0..64 {
                assert_eq!(at.get(x + 7, y + 3), plain.get(x, y), "({x},{y}) moves whole");
            }
        }
        // half size: the middle is still the colour, outside still empty, nothing is out of range
        let mut half = Image::new(32, 32);
        render_strokes(std::slice::from_ref(&op), &Affine::scale(0.5, 0.5), &mut half);
        assert!(is(half.get(16, 16), RED) && half.get(2, 2) == [0.0; 4]);
        // twice the size: edges are one canvas pixel wide, the middle is solid
        let mut twice = Image::new(128, 128);
        render_strokes(std::slice::from_ref(&op), &Affine::scale(2.0, 2.0), &mut twice);
        assert!(is(twice.get(64, 64), RED) && twice.get(4, 4) == [0.0; 4]);
        // turned a quarter about the middle: the square is still a square
        let spin = Affine::translate(32.0, 32.0).then_apply(&Affine::rotate_deg(90.0)).then_apply(&Affine::translate(-32.0, -32.0));
        let mut turned = Image::new(64, 64);
        render_strokes(std::slice::from_ref(&op), &spin, &mut turned);
        assert!(is(turned.get(32, 32), RED));
        assert!(turned.px.iter().all(|v| v.is_finite() && *v >= -1e-6 && *v <= 1.0 + 1e-6));
        // hostile placements draw nothing and do not panic
        for m in [Affine::scale(0.0, 0.0), Affine::scale(f64::NAN, 1.0), Affine::translate(1e300, 1e300), Affine::scale(1e-300, 1e-300)] {
            render_strokes(std::slice::from_ref(&op), &m, &mut Image::new(16, 16));
        }
        render_strokes(std::slice::from_ref(&op), &Affine::IDENTITY, &mut Image::new(0, 0));
    }

    #[test]
    fn a_damaged_fill_draws_nothing() {
        let mut op =
            Stroke::fill_op(RED, Fill { seed: [0, 0], tolerance: 0, rect: [0, 0, u32::MAX, u32::MAX], runs: vec![[1, 1]], gap: 0, grow: 0, reference: None });
        let mut img = Image::new(8, 8);
        render_strokes(std::slice::from_ref(&op), &Affine::IDENTITY, &mut img);
        assert!(img.px.iter().all(|v| *v == 0.0));
        op.fill = Some(std::sync::Arc::new(Fill { seed: [0, 0], tolerance: 0, rect: [0, 0, 2, 2], runs: vec![[1, 3]], gap: 0, grow: 0, reference: None }));
        render_strokes(std::slice::from_ref(&op), &Affine::IDENTITY, &mut img);
        assert!(img.px.iter().all(|v| *v == 0.0));
    }
    // ---- gap closure and edge expansion (#69) ----

    fn with_opts(strokes: &[Stroke], w: u32, h: u32, seed: [u32; 2], tol: u8, gap: u8, grow: u8) -> Result<Fill, FillError> {
        compute_fill(strokes, w, h, seed, tol, FillOptions { gap, grow }, &never).map(|(f, _)| f)
    }

    /// A 64 × 64 square outline (2 px lines on whole pixels) from (16, 16) to (80, 80) whose top edge
    /// is open by exactly `opening` transparent pixels (measured on the raster, not assumed).
    fn open_square(opening: usize) -> Vec<Stroke> {
        let (a, y) = (40.0f32, 16.0f32);
        for b in (a as usize + 1)..(a as usize + 60) {
            let cel =
                vec![pen(2.0, &[(16.0, y), (a, y)]), pen(2.0, &[(b as f32, y), (80.0, y)]), pen(2.0, &[(80.0, y), (80.0, 80.0), (16.0, 80.0), (16.0, y)])];
            let img = render(&cel, 96, 96);
            let row = |yy: usize| (24..80).filter(|x| rgba(&img, *x, yy)[3] == 0.0).count();
            if row(15) == opening && row(16) == opening {
                return cel;
            }
        }
        Vec::new()
    }

    fn leaks(cel: &[Stroke], gap: u8) -> bool {
        match with_opts(cel, 96, 96, [48, 48], 0, gap, 0) {
            Ok(f) => {
                let img = render(&with_fill(cel.to_vec(), f, RED), 96, 96);
                is(rgba(&img, 4, 4), RED)
            }
            Err(_) => false,
        }
    }

    #[test]
    fn a_gap_up_to_twice_the_radius_is_closed_and_one_more_leaks() {
        for r in [1u8, 2, 3, 4, 8] {
            let closed = open_square(2 * usize::from(r));
            let wider = open_square(2 * usize::from(r) + 1);
            assert!(!closed.is_empty() && !wider.is_empty(), "fixtures for r = {r}");
            assert!(leaks(&closed, 0), "without a radius the opening leaks (r = {r})");
            assert!(!leaks(&closed, r), "an opening of {} px is closed by radius {r}", 2 * r);
            assert!(leaks(&wider, r), "an opening of {} px leaks through radius {r}", 2 * r + 1);
            // the closed fill still colours the inside, reaches the lines, and pokes out by at most r
            let f = with_opts(&closed, 96, 96, [48, 48], 0, r, 0).unwrap();
            f.validate(96, 96).unwrap();
            assert_eq!((f.gap, f.grow, f.reference), (r, 0, None));
            let img = render(&with_fill(closed.clone(), f, RED), 96, 96);
            assert!(is(rgba(&img, 48, 48), RED) && is(rgba(&img, 20, 60), RED) && is(rgba(&img, 60, 20), RED), "the inside, up to the walls (r = {r})");
            assert_eq!(rgba(&img, 48, 16usize.saturating_sub(usize::from(r) + 4))[3], 0.0, "the outside beyond the nub stays empty (r = {r})");
            assert_eq!(rgba(&img, 4, 4)[3], 0.0);
        }
    }

    #[test]
    fn gap_closing_is_deterministic_and_the_stored_seed_is_in_the_region() {
        let cel = open_square(6);
        let a = with_opts(&cel, 96, 96, [48, 48], 0, 4, 0).unwrap();
        assert_eq!(a, with_opts(&cel, 96, 96, [48, 48], 0, 4, 0).unwrap());
        // a seed right beside a wall starts from the nearest pixel away from it
        let near = with_opts(&cel, 96, 96, [48, 19], 0, 4, 0).unwrap();
        near.validate(96, 96).unwrap();
        assert_ne!(near.seed, [48, 19]);
        let img = render(&with_fill(cel.clone(), near, RED), 96, 96);
        assert!(is(rgba(&img, 48, 60), RED) && rgba(&img, 4, 4)[3] == 0.0);
        // a corridor narrower than the closing radius has no pixel to start from
        let corridor = vec![pen(2.0, &[(0.0, 10.0), (63.0, 10.0)]), pen(2.0, &[(0.0, 17.0), (63.0, 17.0)])];
        let e = with_opts(&corridor, 64, 64, [30, 13], 0, 8, 0).unwrap_err();
        assert!(matches!(&e, FillError::Refused(why) if why.contains("too close to a line")), "{e}");
        assert!(with_opts(&corridor, 64, 64, [30, 13], 0, 0, 0).is_ok(), "no radius, no problem");
    }

    #[test]
    fn closing_never_crosses_a_solid_wall_or_a_t_junction_or_an_acute_corner() {
        // a T: a wall with a stem; both sides of the stem stay separate regions
        let tee = vec![pen(3.0, &[(0.0, 20.0), (63.0, 20.0)]), pen(3.0, &[(32.0, 20.0), (32.0, 63.0)])];
        for gap in [0u8, 3, 8] {
            let left = with_opts(&tee, 64, 64, [10, 45], 0, gap, 0).unwrap();
            let img = render(&with_fill(tee.clone(), left, RED), 64, 64);
            assert!(is(rgba(&img, 10, 45), RED), "gap {gap}");
            assert_eq!(rgba(&img, 54, 45)[3], 0.0, "the other side of the stem, gap {gap}");
            assert_eq!(rgba(&img, 10, 5)[3], 0.0, "above the wall, gap {gap}");
        }
        // an acute corner (a 20 degree wedge): the tip narrower than the closing radius is left to
        // the edge, but nothing leaks and the wide part is filled
        let wedge = outline(2.0, &[(8.0, 32.0), (88.0, 22.0), (88.0, 42.0)]);
        let f = with_opts(std::slice::from_ref(&wedge), 96, 64, [70, 32], 0, 4, 0).unwrap();
        let img = render(&with_fill(vec![wedge], f, RED), 96, 64);
        assert!(is(rgba(&img, 70, 32), RED) && rgba(&img, 4, 4)[3] == 0.0 && rgba(&img, 40, 5)[3] == 0.0);
    }

    #[test]
    fn grown_edges_go_under_a_line_and_never_cover_it() {
        // a 10 px wide vertical bar splits the canvas; fill the left side
        let bar = vec![stroke(Brush::Pen, BLACK, 10.0, &[(32.0, 0.0), (32.0, 63.0)])];
        for grow in [0u8, 1, 3] {
            let f = with_opts(&bar, 64, 64, [5, 30], 0, 0, grow).unwrap();
            let plane = f.decode().unwrap();
            let [rx, _, rw, _] = f.rect;
            let row = |y: u32| -> Vec<u8> { (0..rw).map(|i| plane[(y - f.rect[1]) as usize * rw as usize + i as usize]).collect() };
            let r = row(30);
            let last_fill = r.iter().rposition(|c| *c == CLASS_FILLED).unwrap();
            let edge = r.iter().skip(last_fill + 1).take_while(|c| **c == CLASS_EDGE).count();
            assert_eq!(edge, usize::from(grow) + 1, "grow {grow}: edge width past the region at x = {}", rx as usize + last_fill);
            assert_eq!((f.grow, f.gap), (grow, 0));
            // drawn in the same cel the bar is never covered
            let img = render(&with_fill(bar.clone(), f.clone(), RED), 64, 64);
            assert!(is(rgba(&img, 32, 30), BLACK) && is(rgba(&img, 30, 30), BLACK), "the line is still black");
            // drawn on its own (a colour cel under line art) the colour reaches under where the line is
            let alone = render(&[Stroke::fill_op(RED, f)], 64, 64);
            let under = (27..38).filter(|x| is(rgba(&alone, *x, 30), RED)).count();
            assert_eq!(under, 1 + usize::from(grow), "colour under the line, grow {grow}");
        }
    }

    #[test]
    fn hostile_options_are_refused() {
        assert!(matches!(with_opts(&[], 16, 16, [1, 1], 0, MAX_FILL_GAP + 1, 0), Err(FillError::Refused(_))));
        assert!(matches!(with_opts(&[], 16, 16, [1, 1], 0, 0, MAX_FILL_GROW + 1), Err(FillError::Refused(_))));
        assert!(matches!(with_opts(&[], 16, 16, [1, 1], 0, 255, 255), Err(FillError::Refused(_))));
        // the largest radius and growth on tiny and awkward canvases never panic
        for (w, h) in [(1u32, 1u32), (1, 40), (40, 1), (3, 3), (17, 5)] {
            let f = with_opts(&[], w, h, [0, 0], 0, MAX_FILL_GAP, MAX_FILL_GROW).unwrap();
            f.validate(w, h).unwrap();
        }
        let wild = stroke(Brush::Pencil, BLACK, 1e6, &[(f32::NAN, 0.0), (-1e30, 1e30), (8.0, 8.0)]);
        let _ = with_opts(&[wild], 16, 16, [1, 1], 255, 8, 8);
    }

    #[test]
    fn closing_and_growing_can_be_cancelled_at_any_look() {
        let cel = open_square(4);
        let mut stopped = 0;
        for n in 0..400u32 {
            let asked = Cell::new(0u32);
            let stop = || {
                asked.set(asked.get() + 1);
                asked.get() > n
            };
            match compute_fill(&cel, 96, 96, [48, 48], 0, FillOptions { gap: 8, grow: 8 }, &stop) {
                Err(FillError::Cancelled) => stopped += 1,
                Ok(_) => break,
                Err(e) => panic!("unexpected {e}"),
            }
        }
        assert!(stopped > 100, "it looks often enough to be stopped ({stopped})");
    }
}
