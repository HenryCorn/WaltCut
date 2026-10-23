# waltcut-audio-dsp

Small, dependency-free, safe Rust helpers for live playback at layer **L1**. The UI uses
`resample::StreamResampler` for device-rate conversion in normal playback and shuttle; shuttle
also uses `PitchShifter` with the private STFT/FFT implementation to preserve pitch. These are
low-level playback helpers shared by two live paths, not an export effects system.

## Reduction status (#23)

**DRAFT IMPLEMENTATION — native macOS acceptance pending.** Export loudness measurement,
normalisation and true-peak limiting, channel-layout conversion, source-monitor multichannel
mixdown, and their DSP modules have been deleted. The soundtrack is stereo: multichannel sources
use the selected first mono/stereo pair (default first two channels), without a surround mix or
surround output path. Export has no channel-layout setting and writes the retained stereo mix.

Static Audio Gain and intrinsic Volume/keyframes remain implemented in the renderer; they do not
use this crate. The crate remains only because playback-rate conversion and pitch-preserving
shuttle processing are live, distinct DSP paths.

## Tests

- Resampler tests cover continuous fractional phase, arbitrary callback blocks and rate changes.
- Pitch-shifter tests cover octave shifts and delayed identity.
- Platform-independent tests: `cargo test -p waltcut-audio-dsp --quiet`.

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
