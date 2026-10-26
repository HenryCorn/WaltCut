//! Stroke smoothing for the Brush tool (#57): optional, low-lag de-jittering of what the pen
//! draws.
//!
//! **Off** (the default) stores exactly the captured samples. **Smooth** passes each kept sample
//! (see `drawing_input`: thinning comes first) through an adaptive moving average, in canvas
//! pixels, one sample at a time and in order, so the result never depends on how the samples were
//! batched into UI frames:
//!
//! * *Position.* `strength` (0..1) sets how much of a small move is absorbed: the smoothed point
//!   moves a fraction `alpha` of the way to the new sample, with `alpha = 1 - 0.9 * strength`
//!   (strength 0 follows the pen, 1 moves a tenth of the way). The filter follows large moves
//!   closely: `alpha` rises to 1 with the square of the pen's own step, reaching 1 when the pen
//!   moved [`FOLLOW_RANGE`] canvas pixels or more between two samples, so a fast stroke or a
//!   deliberate corner is followed and jitter well inside that step is smoothed away. The smoothed stroke therefore
//!   *trails* the pen by up to about [`FOLLOW_RANGE`] canvas pixels while you draw; that lag is
//!   intentional and visible in the live stroke.
//! * *Pressure* has its own strength (`pressure`, 0..1): `alpha = 1 - 0.8 * pressure`, on the
//!   pressure alone, never moving the position.
//! * *Pen-up* (explicit completion): the stroke ends exactly where the pen was lifted. The
//!   smoothed point first catches up along the filter's own curve (at most [`MAX_CATCH_UP`] extra
//!   points, using the strength's own `alpha` and not the fast-follow rate), then the exact end
//!   point is stored. No straight segment from the lagging point to the cursor is added beyond
//!   the final fraction of a pixel; short dots keep their one point.
//! * *Cancellation* (Escape, focus loss) drops the stroke before anything is committed, as it does
//!   without smoothing.
//!
//! **Rope** (#166) is a third mode: the stored point is the end of a string of fixed length
//! (`length` canvas pixels at 100% view, divided by the view's screen-points-per-canvas-pixel when
//! the stroke starts) pulled by the pen. A pen inside the string's reach leaves the point where it
//! is; beyond it the point is dragged along the line to the pen until the string is taut. Pressure
//! is the pen's own at each sample (through the `pressure` strength) and never moves the position.
//! Pen-up glides the point straight to the exact pen-up position (at most [`MAX_CATCH_UP`] points).
//!
//! A mouse is smoothed the same way (it only jitters less).

use serde::{Deserialize, Serialize};

/// Canvas pixels beyond which a sample is followed closely (the most the smoothed point trails).
pub const FOLLOW_RANGE: f32 = 16.0;
/// Most points a pen-up adds while the smoothed point catches up with the pen.
pub const MAX_CATCH_UP: usize = 32;
/// Catching up stops when the smoothed point is this close (canvas pixels) to the pen.
const CLOSE: f32 = 0.25;

/// Longest rope the setting allows, in canvas pixels at 100% view.
pub const MAX_ROPE_LENGTH: f32 = 200.0;
/// The rope length of a fresh setting.
pub const DEFAULT_ROPE_LENGTH: f32 = 24.0;
/// The string of one stroke never exceeds this many canvas pixels, however far the view is zoomed out.
const MAX_ROPE_CANVAS: f32 = 4096.0;
/// Pen-up glides the rope's end this far (canvas pixels) per stored point, when it can.
const ROPE_CATCH_STEP: f32 = 4.0;

/// Whether strokes are smoothed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmoothMode {
    /// The captured samples are stored as they are.
    #[default]
    Off,
    /// Adaptive moving average of position and (separately) pressure.
    Smooth,
    /// Pulled string (#166): the stored point trails the pen on a string of fixed length.
    Rope,
}

