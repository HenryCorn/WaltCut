//! CPU reference implementations of the video effects in `waltcut_project::effect`.
//!
//! Effects operate in place on a layer [`Image`] (premultiplied linear f32). Spatial parameters are
//! authored in full-resolution clip pixels; `FxCtx::px_scale` converts them to the working image
//! (so ½/¼ playback resolution renders the same look). Colour-grading math runs on display-encoded
//! straight colour where artists expect it (contrast, levels, posterize), linear light elsewhere.

use rayon::prelude::*;
use waltcut_color::{linear_to_srgb, luma709, srgb_to_linear};
use waltcut_geom::{Affine, Vec2};
use waltcut_project::{EffectInstance, ParamValue};
use waltcut_time::Tick;

use crate::image::Image;

/// Context for evaluating an effect at a time.
pub struct FxCtx<'a> {
    /// Media time used to evaluate keyframes.
    pub t: Tick,
    /// Working-image pixels per full-resolution clip pixel.
    pub px_scale: f32,
    /// Seconds since the clip start (for animated generators such as Noise/Strobe).
    pub seconds: f64,
    /// Formatted sequence timecode (for the Timecode effect).
    pub timecode: &'a str,
    pub clip_name: &'a str,
    /// The clip's surroundings (other frames, other tracks, sequence geometry) for temporal,
    /// track-reading and reframing effects; `None` in isolated tests.
    pub env: Option<&'a dyn crate::vfx::FxEnv>,
    /// Sequence frame rate (shutter-angle motion blur); also known where `env` is `None`.
    pub frame_rate: f64,
}

pub(crate) fn f(e: &EffectInstance, id: &str, cx: &FxCtx) -> f32 {
    e.f64_at(id, cx.t) as f32
}
pub(crate) fn b(e: &EffectInstance, id: &str) -> bool {
    e.param(id).and_then(|p| p.value.as_bool()).unwrap_or(false)
}
pub(crate) fn choice(e: &EffectInstance, id: &str) -> u32 {
    match e.param(id).map(|p| &p.value) {
        Some(ParamValue::Choice(c)) => *c,
        _ => 0,
    }
}
pub(crate) fn color(e: &EffectInstance, id: &str, cx: &FxCtx) -> [f32; 4] {
    e.param(id).and_then(|p| p.value_at(cx.t).as_color()).unwrap_or([1.0; 4])
}
/// A point param in working-image pixels; NaN components default to the image centre.
pub(crate) fn point(e: &EffectInstance, id: &str, cx: &FxCtx, img: &Image) -> Vec2 {
    let v = e.param(id).map(|p| p.vec2_at(cx.t)).unwrap_or(Vec2::new(f64::NAN, f64::NAN));
    Vec2::new(
        if v.x.is_nan() { img.w as f64 / 2.0 } else { v.x * cx.px_scale as f64 },
        if v.y.is_nan() { img.h as f64 / 2.0 } else { v.y * cx.px_scale as f64 },
    )
}

#[inline]
pub(crate) fn enc(c: [f32; 3]) -> [f32; 3] {
    [linear_to_srgb(c[0].max(0.0)), linear_to_srgb(c[1].max(0.0)), linear_to_srgb(c[2].max(0.0))]
}
#[inline]
pub(crate) fn dec(c: [f32; 3]) -> [f32; 3] {
    [srgb_to_linear(c[0].clamp(0.0, 1.0)), srgb_to_linear(c[1].clamp(0.0, 1.0)), srgb_to_linear(c[2].clamp(0.0, 1.0))]
}

/// Stable seed for the *exact* frame timestamp. A fixed 30 Hz quantizer
/// would reuse the same noise field at 60/120 fps and can repeat at 30 fps
/// through floating-point rounding of seconds.
pub(crate) fn noise_frame_seed(cx: &FxCtx) -> u64 {
    cx.t.0 as u64
}

pub(crate) fn hash3(x: usize, y: usize, z: u64) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ z.wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Apply one effect. Unknown/unimplemented ids are a no-op (they still round-trip in the project).
/// Largest Median radius in source pixels (the parameter's range).
const MEDIAN_MAX_RADIUS: f32 = 100.0;

/// The Median radius in working-image pixels. Radius is authored in source pixels and `px_scale`
/// converts to the working image (no cap at 4 working pixels: that made Radius > 4 look the same
/// at full resolution). It stays within the parameter's range whatever a project file says, since
/// the filter costs radius² per pixel, and within the image.
fn median_radius(radius: f32, px_scale: f32, longest_side: usize) -> isize {
    let scale = if px_scale.is_finite() { px_scale.max(0.0) } else { 1.0 };
    let r = radius * scale;
    if !(r.is_finite() && r > 0.0) {
        return 0;
    }
    let cap = (MEDIAN_MAX_RADIUS * scale).min(longest_side.max(1) as f32);
    r.round().clamp(0.0, cap) as isize
}

