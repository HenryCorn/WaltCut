# waltcut-audio-dsp

Dependency-free, safe Rust DSP at layer **L1**. Audio is planar `f32`. Implementations are
clean-room from public standards (ITU-R BS.1770-4, EBU R128 / Tech 3341 / Tech 3342, ITU-R BS.775),
the RBJ biquad cookbook and textbook phase-vocoder/resampling techniques.

## Reduction status (#23)

**DRAFT IMPLEMENTATION — native macOS acceptance pending.** The advanced audio-effect registry,
catalogue processors, graphical-editor response APIs, filter-design module and mixer-only 5.1
panner are deleted. No effect registry or compatibility shell remains. Music-retiming, sync and
Essential Sound processing were removed in preceding stacked cuts.

This crate still has actual production callers:

| Module/API | Retained caller |
|---|---|
| `resample::StreamResampler` | device-rate playback and shuttle |
| `PitchShifter`, private `stft`, `fft` | pitch-preserving shuttle |
| `LoudnessMeter`, `normalize_gain_db`, `Limiter` | export loudness measurement, gain correction, true-peak limiting and remeasurement |
| `channels` | actual output-layout conversion and separate Source-monitor multichannel mixdown |
| `biquad`, `oversample`, `Smoothed` | dependencies of retained loudness/limiter/pitch processing |

Export loudness and shared channel conversion remain for a later #23 slice. Six-channel export
converts the stereo soundtrack to L/R plus four silent channels; it is not surround mixing.
Do not delete playback rate conversion or shuttle pitch correction with those later cuts.

`effects::effect_tests` checks every effect: neutral settings are (delayed) identity, output is
deterministic and latency constant (impulse lands at `latency()`), plus a level / frequency-response
test on generated tones, noise or impulses and a realtime-factor floor.

## Conventions and tests

- Streaming processors allocate at construction, not in `process`, `set_param` or `reset`.
- Parameter changes are smoothed per sample; retained processors test block-size independence,
  deterministic latency and finite output.
- Recursive state is flushed below approximately `1e-30` to avoid denormals without unsafe CPU flags.
- The loudness meter uses K-weighting, gated integrated loudness/LRA histograms and oversampled
  true peaks. `tests/loudness_oracle.rs` compares generated signals with external ffmpeg `ebur128`
  when available; ffmpeg is never linked, bundled or shipped.
- Resampling tests cover continuous fractional phase, arbitrary callback blocks and rate changes.
- Platform-independent checks: `cargo test -p waltcut-audio-dsp --quiet`.

Actual program callback and written-WAV gain/mute/Volume regressions live in
`crates/ui-egui/tests/audio_timing.rs`. Windows can run this crate's tests, but native engine,
render and UI execution requires macOS. Cross-target Clippy typechecks those tests; it does not
execute them or establish native acceptance.
