//! Drawing-tablet pen input, for when the windowing layer has none.
//!
//! winit 0.30 reports a pen as a mouse on macOS and drops its pressure, so the drawing tools would
//! only ever see full-pressure strokes. AppKit puts the pressure of every tablet event on the
//! `NSEvent` itself; a local event monitor reads it on the way past (the event is passed on
//! untouched). Each pointer event becomes one [`PenSample`]: position, pressure, the event's own
//! monotonic timestamp and the phase (down, move, up, hover). Samples go into a bounded queue
//! that the UI drains once per frame ([`drain_samples`]), so the position and the pressure of a
//! sample always belong together, however many samples arrive within one UI frame. The latest
//! pressure is also kept for readouts ([`pen_pressure`]).
//!
//! The queue is bounded ([`QUEUE_CAPACITY`]). When the UI stalls long enough to fill it, the
//! oldest `Move`/`Hover` samples are dropped first (a stroke keeps its `Down` and `Up` samples, so
//! its endpoints survive) and the loss is counted in [`DrainReport::dropped`] for the UI to show.
//! Samples with a non-finite position are discarded and counted; pressure is clamped to 0..1.
//!
//! Like [`crate::cursor`], this is a read-only, non-media OS call that lives here because this
//! is the only crate allowed `unsafe` (AGENTS.md §0.3, ADR 0001). A monitor that could not be installed reports `None`, and the UI draws at full pressure.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The latest pressure (f32 bits) while the pen touches the tablet.
static PRESSURE: AtomicU32 = AtomicU32::new(0);
/// Whether the latest pointer event came from a tablet.
static TABLET: AtomicBool = AtomicBool::new(false);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Most samples the queue holds between two UI frames (a 240 Hz pen for about 17 s).
pub const QUEUE_CAPACITY: usize = 4096;

/// What an input sample says about the pen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenPhase {
    /// The tip or button went down.
    Down,
    /// Moved while down.
    Move,
    /// The tip or button came up.
    Up,
    /// Moved without touching.
    Hover,
}

/// One pointer event: where, how hard and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    /// Monotonic event time in nanoseconds (AppKit's time since system start, so differences are
    /// the pen's own sampling intervals, whatever the UI frame rate).
    pub t_ns: u64,
    /// Window content coordinates in points, origin top left, as the UI reports its pointer.
    pub x: f32,
    pub y: f32,
    /// 0..1; 1 for a mouse.
    pub pressure: f32,
    pub phase: PenPhase,
    /// Whether the event came from a tablet pen rather than a mouse or trackpad.
    pub tablet: bool,
}

/// What a [`SampleQueue::drain_into`] found besides the samples.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrainReport {
    /// Samples received since the last drain (kept or not).
    pub received: u64,
    /// Samples dropped because the queue was full.
    pub dropped: u64,
    /// Samples discarded for a non-finite position.
    pub invalid: u64,
}

/// A bounded FIFO of [`PenSample`]s with the overflow rules above.
#[derive(Debug)]
pub struct SampleQueue {
    buf: VecDeque<PenSample>,
    cap: usize,
    received: u64,
    dropped: u64,
    invalid: u64,
    /// Pressure of the latest `Down` or `Move`: what the `Up` that ends the stroke reports.
    down_pressure: f32,
}

impl SampleQueue {
    pub const fn new(cap: usize) -> Self {
        Self { buf: VecDeque::new(), cap, received: 0, dropped: 0, invalid: 0, down_pressure: 1.0 }
    }

    /// Like [`push`](Self::push), for a sample straight from the OS: the sample that ends a
    /// stroke (`Up`) takes the pressure of the one before it, as a lifted pen reports none.
    pub fn push_event(&mut self, mut s: PenSample) {
        match s.phase {
            PenPhase::Up => s.pressure = self.down_pressure,
            PenPhase::Down | PenPhase::Move => self.down_pressure = if s.pressure.is_finite() { s.pressure.clamp(0.0, 1.0) } else { 1.0 },
            PenPhase::Hover => {}
        }
        self.push(s);
    }

