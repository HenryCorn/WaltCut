//! Drawing clips: rasterising pen strokes (`waltcut_project::drawing`).
//!
//! Each stroke is a chain of round-capped segments whose radius follows the pen pressure. A stroke's
//! coverage is the *maximum* over its segments (not their sum), so the overlapping joints of a slow,
//! dense stroke don't build up darker blobs; strokes then composite over one another in paint order.
//! The eraser removes coverage from what is already painted in the cel.

use waltcut_geom::{Affine, Vec2};
use waltcut_project::drawing::{Brush, Stroke};

use crate::graphics::premul_linear;
use crate::image::Image;

/// Paint `strokes` onto `img`, mapping canvas pixels to `img` pixels with `m`.
pub fn render_strokes(strokes: &[Stroke], m: &Affine, img: &mut Image) {
    // brush sizes scale with the placement (a drawing scaled to 50 % has half-width lines)
    let k = m.determinant().abs().sqrt() as f32;
    if !k.is_finite() || k <= 0.0 {
        return;
    }
    for s in strokes {
        paint_stroke(s, m, k, img);
    }
}

/// One stroke sample in output pixels: position, radius, opacity.
#[derive(Clone, Copy)]
struct Dab {
    x: f32,
    y: f32,
    r: f32,
    a: f32,
}

