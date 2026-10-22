/// Audio crossfade gains (out, in) for progress p.
pub fn audio_gains(kind: &str, p: f32) -> (f32, f32) {
    match kind {
        "constant_gain" => (1.0 - p, p),
        "exponential_fade" => ((1.0 - p).powi(3), 1.0 - (1.0 - p).powi(3)),
        _ => ((p * std::f32::consts::FRAC_PI_2).cos(), (p * std::f32::consts::FRAC_PI_2).sin()),
    }
}
