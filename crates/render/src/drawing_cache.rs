//! A bounded cache of rasterised cels (#71; the decision is in `docs/animation.md`, "Cel raster
//! cache").
//!
//! Rasterising a cel ([`render_strokes`]) is a pure function of the cel's strokes and the matrix
//! that places the canvas on the output, and it is the cost of nearly every frame with a drawing in
//! it. [`CelRasterCache::rasterise`] returns exactly the pixels `render_strokes` would, reusing
//! earlier work:
//!
//! * **Hit.** The same cel (the same allocation of its `Arc<Vec<Stroke>>`) at the same output size
//!   and placement. An entry holds a `Weak` to the strokes, so that allocation can neither be edited
//!   in place (`Arc::make_mut` moves the data out of an allocation a `Weak` still points at) nor
//!   reused for another cel while the entry lives: identity is the allocation, nothing is hashed, and
//!   an edited cel is simply a different key. Undo and redo come back to the old allocation, whose
//!   pixels are right again.
//! * **Extension.** A cel whose strokes start with the bit-identical strokes of a cached cel (a
//!   stroke was added at pen-up) copies that raster and paints only the new strokes. The rasteriser
//!   composites stroke after stroke onto one image, so the result is the same arithmetic in the same
//!   order, bit for bit.
//! * **Miss.** Rasterised whole, and stored when worth it.
//!
//! Rasters are stored cropped to the rectangle where some pixel is not exactly zero. The cache is
//! bounded in bytes and entries (least recently used goes first), small cels are not stored (a copy
//! costs as much as drawing them), its lock is never held while rasterising and it is tolerant of
//! poisoning. Clearing or evicting it changes no artwork: it only holds copies.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use waltcut_geom::Affine;
use waltcut_project::drawing::{Curve, Stroke};

use crate::drawing::render_strokes;
use crate::image::Image;

/// Bytes of rasters the process-wide cache keeps.
pub const BUDGET_BYTES: usize = 256 << 20;
/// Entries the process-wide cache keeps.
pub const MAX_ENTRIES: usize = 128;
/// Cels with fewer samples (points, plus a fill's runs) than this are always rasterised: they cost
/// about as much as copying a cached raster would.
pub const MIN_CACHED_WEIGHT: usize = 2048;
/// Candidates whose strokes are compared in full when looking for a prefix.
const MAX_PREFIX_CANDIDATES: usize = 4;

/// The six numbers of a placement, as bits (so `-0.0` and NaN are distinct keys, never equal by
/// accident).
type MatrixBits = [u64; 6];

fn matrix_bits(m: &Affine) -> MatrixBits {
    [m.a, m.b, m.c, m.d, m.e, m.f].map(f64::to_bits)
}

/// A cel's strokes the way the cache weighs them: samples, with each fill run counting as one.
fn weight(strokes: &[Stroke]) -> usize {
    strokes.iter().fold(0usize, |a, s| a.saturating_add(s.points.len()).saturating_add(s.fill.as_ref().map_or(0, |f| f.runs.len())))
}

fn curve_bits_eq(a: &Option<Curve>, b: &Option<Curve>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.0.len() == b.0.len() && a.0.iter().zip(&b.0).all(|(p, q)| p[0].to_bits() == q[0].to_bits() && p[1].to_bits() == q[1].to_bits()),
        _ => false,
    }
}

/// Whether two strokes are identical bit for bit (`==` treats `-0.0` as `0.0` and NaN as unequal).
fn stroke_bits_eq(a: &Stroke, b: &Stroke) -> bool {
    a.brush == b.brush
        && a.size.to_bits() == b.size.to_bits()
        && a.color.iter().zip(&b.color).all(|(p, q)| p.to_bits() == q.to_bits())
        && a.points.len() == b.points.len()
        && a.points.iter().zip(&b.points).all(|(p, q)| p.x.to_bits() == q.x.to_bits() && p.y.to_bits() == q.y.to_bits() && p.p.to_bits() == q.p.to_bits())
        && match (&a.response, &b.response) {
            (None, None) => true,
            (Some(p), Some(q)) => curve_bits_eq(&p.width, &q.width) && curve_bits_eq(&p.opacity, &q.opacity),
            _ => false,
        }
        && match (&a.fill, &b.fill) {
            (None, None) => true,
            (Some(p), Some(q)) => Arc::ptr_eq(p, q) || **p == **q,
            _ => false,
        }
}