pub fn apply(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    if !e.enabled || img.w == 0 || img.h == 0 {
        return;
    }
    if crate::vfx::apply(img, e, cx) {
        return;
    }
    // effects with a GPU implementation: evaluated parameters + CPU reference (`gpufx`)
    if let Some(op) = crate::gpufx::FxOp::eval(e, cx, img.w, img.h) {
        op.apply(img);
        return;
    }
    match e.effect.as_str() {
        "median" => {
            let r = median_radius(f(e, "radius", cx), cx.px_scale, img.w.max(img.h));
            if r > 0 {
                median(img, r);
            }
        }
        "noise" => {
            let amt = f(e, "amount", cx) / 100.0;
            let colored = b(e, "color");
            let clip = b(e, "clip");
            let frame = noise_frame_seed(cx);
            let w = img.w;
            img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w {
                    let p = &mut row[x * 4..x * 4 + 4];
                    let a = p[3];
                    if a <= 0.0 {
                        continue;
                    }
                    let n0 = hash3(x, y, frame) - 0.5;
                    let ns = if colored { [n0, hash3(x, y, frame + 7777) - 0.5, hash3(x, y, frame + 99_991) - 0.5] } else { [n0; 3] };
                    let c = enc([p[0] / a, p[1] / a, p[2] / a]);
                    let mut o = [c[0] + ns[0] * amt, c[1] + ns[1] * amt, c[2] + ns[2] * amt];
                    if clip {
                        o = o.map(|v| v.clamp(0.0, 1.0));
                    }
                    let o = dec(o);
                    p[0] = o[0] * a;
                    p[1] = o[1] * a;
                    p[2] = o[2] * a;
                }
            });
        }
        "mosaic" => {
            let bx = f(e, "horizontal", cx).max(1.0) as usize;
            let by = f(e, "vertical", cx).max(1.0) as usize;
            mosaic(img, bx, by);
        }
        "find_edges" => {
            let inv = b(e, "invert");
            let blend = f(e, "blend", cx) / 100.0;
            let src = img.clone();
            let w = img.w;
            img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w {
                    let l = |dx: isize, dy: isize| {
                        let p = src.get_clamped(x as isize + dx, y as isize + dy);
                        luma709(p[0], p[1], p[2])
                    };
                    let gx = l(1, -1) + 2.0 * l(1, 0) + l(1, 1) - l(-1, -1) - 2.0 * l(-1, 0) - l(-1, 1);
                    let gy = l(-1, 1) + 2.0 * l(0, 1) + l(1, 1) - l(-1, -1) - 2.0 * l(0, -1) - l(1, -1);
                    let mut m = (gx * gx + gy * gy).sqrt().min(1.0);
                    if !inv {
                        m = 1.0 - m;
                    }
                    let o = src.get(x, y);
                    let a = o[3];
                    for k in 0..3 {
                        row[x * 4 + k] = m * a * (1.0 - blend) + o[k] * blend;
                    }
                }
            });
        }
        "emboss" => {
            let dir = (f(e, "direction", cx) as f64).to_radians();
            let relief = f(e, "relief", cx) * cx.px_scale.max(0.25);
            let contrast = f(e, "contrast", cx) / 100.0;
            let blend = f(e, "blend", cx) / 100.0;
            let (dx, dy) = ((dir.cos() as f32) * relief, (-dir.sin() as f32) * relief);
            let src = img.clone();
            let w = img.w;
            img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w {
                    let a = src.sample_bilinear_clamped(x as f32 + 0.5 + dx, y as f32 + 0.5 + dy);
                    let bq = src.sample_bilinear_clamped(x as f32 + 0.5 - dx, y as f32 + 0.5 - dy);
                    let v = srgb_to_linear((0.5 + (luma709(a[0], a[1], a[2]) - luma709(bq[0], bq[1], bq[2])) * contrast * 2.0).clamp(0.0, 1.0));
                    let o = src.get(x, y);
                    for k in 0..3 {
                        row[x * 4 + k] = v * o[3] * (1.0 - blend) + o[k] * blend;
                    }
                }
            });
        }
        "replicate" => {
            let n = f(e, "count", cx).clamp(1.0, 16.0) as usize;
            let src = img.clone();
            let (w, h) = (img.w, img.h);
            img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w {
                    let u = (x * n) % w;
                    let v = (y * n) % h;
                    let s = src.sample_bilinear_clamped((u as f32 + 0.5) + (n as f32 - 1.0) * 0.5 / n as f32, v as f32 + 0.5);
                    row[x * 4..x * 4 + 4].copy_from_slice(&s);
                }
            });
        }
        "strobe" => {
            let period = f(e, "period", cx).max(0.001) as f64;
            let dur = f(e, "duration", cx) as f64;
            if (cx.seconds % period) < dur {
                let c = color(e, "color", cx);
                let blend = f(e, "blend", cx) / 100.0;
                let lc = dec([c[0], c[1], c[2]]);
                img.map_rgb(|o, _, _| [lc[0] + (o[0] - lc[0]) * blend, lc[1] + (o[1] - lc[1]) * blend, lc[2] + (o[2] - lc[2]) * blend]);
            }
        }
        "twirl" => {
            let c = point(e, "center", cx, img);
            let ang = (f(e, "angle", cx) as f64).to_radians();
            let rad = f(e, "radius", cx) as f64 / 100.0 * (img.w.min(img.h) as f64);
            warp(img, |x, y| {
                let (dx, dy) = (x - c.x, y - c.y);
                let d = (dx * dx + dy * dy).sqrt();
                if d >= rad {
                    return (x, y);
                }
                let k = 1.0 - d / rad;
                let a = ang * k * k;
                let (s, co) = a.sin_cos();
                (c.x + dx * co - dy * s, c.y + dx * s + dy * co)
            });
        }
        "wave_warp" => {
            let hgt = f(e, "height", cx) as f64 * cx.px_scale as f64;
            let wid = (f(e, "width", cx) as f64 * cx.px_scale as f64).max(1.0);
            let dir = (f(e, "direction", cx) as f64).to_radians();
            let phase = cx.seconds * f(e, "speed", cx) as f64 * std::f64::consts::TAU;
            let (ux, uy) = (dir.sin(), -dir.cos());
            warp(img, |x, y| {
                let along = x * ux + y * uy;
                let off = hgt * (along / wid * std::f64::consts::TAU - phase).sin();
                (x - uy * off, y + ux * off)
            });
        }
        "lens_distortion" => {
            let k = f(e, "curvature", cx) as f64 / 100.0;
            let (cxp, cyp) = (
                img.w as f64 / 2.0 + f(e, "h_decentering", cx) as f64 / 100.0 * img.w as f64 / 2.0,
                img.h as f64 / 2.0 + f(e, "v_decentering", cx) as f64 / 100.0 * img.h as f64 / 2.0,
            );
            let norm = (img.w as f64 / 2.0).hypot(img.h as f64 / 2.0);
            warp(img, |x, y| {
                let (dx, dy) = ((x - cxp) / norm, (y - cyp) / norm);
                let r2 = dx * dx + dy * dy;
                let s = 1.0 - k * r2;
                (cxp + dx * s * norm, cyp + dy * s * norm)
            });
        }
        "basic_3d" => {
            let sw = (f(e, "swivel", cx) as f64).to_radians();
            let tl = (f(e, "tilt", cx) as f64).to_radians();
            // authored in full-resolution pixels; the focal length follows the working image (#1920)
            let dist = f(e, "distance", cx) as f64 * cx.px_scale as f64;
            let (w, h) = (img.w as f64, img.h as f64);
            let focal = w.max(h) * 1.2;
            // inverse perspective mapping: ray through (x,y) intersected with rotated plane
            warp_opt(img, |x, y| {
                let (px, py) = (x - w / 2.0, y - h / 2.0);
                let (ss, cs) = sw.sin_cos();
                let (st, ct) = tl.sin_cos();
                // plane normal after rotation (Ry(swivel) * Rx(tilt)) of (0,0,1)
                let n = [ss * ct, -st, cs * ct];
                let u = [cs, 0.0, -ss];
                let v = [ss * st, ct, cs * st];
                let z0 = focal + dist * 10.0;
                let dir = [px, py, focal];
                let denom = n[0] * dir[0] + n[1] * dir[1] + n[2] * dir[2];
                if denom.abs() < 1e-9 {
                    return None;
                }
                let tt = (n[2] * z0) / denom;
                if tt <= 0.0 {
                    return None;
                }
                let hit = [dir[0] * tt, dir[1] * tt, dir[2] * tt - z0];
                let su = hit[0] * u[0] + hit[1] * u[1] + hit[2] * u[2];
                let sv = hit[0] * v[0] + hit[1] * v[1] + hit[2] * v[2];
                Some((su + w / 2.0, sv + h / 2.0))
            });
        }
        "drop_shadow" => {
            let c = color(e, "color", cx);
            let op = f(e, "opacity", cx) / 100.0;
            let dir = (f(e, "direction", cx) as f64).to_radians();
            let dist = f(e, "distance", cx) as f64 * cx.px_scale as f64;
            let soft = f(e, "softness", cx) * cx.px_scale * 0.5;
            let only = b(e, "only");
            let (dx, dy) = (dir.sin() * dist, -dir.cos() * dist);
            let lc = dec([c[0], c[1], c[2]]);
            let mut sh = img.transformed(img.w, img.h, &Affine::translate(dx, dy));
            sh.px.par_chunks_mut(4).for_each(|p| {
                let a = p[3] * op;
                p[0] = lc[0] * a;
                p[1] = lc[1] * a;
                p[2] = lc[2] * a;
                p[3] = a;
            });
            if soft > 0.3 {
                gaussian(&mut sh, soft, soft, false);
            }
            if !only {
                crate::blend::composite(&mut sh, img, 1.0, crate::blend::Blend::Normal);
            }
            *img = sh;
        }
        "bevel_alpha" => {
            let th = f(e, "thickness", cx) * cx.px_scale;
            let ang = (f(e, "angle", cx) as f64).to_radians();
            let inten = f(e, "intensity", cx) / 100.0;
            let src = img.clone();
            let (lx, ly) = (ang.cos() as f32, -ang.sin() as f32);
            let w = img.w;
            img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w {
                    let a = |dx: f32, dy: f32| src.sample_bilinear_clamped(x as f32 + 0.5 + dx, y as f32 + 0.5 + dy)[3];
                    let gx = a(th, 0.0) - a(-th, 0.0);
                    let gy = a(0.0, th) - a(0.0, -th);
                    let shade = -(gx * lx + gy * ly) * inten;
                    let p = &mut row[x * 4..x * 4 + 4];
                    for k in 0..3 {
                        p[k] = (p[k] + shade * p[3]).clamp(0.0, p[3]);
                    }
                }
            });
        }
        "ultra_key" | "color_key" => key(img, e, cx),
        "luma_key" => {
            let th = f(e, "threshold", cx) / 100.0;
            let cut = f(e, "cutoff", cx) / 100.0;
            img.px.par_chunks_mut(4).for_each(|p| {
                if p[3] <= 0.0 {
                    return;
                }
                let l = linear_to_srgb(luma709(p[0] / p[3], p[1] / p[3], p[2] / p[3]).max(0.0));
                let a = if l <= cut {
                    0.0
                } else if l >= th.max(cut + 1e-3) {
                    1.0
                } else {
                    (l - cut) / (th - cut).max(1e-3)
                };
                for v in p.iter_mut() {
                    *v *= a;
                }
            });
        }
        "four_color_gradient" => {
            let cs = [color(e, "c1", cx), color(e, "c2", cx), color(e, "c3", cx), color(e, "c4", cx)];
            let op = f(e, "opacity", cx) / 100.0;
            let (w, h) = (img.w as f32, img.h as f32);
            let pts = [(0.25 * w, 0.25 * h), (0.75 * w, 0.25 * h), (0.25 * w, 0.75 * h), (0.75 * w, 0.75 * h)];
            let blend_exp = 1.0 + f(e, "blend", cx) / 100.0;
            fill_over(img, op, |x, y| {
                let mut acc = [0.0f32; 3];
                let mut wsum = 0.0;
                for (i, p) in pts.iter().enumerate() {
                    let d = ((x - p.0).powi(2) + (y - p.1).powi(2)).sqrt().max(1.0);
                    let wgt = 1.0 / d.powf(blend_exp);
                    for k in 0..3 {
                        acc[k] += cs[i][k] * wgt;
                    }
                    wsum += wgt;
                }
                dec(acc.map(|v| v / wsum))
            });
        }
        "ramp" => {
            let s = point(e, "start", cx, img);
            let en = point(e, "end", cx, img);
            let sc = color(e, "start_color", cx);
            let ec = color(e, "end_color", cx);
            let radial = choice(e, "shape") == 1;
            let blend = f(e, "blend", cx) / 100.0;
            let (vx, vy) = (en.x - s.x, en.y - s.y);
            let len2 = (vx * vx + vy * vy).max(1e-9);
            fill_over(img, 1.0 - blend, |x, y| {
                let t = if radial {
                    ((x as f64 - s.x).hypot(y as f64 - s.y) / len2.sqrt()) as f32
                } else {
                    (((x as f64 - s.x) * vx + (y as f64 - s.y) * vy) / len2) as f32
                }
                .clamp(0.0, 1.0);
                dec([sc[0] + (ec[0] - sc[0]) * t, sc[1] + (ec[1] - sc[1]) * t, sc[2] + (ec[2] - sc[2]) * t])
            });
        }
        "circle" => {
            let c = point(e, "center", cx, img);
            let r = f(e, "radius", cx) * cx.px_scale;
            let col = color(e, "color", cx);
            let op = f(e, "opacity", cx) / 100.0;
            let lc = dec([col[0], col[1], col[2]]);
            let w = img.w;
            img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w {
                    let d = ((x as f64 + 0.5 - c.x).hypot(y as f64 + 0.5 - c.y)) as f32;
                    let a = (r - d + 0.5).clamp(0.0, 1.0) * op;
                    if a > 0.0 {
                        let p = &mut row[x * 4..x * 4 + 4];
                        for k in 0..3 {
                            p[k] = lc[k] * a + p[k] * (1.0 - a);
                        }
                        p[3] = a + p[3] * (1.0 - a);
                    }
                }
            });
        }
        "grid" => {
            let size = (f(e, "size", cx) * cx.px_scale).max(1.0);
            let border = f(e, "border", cx) * cx.px_scale;
            let col = color(e, "color", cx);
            let op = f(e, "opacity", cx) / 100.0;
            let lc = dec([col[0], col[1], col[2]]);
            let w = img.w;
            let (ox, oy) = (img.w as f32 / 2.0, img.h as f32 / 2.0);
            img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w {
                    let gx = ((x as f32 - ox).rem_euclid(size)).min(size - (x as f32 - ox).rem_euclid(size));
                    let gy = ((y as f32 - oy).rem_euclid(size)).min(size - (y as f32 - oy).rem_euclid(size));
                    let a = ((border * 0.5 - gx.min(gy) + 0.5).clamp(0.0, 1.0)) * op;
                    if a > 0.0 {
                        let p = &mut row[x * 4..x * 4 + 4];
                        for k in 0..3 {
                            p[k] = lc[k] * a + p[k] * (1.0 - a);
                        }
                        p[3] = a + p[3] * (1.0 - a);
                    }
                }
            });
        }
        "lens_flare" => {
            let c = point(e, "center", cx, img);
            let br = f(e, "brightness", cx) / 100.0;
            let blend = f(e, "blend", cx) / 100.0;
            let (w, h) = (img.w as f64, img.h as f64);
            let (mx, my) = (w / 2.0, h / 2.0);
            let scale = w.max(h);
            let wi = img.w;
            img.px.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
                for x in 0..wi {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let d = (px - c.x).hypot(py - c.y) / scale;
                    let mut add = [1.0f32, 0.9, 0.7].map(|k| k * (0.35 * (-d * 18.0).exp() + 0.08 * (-d * 3.0).exp()) as f32);
                    for (i, (t, rad, tint)) in [(0.5, 0.05, [0.4f32, 0.6, 1.0]), (1.3, 0.03, [1.0, 0.5, 0.3]), (1.7, 0.08, [0.4, 1.0, 0.5])].iter().enumerate()
                    {
                        let gx = c.x + (mx - c.x) * 2.0 * t / 2.0 * 2.0;
                        let gy = c.y + (my - c.y) * 2.0 * t / 2.0 * 2.0;
                        let dd = (px - gx).hypot(py - gy) / scale;
                        let ring = (1.0 - ((dd - rad) / 0.01).abs()).max(0.0) as f32 * 0.15 + if dd < *rad { 0.06 } else { 0.0 };
                        let _ = i;
                        for k in 0..3 {
                            add[k] += tint[k] * ring;
                        }
                    }
                    let p = &mut row[x * 4..x * 4 + 4];
                    for k in 0..3 {
                        p[k] += add[k] * br * (1.0 - blend) * p[3].max(0.0);
                    }
                }
            });
        }
        "timecode" | "clip_name" => {
            let (text, family) = if e.effect == "timecode" { (cx.timecode.to_string(), "JetBrains Mono") } else { (cx.clip_name.to_string(), "Inter") };
            let pos = point(e, "position", cx, img);
            let px = (f(e, "size", cx) / 100.0 * img.h as f32 * 0.5).max(6.0);
            let box_alpha = if e.effect == "timecode" { (f(e, "opacity", cx) / 100.0).clamp(0.0, 1.0) * 0.8 } else { 0.8 };
            crate::graphics::burn_text(img, &text, family, pos, px, box_alpha);
        }
        _ => {}
    }
}

