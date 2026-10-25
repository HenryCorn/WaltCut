//! Preview rasters for the Brush tool: the live stroke, the stroke between pen-up and the engine's
//! new frame, and the onion skins (#55).
//!
//! All of them are drawn by the engine's own CPU rasteriser (`waltcut_render::drawing`), with the
//! placement the engine uses (canvas → picture → screen), so what the pen shows, what the cel
//! looks like after pen-up and what export renders have the same coverage and the same compositing
//! (a stroke's coverage is the maximum over its segments, not their sum; the eraser removes
//! coverage from the paint under it). The egui painter is not used for strokes any more: it
//! stacked translucent segments into dark joints and skipped erasers.
//!
//! * The **live stroke** is rasterised each frame into a texture covering its own bounding box.
//!   An **eraser** is shown as its footprint, the coverage it will remove (a translucent white),
//!   computed by running the eraser over a fully opaque layer, i.e. by the engine's eraser code.
//! * An **onion skin** is the neighbouring cel rasterised whole (erasers applied, so erased
//!   areas stay absent), tinted, and cached as a texture until the cel, the placement or the
//!   size changes. It is computed on a worker thread (a big cel takes hundreds of ms); the previous
//!   texture stays up until the new one arrives.
//!
//! Display conversion: the rasteriser works in premultiplied linear light, egui blends
//! premultiplied sRGB, so partially covered pixels (antialiased edges, Marker, Pencil) blend
//! slightly differently over the picture than the engine's linear-light composite; opaque pixels
//! are identical. `display_blend_error` documents and tests the size of that difference.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, PoisonError};

use egui::{Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, pos2, vec2};
use waltcut_geom::Affine;
use waltcut_project::drawing::{Brush, Stroke};
use waltcut_render::Image;
use waltcut_render::drawing::render_strokes;

/// Longest side of a preview raster, in pixels.
pub const MAX_SIDE: usize = 4096;

/// Canvas → raster pixels for a raster whose top-left is the screen point `origin`, given the
/// canvas → screen-points mapping and the display's pixels per point.
pub fn image_mapping(to_screen: &Affine, origin: Pos2, ppp: f32) -> Affine {
    let ppp = if ppp.is_finite() && ppp > 0.0 { ppp as f64 } else { 1.0 };
    Affine::scale(ppp, ppp).then_apply(&Affine::translate(-(origin.x as f64), -(origin.y as f64))).then_apply(to_screen)
}

/// The size in pixels of a raster covering `rect` (points), within [`MAX_SIDE`]; `None` when empty.
pub fn raster_size(rect: Rect, ppp: f32) -> Option<(usize, usize)> {
    let ppp = if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    let (w, h) = ((rect.width() * ppp).ceil(), (rect.height() * ppp).ceil());
    if !(w.is_finite() && h.is_finite()) || w < 1.0 || h < 1.0 {
        return None;
    }
    Some(((w as usize).min(MAX_SIDE), (h as usize).min(MAX_SIDE)))
}

/// A cel's strokes rasterised as the engine does (premultiplied linear light, erasers applied).
pub fn render_cel(strokes: &[Stroke], m: &Affine, w: usize, h: usize) -> Image {
    let mut img = Image::new(w.min(MAX_SIDE), h.min(MAX_SIDE));
    render_strokes(strokes, m, &mut img);
    img
}

/// What an eraser stroke removes: its coverage as a premultiplied white layer. The eraser runs over
/// a fully opaque layer with the engine's code, so `1 - alpha` is what it leaves of any paint.
pub fn eraser_footprint(stroke: &Stroke, m: &Affine, w: usize, h: usize) -> Image {
    let mut img = Image::new(w.min(MAX_SIDE), h.min(MAX_SIDE));
    img.px.fill(1.0);
    let eraser = Stroke { brush: Brush::Eraser, ..stroke.clone() };
    render_strokes(std::slice::from_ref(&eraser), m, &mut img);
    let (px, _) = img.px.as_chunks_mut::<4>();
    for p in px {
        let a = 1.0 - p[3];
        *p = [a; 4];
    }
    img
}

/// A premultiplied linear raster as an egui image (premultiplied sRGB). `tint` replaces the colour
/// (onion skins); `opacity` scales the alpha.
pub fn color_image(img: &Image, tint: Option<[u8; 3]>, opacity: f32) -> ColorImage {
    color_image_tinted(img, tint, 1.0, opacity)
}