    pub fn push(&mut self, mut s: PenSample) {
        self.received = self.received.saturating_add(1);
        if !s.x.is_finite() || !s.y.is_finite() {
            self.invalid = self.invalid.saturating_add(1);
            return;
        }
        s.pressure = if s.pressure.is_finite() { s.pressure.clamp(0.0, 1.0) } else { 1.0 };
        if self.cap == 0 {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        if self.buf.len() >= self.cap {
            // keep the Down and Up samples: they bound a stroke
            let victim = self.buf.iter().position(|q| matches!(q.phase, PenPhase::Move | PenPhase::Hover)).unwrap_or(0);
            self.buf.remove(victim);
            self.dropped = self.dropped.saturating_add(1);
        }
        self.buf.push_back(s);
    }

    /// Move every queued sample, oldest first, to the end of `out`.
    pub fn drain_into(&mut self, out: &mut Vec<PenSample>) -> DrainReport {
        out.extend(self.buf.drain(..));
        let r = DrainReport { received: self.received, dropped: self.dropped, invalid: self.invalid };
        self.received = 0;
        self.dropped = 0;
        self.invalid = 0;
        r
    }
}

static QUEUE: Mutex<SampleQueue> = Mutex::new(SampleQueue::new(QUEUE_CAPACITY));

/// Take the samples queued since the last call, oldest first.
pub fn drain_samples(out: &mut Vec<PenSample>) -> DrainReport {
    QUEUE.lock().unwrap_or_else(PoisonError::into_inner).drain_into(out)
}

/// Queue a sample from the event monitor.
fn push_pointer_sample(s: PenSample) {
    QUEUE.lock().unwrap_or_else(PoisonError::into_inner).push_event(s);
}

fn record(tablet: bool, pressure: f32) {
    TABLET.store(tablet, Ordering::Relaxed);
    let p = if pressure.is_finite() { pressure.clamp(0.0, 1.0) } else { 1.0 };
    PRESSURE.store(p.to_bits(), Ordering::Relaxed);
}

/// The pen's pressure (0..1) if the pointer is currently a tablet pen, else `None` (a mouse, a
/// trackpad, or no monitor on this platform).
pub fn pen_pressure() -> Option<f32> {
    TABLET.load(Ordering::Relaxed).then(|| f32::from_bits(PRESSURE.load(Ordering::Relaxed)))
}

/// Start listening for tablet events. Call once, on the main thread, after the application
/// object exists (from the eframe app creator). Returns whether pressure can be read.
pub fn install_pen_monitor() -> bool {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return true;
    }
    let ok = imp::install();
    if !ok {
        INSTALLED.store(false, Ordering::SeqCst);
    }
    ok
}