/// Fill an opaque generated colour over the image at `op` opacity (Generate category).
pub(crate) fn fill_over(img: &mut Image, op: f32, f: impl Fn(f32, f32) -> [f32; 3] + Sync) {
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let c = f(x as f32 + 0.5, y as f32 + 0.5);
            let p = &mut row[x * 4..x * 4 + 4];
            for k in 0..3 {
                p[k] = c[k] * op + p[k] * (1.0 - op);
            }
            p[3] = op + p[3] * (1.0 - op);
        }
    });
}

pub(crate) fn warp(img: &mut Image, f: impl Fn(f64, f64) -> (f64, f64) + Sync) {
    warp_opt(img, |x, y| Some(f(x, y)));
}

pub(crate) fn warp_opt(img: &mut Image, f: impl Fn(f64, f64) -> Option<(f64, f64)> + Sync) {
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let p = match f(x as f64 + 0.5, y as f64 + 0.5) {
                Some((u, v)) => src.sample_bilinear(u as f32, v as f32),
                None => [0.0; 4],
            };
            row[x * 4..x * 4 + 4].copy_from_slice(&p);
        }
    });
}

pub(crate) fn crop(img: &mut Image, l: f32, t: f32, r: f32, b: f32, feather: f32) {
    let (w, h) = (img.w as f32, img.h as f32);
    crop_px(img, l * w, w * (1.0 - r), t * h, h * (1.0 - b), feather.max(0.0));
}

