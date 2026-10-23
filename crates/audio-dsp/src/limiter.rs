//! Look-ahead true-peak limiter retained for export loudness normalisation.
//!
//! Detection is stereo-linked (the loudest channel drives one common gain) so the image does
//! not shift. Gain computers work in dB; attack/release are one-pole smoothers on the gain.

use crate::oversample::{Interpolator, PolyphaseBank, true_peak_factor};
use crate::{AudioEffect, ParamSpec, ParamValues, Smoothed, Unit, block_len, db_to_gain, param_plumbing};
use std::collections::VecDeque;

/// One-pole coefficient for a time constant in ms.
fn coef(ms: f32, sr: f32) -> f32 {
    if ms <= 0.0 { 0.0 } else { (-1.0 / (ms * 0.001 * sr)).exp() }
}

#[inline]
fn lin_to_db(x: f32) -> f32 {
    20.0 * (x.max(1e-10)).log10()
}

/// Look-ahead brick-wall limiter.
///
/// The required gain for each (optionally true-peak) detected sample is held for the look-ahead
/// window with a sliding minimum, released with a one-pole, and then smoothed with a moving
/// average exactly as long as the look-ahead, so the gain has reached its target when the peak
/// leaves the delay line. A final clip at the ceiling guarantees the sample peak never exceeds it.
pub struct Limiter {
    pv: ParamValues,
    sr: f32,
    in_gain: Smoothed,
    ceiling: f32,
    rel: f32,
    true_peak: bool,
    /// Look-ahead (samples) and detector alignment delay.
    look: usize,
    det_delay: usize,
    bank: PolyphaseBank,
    interp: Vec<Interpolator>,
    /// Per-channel delay line (ring), length `look + det_delay + 1`.
    delay: Vec<Vec<f32>>,
    wpos: usize,
    /// Monotonic deque for the sliding minimum: (sample index, gain).
    minq: VecDeque<(u64, f32)>,
    /// Box filter ring and running sum.
    box_ring: Vec<f32>,
    box_pos: usize,
    box_sum: f64,
    rel_state: f32,
    t: u64,
    gr_meter: f32,
}

impl Limiter {
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::new("input_gain", "Input Boost", 0.0, 24.0, 0.0, Unit::Decibels),
        ParamSpec::new("ceiling", "Ceiling", -24.0, 0.0, -1.0, Unit::Decibels),
        ParamSpec::log("release", "Release", 1.0, 1000.0, 60.0, Unit::Milliseconds),
        ParamSpec::toggle("true_peak", "True Peak", true),
    ];
    /// Fixed look-ahead time.
    pub const LOOKAHEAD_MS: f32 = 5.0;

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let channels = channels.max(1);
        let bank = PolyphaseBank::new(true_peak_factor(sample_rate as f64).max(2));
        let look = ((Self::LOOKAHEAD_MS * 0.001 * sample_rate).round() as usize).max(1);
        let det_delay = bank.delay();
        let dlen = look + det_delay + 1;
        let mut s = Limiter {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            in_gain: Smoothed::with_ms(1.0, sample_rate, 20.0),
            ceiling: 1.0,
            rel: 0.0,
            true_peak: true,
            look,
            det_delay,
            bank,
            interp: vec![Interpolator::default(); channels],
            delay: vec![vec![0.0; dlen]; channels],
            wpos: 0,
            minq: VecDeque::with_capacity(look + 2),
            box_ring: vec![1.0; look + 1],
            box_pos: 0,
            box_sum: (look + 1) as f64,
            rel_state: 1.0,
            t: 0,
            gr_meter: 0.0,
        };
        s.apply_params(true);
        s
    }

    fn apply_params(&mut self, snap: bool) {
        self.in_gain.set(db_to_gain(self.pv.v("input_gain")));
        self.ceiling = db_to_gain(self.pv.v("ceiling"));
        self.rel = coef(self.pv.v("release"), self.sr);
        self.true_peak = self.pv.on("true_peak");
        if snap {
            self.in_gain.snap();
        }
    }

    /// Current gain reduction in dB (≤ 0).
    pub fn gain_reduction_db(&self) -> f32 {
        self.gr_meter
    }
}

