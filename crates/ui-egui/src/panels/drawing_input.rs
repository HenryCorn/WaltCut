//! Pointer and pen samples for the Brush tool: where each came from, how it is thinned, and what
//! the stroke keeps (#54).
//!
//! A stroke is built from one ordered stream of [`Sample`]s (position, pressure, monotonic time,
//! phase). The stream comes from one of two places, per UI frame:
//!
//! * the host's pen queue (`waltcut_platform::pen` in the desktop app): one sample per pointer event with that
//!   event's own pressure, so several positions with different pressures inside one UI frame keep
//!   their pairs. Its positions are checked against the UI's own pointer: a queue that disagrees
//!   by more than [`MAX_POSITION_ERROR`] points (another window's events, a coordinate mismatch)
//!   is not trusted for that frame;
//! * otherwise the UI's own events, walked in order (press, moves, release), at the pressure
//!   read for the frame (a mouse reports full pressure). This is the mouse fallback.
//!
//! Either way a frame yields every sample up to and including the release, so the endpoint of a
//! stroke never depends on how pointer events were batched into frames.
//!
//! **Decimation** (documented policy): a sample is kept when it is at least [`MIN_STEP`] screen
//! points from the last kept one, or its pressure differs from it by at least
//! [`MIN_PRESSURE_STEP`]; the sample that ends the stroke is always kept when it is somewhere
//! else. A stroke holds at most `MAX_STROKE_POINTS` samples: past that, the newest sample replaces
//! the last one (the stroke keeps following the pen and its end stays true) and the stroke is
//! reported as truncated. Timestamps stay with the samples (replay and diagnostics); a committed
//! stroke stores position and pressure only.

use egui::Pos2;
use serde::{Deserialize, Serialize};

/// What a pointer sample says about the pen (the host's pen queue maps its own phases onto this;
/// the UI crate knows no OS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenPhase {
    Down,
    Move,
    Up,
    /// Moved without touching: not input to a stroke.
    Hover,
}

/// One pointer event from the host's pen queue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    /// Monotonic event time, nanoseconds.
    pub t_ns: u64,
    /// Window content coordinates in points, origin top left (as the host measures them).
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub phase: PenPhase,
    /// Whether it came from a tablet pen rather than a mouse or trackpad.
    pub tablet: bool,
}

/// What the host's queue lost since it was last drained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PenQueueReport {
    /// Samples dropped because the queue was full.
    pub dropped: u64,
    /// Samples discarded for a non-finite position.
    pub invalid: u64,
}

/// Smallest movement (screen points) that adds a sample.
pub const MIN_STEP: f32 = 0.75;
/// Smallest pressure change (0..1) that adds a sample without movement.
pub const MIN_PRESSURE_STEP: f32 = 0.02;
/// A platform sample farther than this (points) from the UI's pointer is not trusted.
pub const MAX_POSITION_ERROR: f32 = 12.0;

/// One input sample, in screen points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Monotonic nanoseconds (the platform's event time, or the UI clock in the fallback).
    pub t_ns: u64,
    pub pos: Pos2,
    pub pressure: f32,
    pub phase: PenPhase,
}

/// Where a frame's samples came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// No pointer input yet.
    #[default]
    Idle,
    /// UI pointer events at full or touch pressure (a mouse, a trackpad).
    Pointer,
    /// The platform's per-event pen queue.
    Pen,
    /// The pen queue disagreed with the pointer, so the UI's events were used.
    PenFallback,
}

/// Input diagnostics for the Brush tool, readable through `ui.inspect` (never loaded back).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PenStats {
    pub source: Source,
    /// Samples the stroke code received, kept or not.
    pub samples: u64,
    /// Samples thinned out by the policy above.
    pub decimated: u64,
    /// Samples the platform queue dropped because the UI fell behind (queue overflow).
    pub queue_dropped: u64,
    /// Samples the platform discarded for a non-finite position.
    pub invalid: u64,
    /// Frames where the pen queue disagreed with the pointer and was not used.
    pub position_mismatches: u64,
    /// Strokes abandoned by Escape or by the window losing focus; nothing was committed.
    pub cancelled: u64,
    /// Strokes that hit the point limit.
    pub truncated: u64,
    /// Samples kept in the last stroke, its duration (first to last sample) in ms, and its
    /// average sampling rate.
    pub last_stroke_points: u64,
    pub last_stroke_ms: f64,
    pub last_stroke_hz: f64,
}

/// The samples of the stroke being drawn.
#[derive(Clone, Debug, Default)]
pub struct StrokeBuilder {
    /// Canvas x, y and pressure of each kept sample.
    pub points: Vec<[f32; 3]>,
    /// Event time of each kept sample.
    pub t_ns: Vec<u64>,
    last: Option<(Pos2, f32)>,
    /// Smooths the kept samples as they arrive (#57); None stores them as captured.
    smoother: Option<super::drawing_smooth::Smoother>,
    pub received: u64,
    pub decimated: u64,
    pub truncated: u64,
}