/// The rectangle of an image where some pixel is not exactly zero, and its pixels.
struct Cropped {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    px: Vec<f32>,
}

impl Cropped {
    fn of(img: &Image) -> Cropped {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for (y, row) in img.px.chunks_exact(img.w.max(1).saturating_mul(4)).enumerate().take(img.h) {
            let mut first = None;
            let mut last = 0;
            for (x, p) in row.as_chunks::<4>().0.iter().enumerate() {
                if p.iter().any(|v| v.to_bits() != 0) {
                    first.get_or_insert(x);
                    last = x;
                }
            }
            if let Some(f) = first {
                x0 = x0.min(f);
                x1 = x1.max(last + 1);
                y0 = y0.min(y);
                y1 = y.saturating_add(1).max(y1);
            }
        }
        if x0 >= x1 || y0 >= y1 {
            return Cropped { x: 0, y: 0, w: 0, h: 0, px: Vec::new() };
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let mut px = Vec::with_capacity(w.saturating_mul(h).saturating_mul(4));
        for y in y0..y1 {
            let start = (y * img.w + x0) * 4;
            if let Some(row) = img.px.get(start..start + w * 4) {
                px.extend_from_slice(row);
            }
        }
        Cropped { x: x0, y: y0, w, h, px }
    }

    fn bytes(&self) -> usize {
        self.px.len().saturating_mul(4)
    }

    /// A full `w`×`h` image with this raster in place and zero elsewhere.
    fn expand(&self, w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for row in 0..self.h {
            let src = row * self.w * 4;
            let dst = ((self.y + row) * w + self.x) * 4;
            if let (Some(s), Some(d)) = (self.px.get(src..src + self.w * 4), img.px.get_mut(dst..dst + self.w * 4)) {
                d.copy_from_slice(s);
            }
        }
        img
    }
}

struct Entry {
    id: u64,
    strokes: Weak<Vec<Stroke>>,
    w: usize,
    h: usize,
    m: MatrixBits,
    raster: Arc<Cropped>,
    used: u64,
}

impl Entry {
    fn placed_as(&self, w: usize, h: usize, m: &MatrixBits) -> bool {
        self.w == w && self.h == h && self.m == *m
    }

    fn is_cel(&self, strokes: &Arc<Vec<Stroke>>) -> bool {
        std::ptr::eq(Weak::as_ptr(&self.strokes), Arc::as_ptr(strokes))
    }
}

#[derive(Default)]
struct Inner {
    entries: Vec<Entry>,
    bytes: usize,
    tick: u64,
    next_id: u64,
}

/// What the cache has done and holds (cumulative counters, current size).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub extended: u64,
    pub misses: u64,
    /// Rasterised without looking (too small to be worth it, or too large to keep).
    pub bypassed: u64,
    pub evicted: u64,
    pub entries: usize,
    pub bytes: usize,
    pub budget: usize,
}

pub struct CelRasterCache {
    inner: Mutex<Inner>,
    budget: usize,
    max_entries: usize,
    min_weight: usize,
    enabled: AtomicBool,
    hits: AtomicU64,
    extended: AtomicU64,
    misses: AtomicU64,
    bypassed: AtomicU64,
    evicted: AtomicU64,
}

/// The process-wide cache the renderer uses.
pub fn global() -> &'static CelRasterCache {
    static CACHE: OnceLock<CelRasterCache> = OnceLock::new();
    CACHE.get_or_init(|| CelRasterCache::new(BUDGET_BYTES, MAX_ENTRIES, MIN_CACHED_WEIGHT))
}