impl SmoothMode {
    pub fn param(self) -> &'static str {
        match self {
            SmoothMode::Off => "off",
            SmoothMode::Smooth => "smooth",
            SmoothMode::Rope => "rope",
        }
    }
}

/// The smoothing settings of the Brush tool (saved with the interface state).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Smoothing {
    pub mode: SmoothMode,
    /// Position smoothing, 0..1.
    pub strength: f32,
    /// Pressure smoothing, 0..1.
    pub pressure: f32,
    /// Rope mode: the string's length in canvas pixels at 100% view, 0..=[`MAX_ROPE_LENGTH`].
    pub length: f32,
}

impl Default for Smoothing {
    fn default() -> Self {
        Self { mode: SmoothMode::Off, strength: 0.5, pressure: 0.0, length: DEFAULT_ROPE_LENGTH }
    }
}

fn unit(v: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { default }
}

impl Smoothing {
    /// Position strength as it is used: finite and inside 0..1 whatever the state holds.
    pub fn position(&self) -> f32 {
        unit(self.strength, 0.5)
    }

    pub fn pressure(&self) -> f32 {
        unit(self.pressure, 0.0)
    }

    /// The rope length as it is used: finite and inside 0..=[`MAX_ROPE_LENGTH`] whatever the state holds.
    pub fn rope_length(&self) -> f32 {
        if self.length.is_finite() { self.length.clamp(0.0, MAX_ROPE_LENGTH) } else { DEFAULT_ROPE_LENGTH }
    }

    /// Whether a stroke made now is smoothed at all.
    pub fn is_active(&self) -> bool {
        match self.mode {
            SmoothMode::Off => false,
            SmoothMode::Smooth => self.position() > 0.0 || self.pressure() > 0.0,
            SmoothMode::Rope => self.rope_length() > 0.0 || self.pressure() > 0.0,
        }
    }
}

/// The pulled string of one stroke: the stored point `at` is never farther than `len` canvas pixels
/// from the pen.
#[derive(Clone, Debug)]
struct Rope {
    len: f32,
    at: Option<[f32; 2]>,
    /// The smoothed pressure so far.
    pressure: f32,
}

/// The filter of one stroke.
#[derive(Clone, Debug)]
pub struct Smoother {
    base: f32,
    pressure_alpha: f32,
    /// The smoothed x, y and pressure so far.
    state: Option<[f32; 3]>,
    /// The previous raw sample: how far the pen moved decides how closely the filter follows it.
    prev: Option<[f32; 3]>,
    /// Rope mode instead of the moving average.
    rope: Option<Rope>,
}

impl Smoother {
    /// The filter for a stroke started now at 100% view, or None when smoothing is off.
    pub fn new(s: &Smoothing) -> Option<Smoother> {
        Self::scaled(s, 1.0)
    }

    /// The filter for a stroke started now, or None when smoothing is off. `scale` is screen points
    /// per canvas pixel: a rope keeps its size under the pen whatever the zoom, so its length in
    /// canvas pixels is `length / scale` (a bad scale counts as 1). Fixed for the whole stroke.
    pub fn scaled(s: &Smoothing, scale: f32) -> Option<Smoother> {
        if !s.is_active() {
            return None;
        }
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let rope = (s.mode == SmoothMode::Rope).then(|| Rope { len: (s.rope_length() / scale).clamp(0.0, MAX_ROPE_CANVAS), at: None, pressure: 1.0 });
        Some(Smoother { base: 1.0 - 0.9 * s.position(), pressure_alpha: 1.0 - 0.8 * s.pressure(), state: None, prev: None, rope })
    }

