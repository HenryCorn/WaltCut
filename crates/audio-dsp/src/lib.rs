//! Playback resampling and pitch-preserving shuttle processing.
//!
//! `PitchShifter` and `resample::StreamResampler` are low-level helpers shared by playback and
//! shuttle code. This crate intentionally contains no export processing or channel-layout policy.
//!
//! Conventions: audio is **planar f32** (`&mut [&mut [f32]]`, one slice per channel, all the same
//! length). `process` never allocates; all buffers are sized at construction. Parameters may be
//! changed between blocks and are smoothed per sample, so the output is independent of how the
//! stream is cut into blocks. Recursive state is flushed to zero when it decays below ~1e-30 so
//! denormals never appear (no FTZ/DAZ CPU flags needed, which keeps the crate `unsafe`-free and
//! portable to wasm).

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod fft;
mod pitch;
pub mod resample;
mod stft;

pub use pitch::PitchShifter;

/// Unit of a parameter value (for display and control construction).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Semitones,
}

/// Static description of one effect parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    /// Stable id used by `set_param` / automation (e.g. `"threshold"`, `"b3.freq"`).
    pub id: &'static str,
    /// Human-readable label.
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: Unit,
}

impl ParamSpec {
    /// A continuous (linear) parameter.
    pub const fn new(id: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit) -> Self {
        ParamSpec { id, name, min, max, default, unit }
    }
    /// Clamp (and for toggles, round) a value into this parameter's domain.
    /// NaN maps to the default.
    pub fn sanitize(&self, v: f32) -> f32 {
        if v.is_nan() {
            return self.default;
        }
        v.clamp(self.min, self.max)
    }
}

/// A real-time audio effect operating in place on planar channels.
///
/// Implementations are constructed (allocating) with a sample rate and channel count; after that
/// `process`, `set_param` and `reset` never allocate. If `process` receives more channels than the
/// effect was built for, the extra channels are passed through untouched (channel-generic effects
/// such as gain process all of them).
pub trait AudioEffect: Send {
    /// Processor id.
    fn id(&self) -> &'static str;
    /// Parameter descriptions.
    fn params(&self) -> &'static [ParamSpec];
    /// Set a parameter by id; the value is clamped to its range. Returns `false` for unknown ids.
    /// Takes effect smoothly from the next processed sample.
    fn set_param(&mut self, id: &str, value: f32) -> bool;
    /// Current (target) value of a parameter.
    fn param(&self, id: &str) -> Option<f32>;
    /// Clear all internal state (delay lines, envelopes, filter memories); smoothed parameters
    /// jump to their targets.
    fn reset(&mut self);
    /// Process one block in place. All channel slices must have the same length (the shortest
    /// length is used otherwise). Zero-length blocks are allowed.
    fn process(&mut self, channels: &mut [&mut [f32]]);
    /// Processing latency in samples (look-ahead / STFT delay) for delay compensation.
    fn latency(&self) -> usize {
        0
    }
}

/// Parameter value storage shared by the effect implementations.
#[derive(Clone, Debug)]
pub(crate) struct ParamValues {
    specs: &'static [ParamSpec],
    values: Vec<f32>,
}

impl ParamValues {
    pub(crate) fn new(specs: &'static [ParamSpec]) -> Self {
        ParamValues { specs, values: specs.iter().map(|s| s.default).collect() }
    }
    pub(crate) fn index_of(&self, id: &str) -> Option<usize> {
        self.specs.iter().position(|s| s.id == id)
    }
    /// Returns `true` if the id exists.
    pub(crate) fn set(&mut self, id: &str, v: f32) -> bool {
        match self.index_of(id) {
            Some(i) => {
                self.values[i] = self.specs[i].sanitize(v);
                true
            }
            None => false,
        }
    }
    pub(crate) fn get(&self, id: &str) -> Option<f32> {
        self.index_of(id).map(|i| self.values[i])
    }
    /// Value of a parameter that is known to exist (0 otherwise — a programming error caught by
    /// debug builds).
    pub(crate) fn v(&self, id: &str) -> f32 {
        debug_assert!(self.get(id).is_some(), "unknown parameter {id}");
        self.get(id).unwrap_or(0.0)
    }
}

/// Generates the id/params/set_param/param methods of [`AudioEffect`] for a struct with a
/// `pv: ParamValues` field, a `PARAMS` const and an `apply_params(&mut self, snap: bool)` method.
macro_rules! param_plumbing {
    ($id:expr) => {
        fn id(&self) -> &'static str {
            $id
        }
        fn params(&self) -> &'static [$crate::ParamSpec] {
            Self::PARAMS
        }
        fn set_param(&mut self, id: &str, value: f32) -> bool {
            if self.pv.set(id, value) {
                self.apply_params(false);
                true
            } else {
                false
            }
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.pv.get(id)
        }
    };
}
pub(crate) use param_plumbing;

/// Common block length of planar channels.
#[inline]
pub(crate) fn block_len(channels: &[&mut [f32]]) -> usize {
    channels.iter().map(|c| c.len()).min().unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod testutil {
    pub fn sine(freq: f64, amp: f64, sr: f64, n: usize, phase: f64) -> Vec<f32> {
        (0..n).map(|i| (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / sr + phase).sin()) as f32).collect()
    }

    pub fn rms(x: &[f32]) -> f64 {
        if x.is_empty() {
            return 0.0;
        }
        (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64).sqrt()
    }

    pub fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    /// Amplitude of the `freq` component of `x` (single-bin DFT / Goertzel-style correlation).
    pub fn tone_amplitude(x: &[f32], freq: f64, sr: f64) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let w = 2.0 * std::f64::consts::PI * freq * i as f64 / sr;
            re += v as f64 * w.cos();
            im += v as f64 * w.sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f64
    }
}