/// Fade alpha towards the edges of the rectangle x0..x1 × y0..y1 (working pixels): over `fe`
/// pixels inside it, or a half-pixel antialiased edge when `fe` is 0.
pub(crate) fn crop_px(img: &mut Image, x0: f32, x1: f32, y0: f32, y1: f32, fe: f32) {
    let wi = img.w;
    img.px.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
        let py = y as f32 + 0.5;
        for x in 0..wi {
            let px = x as f32 + 0.5;
            let d = (px - x0).min(x1 - px).min(py - y0).min(y1 - py);
            let a = if fe > 0.0 { (d / fe).clamp(0.0, 1.0) } else { (d + 0.5).clamp(0.0, 1.0) };
            if a < 1.0 {
                for v in &mut row[x * 4..x * 4 + 4] {
                    *v *= a;
                }
            }
        }
    });
}

pub(crate) fn key(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let ultra = e.effect == "ultra_key";
    let kc = if ultra { color(e, "key_color", cx) } else { color(e, "color", cx) };
    let kycc = waltcut_color::rgb_to_ycbcr(kc[0], kc[1], kc[2], waltcut_color::Matrix::Bt709);
    let (tol, soft, spill, output) = if ultra {
        let tol = 0.05 + f(e, "tolerance", cx) / 100.0 * 0.25 + f(e, "transparency", cx) / 100.0 * 0.05;
        (tol, 0.05 + f(e, "soften", cx) / 100.0 * 0.2 + f(e, "pedestal", cx) / 100.0 * 0.05, f(e, "spill", cx) / 100.0, choice(e, "output"))
    } else {
        (f(e, "tolerance", cx) / 255.0 * 0.6 + 0.01, f(e, "feather", cx) / 50.0 * 0.2 + 0.01, 0.0, 0)
    };
    let dom = if kc[1] >= kc[0] && kc[1] >= kc[2] {
        1
    } else if kc[2] >= kc[0] {
        2
    } else {
        0
    };
    img.px.par_chunks_mut(4).for_each(|p| {
        if p[3] <= 0.0 {
            return;
        }
        let c = enc([p[0] / p[3], p[1] / p[3], p[2] / p[3]]);
        let ycc = waltcut_color::rgb_to_ycbcr(c[0], c[1], c[2], waltcut_color::Matrix::Bt709);
        let d = ((ycc[1] - kycc[1]).powi(2) + (ycc[2] - kycc[2]).powi(2)).sqrt() + (ycc[0] - kycc[0]).abs() * 0.15;
        let alpha = ((d - tol) / soft).clamp(0.0, 1.0);
        let mut o = c;
        if spill > 0.0 {
            let others = (o[(dom + 1) % 3] + o[(dom + 2) % 3]) / 2.0;
            if o[dom] > others {
                o[dom] -= (o[dom] - others) * spill;
            }
        }
        let a = p[3] * alpha;
        let lo = dec(o);
        match output {
            1 => {
                p.copy_from_slice(&[alpha * p[3], alpha * p[3], alpha * p[3], p[3]]);
            }
            _ => {
                p[0] = lo[0] * a;
                p[1] = lo[1] * a;
                p[2] = lo[2] * a;
                p[3] = a;
            }
        }
    });
}

