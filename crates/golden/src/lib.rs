//! Golden-image tests for WaltCut's renderer (test-only crate; the library is empty).
//!
//! `tests/golden.rs` builds small procedural projects (no media files), renders one frame of each
//! through the CPU reference compositor (`waltcut-render`) and compares it with a committed PNG
//! in `goldens/` using `waltcut_testkit::golden`. The same scenes are composited on the GPU
//! (`waltcut-gpu`) when an adapter is available and compared with the CPU result.
//!
//! Regenerate references with `WALTCUT_BLESS=1 cargo test -p waltcut-golden`.