fn clean_pressure(p: f32) -> f32 {
    if p.is_finite() { p.clamp(0.0, 1.0) } else { 1.0 }
}

impl StrokeBuilder {
    /// A builder for a stroke started with these smoothing settings (Off stores samples as captured).
    pub fn with_smoothing(settings: &super::drawing_smooth::Smoothing) -> Self {
        Self { smoother: super::drawing_smooth::Smoother::new(settings), ..Self::default() }
    }

    /// Offer a sample (`canvas` is its canvas position). Returns whether it was kept.
    pub fn push(&mut self, s: &Sample, canvas: (f32, f32)) -> bool {
        self.received += 1;
        if !s.pos.x.is_finite() || !s.pos.y.is_finite() || !canvas.0.is_finite() || !canvas.1.is_finite() {
            self.decimated += 1;
            return false;
        }
        let p = clean_pressure(s.pressure);
        let keep = self.last.is_none_or(|(q, qp)| q.distance(s.pos) >= MIN_STEP || (p - qp).abs() >= MIN_PRESSURE_STEP);
        if !keep {
            self.decimated += 1;
            return false;
        }
        let point = match self.smoother.as_mut() {
            Some(f) => f.step([canvas.0, canvas.1, p]),
            None => [canvas.0, canvas.1, p],
        };
        self.store(s, point, p);
        true
    }

    /// The sample that ends the stroke: kept unless it is exactly where the last kept one is.
    pub fn finish(&mut self, s: &Sample, canvas: (f32, f32)) {
        self.received += 1;
        if !s.pos.x.is_finite() || !s.pos.y.is_finite() || !canvas.0.is_finite() || !canvas.1.is_finite() {
            return;
        }
        let pressure = clean_pressure(s.pressure);
        if let Some(f) = self.smoother.as_mut() {
            // the smoothed point catches up with the pen and the stroke ends exactly where it was lifted
            for point in f.finish([canvas.0, canvas.1, pressure]) {
                if self.points.last() != Some(&point) {
                    self.store(s, point, pressure);
                }
            }
            return;
        }
        if self.last.is_some_and(|(q, _)| q == s.pos) {
            return;
        }
        self.store(s, [canvas.0, canvas.1, pressure], pressure);
    }

    fn store(&mut self, s: &Sample, point: [f32; 3], pressure: f32) {
        if self.points.len() < waltcut_project::drawing::MAX_STROKE_POINTS {
            self.points.push(point);
            self.t_ns.push(s.t_ns);
        } else {
            // full: keep following the pen, so the stroke still ends where the pen did
            if let Some(p) = self.points.last_mut() {
                *p = point;
            }
            if let Some(t) = self.t_ns.last_mut() {
                *t = s.t_ns;
            }
            self.truncated += 1;
        }
        self.last = Some((s.pos, pressure));
    }

    /// Duration from the first to the last kept sample, in milliseconds.
    pub fn duration_ms(&self) -> f64 {
        match (self.t_ns.first(), self.t_ns.last()) {
            (Some(a), Some(b)) => b.saturating_sub(*a) as f64 / 1e6,
            _ => 0.0,
        }
    }
}

/// What one UI frame saw of the pointer.
pub struct FrameInput<'a> {
    /// Samples drained from the platform's pen queue this frame (oldest first).
    pub platform: &'a [PenSample],
    /// The UI's pointer events of the frame in order: `(position, Some(pressed))` for a button
    /// press or release, `(position, None)` for a move.
    pub events: &'a [(Pos2, Option<bool>)],
    /// Whether a button was already held when the frame began.
    pub was_down: bool,
    /// The UI's current pointer position, to check the platform's against.
    pub pointer_now: Option<Pos2>,
    /// The `ctx.zoom_factor()`: platform points ÷ this = UI points.
    pub zoom: f32,
    /// Pressure for the UI's own events (a mouse is 1).
    pub fallback_pressure: f32,
    /// UI clock for the UI's own events, nanoseconds.
    pub now_ns: u64,
}