// ---------- blur kernels ----------

/// Box radii for an n-pass box blur approximating a Gaussian of sigma (Kovesi / Wells).
fn boxes_for_gauss(sigma: f32, n: usize) -> Vec<usize> {
    let wideal = ((12.0 * sigma * sigma / n as f32) + 1.0).sqrt();
    let mut wl = wideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let mideal = (12.0 * sigma * sigma - (n as i32 * wl * wl) as f32 - 4.0 * n as f32 * wl as f32 - 3.0 * n as f32) / (-4.0 * wl as f32 - 4.0);
    let m = mideal.round() as i32;
    (0..n as i32).map(|i| (((if i < m { wl } else { wu }) - 1) / 2).max(0) as usize).collect()
}

fn box_rows(px: &mut [f32], w: usize, r: usize, repeat: bool) {
    if r == 0 {
        return;
    }
    px.par_chunks_mut(w * 4).for_each_init(
        || vec![0f32; w * 4],
        |tmp, row| {
            tmp.copy_from_slice(row);
            let inv = 1.0 / (2 * r + 1) as f32;
            let fetch = |i: isize| -> usize { if repeat { i.clamp(0, w as isize - 1) as usize } else { i as usize } };
            let mut acc = [0f32; 4];
            for i in -(r as isize)..=(r as isize) {
                if repeat || (i >= 0 && (i as usize) < w) {
                    let j = fetch(i);
                    for k in 0..4 {
                        acc[k] += tmp[j * 4 + k];
                    }
                }
            }
            for x in 0..w {
                for k in 0..4 {
                    row[x * 4 + k] = acc[k] * inv;
                }
                let out_i = x as isize - r as isize;
                let in_i = x as isize + r as isize + 1;
                if repeat || out_i >= 0 {
                    let j = fetch(out_i);
                    for k in 0..4 {
                        acc[k] -= tmp[j * 4 + k];
                    }
                }
                if repeat || (in_i as usize) < w {
                    let j = fetch(in_i);
                    for k in 0..4 {
                        acc[k] += tmp[j * 4 + k];
                    }
                }
            }
        },
    );
}