fn paint_stroke(s: &Stroke, m: &Affine, k: f32, img: &mut Image) {
    if img.w == 0 || img.h == 0 || s.points.is_empty() || !s.size.is_finite() || s.size <= 0.0 {
        return;
    }
    let half = s.size * 0.5 * k;
    let dabs: Vec<Dab> = s
        .points
        .iter()
        .filter(|p| p.x.is_finite() && p.y.is_finite())
        .map(|p| {
            let q = m.apply(Vec2::new(p.x as f64, p.y as f64));
            let (rf, af) = s.response_at(p.p);
            Dab { x: q.x as f32, y: q.y as f32, r: (half * rf).max(0.35), a: af }
        })
        .collect();
    let Some(first) = dabs.first() else { return };
    // the stroke's box on the image, clipped
    let pad = dabs.iter().fold(0.0f32, |a, d| a.max(d.r)) + 2.0;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
    for d in &dabs {
        x0 = x0.min(d.x);
        y0 = y0.min(d.y);
        x1 = x1.max(d.x);
        y1 = y1.max(d.y);
    }
    let bx0 = ((x0 - pad).floor().max(0.0) as usize).min(img.w);
    let by0 = ((y0 - pad).floor().max(0.0) as usize).min(img.h);
    let bx1 = ((x1 + pad).ceil().max(0.0) as usize).min(img.w);
    let by1 = ((y1 + pad).ceil().max(0.0) as usize).min(img.h);
    let (bw, bh) = (bx1.saturating_sub(bx0), by1.saturating_sub(by0));
    if bw == 0 || bh == 0 {
        return;
    }
    let mut cov = vec![0.0f32; bw * bh];
    // pencil edges are softer: the antialiasing ramp is wider
    let soft = if s.brush == Brush::Pencil { 1.6 } else { 0.5 };
    let single = [*first, *first];
    let segments: Box<dyn Iterator<Item = &[Dab]>> = if dabs.len() == 1 { Box::new(std::iter::once(&single[..])) } else { Box::new(dabs.windows(2)) };
    for seg in segments {
        let [a, b] = seg else { continue };
        let r = a.r.max(b.r) + soft + 1.0;
        let sx0 = ((a.x.min(b.x) - r).floor() as isize).max(bx0 as isize);
        let sy0 = ((a.y.min(b.y) - r).floor() as isize).max(by0 as isize);
        let sx1 = ((a.x.max(b.x) + r).ceil() as isize).min(bx1 as isize);
        let sy1 = ((a.y.max(b.y) + r).ceil() as isize).min(by1 as isize);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = dx * dx + dy * dy;
        for y in sy0..sy1 {
            let py = y as f32 + 0.5;
            for x in sx0..sx1 {
                let px = x as f32 + 0.5;
                // nearest point on the segment, and the radius and opacity there
                let t = if len2 > 1e-12 { (((px - a.x) * dx + (py - a.y) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let (cx, cy) = (a.x + dx * t, a.y + dy * t);
                let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                let rad = a.r + (b.r - a.r) * t;
                let edge = ((rad - d) / (2.0 * soft) + 0.5).clamp(0.0, 1.0);
                if edge <= 0.0 {
                    continue;
                }
                let c = edge * (a.a + (b.a - a.a) * t);
                let (ux, uy) = ((x as usize).wrapping_sub(bx0), (y as usize).wrapping_sub(by0));
                if let Some(v) = cov.get_mut(uy.wrapping_mul(bw).wrapping_add(ux))
                    && ux < bw
                    && c > *v
                {
                    *v = c;
                }
            }
        }
    }
    let color = premul_linear(s.color);
    let alpha = s.color[3].clamp(0.0, 1.0);
    for row in 0..bh {
        for col in 0..bw {
            let Some(&c) = cov.get(row * bw + col) else { continue };
            if c <= 0.0 {
                continue;
            }
            let i = ((by0 + row) * img.w + bx0 + col) * 4;
            let Some(px) = img.px.get_mut(i..i + 4) else { continue };
            if s.brush == Brush::Eraser {
                let keep = 1.0 - c * alpha;
                for v in px.iter_mut() {
                    *v *= keep;
                }
            } else {
                // source-over in premultiplied linear light
                let inv = 1.0 - c * color[3];
                for (v, src) in px.iter_mut().zip(color) {
                    *v = src * c + *v * inv;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waltcut_project::drawing::StrokePoint;

    fn stroke(brush: Brush, pts: &[(f32, f32, f32)], size: f32) -> Stroke {
        Stroke { brush, color: [1.0, 1.0, 1.0, 1.0], size, points: pts.iter().map(|&(x, y, p)| StrokePoint { x, y, p }).collect(), response: None }
    }

    fn alpha(img: &Image, x: usize, y: usize) -> f32 {
        img.px[(y * img.w + x) * 4 + 3]
    }

    #[test]
    fn cels_and_erasers_keep_animated_motion_opacity_and_blend_in_both_plans() {
        use waltcut_project::drawing::{Cel, Drawing};
        use waltcut_project::{ItemKind, Keyframe, Label, ParamValue, Project, SequenceSettings, TrackKind};
        use waltcut_time::{FrameRate, Tick, TimeRange};

        let rate = FrameRate::FPS_24;
        let mut p = Project::new("retained drawing");
        let mut drawing = Drawing::new(64, 32, rate);
        drawing.cels = vec![
            Cel {
                frame: 0,
                strokes: vec![stroke(Brush::Pen, &[(8.0, 16.0, 1.0), (56.0, 16.0, 1.0)], 6.0), stroke(Brush::Eraser, &[(32.0, 16.0, 1.0)], 10.0)].into(),
            },
            Cel { frame: 24, strokes: vec![].into() },
        ];
        let item = p.add_item("cels", Label::Iris, ItemKind::Drawing(drawing.into()), None);
        let seq = p.new_sequence("animation", SequenceSettings { width: 64, height: 32, frame_rate: rate, ..Default::default() }, 1, 0, None);
        let mut clip = p.make_track_item(item, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, rate.tick_of(48)), rate).unwrap();
        clip.effect_mut("motion").unwrap().param_mut("position").unwrap().value = ParamValue::Vec2(Vec2::new(36.0, 16.0));
        clip.effect_mut("motion").unwrap().param_mut("scale").unwrap().keyframes =
            vec![Keyframe::new(Tick::ZERO, ParamValue::Float(100.0)), Keyframe::new(rate.tick_of(24), ParamValue::Float(50.0))];
        clip.effect_mut("opacity").unwrap().param_mut("opacity").unwrap().keyframes =
            vec![Keyframe::new(Tick::ZERO, ParamValue::Float(0.0)), Keyframe::new(rate.tick_of(24), ParamValue::Float(100.0))];
        let multiply = waltcut_project::effect::BLEND_MODES.iter().position(|m| *m == "Multiply").unwrap();
        clip.effect_mut("opacity").unwrap().param_mut("blend").unwrap().value = ParamValue::Choice(multiply as u32);
        p.sequence_mut(seq).unwrap().video_tracks[0].items.push(clip);
        let sources = crate::SourceMap::default();
        let opts = crate::RenderOptions::default();
        for frame in [12, 30] {
            let time = rate.tick_of(frame);
            let cpu = crate::render_sequence(&p, seq, time, opts, &sources);
            let plan = crate::plan::plan_frame(&p, seq, time, opts, &sources);
            let crate::plan::FramePlan::Layers { layers, .. } = &plan else { panic!("drawing must stay a composited layer") };
            assert_eq!(layers[0].blend, crate::Blend::Multiply);
            let planned = crate::plan::execute_cpu(&plan);
            assert_eq!(cpu.px, planned.px);
            if frame == 12 {
                assert!((alpha(&cpu, 22, 16) - 0.5).abs() < 0.01, "held cel with animated opacity");
                assert_eq!(alpha(&cpu, 36, 16), 0.0, "eraser follows Motion");
                assert_eq!(alpha(&cpu, 10, 16), 0.0, "animated Motion scales and positions the cel");
            } else {
                assert!(cpu.px.iter().all(|v| *v == 0.0), "next empty cel replaces the held drawing");
            }
        }
    }

    #[test]
    fn a_line_covers_its_path_and_nothing_else() {
        let mut img = Image::new(64, 32);
        render_strokes(&[stroke(Brush::Pen, &[(8.0, 16.0, 1.0), (56.0, 16.0, 1.0)], 6.0)], &Affine::scale(1.0, 1.0), &mut img);
        assert!(alpha(&img, 32, 16) > 0.99);
        assert!(alpha(&img, 32, 2) == 0.0);
        // dense, overlapping samples don't accumulate past full coverage
        assert!(img.px.iter().all(|v| *v <= 1.0 + 1e-6));
    }

    #[test]
    fn pressure_thins_the_line() {
        let run = |p: f32| {
            let mut img = Image::new(64, 64);
            render_strokes(&[stroke(Brush::Pen, &[(10.0, 32.0, p), (54.0, 32.0, p)], 20.0)], &Affine::scale(1.0, 1.0), &mut img);
            (0..64).filter(|y| alpha(&img, 32, *y) > 0.5).count()
        };
        assert!(run(0.2) < run(1.0));
    }

    #[test]
    fn eraser_removes_paint() {
        let mut img = Image::new(32, 32);
        let pen = stroke(Brush::Pen, &[(4.0, 16.0, 1.0), (28.0, 16.0, 1.0)], 8.0);
        let eraser = stroke(Brush::Eraser, &[(16.0, 4.0, 1.0), (16.0, 28.0, 1.0)], 8.0);
        render_strokes(&[pen, eraser], &Affine::scale(1.0, 1.0), &mut img);
        assert!(alpha(&img, 6, 16) > 0.99);
        assert!(alpha(&img, 16, 16) < 0.01);
    }

    #[test]
    fn hostile_strokes_and_placements_never_panic() {
        let mut img = Image::new(16, 16);
        let wild = stroke(Brush::Pencil, &[(f32::NAN, 0.0, 1.0), (-1e30, 1e30, f32::INFINITY), (8.0, 8.0, -4.0)], 1e6);
        render_strokes(std::slice::from_ref(&wild), &Affine::scale(1.0, 1.0), &mut img);
        render_strokes(&[wild], &Affine::scale(0.0, 0.0), &mut img);
        render_strokes(&[stroke(Brush::Pen, &[(8.0, 8.0, 1.0)], 4.0)], &Affine::scale(2.0, 2.0), &mut Image::new(0, 0));
        // a single dab still draws a dot
        let mut dot = Image::new(16, 16);
        render_strokes(&[stroke(Brush::Pen, &[(8.0, 8.0, 1.0)], 6.0)], &Affine::scale(1.0, 1.0), &mut dot);
        assert!(alpha(&dot, 8, 8) > 0.99);
    }
}

#[cfg(test)]
mod response_render_tests {
    use super::*;
    use waltcut_project::drawing::{Curve, PressureResponse, StrokePoint};

    fn render(s: &Stroke) -> Vec<f32> {
        let mut img = Image::new(64, 32);
        paint_stroke(s, &Affine::IDENTITY, 1.0, &mut img);
        img.px.clone()
    }

    fn line(brush: Brush, p: f32) -> Stroke {
        Stroke {
            brush,
            color: [0.0, 0.0, 0.0, 1.0],
            size: 12.0,
            points: (0..20).map(|i| StrokePoint { x: 8.0 + i as f32 * 2.5, y: 16.0, p }).collect(),
            response: None,
        }
    }

    #[test]
    fn a_stroke_without_a_response_renders_as_it_always_did_and_the_curves_change_it() {
        for brush in [Brush::Pen, Brush::Pencil] {
            let plain = line(brush, 0.4);
            let mut identity = plain.clone();
            identity.response = Some(PressureResponse::default());
            assert_eq!(render(&plain), render(&identity), "{brush:?}: an empty response is the brush's own");
            let mut soft = plain.clone();
            soft.response = Some(PressureResponse { width: Curve::preset("soft"), opacity: Curve::preset("soft") });
            assert_ne!(render(&plain), render(&soft), "{brush:?}: a curve changes the line");
            // the same stroke renders the same every time (deterministic replay)
            assert_eq!(render(&soft), render(&soft.clone()));
        }
    }

    #[test]
    fn width_and_opacity_are_independent() {
        let base = line(Brush::Pencil, 0.4);
        let mut width_only = base.clone();
        width_only.response = Some(PressureResponse { width: Curve::preset("soft"), opacity: None });
        let mut opacity_only = base.clone();
        opacity_only.response = Some(PressureResponse { width: None, opacity: Curve::preset("soft") });
        let (a, b, c) = (render(&base), render(&width_only), render(&opacity_only));
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        // the Marker has constant width and opacity: curves do nothing to it
        let mut marker = line(Brush::Marker, 0.4);
        let before = render(&marker);
        marker.response = Some(PressureResponse { width: Curve::preset("firm"), opacity: Curve::preset("firm") });
        assert_eq!(render(&marker), before);
    }
}
