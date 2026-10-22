//! Retained parameter schemas and instances on track items.
//!
//! Retained intrinsic and graphic definitions drive Effect Controls and the MCP schema.
//! Motion/Opacity/Volume processing lives in render; text/shape data shares the parameter engine.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use waltcut_geom::Vec2;

use crate::keyframe::{Param, ParamValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectKind {
    Video,
    Audio,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum ParamKind {
    /// Scalar with a slider range (`soft_*` is the slider range; hard min/max clamp typed values).
    Float {
        min: f64,
        max: f64,
        soft_min: f64,
        soft_max: f64,
        unit: &'static str,
        decimals: u8,
    },
    Point,
    Color,
    Bool,
    Choice(&'static [&'static str]),
    Angle,
    Text,
    /// Tone curve (x in, y out); `hue` curves use a rainbow baseline with y = 0.5 as neutral.
    Curve {
        hue: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParamDef {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    pub default: ParamValue,
    /// Can have keyframes (the stopwatch appears).
    pub animatable: bool,
    /// Group (twirl) label within the effect, e.g. "Basic Correction".
    pub group: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectDef {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: EffectKind,
    pub params: Vec<ParamDef>,
    /// Intrinsic (fixed) effects appear on every clip and cannot be deleted.
    pub intrinsic: bool,
    /// GPU accelerated badge in the Effects panel.
    pub accelerated: bool,
    /// 32-bit colour badge.
    pub float32: bool,
    /// YUV badge.
    pub yuv: bool,
}

impl EffectDef {
    pub fn param(&self, id: &str) -> Option<&ParamDef> {
        self.params.iter().find(|p| p.id == id)
    }
    pub fn instance(&self) -> EffectInstance {
        EffectInstance {
            effect: self.id.to_string(),
            enabled: true,
            params: self.params.iter().map(|p| (p.id.to_string(), Param::new(p.default.clone()))).collect(),
            layer: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectInstance {
    #[serde(deserialize_with = "deserialize_effect_id")]
    pub effect: String,
    pub enabled: bool,
    pub params: BTreeMap<String, Param>,
    /// Graphic layers only: stable uid, per-character styles and responsive pin
    /// ([`crate::graphic_design::LayerExtra`]). Schema v12.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<Box<crate::graphic_design::LayerExtra>>,
}

fn deserialize_effect_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let id = String::deserialize(deserializer)?;
    if find_effect(&id).is_none() {
        return Err(serde::de::Error::custom(format!("unsupported effect `{id}`")));
    }
    Ok(id)
}

impl EffectInstance {
    pub fn def(&self) -> Option<&'static EffectDef> {
        find_effect(&self.effect)
    }
    pub fn param(&self, id: &str) -> Option<&Param> {
        self.params.get(id)
    }
    pub fn param_mut(&mut self, id: &str) -> Option<&mut Param> {
        self.params.get_mut(id)
    }
    pub fn f64_at(&self, id: &str, t: waltcut_time::Tick) -> f64 {
        self.params.get(id).map(|p| p.f64_at(t)).or_else(|| self.def().and_then(|d| d.param(id)).and_then(|p| p.default.as_f64())).unwrap_or(0.0)
    }
    pub fn vec2_at(&self, id: &str, t: waltcut_time::Tick) -> Vec2 {
        self.params.get(id).map(|p| p.vec2_at(t)).unwrap_or_default()
    }
    pub fn is_animated(&self) -> bool {
        self.params.values().any(Param::is_animated)
    }
}

pub const BLEND_MODES: &[&str] = &[
    "Normal",
    "Dissolve",
    "Darken",
    "Multiply",
    "Color Burn",
    "Linear Burn",
    "Darker Color",
    "Lighten",
    "Screen",
    "Color Dodge",
    "Linear Dodge (Add)",
    "Lighter Color",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "Vivid Light",
    "Linear Light",
    "Pin Light",
    "Hard Mix",
    "Difference",
    "Exclusion",
    "Subtract",
    "Divide",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];

pub(crate) fn fs(
    id: &'static str,
    label: &'static str,
    def: f64,
    (min, max): (f64, f64),
    (smin, smax): (f64, f64),
    unit: &'static str,
    decimals: u8,
) -> ParamDef {
    ParamDef {
        id,
        label,
        kind: ParamKind::Float { min, max, soft_min: smin, soft_max: smax, unit, decimals },
        default: ParamValue::Float(def),
        animatable: true,
        group: None,
    }
}
pub(crate) fn pt(id: &'static str, label: &'static str, x: f64, y: f64) -> ParamDef {
    ParamDef { id, label, kind: ParamKind::Point, default: ParamValue::Vec2(Vec2::new(x, y)), animatable: true, group: None }
}
pub(crate) fn b(id: &'static str, label: &'static str, v: bool) -> ParamDef {
    ParamDef { id, label, kind: ParamKind::Bool, default: ParamValue::Bool(v), animatable: false, group: None }
}
pub(crate) fn ch(id: &'static str, label: &'static str, opts: &'static [&'static str], def: u32) -> ParamDef {
    ParamDef { id, label, kind: ParamKind::Choice(opts), default: ParamValue::Choice(def), animatable: false, group: None }
}
pub(crate) fn ang(id: &'static str, label: &'static str, def: f64) -> ParamDef {
    ParamDef { id, label, kind: ParamKind::Angle, default: ParamValue::Float(def), animatable: true, group: None }
}
fn build_effects() -> Vec<EffectDef> {
    let mut v = vec![
        // ---- intrinsic ----
        EffectDef {
            id: "motion",
            name: "Motion",
            kind: EffectKind::Video,
            params: vec![
                pt("position", "Position", f64::NAN, f64::NAN),
                fs("scale", "Scale", 100.0, (0.0, 10000.0), (0.0, 100.0), "", 1),
                fs("scale_width", "Scale Width", 100.0, (0.0, 10000.0), (0.0, 100.0), "", 1),
                b("uniform_scale", "Uniform Scale", true),
                ang("rotation", "Rotation", 0.0),
                pt("anchor", "Anchor Point", f64::NAN, f64::NAN),
            ],
            intrinsic: true,
            accelerated: true,
            float32: true,
            yuv: true,
        },
        EffectDef {
            id: "opacity",
            name: "Opacity",
            kind: EffectKind::Video,
            params: vec![fs("opacity", "Opacity", 100.0, (0.0, 100.0), (0.0, 100.0), "%", 1), ch("blend", "Blend Mode", BLEND_MODES, 0)],
            intrinsic: true,
            accelerated: true,
            float32: true,
            yuv: true,
        },
        EffectDef {
            id: "volume",
            name: "Volume",
            kind: EffectKind::Audio,
            params: vec![b("bypass", "Bypass", false), fs("level", "Level", 0.0, (-287.5, 15.0), (-60.0, 15.0), "dB", 1)],
            intrinsic: true,
            accelerated: false,
            float32: true,
            yuv: false,
        },
    ];
    v.extend(crate::graphic::layer_defs());
    v
}

pub fn effect_defs() -> &'static [EffectDef] {
    static DEFS: OnceLock<Vec<EffectDef>> = OnceLock::new();
    DEFS.get_or_init(build_effects)
}

pub fn find_effect(id: &str) -> Option<&'static EffectDef> {
    effect_defs().iter().find(|e| e.id == id)
}

/// Case-insensitive lookup by id or display name.
pub fn find_effect_by_name(name: &str) -> Option<&'static EffectDef> {
    let n = name.to_ascii_lowercase();
    effect_defs().iter().find(|e| e.id == n || e.name.to_ascii_lowercase() == n)
}

/// Intrinsic video effects every video track item carries (Motion, Opacity).
pub fn intrinsic_video() -> Vec<EffectInstance> {
    ["motion", "opacity"].iter().filter_map(|id| find_effect(id)).map(EffectDef::instance).collect()
}

pub fn intrinsic_audio() -> Vec<EffectInstance> {
    ["volume"].iter().filter_map(|id| find_effect(id)).map(EffectDef::instance).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_ids_and_defaults() {
        let mut ids = std::collections::HashSet::new();
        for e in effect_defs() {
            assert!(ids.insert(e.id), "duplicate {}", e.id);
            let inst = e.instance();
            assert_eq!(inst.params.len(), e.params.len());
        }
        assert!(effect_defs().iter().filter(|d| d.kind == EffectKind::Video).all(|d| d.intrinsic || crate::graphic::is_layer(&d.instance())));
        assert!(find_effect_by_name("Gaussian Blur").is_none());
        assert_eq!(intrinsic_video().len(), 2);
        assert_eq!(intrinsic_audio().len(), 1);
        assert!(effect_defs().iter().filter(|d| d.kind == EffectKind::Audio).all(|d| d.id == "volume" && d.intrinsic));
    }

    #[test]
    fn persisted_removed_effects_are_rejected_not_silently_discarded() {
        let instance = find_effect("opacity").unwrap().instance();
        let original = serde_json::to_value(&instance).unwrap();
        for id in [
            "gaussian_blur",
            "crop",
            "ultra_key",
            "warp_stabilizer",
            "cross_dissolve",
            "wipe",
            "time_remap",
            "remix",
            "unknown",
            "amplify",
            "channel_mixer_a",
            "channel_volume_a",
            "deesser",
            "dynamics_rack",
            "dynamics",
            "parametric_eq",
            "hard_limiter",
            "multiband_compressor",
            "single_band_compressor",
            "tube_compressor",
            "analog_delay",
            "delay",
            "multitap_delay",
            "bandpass",
            "bass",
            "fft_filter",
            "graphic_eq",
            "graphic_eq_20",
            "graphic_eq_30",
            "highpass",
            "lowpass",
            "notch",
            "scientific_filter",
            "simple_notch",
            "simple_eq",
            "treble",
            "chorus_flanger",
            "flanger",
            "phaser",
            "declicker",
            "dehummer",
            "denoise",
            "dereverb",
            "speech_enhance",
            "convolution_reverb",
            "studio_reverb",
            "surround_reverb",
            "binauralizer",
            "distortion",
            "fill_left",
            "fill_right",
            "guitar_suite",
            "invert_a",
            "loudness_radar",
            "mastering",
            "panner_ambisonics",
            "swap_channels",
            "vocal_enhancer",
            "stereo_expander",
            "stereo_width",
            "pitch_shifter",
            "balance_a",
            "mute",
            "volume_a",
            "channel_volume",
            "panner",
            "constant_power",
            "constant_gain",
            "exponential_fade",
        ] {
            let mut removed = original.clone();
            removed["effect"] = serde_json::json!(id);
            let error = serde_json::from_value::<EffectInstance>(removed).unwrap_err();
            assert!(error.to_string().contains("unsupported effect"), "{id}: {error}");
        }
        for def in effect_defs() {
            let instance = def.instance();
            let json = serde_json::to_string(&instance).unwrap();
            let decoded: EffectInstance = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded.effect, def.id);
            assert_eq!(decoded.params.len(), instance.params.len());
        }
    }

    #[test]
    fn removed_track_transition_data_and_insert_flags_are_rejected() {
        let track = crate::Track::new(crate::TrackId(1), crate::TrackKind::Audio, "A1".into());
        let mut json = serde_json::to_value(&track).unwrap();
        json["transitions"] = serde_json::json!([]);
        assert!(serde_json::from_value::<crate::Track>(json).is_err());
        let mut json = serde_json::to_value(find_effect("volume").unwrap().instance()).unwrap();
        json["post_fader"] = serde_json::json!(true);
        assert!(serde_json::from_value::<EffectInstance>(json).is_err());
    }
}