fn transpose(img: &Image) -> Image {
    let (w, h) = (img.w, img.h);
    let mut out = Image::new(h, w);
    const B: usize = 32;
    out.px.par_chunks_mut(h * 4 * B).enumerate().for_each(|(bi, chunk)| {
        let x0 = bi * B;
        let rows = chunk.len() / (h * 4);
        for y in 0..h {
            for dx in 0..rows {
                let x = x0 + dx;
                let s = (y * w + x) * 4;
                let d = (dx * h + y) * 4;
                chunk[d..d + 4].copy_from_slice(&img.px[s..s + 4]);
            }
        }
    });
    out
}

/// Gaussian blur via 3 box passes per axis (O(1) per pixel for any radius).
pub fn gaussian(img: &mut Image, sigma_x: f32, sigma_y: f32, repeat_edge: bool) {
    let (rx, ry) = gaussian_boxes(img.w, img.h, sigma_x, sigma_y);
    box_blur(img, &rx, &ry, repeat_edge);
}

/// The box radii [`gaussian`] runs on a `w`×`h` image, per axis (empty: that axis is untouched).
pub(crate) fn gaussian_boxes(w: usize, h: usize, sigma_x: f32, sigma_y: f32) -> (Vec<u32>, Vec<u32>) {
    // beyond a few image sizes every radius gives the same (flat) result; cap to keep box sizes sane
    let sigma_x = if sigma_x.is_finite() { sigma_x.min(w.max(8) as f32 * 2.0) } else { 0.0 };
    let sigma_y = if sigma_y.is_finite() { sigma_y.min(h.max(8) as f32 * 2.0) } else { 0.0 };
    let radii = |s: f32| if s > 0.3 { boxes_for_gauss(s, 3).into_iter().map(|r| r.min(u32::MAX as usize) as u32).collect() } else { Vec::new() };
    (radii(sigma_x), radii(sigma_y))
}