#[allow(unsafe_code)]
mod imp {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventSubtype, NSEventType};

    use super::{PenPhase, PenSample};

    /// A pointer event as a sample (None for an event that is not a pointer event or has no
    /// window to measure against).
    fn sample_of(e: &NSEvent, ty: NSEventType, tablet: bool, pressure: f32) -> Option<PenSample> {
        let phase = match ty {
            NSEventType::LeftMouseDown | NSEventType::RightMouseDown => PenPhase::Down,
            NSEventType::LeftMouseDragged | NSEventType::RightMouseDragged => PenPhase::Move,
            NSEventType::LeftMouseUp | NSEventType::RightMouseUp => PenPhase::Up,
            NSEventType::MouseMoved => PenPhase::Hover,
            _ => return None,
        };
        let mtm = MainThreadMarker::new()?;
        let height = e.window(mtm)?.contentView()?.frame().size.height;
        let loc = e.locationInWindow();
        // AppKit's origin is the bottom left; the UI's is the top left
        let (x, y) = (loc.x as f32, (height - loc.y) as f32);
        let secs = e.timestamp();
        let t_ns = if secs.is_finite() && secs > 0.0 { (secs * 1e9) as u64 } else { 0 };
        Some(PenSample { t_ns, x, y, pressure, phase, tablet })
    }

    pub fn install() -> bool {
        let mask = NSEventMask::LeftMouseDown
            | NSEventMask::LeftMouseDragged
            | NSEventMask::LeftMouseUp
            | NSEventMask::RightMouseDown
            | NSEventMask::RightMouseDragged
            | NSEventMask::RightMouseUp
            | NSEventMask::MouseMoved
            | NSEventMask::TabletPoint
            | NSEventMask::TabletProximity;
        let handler = RcBlock::new(|ev: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit hands the monitor a valid event that outlives the handler call.
            let e = unsafe { ev.as_ref() };
            let ty = e.r#type();
            if ty == NSEventType::TabletProximity {
                super::record(e.isEnteringProximity(), 0.0);
            } else if ty == NSEventType::TabletPoint {
                // beside the mouse events a pen generates: only the readout's pressure
                super::record(true, e.pressure());
            } else {
                // mouse events a pen generates carry the tablet subtype and the pen's pressure
                let tablet = e.subtype() == NSEventSubtype::TabletPoint;
                let pressure = if matches!(ty, NSEventType::LeftMouseUp | NSEventType::RightMouseUp) { 0.0 } else { e.pressure() };
                super::record(tablet, pressure);
                if let Some(s) = sample_of(e, ty, tablet, if tablet { e.pressure() } else { 1.0 }) {
                    super::push_pointer_sample(s);
                }
            }
            // pass the event on unchanged
            ev.as_ptr()
        });
        // SAFETY: the handler always returns the valid event pointer it was given.
        let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) };
        // The monitor lives as long as the app: AppKit keeps the handler alive, and the token is
        // only needed to remove it, which we never do.
        let ok = monitor.is_some();
        std::mem::forget(monitor);
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t_ns: u64, x: f32, pressure: f32, phase: PenPhase) -> PenSample {
        PenSample { t_ns, x, y: 5.0, pressure, phase, tablet: true }
    }

    #[test]
    fn samples_keep_order_position_pressure_and_time() {
        let mut q = SampleQueue::new(16);
        let input = [s(10, 1.0, 0.1, PenPhase::Down), s(14, 2.0, 0.4, PenPhase::Move), s(18, 3.0, 0.9, PenPhase::Move), s(22, 4.0, 0.0, PenPhase::Up)];
        for x in input {
            q.push_event(x);
        }
        let mut out = Vec::new();
        let r = q.drain_into(&mut out);
        assert_eq!(r, DrainReport { received: 4, dropped: 0, invalid: 0 });
        let pairs: Vec<(u64, f32, f32)> = out.iter().map(|x| (x.t_ns, x.x, x.pressure)).collect();
        // the Up that ends the stroke keeps the pressure of the sample before it
        assert_eq!(pairs, vec![(10, 1.0, 0.1), (14, 2.0, 0.4), (18, 3.0, 0.9), (22, 4.0, 0.9)]);
        assert_eq!(out.last().map(|x| x.phase), Some(PenPhase::Up));
        // and across drains: the Up arrives in a later frame than the last Move
        q.push_event(s(30, 1.0, 0.7, PenPhase::Move));
        q.drain_into(&mut out);
        q.push_event(s(34, 1.0, 0.0, PenPhase::Up));
        out.clear();
        q.drain_into(&mut out);
        assert_eq!(out.first().map(|x| x.pressure), Some(0.7));
    }

    #[test]
    fn overflow_drops_the_oldest_moves_first_and_counts_them() {
        let mut q = SampleQueue::new(4);
        q.push(s(0, 0.0, 1.0, PenPhase::Down));
        for i in 1..=8u64 {
            q.push(s(i, i as f32, 1.0, PenPhase::Move));
        }
        q.push(s(9, 9.0, 1.0, PenPhase::Up));
        let mut out = Vec::new();
        let r = q.drain_into(&mut out);
        assert_eq!(r.received, 10);
        assert_eq!(r.dropped, 6);
        assert_eq!(out.len(), 4);
        // both ends of the stroke survive; the newest moves are what is left in between
        assert_eq!(out.first().map(|x| x.phase), Some(PenPhase::Down));
        assert_eq!(out.last().map(|x| x.phase), Some(PenPhase::Up));
        assert_eq!(out.iter().map(|x| x.t_ns).collect::<Vec<_>>(), vec![0, 7, 8, 9]);
        // the counters restart after a drain
        assert_eq!(q.drain_into(&mut out), DrainReport::default());
    }

    #[test]
    fn hostile_samples_are_discarded_or_clamped_and_a_zero_capacity_queue_is_safe() {
        let mut q = SampleQueue::new(8);
        q.push(s(0, f32::NAN, 0.5, PenPhase::Move));
        q.push(PenSample { y: f32::INFINITY, ..s(1, 1.0, 0.5, PenPhase::Move) });
        q.push(s(2, 1.0, f32::NAN, PenPhase::Move));
        q.push(s(3, 1.0, 9.0, PenPhase::Move));
        q.push(s(4, 1.0, -3.0, PenPhase::Move));
        let mut out = Vec::new();
        let r = q.drain_into(&mut out);
        assert_eq!((r.received, r.invalid, r.dropped), (5, 2, 0));
        assert_eq!(out.iter().map(|x| x.pressure).collect::<Vec<_>>(), vec![1.0, 1.0, 0.0]);
        let mut none = SampleQueue::new(0);
        none.push(s(0, 1.0, 1.0, PenPhase::Down));
        assert_eq!(none.drain_into(&mut out).dropped, 1);
    }

    #[test]
    fn samples_are_clamped_and_mice_report_none() {
        record(true, 7.0);
        assert_eq!(pen_pressure(), Some(1.0));
        record(true, f32::NAN);
        assert_eq!(pen_pressure(), Some(1.0));
        record(true, 0.25);
        assert_eq!(pen_pressure(), Some(0.25));
        record(false, 0.5);
        assert_eq!(pen_pressure(), None);
    }
}