impl AudioEffect for Limiter {
    param_plumbing!("limiter");
    fn latency(&self) -> usize {
        self.look + self.det_delay
    }
    fn reset(&mut self) {
        self.in_gain.snap();
        self.interp.iter_mut().for_each(Interpolator::reset);
        self.delay.iter_mut().for_each(|d| d.iter_mut().for_each(|v| *v = 0.0));
        self.wpos = 0;
        self.minq.clear();
        self.box_ring.iter_mut().for_each(|v| *v = 1.0);
        self.box_pos = 0;
        self.box_sum = self.box_ring.len() as f64;
        self.rel_state = 1.0;
        self.t = 0;
        self.gr_meter = 0.0;
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        let nch = channels.len().min(self.delay.len());
        let dlen = self.delay[0].len();
        let window = (self.look + 1) as u64;
        for i in 0..n {
            let g_in = self.in_gain.tick();
            let det_pos = (self.wpos + dlen - self.det_delay) % dlen;
            let out_pos = (self.wpos + 1) % dlen; // oldest = look + det_delay samples ago
            let mut det = 0.0f32;
            for ch in 0..nch {
                let x = channels[ch][i] * g_in;
                self.delay[ch][self.wpos] = x;
                let aligned = self.delay[ch][det_pos].abs();
                let tp = if self.true_peak { self.interp[ch].push_peak(&self.bank, x) } else { 0.0 };
                det = det.max(aligned.max(tp));
            }
            let greq = if det > self.ceiling { self.ceiling / det } else { 1.0 };
            // Sliding minimum over the last `window` values.
            while let Some(&(_, v)) = self.minq.back() {
                if v >= greq {
                    self.minq.pop_back();
                } else {
                    break;
                }
            }
            self.minq.push_back((self.t, greq));
            while let Some(&(idx, _)) = self.minq.front() {
                if idx + window <= self.t {
                    self.minq.pop_front();
                } else {
                    break;
                }
            }
            let m = self.minq.front().map_or(1.0, |&(_, v)| v);
            // Release.
            self.rel_state = if m < self.rel_state { m } else { self.rel_state + (m - self.rel_state) * (1.0 - self.rel) };
            // Moving average over look+1 samples.
            self.box_sum += self.rel_state as f64 - self.box_ring[self.box_pos] as f64;
            self.box_ring[self.box_pos] = self.rel_state;
            self.box_pos = (self.box_pos + 1) % self.box_ring.len();
            let g = (self.box_sum / self.box_ring.len() as f64) as f32;
            for ch in 0..nch {
                let y = self.delay[ch][out_pos] * g;
                channels[ch][i] = y.clamp(-self.ceiling, self.ceiling);
            }
            self.wpos = (self.wpos + 1) % dlen;
            self.t += 1;
            // Periodically re-sum to cancel floating-point drift.
            if self.t.is_multiple_of(65536) {
                self.box_sum = self.box_ring.iter().map(|&v| v as f64).sum();
            }
            self.gr_meter = lin_to_db(g);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loudness::LoudnessMeter;
    use crate::testutil::*;

    const SR: f32 = 48000.0;

    #[test]
    fn limiter_never_exceeds_ceiling() {
        let mut rng = Rng::new(7);
        for tp in [0.0, 1.0] {
            let mut lim = Limiter::new(SR, 2);
            lim.set_param("ceiling", -1.0);
            lim.set_param("input_gain", 12.0);
            lim.set_param("true_peak", tp);
            let ceiling = db_to_gain(-1.0);
            let mut all_l = Vec::new();
            let mut all_r = Vec::new();
            for block in 0..200 {
                let len = 1 + (rng.next_u64() % 700) as usize;
                let mut l: Vec<f32> = (0..len).map(|_| rng.uniform() * 0.8).collect();
                let mut r: Vec<f32> = (0..len).map(|_| rng.uniform() * 0.3).collect();
                if block % 7 == 0 {
                    l[len / 2] = 4.0; // impulse
                    r[0] = -3.0;
                }
                lim.process(&mut [&mut l, &mut r]);
                assert!(l.iter().chain(&r).all(|v| v.abs() <= ceiling + 1e-7));
                all_l.extend_from_slice(&l);
                all_r.extend_from_slice(&r);
            }
            if tp == 1.0 {
                // With true-peak detection the reconstructed peak stays close to the ceiling.
                let mut m = LoudnessMeter::new(SR as f64, 2);
                m.process(&[&all_l, &all_r]);
                assert!(m.true_peak_dbtp() < -1.0 + 0.6, "TP {}", m.true_peak_dbtp());
            }
        }
    }

    #[test]
    fn limiter_is_transparent_below_ceiling_with_reported_latency() {
        let mut lim = Limiter::new(SR, 1);
        let lat = lim.latency();
        let x = sine(1000.0, 0.3, SR as f64, 4800, 0.0);
        let mut y = x.clone();
        lim.process(&mut [&mut y]);
        for i in lat..4800 {
            assert!((y[i] - x[i - lat]).abs() < 1e-6);
        }
    }

    #[test]
    fn limiter_reduces_loud_sine_to_ceiling_smoothly() {
        let mut lim = Limiter::new(SR, 1);
        lim.set_param("ceiling", -6.0);
        let mut x = sine(200.0, 1.0, SR as f64, 48000, 0.0);
        lim.process(&mut [&mut x]);
        let tail = &x[24000..];
        let peak = tail.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!((lin_to_db(peak) + 6.0).abs() < 0.3, "{}", lin_to_db(peak));
    }
}
