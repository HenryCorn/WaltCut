//! Drawing-tablet pen pressure, for when the windowing layer has none.
//!
//! winit 0.30 reports a pen as a mouse on macOS and drops its pressure, so the drawing tools would
//! only ever see full-pressure strokes. AppKit puts the pressure of every tablet event on the
//! `NSEvent` itself; a local event monitor reads it on the way past (the event is passed on
//! untouched) and keeps the latest sample here for the UI to read while it builds a stroke.
//!
//! Like [`crate::cursor`], this is a read-only, non-media OS call that lives here because this
//! is the only crate allowed `unsafe` (AGENTS.md §0.3, ADR 0001). Other platforms (and a monitor
//! that could not be installed) report `None`, and the UI draws at full pressure.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The latest pressure (f32 bits) while the pen touches the tablet.
static PRESSURE: AtomicU32 = AtomicU32::new(0);
/// Whether the latest pointer event came from a tablet.
static TABLET: AtomicBool = AtomicBool::new(false);
static INSTALLED: AtomicBool = AtomicBool::new(false);

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

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod imp {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventSubtype, NSEventType};

    pub fn install() -> bool {
        let mask = NSEventMask::LeftMouseDown
            | NSEventMask::LeftMouseDragged
            | NSEventMask::LeftMouseUp
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
                super::record(true, e.pressure());
            } else if matches!(ty, NSEventType::LeftMouseDown | NSEventType::LeftMouseDragged | NSEventType::LeftMouseUp | NSEventType::MouseMoved) {
                // mouse events a pen generates carry the tablet subtype and the pen's pressure
                let tablet = e.subtype() == NSEventSubtype::TabletPoint;
                super::record(tablet, if ty == NSEventType::LeftMouseUp { 0.0 } else { e.pressure() });
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

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn install() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