/// Box passes of radii `rx` along rows, then `ry` along columns.
pub(crate) fn box_blur(img: &mut Image, rx: &[u32], ry: &[u32], repeat_edge: bool) {
    for r in rx {
        box_rows(&mut img.px, img.w, *r as usize, repeat_edge);
    }
    if !ry.is_empty() {
        let mut t = transpose(img);
        for r in ry {
            box_rows(&mut t.px, t.w, *r as usize, repeat_edge);
        }
        *img = transpose(&t);
    }
}

/// Unsharp mask against a repeat-edge box-Gaussian of radii `rx` / `ry`.
pub(crate) fn unsharp_boxes(img: &mut Image, rx: &[u32], ry: &[u32], amount: f32, threshold: f32) {
    let mut blurred = img.clone();
    box_blur(&mut blurred, rx, ry, true);
    img.px.par_chunks_mut(4).zip(blurred.px.par_chunks(4)).for_each(|(p, bq)| {
        for k in 0..3 {
            let d = p[k] - bq[k];
            if d.abs() >= threshold * p[3] {
                p[k] = (p[k] + d * amount).clamp(0.0, p[3].max(p[k] + d * amount).max(0.0));
                p[k] = p[k].max(0.0);
            }
        }
    });
}

/// Average of `steps` edge-clamped bilinear taps along (dx, dy), centred on each pixel (no-op
/// below 2 steps).
pub(crate) fn directional_taps(img: &mut Image, dx: f32, dy: f32, steps: usize) {
    if steps < 2 {
        return;
    }
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut acc = [0f32; 4];
            for s in 0..steps {
                let t = s as f32 / (steps - 1) as f32 - 0.5;
                let p = src.sample_bilinear_clamped(x as f32 + 0.5 + dx * t, y as f32 + 0.5 + dy * t);
                for k in 0..4 {
                    acc[k] += p[k];
                }
            }
            for k in 0..4 {
                row[x * 4 + k] = acc[k] / steps as f32;
            }
        }
    });
}