/// Like [`color_image`], with the tint at `strength` (0 = the cel's own colours, 1 = the tint).
pub fn color_image_tinted(img: &Image, tint: Option<[u8; 3]>, strength: f32, opacity: f32) -> ColorImage {
    let strength = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 1.0 };
    let opacity = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 1.0 };
    let mut out = ColorImage::filled([img.w.max(1), img.h.max(1)], Color32::TRANSPARENT);
    if img.w == 0 || img.h == 0 {
        return out;
    }
    for (dst, p) in out.pixels.iter_mut().zip(img.px.as_chunks::<4>().0) {
        let a = p[3].clamp(0.0, 1.0);
        if a <= 0.0 {
            continue;
        }
        let alpha = a * opacity;
        let enc = |v: f32| (waltcut_color::linear_to_srgb((v / a).clamp(0.0, 1.0)) * alpha * 255.0).round() as u8;
        let (r, g, b) = match tint {
            Some([tr, tg, tb]) => {
                // premultiplied: mix the cel's own colour and the tint, both at this alpha
                let mix = |own: u8, t: u8| (own as f32 + (t as f32 * alpha - own as f32) * strength).round() as u8;
                (mix(enc(p[0]), tr), mix(enc(p[1]), tg), mix(enc(p[2]), tb))
            }
            None => (enc(p[0]), enc(p[1]), enc(p[2])),
        };
        *dst = Color32::from_rgba_premultiplied(r, g, b, (alpha * 255.0).round() as u8);
    }
    out
}

/// Largest difference (0..255, any channel) between blending `img`'s partially covered pixels over
/// a flat backdrop the way egui does (premultiplied sRGB) and the way the engine does (linear light,
/// then encoded). Opaque and empty pixels never differ.
pub fn display_blend_error(img: &Image, backdrop_srgb: f32) -> f32 {
    let ci = color_image(img, None, 1.0);
    let back_lin = waltcut_color::srgb_to_linear(backdrop_srgb);
    let mut worst = 0.0f32;
    for (c, p) in ci.pixels.iter().zip(img.px.as_chunks::<4>().0) {
        let a = p[3].clamp(0.0, 1.0);
        for ch in 0..3 {
            // egui: premultiplied sRGB over the backdrop
            let src = [c.r(), c.g(), c.b()][ch] as f32 / 255.0;
            let egui_out = src + backdrop_srgb * (1.0 - c.a() as f32 / 255.0);
            // engine: premultiplied linear over the backdrop, then encoded
            let engine_out = waltcut_color::linear_to_srgb((p[ch] + back_lin * (1.0 - a)).clamp(0.0, 1.0));
            worst = worst.max(((egui_out - engine_out) * 255.0).abs());
        }
    }
    worst
}

