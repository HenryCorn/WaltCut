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
//! A mouse is smoothed the same way (it only jitters less).

use serde::{Deserialize, Serialize};

/// Canvas pixels beyond which a sample is followed closely (the most the smoothed point trails).
pub const FOLLOW_RANGE: f32 = 16.0;
/// Most points a pen-up adds while the smoothed point catches up with the pen.
pub const MAX_CATCH_UP: usize = 32;
/// Catching up stops when the smoothed point is this close (canvas pixels) to the pen.
const CLOSE: f32 = 0.25;

/// Whether strokes are smoothed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmoothMode {
    /// The captured samples are stored as they are.
    #[default]
    Off,
    /// Adaptive moving average of position and (separately) pressure.
    Smooth,
}

impl SmoothMode {
    pub fn param(self) -> &'static str {
        match self {
            SmoothMode::Off => "off",
            SmoothMode::Smooth => "smooth",
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
}

impl Default for Smoothing {
    fn default() -> Self {
        Self { mode: SmoothMode::Off, strength: 0.5, pressure: 0.0 }
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

    /// Whether a stroke made now is smoothed at all.
    pub fn is_active(&self) -> bool {
        self.mode == SmoothMode::Smooth && (self.position() > 0.0 || self.pressure() > 0.0)
    }
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
}

impl Smoother {
    /// The filter for a stroke started now, or None when smoothing is off.
    pub fn new(s: &Smoothing) -> Option<Smoother> {
        s.is_active().then(|| Smoother { base: 1.0 - 0.9 * s.position(), pressure_alpha: 1.0 - 0.8 * s.pressure(), state: None, prev: None })
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
        self.advance(raw, true)
    }

    /// The points that end the stroke at `end`: the smoothed point catching up with the pen, then
    /// `end` itself.
    pub fn finish(&mut self, end: [f32; 3]) -> Vec<[f32; 3]> {
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
        Smoothing { mode: SmoothMode::Smooth, strength, pressure }
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
}