    /// One rope step: drag the string's end towards the pen only as far as the string is taut.
    fn rope_step(&mut self, raw: [f32; 3]) -> [f32; 3] {
        let pressure_alpha = self.pressure_alpha;
        let Some(r) = self.rope.as_mut() else { return raw };
        if !raw[0].is_finite() || !raw[1].is_finite() {
            // never happens for a kept sample; do not let it into the state
            return r.at.map_or(raw, |a| [a[0], a[1], r.pressure]);
        }
        let p = if raw[2].is_finite() { raw[2].clamp(0.0, 1.0) } else { 1.0 };
        let Some(a) = r.at else {
            r.at = Some([raw[0], raw[1]]);
            r.pressure = p;
            return [raw[0], raw[1], p];
        };
        r.pressure = (r.pressure + pressure_alpha * (p - r.pressure)).clamp(0.0, 1.0);
        let (dx, dy) = (raw[0] - a[0], raw[1] - a[1]);
        let d = dx.hypot(dy);
        let mut next = if d > r.len && d.is_finite() {
            // pen to string end is `len` long along the line between them
            let k = r.len / d;
            [raw[0] - dx * k, raw[1] - dy * k]
        } else if d.is_finite() {
            a
        } else {
            [raw[0], raw[1]]
        };
        if !next[0].is_finite() || !next[1].is_finite() {
            next = [raw[0], raw[1]];
        }
        r.at = Some(next);
        [next[0], next[1], r.pressure]
    }