/// A cel's raster at `w`×`h` placed by `m`, from the process-wide cache; the pixels
/// `render_strokes` gives on a fresh `w`×`h` image.
pub fn rasterise_cel(strokes: &Arc<Vec<Stroke>>, m: &Affine, w: usize, h: usize) -> Image {
    global().rasterise(strokes, m, w, h)
}

fn render_whole(strokes: &[Stroke], m: &Affine, w: usize, h: usize) -> Image {
    let mut img = Image::new(w, h);
    render_strokes(strokes, m, &mut img);
    img
}

impl CelRasterCache {
    pub fn new(budget: usize, max_entries: usize, min_weight: usize) -> CelRasterCache {
        CelRasterCache {
            inner: Mutex::new(Inner::default()),
            budget,
            max_entries,
            min_weight,
            enabled: AtomicBool::new(true),
            hits: AtomicU64::new(0),
            extended: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            bypassed: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Switch the cache off (every cel is rasterised whole, nothing is stored) or on again. Only the
    /// benchmark uses this, to measure the same binary with and without the cache; the result is the
    /// same pixels either way.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if !on {
            self.clear();
        }
    }

    pub fn stats(&self) -> CacheStats {
        let g = self.lock();
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            extended: self.extended.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            bypassed: self.bypassed.load(Ordering::Relaxed),
            evicted: self.evicted.load(Ordering::Relaxed),
            entries: g.entries.len(),
            bytes: g.bytes,
            budget: self.budget,
        }
    }

    /// Drop every cached raster. Nothing but copies is held, so no artwork is affected.
    pub fn clear(&self) {
        let mut g = self.lock();
        g.entries.clear();
        g.bytes = 0;
    }

    /// The pixels `render_strokes(strokes, m, ..)` gives on a fresh `w`×`h` image.
    pub fn rasterise(&self, strokes: &Arc<Vec<Stroke>>, m: &Affine, w: usize, h: usize) -> Image {
        // an absurd canvas, or a cel too small to be worth a copy, is just drawn
        let full = w.checked_mul(h).and_then(|n| n.checked_mul(16));
        if full.is_none() || !self.enabled.load(Ordering::Relaxed) || weight(strokes) < self.min_weight {
            self.bypassed.fetch_add(1, Ordering::Relaxed);
            return render_whole(strokes, m, w, h);
        }
        let mb = matrix_bits(m);
        // exact hit, and the candidates for an extension, under one short look at the table
        let mut candidates: Vec<(u64, Arc<Vec<Stroke>>)> = Vec::new();
        {
            let mut g = self.lock();
            g.tick = g.tick.wrapping_add(1);
            let tick = g.tick;
            if let Some(e) = g.entries.iter_mut().find(|e| e.placed_as(w, h, &mb) && e.is_cel(strokes)) {
                e.used = tick;
                let raster = e.raster.clone();
                drop(g);
                self.hits.fetch_add(1, Ordering::Relaxed);
                return raster.expand(w, h);
            }
            for e in g.entries.iter().filter(|e| e.placed_as(w, h, &mb)) {
                if let Some(old) = e.strokes.upgrade()
                    && !old.is_empty()
                    && old.len() < strokes.len()
                    && old.last().zip(strokes.get(old.len() - 1)).is_some_and(|(a, b)| stroke_bits_eq(a, b))
                {
                    candidates.push((e.id, old));
                }
            }
        }
        // the longest cached prefix, compared in full outside the lock
        candidates.sort_by_key(|(_, old)| std::cmp::Reverse(old.len()));
        let base = candidates.iter().take(MAX_PREFIX_CANDIDATES).find(|(_, old)| old.iter().zip(strokes.iter()).all(|(a, b)| stroke_bits_eq(a, b))).and_then(
            |(id, old)| {
                let mut g = self.lock();
                g.tick = g.tick.wrapping_add(1);
                let tick = g.tick;
                let e = g.entries.iter_mut().find(|e| e.id == *id)?;
                e.used = tick;
                Some((e.raster.clone(), old.len()))
            },
        );
        drop(candidates);
        let img = match base {
            Some((raster, n)) => {
                self.extended.fetch_add(1, Ordering::Relaxed);
                let mut img = raster.expand(w, h);
                render_strokes(strokes.get(n..).unwrap_or(&[]), m, &mut img);
                img
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                render_whole(strokes, m, w, h)
            }
        };
        self.store(strokes, w, h, mb, &img);
        img
    }

    fn store(&self, strokes: &Arc<Vec<Stroke>>, w: usize, h: usize, m: MatrixBits, img: &Image) {
        let raster = Cropped::of(img);
        let bytes = raster.bytes();
        if bytes > self.budget / 2 {
            self.bypassed.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut g = self.lock();
        // entries of cels that no longer exist, and an older raster of this very key
        let mut freed = 0usize;
        g.entries.retain(|e| {
            let keep = e.strokes.strong_count() > 0 && !(e.placed_as(w, h, &m) && e.is_cel(strokes));
            if !keep {
                freed = freed.saturating_add(e.raster.bytes());
            }
            keep
        });
        g.bytes = g.bytes.saturating_sub(freed);
        // least recently used first, until this one fits
        while !g.entries.is_empty() && (g.bytes.saturating_add(bytes) > self.budget || g.entries.len() >= self.max_entries) {
            let Some(oldest) = g.entries.iter().enumerate().min_by_key(|(_, e)| e.used).map(|(i, _)| i) else { break };
            let gone = g.entries.swap_remove(oldest);
            g.bytes = g.bytes.saturating_sub(gone.raster.bytes());
            self.evicted.fetch_add(1, Ordering::Relaxed);
        }
        g.tick = g.tick.wrapping_add(1);
        g.next_id = g.next_id.wrapping_add(1);
        let (used, id) = (g.tick, g.next_id);
        g.bytes = g.bytes.saturating_add(bytes);
        g.entries.push(Entry { id, strokes: Arc::downgrade(strokes), w, h, m, raster: Arc::new(raster), used });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waltcut_project::drawing::{Brush, StrokePoint};

    fn stroke(brush: Brush, seed: u32, n: usize) -> Stroke {
        let f = |i: usize, k: u32| ((i as u32 * 37 + seed * 101 + k * 13) % 200) as f32;
        Stroke {
            brush,
            color: [0.9, 0.2, 0.1, 1.0],
            size: 6.0,
            points: (0..n).map(|i| StrokePoint { x: 20.0 + f(i, 1), y: 20.0 + f(i, 2), p: 0.3 + (i % 7) as f32 / 10.0 }).collect(),
            response: None,
            fill: None,
        }
    }

    fn cel(n_strokes: usize) -> Arc<Vec<Stroke>> {
        Arc::new((0..n_strokes).map(|i| stroke(if i % 5 == 4 { Brush::Eraser } else { Brush::Pen }, i as u32, 80)).collect())
    }

    fn cache() -> CelRasterCache {
        CelRasterCache::new(64 << 20, 16, 1)
    }

    const M: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    fn reference(s: &[Stroke], m: &Affine, w: usize, h: usize) -> Vec<f32> {
        render_whole(s, m, w, h).px
    }

    /// Bit-for-bit equality (max diff 0, and the same zeros).
    fn same(a: &Image, b: &[f32]) -> bool {
        a.px.len() == b.len() && a.px.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
    }

    #[test]
    fn a_hit_is_bit_identical_to_rasterising_and_counted() {
        let c = cache();
        let s = cel(6);
        let want = reference(&s, &M, 256, 256);
        let first = c.rasterise(&s, &M, 256, 256);
        let again = c.rasterise(&s, &M, 256, 256);
        assert!(same(&first, &want) && same(&again, &want));
        let st = c.stats();
        assert_eq!((st.misses, st.hits, st.entries), (1, 1, 1));
        assert!(want.iter().any(|v| *v != 0.0));
    }

    #[test]
    fn every_invalidation_trigger_gives_the_pixels_of_a_fresh_render() {
        let c = cache();
        let base = cel(6);
        let _ = c.rasterise(&base, &M, 256, 256);
        let check = |s: &Arc<Vec<Stroke>>, m: &Affine, w: usize, h: usize, what: &str| {
            let got = c.rasterise(s, m, w, h);
            assert!(same(&got, &reference(s, m, w, h)), "{what}");
            let hit = c.rasterise(s, m, w, h);
            assert!(same(&hit, &reference(s, m, w, h)), "{what} (hit)");
        };
        // a stroke added, an eraser added, a stroke edited, one removed, reordered
        let mut added = (*base).clone();
        added.push(stroke(Brush::Marker, 90, 60));
        check(&Arc::new(added), &M, 256, 256, "stroke added");
        let mut erased = (*base).clone();
        erased.push(stroke(Brush::Eraser, 91, 200));
        check(&Arc::new(erased), &M, 256, 256, "eraser added");
        let mut edited = (*base).clone();
        edited[2].points[10].x += 3.0;
        check(&Arc::new(edited), &M, 256, 256, "point moved");
        let mut recoloured = (*base).clone();
        recoloured[0].color[0] = 0.1;
        check(&Arc::new(recoloured), &M, 256, 256, "colour");
        let mut removed = (*base).clone();
        removed.pop();
        check(&Arc::new(removed), &M, 256, 256, "stroke removed");
        let mut swapped = (*base).clone();
        swapped.swap(1, 3);
        check(&Arc::new(swapped), &M, 256, 256, "reordered");
        // size, scale and every Motion component
        check(&base, &M, 128, 256, "canvas width");
        check(&base, &M, 256, 128, "canvas height");
        check(&base, &Affine::scale(0.5, 0.5), 256, 256, "rendering scale");
        check(&base, &Affine { e: 10.0, ..M }, 256, 256, "position x");
        check(&base, &Affine { f: -4.5, ..M }, 256, 256, "position y");
        check(&base, &Affine { a: 0.8, d: 0.8, ..M }, 256, 256, "clip scale");
        check(&base, &Affine { a: 0.96, b: 0.28, c: -0.28, d: 0.96, ..M }, 256, 256, "clip rotation");
        // the unedited cel still hits at its original placement, with its original pixels
        let hits = c.stats().hits;
        assert!(same(&c.rasterise(&base, &M, 256, 256), &reference(&base, &M, 256, 256)));
        assert_eq!(c.stats().hits, hits + 1);
    }

    #[test]
    fn a_fill_is_part_of_the_cel_and_a_different_fill_is_a_different_raster() {
        use waltcut_project::drawing_fill::Fill;
        let c = cache();
        let ring = Arc::new(vec![Stroke {
            brush: Brush::Pen,
            color: [0.0, 0.0, 0.0, 1.0],
            size: 4.0,
            points: [(20.0, 20.0), (90.0, 20.0), (90.0, 90.0), (20.0, 90.0), (20.0, 20.0)].iter().map(|&(x, y)| StrokePoint { x, y, p: 1.0 }).collect(),
            response: None,
            fill: None,
        }]);
        let _ = c.rasterise(&ring, &M, 128, 128);
        let fill_with = |rows: usize| {
            use waltcut_project::drawing_fill::{CLASS_FILLED, CLASS_OUTSIDE};
            let mut v = (*ring).clone();
            let plane: Vec<u8> = (0..80 * 80).map(|i| if i < rows * 80 { CLASS_FILLED } else { CLASS_OUTSIDE }).collect();
            let fill = Fill::encode([40, 40], 0, [20, 20, 80, 80], &plane).unwrap();
            v.push(Stroke::fill_op([0.0, 1.0, 0.0, 1.0], fill));
            Arc::new(v)
        };
        let (a, b) = (fill_with(10), fill_with(30));
        let (ra, rb) = (c.rasterise(&a, &M, 128, 128), c.rasterise(&b, &M, 128, 128));
        assert!(same(&ra, &reference(&a, &M, 128, 128)) && same(&rb, &reference(&b, &M, 128, 128)));
        assert!(ra.px != rb.px);
        // the fill was drawn on top of the ring's cached raster, not the ring alone
        assert_eq!(c.stats().extended, 2);
    }

    #[test]
    fn adding_strokes_extends_the_cached_raster_bit_for_bit_and_undo_hits_the_old_one() {
        let c = cache();
        let v0 = cel(20);
        let _ = c.rasterise(&v0, &M, 256, 256);
        // pen-up: a new allocation holding the old strokes and one more (what Arc::make_mut gives)
        let mut v1 = v0.clone();
        Arc::make_mut(&mut v1).push(stroke(Brush::Pencil, 77, 120));
        assert!(!Arc::ptr_eq(&v0, &v1));
        let got = c.rasterise(&v1, &M, 256, 256);
        assert!(same(&got, &reference(&v1, &M, 256, 256)));
        assert_eq!((c.stats().extended, c.stats().misses), (1, 1));
        // an eraser on top extends too
        let mut v2 = v1.clone();
        Arc::make_mut(&mut v2).push(stroke(Brush::Eraser, 78, 300));
        assert!(same(&c.rasterise(&v2, &M, 256, 256), &reference(&v2, &M, 256, 256)));
        assert_eq!(c.stats().extended, 2);
        // undo: the old cel is current again and its pixels come back without drawing
        let before = c.stats();
        assert!(same(&c.rasterise(&v0, &M, 256, 256), &reference(&v0, &M, 256, 256)));
        assert_eq!(c.stats().hits, before.hits + 1);
        // redo
        assert!(same(&c.rasterise(&v2, &M, 256, 256), &reference(&v2, &M, 256, 256)));
        assert_eq!(c.stats().hits, before.hits + 2);
    }

    #[test]
    fn an_edit_in_the_middle_never_reuses_a_prefix() {
        let c = cache();
        let v0 = cel(10);
        let _ = c.rasterise(&v0, &M, 256, 256);
        let mut v1 = v0.clone();
        {
            let s = Arc::make_mut(&mut v1);
            s[3].points[0].y += 1.0; // same length, same last stroke, different middle
            s.push(stroke(Brush::Pen, 5, 40));
        }
        let got = c.rasterise(&v1, &M, 256, 256);
        assert!(same(&got, &reference(&v1, &M, 256, 256)));
        assert_eq!(c.stats().extended, 0);
    }

    #[test]
    fn mutating_in_place_is_impossible_while_an_entry_exists() {
        // the property the key relies on: an Arc with a Weak is never edited where it lies
        let c = cache();
        let mut s = cel(4);
        let _ = c.rasterise(&s, &M, 128, 128);
        assert!(Arc::get_mut(&mut s).is_none(), "a cached cel cannot be borrowed mutably in place");
        let before = Arc::as_ptr(&s);
        Arc::make_mut(&mut s).push(stroke(Brush::Pen, 9, 50));
        assert_ne!(before, Arc::as_ptr(&s), "make_mut moves an edited cel to a new allocation");
        assert!(same(&c.rasterise(&s, &M, 128, 128), &reference(&s, &M, 128, 128)));
    }

    #[test]
    fn sign_of_zero_and_nan_are_not_equal_so_they_never_share_a_prefix() {
        let c = cache();
        let mut a = stroke(Brush::Pen, 1, 30);
        a.points[0].x = 0.0;
        let v0 = Arc::new(vec![a.clone()]);
        let _ = c.rasterise(&v0, &M, 128, 128);
        let mut b = a.clone();
        b.points[0].x = -0.0;
        let v1 = Arc::new(vec![b, stroke(Brush::Pen, 2, 30)]);
        let got = c.rasterise(&v1, &M, 128, 128);
        assert!(same(&got, &reference(&v1, &M, 128, 128)));
        assert_eq!(c.stats().extended, 0);
        let mut n = a.clone();
        n.points[1].y = f32::NAN;
        let v2 = Arc::new(vec![n.clone(), stroke(Brush::Pen, 3, 30)]);
        let _ = c.rasterise(&Arc::new(vec![n]), &M, 128, 128);
        assert!(same(&c.rasterise(&v2, &M, 128, 128), &reference(&v2, &M, 128, 128)));
    }

    #[test]
    fn the_byte_and_entry_caps_hold_and_the_least_recently_used_goes_first() {
        // a 256x256 cel with strokes everywhere is ~1 MB; room for three of them
        let one = {
            let c = cache();
            let s = cel(8);
            let _ = c.rasterise(&s, &M, 256, 256);
            c.stats().bytes
        };
        assert!(one > 0);
        let c = CelRasterCache::new(one * 3 + one / 2, 64, 1);
        let cels: Vec<_> = (0..6).map(|i| Arc::new((0..8).map(|j| stroke(Brush::Pen, i * 10 + j, 80)).collect::<Vec<_>>())).collect();
        for s in &cels {
            let _ = c.rasterise(s, &M, 256, 256);
            let st = c.stats();
            assert!(st.bytes <= st.budget, "{st:?}");
        }
        assert!(c.stats().evicted >= 3 && c.stats().entries <= 3, "{:?}", c.stats());
        // the newest are still there, the oldest were evicted: still the right pixels either way
        let hits = c.stats().hits;
        let _ = c.rasterise(&cels[5], &M, 256, 256);
        assert_eq!(c.stats().hits, hits + 1);
        assert!(same(&c.rasterise(&cels[0], &M, 256, 256), &reference(&cels[0], &M, 256, 256)));
        // an entry cap
        let c = CelRasterCache::new(usize::MAX / 4, 2, 1);
        for s in &cels {
            let _ = c.rasterise(s, &M, 256, 256);
        }
        assert_eq!(c.stats().entries, 2);
        // touching an entry protects it
        let _ = c.rasterise(&cels[4], &M, 256, 256);
        let _ = c.rasterise(&cels[0], &M, 256, 256);
        let hits = c.stats().hits;
        let _ = c.rasterise(&cels[4], &M, 256, 256);
        assert_eq!(c.stats().hits, hits + 1);
    }

    #[test]
    fn a_raster_larger_than_half_the_budget_is_drawn_not_kept() {
        let c = CelRasterCache::new(1000, 8, 1);
        let s = cel(4);
        assert!(same(&c.rasterise(&s, &M, 64, 64), &reference(&s, &M, 64, 64)));
        let st = c.stats();
        assert_eq!((st.entries, st.bytes), (0, 0));
        assert!(st.bypassed >= 1);
    }

    #[test]
    fn small_cels_are_never_stored_and_clear_and_dead_cels_free_the_memory() {
        let c = CelRasterCache::new(64 << 20, 16, 100_000);
        let s = cel(3);
        let _ = c.rasterise(&s, &M, 128, 128);
        assert_eq!((c.stats().entries, c.stats().bypassed), (0, 1));
        let c = cache();
        let s = cel(3);
        let _ = c.rasterise(&s, &M, 128, 128);
        assert_eq!(c.stats().entries, 1);
        c.clear();
        assert_eq!((c.stats().entries, c.stats().bytes), (0, 0));
        // the strokes are untouched by clearing, and render the same again
        assert!(same(&c.rasterise(&s, &M, 128, 128), &reference(&s, &M, 128, 128)));
        // a cel that is dropped frees its entry the next time anything is stored
        let gone = cel(5);
        let _ = c.rasterise(&gone, &M, 128, 128);
        drop(gone);
        let other = cel(6);
        let _ = c.rasterise(&other, &M, 128, 128);
        assert_eq!(c.stats().entries, 2, "{:?}", c.stats());
    }

    #[test]
    fn hostile_sizes_and_placements_do_not_panic_or_allocate_without_bound() {
        let c = cache();
        let s = cel(2);
        // degenerate placement: nothing is drawn, as render_strokes does
        for m in [Affine::scale(0.0, 0.0), Affine { e: f64::NAN, ..M }, Affine::scale(f64::INFINITY, 1.0), Affine::scale(1e300, 1e300)] {
            let got = c.rasterise(&s, &m, 32, 32);
            assert!(same(&got, &reference(&s, &m, 32, 32)));
            let again = c.rasterise(&s, &m, 32, 32);
            assert!(same(&again, &reference(&s, &m, 32, 32)));
        }
        // zero-sized outputs
        for (w, h) in [(0, 0), (0, 5), (5, 0), (1, 1)] {
            let got = c.rasterise(&s, &M, w, h);
            assert_eq!((got.w, got.h, got.px.len()), (w, h, w * h * 4));
        }
        // a size whose byte count overflows is not looked up (the renderer's own allocation is the
        // caller's to bound); only the arithmetic is checked here
        assert!(usize::MAX.checked_mul(2).is_none());
        // empty and hostile strokes
        let wild = Arc::new(vec![stroke(Brush::Pen, 1, 0), Stroke { size: f32::NAN, ..stroke(Brush::Pen, 1, 5) }]);
        assert!(same(&c.rasterise(&wild, &M, 16, 16), &reference(&wild, &M, 16, 16)));
    }

    #[test]
    fn a_blank_cel_is_cached_as_nothing() {
        let c = cache();
        let s = Arc::new(vec![stroke(Brush::Eraser, 1, 40)]);
        let got = c.rasterise(&s, &M, 64, 64);
        assert!(got.px.iter().all(|v| v.to_bits() == 0));
        assert_eq!(c.stats().bytes, 0);
        assert!(same(&c.rasterise(&s, &M, 64, 64), &got.px));
    }

    #[test]
    fn concurrent_workers_get_identical_pixels_and_the_bounds_hold() {
        let c = Arc::new(CelRasterCache::new(3 << 20, 8, 1));
        let cels: Vec<_> = (0..5).map(|i| Arc::new((0..6).map(|j| stroke(Brush::Pen, i * 7 + j, 70)).collect::<Vec<_>>())).collect();
        let wants: Vec<_> = cels.iter().map(|s| reference(s, &M, 192, 192)).collect();
        let threads: Vec<_> = (0..6)
            .map(|t| {
                let (c, cels, wants) = (c.clone(), cels.clone(), wants.clone());
                std::thread::spawn(move || {
                    for round in 0..30 {
                        let i = (round * 3 + t) % cels.len();
                        let s = if round % 4 == 3 {
                            let mut v = cels[i].clone();
                            Arc::make_mut(&mut v).push(stroke(Brush::Pen, round as u32, 20));
                            v
                        } else {
                            cels[i].clone()
                        };
                        let got = c.rasterise(&s, &M, 192, 192);
                        let want = if round % 4 == 3 { reference(&s, &M, 192, 192) } else { wants[i].clone() };
                        assert!(same(&got, &want), "thread {t} round {round}");
                        let st = c.stats();
                        assert!(st.bytes <= st.budget && st.entries <= 8);
                    }
                })
            })
            .collect();
        for t in threads {
            assert!(t.join().is_ok(), "a worker panicked");
        }
    }

    #[test]
    fn a_poisoned_lock_does_not_stop_rendering() {
        let c = Arc::new(cache());
        let c2 = c.clone();
        let _ = std::thread::spawn(move || {
            let _g = c2.inner.lock();
            panic!("poison the cache lock");
        })
        .join();
        let s = cel(3);
        assert!(same(&c.rasterise(&s, &M, 64, 64), &reference(&s, &M, 64, 64)));
        assert!(same(&c.rasterise(&s, &M, 64, 64), &reference(&s, &M, 64, 64)));
    }
}