/// The frame's samples in order, and where they came from. Only samples of a press (Down, Moves,
/// Up) are returned; hovering is not input to a stroke.
pub fn frame_samples(input: &FrameInput) -> (Vec<Sample>, Source) {
    let zoom = if input.zoom.is_finite() && input.zoom > 0.0 { input.zoom } else { 1.0 };
    let platform: Vec<Sample> = input
        .platform
        .iter()
        .filter(|s| s.phase != PenPhase::Hover)
        .map(|s| Sample { t_ns: s.t_ns, pos: Pos2::new(s.x / zoom, s.y / zoom), pressure: s.pressure, phase: s.phase })
        .collect();
    let any_pen = input.platform.iter().any(|s| s.tablet);
    if let Some(last) = platform.last() {
        // a pen queue is trusted for a frame when its newest position is where the pointer is
        let agrees = input.pointer_now.is_none_or(|p| p.distance(last.pos) <= MAX_POSITION_ERROR);
        if agrees {
            return (platform, if any_pen { Source::Pen } else { Source::Pointer });
        }
    }
    let mut out = Vec::new();
    let mut down = input.was_down;
    for &(pos, ev) in input.events {
        match ev {
            Some(true) => {
                down = true;
                out.push(Sample { t_ns: input.now_ns, pos, pressure: input.fallback_pressure, phase: PenPhase::Down });
            }
            Some(false) => {
                if down {
                    out.push(Sample { t_ns: input.now_ns, pos, pressure: input.fallback_pressure, phase: PenPhase::Up });
                }
                down = false;
            }
            None if down => out.push(Sample { t_ns: input.now_ns, pos, pressure: input.fallback_pressure, phase: PenPhase::Move }),
            None => {}
        }
    }
    let source = if !platform.is_empty() {
        Source::PenFallback
    } else if any_pen {
        Source::Pen
    } else {
        Source::Pointer
    };
    (out, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pen(t_ns: u64, x: f32, pressure: f32, phase: PenPhase) -> PenSample {
        PenSample { t_ns, x, y: 50.0, pressure, phase, tablet: true }
    }

    fn input<'a>(platform: &'a [PenSample], events: &'a [(Pos2, Option<bool>)], pointer_now: Option<Pos2>) -> FrameInput<'a> {
        FrameInput { platform, events, was_down: false, pointer_now, zoom: 1.0, fallback_pressure: 1.0, now_ns: 7 }
    }

    fn run(samples: &[Sample]) -> StrokeBuilder {
        let mut b = StrokeBuilder::default();
        for s in samples {
            match s.phase {
                PenPhase::Up => b.finish(s, (s.pos.x, s.pos.y)),
                _ => {
                    b.push(s, (s.pos.x, s.pos.y));
                }
            }
        }
        b
    }

    #[test]
    fn pressure_stays_with_its_position_within_one_frame() {
        let q = [pen(1, 10.0, 0.1, PenPhase::Down), pen(2, 20.0, 0.5, PenPhase::Move), pen(3, 30.0, 0.9, PenPhase::Move), pen(4, 40.0, 0.9, PenPhase::Up)];
        let (s, source) = frame_samples(&input(&q, &[], Some(Pos2::new(40.0, 50.0))));
        assert_eq!(source, Source::Pen);
        let b = run(&s);
        assert_eq!(b.points, vec![[10.0, 50.0, 0.1], [20.0, 50.0, 0.5], [30.0, 50.0, 0.9], [40.0, 50.0, 0.9]]);
        assert_eq!(b.t_ns, vec![1, 2, 3, 4]);
    }

    #[test]
    fn replay_is_independent_of_frame_batching() {
        let q: Vec<PenSample> = (0..40)
            .map(|i| {
                pen(
                    i as u64 * 4_000_000,
                    10.0 + i as f32 * 2.0,
                    (i as f32 / 40.0).min(1.0),
                    if i == 0 {
                        PenPhase::Down
                    } else if i == 39 {
                        PenPhase::Up
                    } else {
                        PenPhase::Move
                    },
                )
            })
            .collect();
        let whole = {
            let (s, _) = frame_samples(&input(&q, &[], None));
            run(&s)
        };
        for batch in [1usize, 2, 3, 7, 13, 40] {
            let mut b = StrokeBuilder::default();
            for chunk in q.chunks(batch) {
                let (s, _) = frame_samples(&input(chunk, &[], None));
                for x in &s {
                    match x.phase {
                        PenPhase::Up => b.finish(x, (x.pos.x, x.pos.y)),
                        _ => {
                            b.push(x, (x.pos.x, x.pos.y));
                        }
                    }
                }
            }
            assert_eq!(b.points, whole.points, "batches of {batch}");
            assert_eq!(b.t_ns, whole.t_ns, "batches of {batch}");
        }
        // the final endpoint is the last sample
        assert_eq!(whole.points.last().map(|p| p[0]), Some(88.0));
    }

    #[test]
    fn a_pointer_events_frame_keeps_the_moves_in_the_release_frame() {
        // the UI's own events: press, two moves and the release all in one frame
        let ev = [(Pos2::new(5.0, 5.0), Some(true)), (Pos2::new(15.0, 5.0), None), (Pos2::new(25.0, 5.0), None), (Pos2::new(35.0, 5.0), Some(false))];
        let (s, source) = frame_samples(&input(&[], &ev, Some(Pos2::new(35.0, 5.0))));
        assert_eq!(source, Source::Pointer);
        assert_eq!(s.iter().map(|x| x.phase).collect::<Vec<_>>(), vec![PenPhase::Down, PenPhase::Move, PenPhase::Move, PenPhase::Up]);
        let b = run(&s);
        assert_eq!(b.points.last().map(|p| p[0]), Some(35.0));
        // hovering before the press is not part of the stroke; neither is movement after the release
        let ev = [(Pos2::new(1.0, 1.0), None), (Pos2::new(5.0, 5.0), Some(true)), (Pos2::new(9.0, 5.0), Some(false)), (Pos2::new(50.0, 5.0), None)];
        let (s, _) = frame_samples(&input(&[], &ev, None));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn a_platform_queue_that_disagrees_with_the_pointer_is_not_trusted() {
        let q = [pen(1, 500.0, 0.3, PenPhase::Down), pen(2, 510.0, 0.3, PenPhase::Move)];
        let ev = [(Pos2::new(10.0, 50.0), Some(true)), (Pos2::new(20.0, 50.0), None)];
        let (s, source) = frame_samples(&input(&q, &ev, Some(Pos2::new(20.0, 50.0))));
        assert_eq!(source, Source::PenFallback);
        assert_eq!(s.iter().map(|x| x.pos.x).collect::<Vec<_>>(), vec![10.0, 20.0]);
        // the interface zoom divides platform points down to UI points
        let z = FrameInput { zoom: 2.0, ..input(&q, &[], Some(Pos2::new(255.0, 25.0))) };
        let (s, source) = frame_samples(&z);
        assert_eq!(source, Source::Pen);
        assert_eq!(s.last().map(|x| x.pos.x), Some(255.0));
    }

    #[test]
    fn decimation_keeps_pressure_changes_and_the_endpoint() {
        let at = |t: u64, x: f32, p: f32, phase| Sample { t_ns: t, pos: Pos2::new(x, 0.0), pressure: p, phase };
        let b = run(&[
            at(0, 0.0, 0.5, PenPhase::Down),
            at(1, 0.2, 0.5, PenPhase::Move),  // too close, same pressure: thinned
            at(2, 0.3, 0.9, PenPhase::Move),  // too close but the pressure moved: kept
            at(3, 0.35, 0.9, PenPhase::Move), // thinned
            at(4, 0.4, 0.9, PenPhase::Up),    // the end is somewhere else than the last kept one: kept
        ]);
        assert_eq!(b.points.len(), 3);
        assert_eq!(b.decimated, 2);
        assert_eq!(b.points.last().map(|p| p[0]), Some(0.4));
        // ending exactly where the last kept sample is adds nothing
        let b = run(&[at(0, 1.0, 0.5, PenPhase::Down), at(1, 1.0, 0.5, PenPhase::Up)]);
        assert_eq!(b.points.len(), 1);
    }

    #[test]
    fn long_strokes_and_hostile_values_are_bounded() {
        let mut b = StrokeBuilder::default();
        let max = waltcut_project::drawing::MAX_STROKE_POINTS;
        for i in 0..max + 50 {
            b.push(&Sample { t_ns: i as u64, pos: Pos2::new(i as f32 * 2.0, 0.0), pressure: 1.0, phase: PenPhase::Move }, (i as f32 * 2.0, 0.0));
        }
        assert_eq!(b.points.len(), max);
        assert_eq!(b.truncated, 50);
        // the end still follows the pen
        assert_eq!(b.points.last().map(|p| p[0]), Some((max + 49) as f32 * 2.0));
        let mut h = StrokeBuilder::default();
        assert!(!h.push(&Sample { t_ns: 0, pos: Pos2::new(f32::NAN, 0.0), pressure: 1.0, phase: PenPhase::Down }, (0.0, 0.0)));
        assert!(h.push(&Sample { t_ns: 0, pos: Pos2::new(1.0, 1.0), pressure: f32::NAN, phase: PenPhase::Down }, (1.0, 1.0)));
        h.finish(&Sample { t_ns: 1, pos: Pos2::new(f32::INFINITY, 0.0), pressure: 1.0, phase: PenPhase::Up }, (f32::INFINITY, 0.0));
        assert_eq!(h.points, vec![[1.0, 1.0, 1.0]]);
        assert_eq!(StrokeBuilder::default().duration_ms(), 0.0);
        // a hostile zoom factor or an empty frame does not panic
        let (s, _) = frame_samples(&FrameInput { zoom: 0.0, ..input(&[], &[], None) });
        assert!(s.is_empty());
    }
}