    /// Pen-up for a rope: the string's end glides straight to the pen in equal steps (at most
    /// [`MAX_CATCH_UP`]), the last being the exact end.
    fn rope_finish(&mut self, end: [f32; 3]) -> Vec<[f32; 3]> {
        let Some(r) = self.rope.as_mut() else { return vec![end] };
        if !end[0].is_finite() || !end[1].is_finite() {
            return Vec::new();
        }
        let pressure = if end[2].is_finite() { end[2].clamp(0.0, 1.0) } else { 1.0 };
        let end = [end[0], end[1], pressure];
        let Some(a) = r.at.replace([end[0], end[1]]) else { return vec![end] };
        let d = (end[0] - a[0]).hypot(end[1] - a[1]);
        let from = r.pressure;
        r.pressure = pressure;
        if d <= CLOSE || !d.is_finite() {
            return vec![end];
        }
        let n = ((d / ROPE_CATCH_STEP).ceil().max(1.0) as usize).min(MAX_CATCH_UP);
        let mut out: Vec<[f32; 3]> = (1..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                [a[0] + (end[0] - a[0]) * t, a[1] + (end[1] - a[1]) * t, from + (pressure - from) * t]
            })
            .collect();
        out.push(end);
        out
    }

    fn advance(&mut self, raw: [f32; 3], follow: bool) -> [f32; 3] {
        let moved = self.prev.replace(raw);
        let Some(s) = self.state else {
            self.state = Some(raw);
            return raw;
        };
        // how far the pen itself moved since the last sample (not how far the smoothed point lags,
        // which would ease the smoothing off the faster you draw)
        let d = moved.map_or(0.0, |m| (raw[0] - m[0]).hypot(raw[1] - m[1]));
        let alpha = if follow && d.is_finite() { self.base + (1.0 - self.base) * (d / FOLLOW_RANGE).min(1.0).powi(2) } else { self.base };
        let next = [s[0] + alpha * (raw[0] - s[0]), s[1] + alpha * (raw[1] - s[1]), (s[2] + self.pressure_alpha * (raw[2] - s[2])).clamp(0.0, 1.0)];
        self.state = Some(next);
        next
    }

    /// The smoothed point for the next raw sample (canvas x, y and pressure).
    pub fn step(&mut self, raw: [f32; 3]) -> [f32; 3] {
        if self.rope.is_some() {
            return self.rope_step(raw);
        }
        self.advance(raw, true)
    }

    /// The points that end the stroke at `end`: the smoothed point catching up with the pen, then
    /// `end` itself.
    pub fn finish(&mut self, end: [f32; 3]) -> Vec<[f32; 3]> {
        if self.rope.is_some() {
            return self.rope_finish(end);
        }
        let mut out = Vec::new();
        if self.state.is_some() {
            for _ in 0..MAX_CATCH_UP {
                let Some(s) = self.state else { break };
                if (end[0] - s[0]).hypot(end[1] - s[1]) <= CLOSE {
                    break;
                }
                out.push(self.advance(end, false));
            }
        }
        self.state = Some(end);
        // the stroke ends exactly where the pen was lifted
        match out.last_mut() {
            Some(last) if (last[0] - end[0]).hypot(last[1] - end[1]) <= CLOSE => *last = end,
            _ => out.push(end),
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smooth(strength: f32, pressure: f32) -> Smoothing {
        Smoothing { mode: SmoothMode::Smooth, strength, pressure, ..Smoothing::default() }
    }

    fn rope(length: f32, pressure: f32) -> Smoothing {
        Smoothing { mode: SmoothMode::Rope, strength: 0.5, pressure, length }
    }

    /// The smoothed stroke of a raw trace, ending as a pen-up does.
    fn run(s: &Smoothing, raw: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let Some(mut f) = Smoother::new(s) else { return raw.to_vec() };
        let mut out = Vec::new();
        for (i, r) in raw.iter().enumerate() {
            if i + 1 == raw.len() {
                out.extend(f.finish(*r));
            } else {
                out.push(f.step(*r));
            }
        }
        out
    }

    /// A horizontal line with a deterministic ±2 px zigzag.
    fn jittery_line() -> Vec<[f32; 3]> {
        (0..100).map(|i| [i as f32 * 3.0, 50.0 + if i % 2 == 0 { 2.0 } else { -2.0 }, 1.0]).collect()
    }

    #[test]
    fn off_stores_the_captured_samples_untouched() {
        assert!(Smoother::new(&Smoothing::default()).is_none());
        // Smooth with nothing to smooth is off too
        assert!(Smoother::new(&smooth(0.0, 0.0)).is_none());
        let raw = jittery_line();
        assert_eq!(run(&Smoothing::default(), &raw), raw);
    }

    #[test]
    fn smoothing_reduces_jitter_and_stronger_is_smoother() {
        let raw = jittery_line();
        // once the filter has settled from the first sample's own jitter
        let wobble = |pts: &[[f32; 3]]| pts[30..90].iter().map(|p| (p[1] - 50.0).abs()).fold(0.0f32, f32::max);
        let (off, light, strong) = (wobble(&raw), wobble(&run(&smooth(0.3, 0.0), &raw)), wobble(&run(&smooth(0.9, 0.0), &raw)));
        eprintln!("jitter: off {off}, light {light}, strong {strong}");
        // the filter follows a moving pen more closely than a still one, so these are for 3 px steps
        assert!(light < off * 0.9, "{off} → {light}");
        assert!(strong < light, "{light} → {strong}");
        assert!(strong < off * 0.4, "{off} → {strong}");
    }

    #[test]
    fn a_deliberate_corner_is_kept_within_the_documented_tolerance() {
        // a 90° corner drawn fast: 8 px steps right, then 8 px steps down; it is rounded by about
        // 0.4, 2.2 and 7 px at strengths 0.3, 0.6 and 1 (measured), never more than half the follow range
        let mut raw: Vec<[f32; 3]> = (0..=20).map(|i| [i as f32 * 8.0, 0.0, 1.0]).collect();
        raw.extend((1..=20).map(|i| [160.0, i as f32 * 8.0, 1.0]));
        for strength in [0.3, 0.6, 1.0] {
            let out = run(&smooth(strength, 0.0), &raw);
            // distance from the ideal polyline (0,0) → (160,0) → (160,160)
            let seg = |p: &[f32; 3], a: (f32, f32), b: (f32, f32)| {
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let t = (((p[0] - a.0) * dx + (p[1] - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                (p[0] - (a.0 + t * dx)).hypot(p[1] - (a.1 + t * dy))
            };
            let off_path = |p: &[f32; 3]| seg(p, (0.0, 0.0), (160.0, 0.0)).min(seg(p, (160.0, 0.0), (160.0, 160.0)));
            let worst = out.iter().map(off_path).fold(0.0f32, f32::max);
            assert!(worst <= FOLLOW_RANGE / 2.0, "strength {strength}: the corner is rounded by {worst} px");
        }
    }

    #[test]
    fn the_stroke_ends_exactly_at_the_pen_without_a_straight_finishing_segment() {
        let raw: Vec<[f32; 3]> = (0..=30).map(|i| [i as f32 * 4.0, (i as f32 * 0.4).sin() * 10.0, 1.0]).collect();
        for strength in [0.2, 0.7, 1.0] {
            let out = run(&smooth(strength, 0.0), &raw);
            assert_eq!(out.last(), raw.last(), "strength {strength}");
            // the last step is a fraction of a pixel or follows the curve, never a long jump
            let n = out.len();
            let last_step = (out[n - 1][0] - out[n - 2][0]).hypot(out[n - 1][1] - out[n - 2][1]);
            assert!(last_step <= 4.0 * 1.2, "strength {strength}: a final segment of {last_step} px");
            assert!(out.len() <= raw.len() + MAX_CATCH_UP + 1);
        }
    }

    #[test]
    fn dots_and_two_point_strokes_are_deterministic() {
        let s = smooth(0.8, 0.5);
        assert_eq!(run(&s, &[[5.0, 5.0, 0.7]]), vec![[5.0, 5.0, 0.7]]);
        // a tap: down and up at the same place stays one point
        let mut f = Smoother::new(&s).unwrap();
        assert_eq!(f.step([5.0, 5.0, 0.7]), [5.0, 5.0, 0.7]);
        assert_eq!(f.finish([5.0, 5.0, 0.7]), vec![[5.0, 5.0, 0.7]]);
        // a short drag ends at the end and the same input always gives the same output
        let raw = [[0.0, 0.0, 1.0], [3.0, 1.0, 1.0], [6.0, 0.0, 1.0]];
        assert_eq!(run(&s, &raw), run(&s, &raw));
        assert_eq!(run(&s, &raw).last(), Some(&[6.0, 0.0, 1.0]));
    }

    #[test]
    fn rapid_reversals_stay_bounded_by_the_samples_and_deterministic() {
        let raw: Vec<[f32; 3]> = (0..200).map(|i| [if i % 2 == 0 { 0.0 } else { 40.0 }, i as f32, 1.0]).collect();
        let out = run(&smooth(1.0, 0.0), &raw);
        assert!(out.iter().all(|p| (0.0..=40.0).contains(&p[0])), "the filter never overshoots the samples");
        assert_eq!(out, run(&smooth(1.0, 0.0), &raw));
    }

    #[test]
    fn pressure_is_smoothed_apart_from_position() {
        let raw: Vec<[f32; 3]> = (0..60).map(|i| [i as f32 * 3.0, 0.0, if i % 2 == 0 { 0.9 } else { 0.5 }]).collect();
        let only_pressure = run(&smooth(0.0, 0.8), &raw);
        let positions: Vec<f32> = only_pressure.iter().map(|p| p[0]).collect();
        assert_eq!(positions, raw.iter().map(|p| p[0]).collect::<Vec<_>>(), "position untouched when only pressure is smoothed");
        let swing = |pts: &[[f32; 3]]| pts[20..50].iter().map(|p| p[2]).fold(0.0f32, f32::max) - pts[20..50].iter().map(|p| p[2]).fold(1.0f32, f32::min);
        assert!(swing(&only_pressure) < swing(&raw) * 0.5);
        let only_position = run(&smooth(0.8, 0.0), &raw);
        assert!(
            only_position.iter().zip(&raw).all(|(a, b)| a[2] == b[2] || b[2] == 0.0 || (a[2] - b[2]).abs() < 1e-6),
            "pressure untouched when only position is smoothed"
        );
    }

    #[test]
    fn hostile_values_cannot_panic_or_escape() {
        for s in [smooth(f32::NAN, f32::NAN), smooth(-3.0, 9.0), smooth(f32::INFINITY, f32::NEG_INFINITY)] {
            assert!((0.0..=1.0).contains(&s.position()) && (0.0..=1.0).contains(&s.pressure()));
            let raw = [[0.0, 0.0, 1.0], [f32::NAN, 5.0, 0.5], [1e30, -1e30, 7.0], [f32::INFINITY, 0.0, f32::NAN], [2.0, 2.0, 1.0]];
            let _ = run(&s, &raw);
        }
        let mut f = Smoother::new(&smooth(1.0, 1.0)).unwrap();
        let _ = f.finish([f32::NAN, f32::NAN, f32::NAN]);
        let _ = f.finish([1.0, 1.0, 1.0]);
    }

    /// Cost of the filter per sample on this machine (not a pen-to-screen latency).
    #[test]
    fn the_filter_costs_next_to_nothing_per_sample() {
        let raw = jittery_line();
        let start = std::time::Instant::now();
        let mut n = 0usize;
        for _ in 0..2_000 {
            n += run(&smooth(0.7, 0.3), &raw).len();
        }
        let per_sample_ns = start.elapsed().as_nanos() as f64 / (2_000.0 * raw.len() as f64);
        eprintln!("smoothing cost: {per_sample_ns:.0} ns per sample ({n} points)");
        assert!(per_sample_ns < 20_000.0, "{per_sample_ns} ns per sample");
    }

    // ---- rope (#166) ----

    fn dist(a: &[f32; 3], b: &[f32; 3]) -> f32 {
        (a[0] - b[0]).hypot(a[1] - b[1])
    }

    /// The rope's stored points for a raw trace, one `step` per sample (no pen-up).
    fn rope_steps(len: f32, raw: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let mut f = Smoother::new(&rope(len, 0.0)).unwrap();
        raw.iter().map(|r| f.step(*r)).collect()
    }

    #[test]
    fn the_string_is_never_longer_than_its_length_and_slack_does_not_move_it() {
        // a zigzag well inside a 20 px string leaves the point where the pen first came down
        let wobble: Vec<[f32; 3]> = (0..50).map(|i| [if i % 2 == 0 { 3.0 } else { -3.0 }, 0.0, 1.0]).collect();
        assert!(rope_steps(20.0, &wobble).iter().all(|p| *p == wobble[0]));
        // a long straight pull: the point trails by exactly the length once taut, on the line
        let line: Vec<[f32; 3]> = (0..100).map(|i| [i as f32 * 5.0, i as f32 * 2.0, 1.0]).collect();
        let out = rope_steps(30.0, &line);
        for (o, r) in out.iter().zip(&line) {
            assert!(dist(o, r) <= 30.0 + 1e-3, "{o:?} vs {r:?}");
        }
        let (o, r) = (out[99], line[99]);
        assert!((dist(&o, &r) - 30.0).abs() < 1e-3, "taut: {}", dist(&o, &r));
        // on the straight line between the first sample and the pen
        let cross = (o[0] * line[99][1] - o[1] * line[99][0]).abs() / line[99][0].hypot(line[99][1]);
        assert!(cross < 1e-2, "{cross}");
    }

    #[test]
    fn a_corner_is_cut_by_the_string_but_the_stroke_still_turns() {
        // 0..200 right, then 200 down in 5 px steps; a 20 px string
        let mut raw: Vec<[f32; 3]> = (0..=40).map(|i| [i as f32 * 5.0, 0.0, 1.0]).collect();
        raw.extend((1..=40).map(|i| [200.0, i as f32 * 5.0, 1.0]));
        let out = run(&rope(20.0, 0.0), &raw);
        let rounding = out.iter().map(|p| (200.0 - p[0]).min(p[1].abs() + 0.0)).fold(0.0f32, f32::max);
        // the point nearest the corner is cut inside it by less than the string length
        assert!(rounding > 1.0 && rounding < 20.0, "corner rounded by {rounding}");
        // after the corner the point is on the downward leg
        let mid = out[out.len() - 8];
        assert!((mid[0] - 200.0).abs() < 1e-3 || mid[0] > 190.0, "{mid:?}");
        assert_eq!(out.last(), raw.last());
        // a longer string cuts the corner deeper
        let deeper = |len: f32| run(&rope(len, 0.0), &raw).iter().map(|p| (p[0] - 200.0).abs().min(p[1])).fold(f32::MAX, f32::min);
        assert!(deeper(60.0) >= deeper(20.0));
    }

    #[test]
    fn pen_up_ends_exactly_at_the_pen_with_short_even_steps() {
        let raw: Vec<[f32; 3]> = (0..=30).map(|i| [i as f32 * 6.0, (i as f32 * 0.5).sin() * 8.0, 0.8]).collect();
        for len in [5.0, 24.0, 90.0, 200.0] {
            let out = run(&rope(len, 0.0), &raw);
            assert_eq!(out.last(), raw.last(), "length {len}");
            assert!(out.len() <= raw.len() + MAX_CATCH_UP);
            // the tail is a straight glide: no step is longer than max(4 px, string/32) plus slack
            let n = out.len();
            let tail = dist(&out[n - 1], &out[n - 2]);
            assert!(tail <= (len / MAX_CATCH_UP as f32).max(ROPE_CATCH_STEP) + 1e-3, "length {len}: final step {tail}");
        }
    }

    #[test]
    fn a_tap_stays_one_point_and_zero_length_follows_the_pen() {
        let s = rope(40.0, 0.0);
        assert_eq!(run(&s, &[[5.0, 5.0, 0.7]]), vec![[5.0, 5.0, 0.7]]);
        let mut f = Smoother::new(&s).unwrap();
        assert_eq!(f.step([5.0, 5.0, 0.7]), [5.0, 5.0, 0.7]);
        assert_eq!(f.finish([5.0, 5.0, 0.7]), vec![[5.0, 5.0, 0.7]]);
        // a drag that never pulls the string taut ends with its one glide to the pen
        let out = run(&s, &[[0.0, 0.0, 1.0], [10.0, 0.0, 1.0], [20.0, 0.0, 1.0]]);
        assert_eq!(out.first(), Some(&[0.0, 0.0, 1.0]));
        assert_eq!(out.last(), Some(&[20.0, 0.0, 1.0]));
        // length 0 and no pressure smoothing is not a filter at all
        assert!(Smoother::new(&rope(0.0, 0.0)).is_none());
        // length 0 with pressure smoothing follows the pen's position
        let raw = [[0.0, 0.0, 1.0], [7.0, 3.0, 0.5], [9.0, 9.0, 0.9]];
        let mut f = Smoother::new(&rope(0.0, 0.5)).unwrap();
        let out: Vec<_> = raw.iter().map(|r| f.step(*r)).collect();
        assert!(out.iter().zip(&raw).all(|(a, b)| a[0] == b[0] && a[1] == b[1]));
    }

    #[test]
    fn rope_is_a_pure_function_of_the_samples_so_batching_cannot_matter() {
        let raw: Vec<[f32; 3]> = (0..80).map(|i| [i as f32 * 3.0, (i as f32 * 0.3).sin() * 25.0, 1.0]).collect();
        let whole = run(&rope(18.0, 0.2), &raw);
        // the same samples fed in chunks of every size, with a fresh filter state carried across
        for chunk in [1usize, 2, 3, 7, 80] {
            let mut f = Smoother::new(&rope(18.0, 0.2)).unwrap();
            let mut out = Vec::new();
            for part in raw[..raw.len() - 1].chunks(chunk) {
                let batch: Vec<_> = part.iter().map(|r| f.step(*r)).collect();
                out.extend(batch);
            }
            out.extend(f.finish(raw[raw.len() - 1]));
            assert_eq!(out, whole, "chunk {chunk}");
        }
    }

    #[test]
    fn the_string_scales_with_the_view_zoom_and_is_fixed_for_the_stroke() {
        let line: Vec<[f32; 3]> = (0..60).map(|i| [i as f32 * 5.0, 0.0, 1.0]).collect();
        let trail = |scale: f32| {
            let mut f = Smoother::scaled(&rope(30.0, 0.0), scale).unwrap();
            let last = line.iter().map(|r| f.step(*r)).last().unwrap();
            dist(&last, &line[59])
        };
        assert!((trail(1.0) - 30.0).abs() < 1e-3);
        assert!((trail(2.0) - 15.0).abs() < 1e-3, "zoomed in, the string is shorter in canvas px");
        assert!((trail(0.5) - 60.0).abs() < 1e-3);
        // bad scales count as 1
        for bad in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            assert!((trail(bad) - 30.0).abs() < 1e-3, "scale {bad}");
        }
    }

    #[test]
    fn rope_pressure_is_the_pens_own_and_does_not_move_the_position() {
        let raw: Vec<[f32; 3]> = (0..40).map(|i| [i as f32 * 4.0, 0.0, if i % 2 == 0 { 0.9 } else { 0.4 }]).collect();
        let out = rope_steps(10.0, &raw);
        assert!(out.iter().zip(&raw).all(|(a, b)| a[2] == b[2]), "pressure 0 passes the pen's pressure");
        let smoothed = {
            let mut f = Smoother::new(&rope(10.0, 0.8)).unwrap();
            raw.iter().map(|r| f.step(*r)).collect::<Vec<_>>()
        };
        assert_eq!(smoothed.iter().map(|p| p[0]).collect::<Vec<_>>(), out.iter().map(|p| p[0]).collect::<Vec<_>>());
        let swing = |pts: &[[f32; 3]]| pts[10..].iter().map(|p| p[2]).fold(0.0f32, f32::max) - pts[10..].iter().map(|p| p[2]).fold(1.0f32, f32::min);
        assert!(swing(&smoothed) < swing(&out) * 0.5);
    }

    #[test]
    fn hostile_rope_values_are_clamped_and_cannot_panic() {
        for len in [f32::NAN, -5.0, f32::INFINITY, f32::NEG_INFINITY, 1e30, 0.0] {
            let s = rope(len, f32::NAN);
            assert!((0.0..=MAX_ROPE_LENGTH).contains(&s.rope_length()), "{len}");
            for scale in [1.0, 0.0, 1e-30, 1e30, f32::NAN] {
                let Some(mut f) = Smoother::scaled(&s, scale) else { continue };
                for r in [[0.0, 0.0, 1.0], [f32::NAN, 5.0, 0.5], [1e30, -1e30, 7.0], [f32::INFINITY, 0.0, f32::NAN], [-1e38, 1e38, 1.0], [2.0, 2.0, 1.0]] {
                    let p = f.step(r);
                    assert!(p[0].is_finite() && p[1].is_finite() && p[2].is_finite(), "{len} {scale} {r:?} -> {p:?}");
                }
                for e in [[f32::NAN, 0.0, 0.0], [1e30, 1e30, 2.0], [3.0, 3.0, f32::NAN]] {
                    let out = f.finish(e);
                    assert!(out.len() <= MAX_CATCH_UP + 1 && out.iter().flatten().all(|v| v.is_finite()));
                }
            }
        }
    }

    #[test]
    fn the_rope_costs_next_to_nothing_per_sample() {
        let raw = jittery_line();
        let start = std::time::Instant::now();
        let mut n = 0usize;
        for _ in 0..2_000 {
            n += run(&rope(24.0, 0.3), &raw).len();
        }
        let per_sample_ns = start.elapsed().as_nanos() as f64 / (2_000.0 * raw.len() as f64);
        eprintln!("rope cost: {per_sample_ns:.0} ns per sample ({n} points)");
        assert!(per_sample_ns < 20_000.0, "{per_sample_ns} ns per sample");
    }
}