/// The live (or just-committed) stroke as a texture over its own bounding box, drawn on `painter`.
/// Does nothing when the stroke is off the picture or the mapping is degenerate.
pub fn paint_stroke_preview(painter: &egui::Painter, name: &str, stroke: &Stroke, to_screen: &Affine, pic: Rect) {
    let ctx = painter.ctx();
    let ppp = ctx.pixels_per_point();
    let scale = to_screen.determinant().abs().sqrt() as f32;
    if !scale.is_finite() || scale <= 0.0 || stroke.points.is_empty() {
        return;
    }
    // bounding box on the screen, with the widest the brush gets
    let r = stroke.size * 0.5 * scale + 3.0;
    let mut bb = Rect::NOTHING;
    for p in &stroke.points {
        let q = to_screen.apply(waltcut_geom::Vec2::new(p.x as f64, p.y as f64));
        if q.x.is_finite() && q.y.is_finite() {
            bb.extend_with(pos2(q.x as f32, q.y as f32));
        }
    }
    if !bb.is_positive() && bb.min.x > bb.max.x {
        return;
    }
    let rect = bb.expand(r).intersect(pic);
    let Some((w, h)) = raster_size(rect, ppp) else { return };
    let m = image_mapping(to_screen, rect.min, ppp);
    let image = if stroke.brush == Brush::Eraser {
        color_image(&eraser_footprint(stroke, &m, w, h), Some([255, 255, 255]), 0.55)
    } else {
        color_image(&render_cel(std::slice::from_ref(stroke), &m, w, h), None, 1.0)
    };
    let tex = ctx.load_texture(name, image, TextureOptions::LINEAR);
    painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

// ------------------------------------------------------------------------------------ onion skins

/// What an onion-skin texture was rendered from.
#[derive(Clone, Debug, PartialEq)]
pub struct OnionKey {
    pub clip: u64,
    pub cel: i64,
    /// Hash of the cel's strokes.
    pub content: u64,
    /// The placement (canvas → screen points) as bits, and the raster's origin and size.
    pub placement: [u64; 6],
    pub origin: [u32; 2],
    pub size: (usize, usize),
    pub tint: [u8; 3],
    /// Tint strength in thousandths.
    pub strength: u32,
    pub opacity: u32,
}

/// Hash of everything that decides how a cel looks.
pub fn content_hash(strokes: &[Stroke]) -> u64 {
    let mut h = DefaultHasher::new();
    strokes.len().hash(&mut h);
    for s in strokes {
        (s.brush as u8).hash(&mut h);
        for c in s.color {
            c.to_bits().hash(&mut h);
        }
        s.size.to_bits().hash(&mut h);
        s.points.len().hash(&mut h);
        for p in &s.points {
            p.x.to_bits().hash(&mut h);
            p.y.to_bits().hash(&mut h);
            p.p.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

type OnionResult = Result<(OnionKey, ColorImage), ()>;

/// One onion skin (the previous or the next cel): the texture on show and the job making the next.
#[derive(Default)]
pub struct OnionSlot {
    shown: Option<(OnionKey, TextureHandle, Rect)>,
    inflight: Option<OnionKey>,
    rx: Option<Receiver<OnionResult>>,
    /// `(project revision, cel frame)` → content hash, so a still cel is not rehashed every frame.
    memo: Option<(u64, i64, u64)>,
    /// The cel's hash is for a cel of this clip.
    memo_clip: u64,
    /// Jobs that failed (a worker panic), for diagnostics.
    pub failed: u64,
}

impl OnionSlot {
    /// What the texture on show was rendered from (for tests and diagnostics).
    pub fn shown_key(&self) -> Option<&OnionKey> {
        self.shown.as_ref().map(|(k, _, _)| k)
    }
}

/// An onion slot shared through egui's data store.
#[derive(Clone, Default)]
pub struct SharedSlot(pub Arc<Mutex<OnionSlot>>);

/// Draw one onion skin. `cel` is `(frame, strokes)` of the neighbouring cel, if any. Starts a
/// background job when what is wanted differs from what is shown or being made.
#[allow(clippy::too_many_arguments)]
pub fn paint_onion(
    painter: &egui::Painter,
    slot: &SharedSlot,
    revision: u64,
    clip: u64,
    cel: Option<(i64, &[Stroke])>,
    tint: [u8; 3],
    strength: f32,
    opacity: f32,
    to_screen: &Affine,
    pic: Rect,
) {
    let ctx = painter.ctx().clone();
    let mut s = slot.0.lock().unwrap_or_else(PoisonError::into_inner);
    // collect a finished job
    let mut finished = None;
    if let Some(rx) = &s.rx {
        match rx.try_recv() {
            Ok(r) => finished = Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => finished = Some(Err(())),
        }
    }
    if let Some(r) = finished {
        s.rx = None;
        s.inflight = None;
        match r {
            Ok((key, image)) => {
                let rect = Rect::from_min_size(pos2(f32::from_bits(key.origin[0]), f32::from_bits(key.origin[1])), vec2(0.0, 0.0));
                let ppp = ctx.pixels_per_point().max(0.01);
                let rect = Rect::from_min_size(rect.min, vec2(key.size.0 as f32 / ppp, key.size.1 as f32 / ppp));
                let tex = ctx.load_texture(format!("onion-{}-{:x}", key.clip, key.tint[0] as u32), image, TextureOptions::LINEAR);
                s.shown = Some((key, tex, rect));
            }
            Err(()) => s.failed += 1,
        }
    }
    let Some((frame, strokes)) = cel else {
        s.shown = None;
        return;
    };
    let ppp = ctx.pixels_per_point();
    let Some((w, h)) = raster_size(pic, ppp) else { return };
    let content = match s.memo {
        Some((rev, f, hash)) if rev == revision && f == frame && s.memo_clip == clip => hash,
        _ => {
            let hash = content_hash(strokes);
            s.memo = Some((revision, frame, hash));
            s.memo_clip = clip;
            hash
        }
    };
    let want = OnionKey {
        clip,
        cel: frame,
        content,
        placement: [to_screen.a, to_screen.b, to_screen.c, to_screen.d, to_screen.e, to_screen.f].map(f64::to_bits),
        origin: [pic.min.x.to_bits(), pic.min.y.to_bits()],
        size: (w, h),
        tint,
        strength: (strength.clamp(0.0, 1.0) * 1000.0) as u32,
        opacity: (opacity.clamp(0.0, 1.0) * 1000.0) as u32,
    };
    let current = s.shown.as_ref().is_some_and(|(k, _, _)| *k == want);
    if !current && s.inflight.is_none() {
        let (tx, rx) = channel();
        let (strokes, m, key) = (strokes.to_vec(), image_mapping(to_screen, pic.min, ppp), want.clone());
        s.inflight = Some(want);
        s.rx = Some(rx);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let img = render_cel(&strokes, &m, key.size.0, key.size.1);
                (key.clone(), color_image_tinted(&img, Some(key.tint), key.strength as f32 / 1000.0, key.opacity as f32 / 1000.0))
            }));
            let _ = tx.send(out.map_err(|_| ()));
            repaint.request_repaint();
        });
    }
    if let Some((_, tex, rect)) = &s.shown {
        painter.image(tex.id(), *rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waltcut_project::drawing::StrokePoint;

    fn stroke(brush: Brush, pts: &[(f32, f32, f32)], size: f32, color: [f32; 4]) -> Stroke {
        Stroke { brush, color, size, points: pts.iter().map(|&(x, y, p)| StrokePoint { x, y, p }).collect() }
    }
    fn alpha(img: &Image, x: usize, y: usize) -> f32 {
        img.px[(y * img.w + x) * 4 + 3]
    }
    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    #[test]
    fn eraser_footprint_is_exactly_what_the_engine_erases() {
        let m = Affine::scale(1.0, 1.0);
        let pen = stroke(Brush::Pen, &[(4.0, 16.0, 1.0), (60.0, 16.0, 1.0)], 8.0, WHITE);
        let eraser = stroke(Brush::Eraser, &[(30.0, 2.0, 0.7), (34.0, 30.0, 0.7)], 10.0, WHITE);
        let before = render_cel(std::slice::from_ref(&pen), &m, 64, 32);
        let after = render_cel(&[pen, eraser.clone()], &m, 64, 32);
        let foot = eraser_footprint(&eraser, &m, 64, 32);
        for y in 0..32 {
            for x in 0..64 {
                let predicted = alpha(&before, x, y) * (1.0 - alpha(&foot, x, y));
                assert!((predicted - alpha(&after, x, y)).abs() < 1e-5, "({x},{y}): {predicted} vs {}", alpha(&after, x, y));
            }
        }
        assert!(alpha(&foot, 32, 16) > 0.99 && alpha(&foot, 5, 5) == 0.0);
    }

    #[test]
    fn onion_of_a_cel_with_an_eraser_leaves_the_erased_area_empty() {
        // regression: onion skins painted every non-eraser stroke, so erased paint ghosted
        let m = Affine::scale(1.0, 1.0);
        let pen = stroke(Brush::Pen, &[(4.0, 16.0, 1.0), (60.0, 16.0, 1.0)], 8.0, WHITE);
        let eraser = stroke(Brush::Eraser, &[(32.0, 2.0, 1.0), (32.0, 30.0, 1.0)], 12.0, WHITE);
        let img = render_cel(&[pen, eraser], &m, 64, 32);
        let ci = color_image(&img, Some([230, 70, 70]), 0.35);
        let at = |x: usize, y: usize| ci.pixels[y * 64 + x];
        assert_eq!(at(32, 16).a(), 0, "erased");
        assert!(at(10, 16).a() > 80, "kept, tinted at the onion opacity");
        assert!((at(10, 16).r() as i32 - (230.0f32 * at(10, 16).a() as f32 / 255.0).round() as i32).abs() <= 1);
    }

    #[test]
    fn tint_strength_mixes_the_cels_own_colour_with_the_tint() {
        let m = Affine::scale(1.0, 1.0);
        let img = render_cel(&[stroke(Brush::Pen, &[(4.0, 8.0, 1.0), (60.0, 8.0, 1.0)], 8.0, [0.0, 0.0, 1.0, 1.0])], &m, 64, 16);
        let at = |ci: &ColorImage| ci.pixels[8 * 64 + 30];
        let (own, full, none, half) = (
            color_image(&img, None, 1.0),
            color_image_tinted(&img, Some([230, 70, 70]), 1.0, 1.0),
            color_image_tinted(&img, Some([230, 70, 70]), 0.0, 1.0),
            color_image_tinted(&img, Some([230, 70, 70]), 0.5, 1.0),
        );
        assert_eq!((at(&full).r(), at(&full).g(), at(&full).b()), (230, 70, 70));
        assert_eq!(at(&none), at(&own), "strength 0 keeps the cel's colours");
        assert_eq!((at(&half).r(), at(&half).b()), (115, 163), "halfway between blue (0,0,255) and the tint");
        // hostile strength and opacity are clamped, never a panic
        let _ = color_image_tinted(&img, Some([1, 2, 3]), f32::NAN, f32::INFINITY);
        assert_eq!(at(&color_image_tinted(&img, Some([230, 70, 70]), 9.0, 1.0)), at(&full));
    }

    #[test]
    fn overlapping_translucent_segments_have_no_dark_joints() {
        // a dense Marker polyline: coverage is the maximum over segments, so a joint is as dark as
        // the middle of a segment
        let pts: Vec<(f32, f32, f32)> = (0..8).map(|i| (8.0 + 8.0 * i as f32, 16.0, 1.0)).collect();
        let img = render_cel(&[stroke(Brush::Marker, &pts, 10.0, [0.0, 0.0, 0.0, 1.0])], &Affine::scale(1.0, 1.0), 80, 32);
        // x = 24 is a joint (a sample), x = 28 is the middle of a segment
        let (joint, middle) = (alpha(&img, 24, 16), alpha(&img, 28, 16));
        assert!((joint - 0.6).abs() < 0.02, "Marker is 60 % everywhere: {joint}");
        for x in 14..58 {
            assert!((alpha(&img, x, 16) - 0.6).abs() < 0.02 && (alpha(&img, x, 16) - middle).abs() < 0.02, "x = {x}: {}", alpha(&img, x, 16));
        }
    }

    #[test]
    fn display_blending_differs_from_the_engine_only_on_partial_coverage_and_within_a_bound() {
        let m = Affine::scale(1.0, 1.0);
        let opaque = render_cel(&[stroke(Brush::Pen, &[(8.0, 8.0, 1.0), (56.0, 8.0, 1.0)], 6.0, [0.9, 0.2, 0.1, 1.0])], &m, 64, 16);
        // inside a hard line every pixel is the same under both blends; only antialiased edges differ
        let mid = (8 * 64 + 30) * 4;
        let ci = color_image(&opaque, None, 1.0);
        assert_eq!(ci.pixels[8 * 64 + 30].a(), 255);
        assert!(opaque.px[mid + 3] > 0.999);
        let marker = render_cel(&[stroke(Brush::Marker, &[(8.0, 8.0, 1.0), (56.0, 8.0, 1.0)], 6.0, [0.9, 0.2, 0.1, 1.0])], &m, 64, 16);
        // documented bound: a 60 % Marker over mid-grey differs by at most this many levels of 255
        let e = display_blend_error(&marker, 0.5);
        assert!(e < 20.0, "worst display difference {e} / 255");
        assert_eq!(display_blend_error(&Image::new(8, 8), 0.5), 0.0);
    }

    #[test]
    fn preview_placement_matches_the_engine_under_zoom_and_rotation() {
        // a cel rendered through `image_mapping` of a screen mapping equals the cel rendered by the
        // engine placement it came from, shifted by the raster origin
        let canvas_to_screen = Affine::translate(100.0, 50.0).then_apply(&Affine::scale(0.5, 0.5)).then_apply(&Affine::rotate_deg(17.0));
        let s = stroke(Brush::Pencil, &[(20.0, 20.0, 0.4), (300.0, 150.0, 0.9)], 12.0, [0.1, 0.4, 0.8, 1.0]);
        let whole = render_cel(std::slice::from_ref(&s), &canvas_to_screen, 400, 300);
        let origin = pos2(40.0, 30.0);
        let part = render_cel(std::slice::from_ref(&s), &image_mapping(&canvas_to_screen, origin, 1.0), 200, 150);
        for y in 0..150 {
            for x in 0..200 {
                let (a, b) = (part.px[(y * 200 + x) * 4 + 3], whole.px[((y + 30) * 400 + x + 40) * 4 + 3]);
                assert!((a - b).abs() < 1e-4, "({x},{y}) {a} vs {b}");
            }
        }
        assert!(whole.px.as_chunks::<4>().0.iter().any(|p| p[3] > 0.5));
    }

    #[test]
    fn the_preview_raster_equals_the_engine_frame_for_every_brush_under_motion() {
        // the live/pen-up preview, the committed cel and the engine's frame (what export renders)
        // rasterise through the same code with the same placement: identical, not just close
        use waltcut_project::drawing::{Cel, Drawing};
        use waltcut_project::{ItemKind, Label, ParamValue, Project, SequenceSettings, TrackKind};
        use waltcut_time::{FrameRate, Tick, TimeRange};
        let rate = FrameRate::FPS_24;
        let strokes = vec![
            stroke(Brush::Pen, &[(8.0, 40.0, 0.3), (80.0, 30.0, 1.0), (120.0, 60.0, 0.6)], 9.0, [0.9, 0.1, 0.1, 1.0]),
            stroke(Brush::Pencil, &[(10.0, 10.0, 0.2), (110.0, 20.0, 0.9)], 7.0, [0.1, 0.1, 0.9, 1.0]),
            stroke(Brush::Marker, &[(20.0, 70.0, 1.0), (60.0, 20.0, 1.0), (100.0, 70.0, 1.0)], 16.0, [0.1, 0.8, 0.2, 1.0]),
            stroke(Brush::Eraser, &[(64.0, 5.0, 0.8), (64.0, 75.0, 0.8)], 12.0, WHITE),
        ];
        let mut drawing = Drawing::new(128, 80, rate);
        drawing.cels = vec![Cel { frame: 0, strokes: strokes.clone().into() }];
        let mut p = Project::new("preview equals engine");
        let item = p.add_item("cels", Label::Iris, ItemKind::Drawing(drawing.into()), None);
        let seq = p.new_sequence("s", SequenceSettings { width: 160, height: 100, frame_rate: rate, ..Default::default() }, 1, 0, None);
        let mut clip = p.make_track_item(item, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, rate.tick_of(24)), rate).expect("clip");
        clip.effect_mut("motion").expect("motion").param_mut("position").expect("position").value = ParamValue::Vec2(waltcut_geom::Vec2::new(90.0, 45.0));
        clip.effect_mut("motion").expect("motion").param_mut("scale").expect("scale").value = ParamValue::Float(70.0);
        clip.effect_mut("motion").expect("motion").param_mut("rotation").expect("rotation").value = ParamValue::Float(12.0);
        p.sequence_mut(seq).expect("sequence").video_tracks[0].items.push(clip);
        let frame = waltcut_render::render_sequence(&p, seq, Tick::ZERO, waltcut_render::RenderOptions::default(), &waltcut_render::SourceMap::default());
        let sq = p.sequence(seq).expect("sequence");
        let it = &sq.video_tracks[0].items[0];
        let motion = waltcut_render::motion_matrix(sq, it, (128, 80), None, Tick::ZERO);
        let preview = render_cel(&strokes, &motion, 160, 100);
        assert_eq!((frame.w, frame.h), (preview.w, preview.h));
        let worst = frame.px.iter().zip(&preview.px).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "preview vs engine frame: worst difference {worst}");
        assert!(frame.px.as_chunks::<4>().0.iter().any(|p| p[3] > 0.5), "the frame has artwork");
    }

    #[test]
    fn hostile_sizes_and_mappings_never_panic() {
        assert_eq!(raster_size(Rect::from_min_size(pos2(0.0, 0.0), vec2(1e9, 10.0)), 2.0), Some((MAX_SIDE, 20)));
        assert_eq!(raster_size(Rect::NOTHING, 1.0), None);
        assert_eq!(raster_size(Rect::from_min_size(pos2(0.0, 0.0), vec2(f32::NAN, 10.0)), 1.0), None);
        let m = image_mapping(&Affine::scale(0.0, 0.0), pos2(f32::NAN, 0.0), f32::INFINITY);
        let wild = stroke(Brush::Pen, &[(f32::NAN, 0.0, 1.0), (1e30, 1e30, f32::INFINITY)], 1e9, [f32::NAN; 4]);
        let _ = render_cel(std::slice::from_ref(&wild), &m, 16, 16);
        let _ = eraser_footprint(&wild, &m, 0, 0);
        let _ = color_image(&Image::new(0, 0), None, f32::NAN);
        assert_eq!(content_hash(&[]), content_hash(&[]));
        assert_ne!(content_hash(std::slice::from_ref(&wild)), content_hash(&[]));
    }
}