pub(crate) fn median(img: &mut Image, r: isize) {
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each_init(Vec::new, |buf: &mut Vec<f32>, (y, row)| {
        for x in 0..w {
            for k in 0..4 {
                buf.clear();
                for dy in -r..=r {
                    for dx in -r..=r {
                        buf.push(src.get_clamped(x as isize + dx, y as isize + dy)[k]);
                    }
                }
                let mid = buf.len() / 2;
                buf.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
                row[x * 4 + k] = buf[mid];
            }
        }
    });
}

pub(crate) fn mosaic(img: &mut Image, bx: usize, by: usize) {
    let (w, h) = (img.w, img.h);
    let bw = (w as f32 / bx as f32).max(1.0);
    let bh = (h as f32 / by as f32).max(1.0);
    let src = img.clone();
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let cy = ((y as f32 / bh).floor() + 0.5) * bh;
        for x in 0..w {
            let cxp = ((x as f32 / bw).floor() + 0.5) * bw;
            let p = src.sample_bilinear_clamped(cxp, cy);
            row[x * 4..x * 4 + 4].copy_from_slice(&p);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::median_radius;

    /// A project file's Median radius stays within the parameter's 0–100 range (scaled to the
    /// working image) and the image: 1e9 used to run a radius-sized window over every pixel.
    #[test]
    fn median_radius_stays_in_the_parameter_range() {
        assert_eq!(median_radius(1e9, 1.0, 4096), 100);
        assert_eq!(median_radius(1e9, 0.5, 4096), 50);
        assert_eq!(median_radius(1e9, 1.0, 40), 40);
        assert_eq!(median_radius(12.0, 1.0, 4096), 12, "Radius above 4 keeps its full size");
        assert_eq!(median_radius(f32::NAN, 1.0, 4096), 0);
        assert_eq!(median_radius(-3.0, 1.0, 4096), 0);
        assert_eq!(median_radius(8.0, f32::INFINITY, 4096), 8);
    }

    use super::*;
    use waltcut_project::find_effect;

    fn cx() -> FxCtx<'static> {
        FxCtx { t: Tick::ZERO, px_scale: 1.0, seconds: 0.0, timecode: "00:00:01:00", clip_name: "x", env: None, frame_rate: 24.0 }
    }

    #[test]
    fn every_effect_runs_and_stays_finite() {
        for def in waltcut_project::effect_defs() {
            if def.kind != waltcut_project::EffectKind::Video || def.intrinsic {
                continue;
            }
            let mut img = Image::filled(24, 16, [0.2, 0.4, 0.1, 1.0]);
            let mut e = def.instance();
            // push params away from identity so code paths run
            for (id, p) in e.params.iter_mut() {
                if let ParamValue::Float(v) = &mut p.value
                    && let Some(waltcut_project::ParamKind::Float { soft_max, .. }) = def.param(id).map(|d| &d.kind)
                {
                    *v = (*v + soft_max * 0.3).min(*soft_max);
                }
            }
            apply(&mut img, &e, &cx());
            assert!(img.px.iter().all(|v| v.is_finite()), "{}", def.id);
        }
    }

    #[test]
    fn gaussian_preserves_mean_and_blurs() {
        let mut img = Image::new(64, 64);
        for y in 28..36 {
            for x in 28..36 {
                let i = (y * 64 + x) * 4;
                img.px[i..i + 4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
            }
        }
        let sum0: f32 = img.px.iter().sum();
        gaussian(&mut img, 4.0, 4.0, false);
        let sum1: f32 = img.px.iter().sum();
        assert!((sum0 - sum1).abs() / sum0 < 0.02, "{sum0} {sum1}");
        assert!(img.get(32, 32)[0] < 0.9 && img.get(24, 32)[0] > 0.01);
    }

    #[test]
    fn flip_and_crop() {
        let mut img = Image::new(4, 1);
        img.px[0..4].copy_from_slice(&[1.0, 0.0, 0.0, 1.0]);
        apply(&mut img, &find_effect("horizontal_flip").unwrap().instance(), &cx());
        assert_eq!(img.get(3, 0), [1.0, 0.0, 0.0, 1.0]);
        let mut e = find_effect("crop").unwrap().instance();
        e.params.get_mut("right").unwrap().value = ParamValue::Float(50.0);
        apply(&mut img, &e, &cx());
        assert_eq!(img.get(3, 0)[3], 0.0);
    }
}
