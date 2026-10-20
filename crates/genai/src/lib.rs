//! Generative image AI: re-style a frame from a text prompt.
//!
//! A [`Restyler`] takes one picture (PNG) and a [`RestyleRequest`] and returns the new picture as
//! encoded image bytes (PNG or JPEG). Backends are interchangeable and chosen by
//! [`ProviderSettings`]:
//!
//! | Provider | Where it runs | State |
//! |---|---|---|
//! | [`Provider::Fal`] | fal.ai cloud, pay per image (feature `cloud`) | implemented |
//! | [`Provider::LocalStableDiffusion`] | this machine | planned; reports [`GenError::Unsupported`] |
//!
//! Nothing here knows about projects or timelines: the engine renders the frame, hands the PNG
//! over, and places what comes back.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod fal;

use serde::{Deserialize, Serialize};

/// The largest picture sent to or accepted from a backend (bytes).
pub const MAX_IMAGE_BYTES: usize = 40 << 20;

#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error("{0}")]
    Unsupported(String),
    #[error("AI provider is not set up: {0}")]
    NotConfigured(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("the AI provider answered with an error: {0}")]
    Provider(String),
    #[error("unexpected answer from the AI provider: {0}")]
    BadResponse(String),
}

pub type Result<T> = std::result::Result<T, GenError>;

/// One re-style: what to make of the picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RestyleRequest {
    pub prompt: String,
    #[serde(default)]
    pub negative_prompt: String,
    /// How far from the original the result may go, 0..1 (low keeps the composition, high
    /// repaints it).
    pub strength: f32,
    /// Fixed seed: the same seed on neighbouring frames keeps the look more consistent.
    #[serde(default)]
    pub seed: Option<u64>,
}

impl RestyleRequest {
    /// Refuse requests no backend can serve.
    pub fn validate(&self) -> Result<()> {
        if self.prompt.trim().is_empty() {
            return Err(GenError::BadRequest("describe the style in `prompt`".into()));
        }
        if self.prompt.len() > 4000 || self.negative_prompt.len() > 4000 {
            return Err(GenError::BadRequest("prompts are limited to 4000 characters".into()));
        }
        if !self.strength.is_finite() || !(0.0..=1.0).contains(&self.strength) {
            return Err(GenError::BadRequest("`strength` is between 0 and 1".into()));
        }
        Ok(())
    }
}

/// A picture to re-style: PNG bytes and their size in pixels.
#[derive(Clone, Debug)]
pub struct Picture {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A backend that re-styles pictures. Called from background jobs.
pub trait Restyler: Send + Sync {
    /// Short name for job labels and errors.
    fn name(&self) -> &str;
    /// The re-styled picture as encoded image bytes (PNG or JPEG).
    fn restyle(&self, picture: &Picture, req: &RestyleRequest) -> Result<Vec<u8>>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    /// fal.ai (cloud).
    #[default]
    Fal,
    /// Stable Diffusion on this machine (planned).
    LocalStableDiffusion,
}

impl Provider {
    pub fn from_name(s: &str) -> Option<Provider> {
        match s.trim().to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
            "fal" | "falai" => Some(Provider::Fal),
            "local" | "localstablediffusion" | "stablediffusion" | "sd" => Some(Provider::LocalStableDiffusion),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Provider::Fal => "fal.ai (cloud)",
            Provider::LocalStableDiffusion => "Stable Diffusion (local)",
        }
    }
}

/// Which backend to use and how to reach it (saved with the user preferences).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderSettings {
    pub provider: Provider,
    /// fal.ai model endpoint id.
    pub fal_endpoint: String,
    /// fal.ai API key. Empty: the `FAL_KEY` environment variable. Never serialized: the engine
    /// keeps it in its own owner-only file, out of the preferences agents can read.
    #[serde(skip)]
    pub fal_key: String,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self { provider: Provider::Fal, fal_endpoint: fal::DEFAULT_ENDPOINT.into(), fal_key: String::new() }
    }
}

impl ProviderSettings {
    /// The fal.ai key: the saved one, else `FAL_KEY` from the environment.
    pub fn fal_key(&self) -> Option<String> {
        let k = self.fal_key.trim();
        if !k.is_empty() {
            return Some(k.to_string());
        }
        std::env::var("FAL_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
    }

    /// The backend these settings describe.
    pub fn restyler(&self) -> Result<Box<dyn Restyler>> {
        match self.provider {
            Provider::Fal => {
                let key = self.fal_key().ok_or_else(|| GenError::NotConfigured("add a fal.ai API key (or set FAL_KEY)".into()))?;
                Ok(Box::new(fal::Fal::new(&self.fal_endpoint, key)?))
            }
            Provider::LocalStableDiffusion => Err(GenError::Unsupported("local Stable Diffusion is not available yet; use fal.ai for now".into())),
        }
    }
}

/// `data:` URI of PNG bytes.
pub fn png_data_uri(png: &[u8]) -> String {
    use base64::Engine as _;
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
}

/// Bytes of a base64 `data:` URI.
pub fn decode_data_uri(uri: &str) -> Result<Vec<u8>> {
    use base64::Engine as _;
    let (head, data) = uri.split_once(',').ok_or_else(|| GenError::BadResponse("malformed data URI".into()))?;
    if !head.starts_with("data:") || !head.ends_with(";base64") {
        return Err(GenError::BadResponse("only base64 data URIs are supported".into()));
    }
    if data.len() > MAX_IMAGE_BYTES / 3 * 4 + 4 {
        return Err(GenError::BadResponse("image too large".into()));
    }
    base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|e| GenError::BadResponse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> RestyleRequest {
        RestyleRequest { prompt: "watercolor".into(), negative_prompt: String::new(), strength: 0.5, seed: Some(7) }
    }

    #[test]
    fn validation_refuses_hostile_requests() {
        assert!(req().validate().is_ok());
        for bad in [
            RestyleRequest { prompt: "  ".into(), ..req() },
            RestyleRequest { strength: f32::NAN, ..req() },
            RestyleRequest { strength: 1.5, ..req() },
            RestyleRequest { prompt: "x".repeat(5000), ..req() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn data_uris_round_trip() {
        let bytes = vec![0u8, 1, 2, 250, 255];
        let uri = png_data_uri(&bytes);
        assert!(uri.starts_with("data:image/png;base64,"));
        assert_eq!(decode_data_uri(&uri).unwrap(), bytes);
        assert!(decode_data_uri("data:image/png,raw").is_err());
        assert!(decode_data_uri("nonsense").is_err());
    }

    #[test]
    fn local_sd_is_planned_and_fal_needs_a_key() {
        let s = ProviderSettings { provider: Provider::LocalStableDiffusion, ..Default::default() };
        assert!(matches!(s.restyler(), Err(GenError::Unsupported(_))));
        assert_eq!(Provider::from_name("Local Stable-Diffusion"), Some(Provider::LocalStableDiffusion));
        assert_eq!(Provider::from_name("fal.ai").or(Provider::from_name("fal")), Some(Provider::Fal));
        let s = ProviderSettings { fal_key: "  secret ".into(), ..Default::default() };
        assert_eq!(s.fal_key().as_deref(), Some("secret"));
    }
}
