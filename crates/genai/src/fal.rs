//! fal.ai: hosted image models over a synchronous HTTPS call.
//!
//! `POST https://fal.run/<endpoint>` with `Authorization: Key <key>` and a JSON body; the reply
//! lists the generated images, as `data:` URIs when `sync_mode` is set (else as HTTPS URLs, which
//! are fetched). The default endpoint is SDXL Lightning image-to-image: 4 inference steps, so it is
//! among the cheapest image-to-image models fal hosts. Any fal image-to-image endpoint that takes
//! `image_url`, `prompt` and `strength` works.

use serde_json::{Value, json};

use crate::{GenError, Picture, RestyleRequest, Restyler, Result};

pub const DEFAULT_ENDPOINT: &str = "fal-ai/fast-lightning-sdxl/image-to-image";
const BASE: &str = "https://fal.run/";

/// Check an endpoint id (`owner/model[/variant…]`): it becomes part of the request URL.
pub fn check_endpoint(endpoint: &str) -> Result<()> {
    let ok = !endpoint.is_empty()
        && endpoint.len() <= 200
        && endpoint.split('/').count() >= 2
        && endpoint.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
        && endpoint.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    if ok { Ok(()) } else { Err(GenError::BadRequest(format!("`{endpoint}` is not a fal endpoint id (like {DEFAULT_ENDPOINT})"))) }
}

/// Output size for a picture: its aspect ratio at about one megapixel (what SDXL is trained on),
/// in multiples of 8.
pub fn output_size(width: u32, height: u32) -> (u32, u32) {
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);
    let k = (1_048_576.0 / (w * h)).sqrt();
    let r8 = |v: f64| (((v / 8.0).round() as u32).clamp(32, 256)) * 8;
    (r8(w * k), r8(h * k))
}

/// The JSON body of one request.
pub fn request_body(picture: &Picture, req: &RestyleRequest) -> Value {
    let (w, h) = output_size(picture.width, picture.height);
    let mut body = json!({
        "image_url": crate::png_data_uri(&picture.png),
        "prompt": req.prompt,
        "strength": req.strength,
        "image_size": {"width": w, "height": h},
        "num_images": 1,
        "format": "png",
        "output_format": "png",
        "sync_mode": true,
        "enable_safety_checker": true,
    });
    if !req.negative_prompt.trim().is_empty() {
        body["negative_prompt"] = json!(req.negative_prompt);
    }
    if let Some(seed) = req.seed {
        body["seed"] = json!(seed);
    }
    body
}

/// Where the first generated image is: a `data:` URI or an HTTPS URL.
pub fn image_location(reply: &Value) -> Result<String> {
    if reply.get("has_nsfw_concepts").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_bool) == Some(true) {
        return Err(GenError::Provider("the safety checker blocked this frame".into()));
    }
    let url = reply
        .get("images")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|i| i.get("url"))
        .and_then(Value::as_str)
        .ok_or_else(|| GenError::BadResponse(format!("no image in the reply: {}", truncate(&reply.to_string()))))?;
    if url.starts_with("data:") || url.starts_with("https://") {
        Ok(url.to_string())
    } else {
        Err(GenError::BadResponse("the image URL is neither data: nor https:".into()))
    }
}

/// A readable message from an error reply (`{"detail": …}`).
pub fn error_message(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body).ok().and_then(|v| v.get("detail").cloned()).map(|d| match d {
        Value::String(s) => s,
        other => other.to_string(),
    });
    match status {
        401 | 403 => "fal.ai refused the API key".into(),
        402 => "the fal.ai account is out of credit".into(),
        _ => format!("HTTP {status}: {}", truncate(&detail.unwrap_or_else(|| body.to_string()))),
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(300).collect()
}

pub struct Fal {
    endpoint: String,
    key: String,
}

impl Fal {
    pub fn new(endpoint: &str, key: String) -> Result<Fal> {
        let endpoint = endpoint.trim().trim_matches('/');
        check_endpoint(endpoint)?;
        Ok(Fal { endpoint: endpoint.to_string(), key })
    }
}

impl Restyler for Fal {
    fn name(&self) -> &str {
        "fal.ai"
    }

    fn restyle(&self, picture: &Picture, req: &RestyleRequest) -> Result<Vec<u8>> {
        req.validate()?;
        if picture.png.len() > crate::MAX_IMAGE_BYTES {
            return Err(GenError::BadRequest("the frame is too large to send".into()));
        }
        let body = request_body(picture, req);
        let reply = http::post_json(&format!("{BASE}{}", self.endpoint), &self.key, &body)?;
        let loc = image_location(&reply)?;
        if loc.starts_with("data:") { crate::decode_data_uri(&loc) } else { http::get_bytes(&loc) }
    }
}

#[cfg(feature = "cloud")]
mod http {
    use std::io::Read;

    use serde_json::Value;

    use crate::{GenError, MAX_IMAGE_BYTES, Result};

    fn agent() -> ureq::Agent {
        let provider = std::sync::Arc::new(rustls_rustcrypto::provider());
        ureq::Agent::config_builder()
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::Rustls)
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .unversioned_rustls_crypto_provider(provider)
                    .build(),
            )
            .timeout_connect(Some(std::time::Duration::from_secs(30)))
            .timeout_global(Some(std::time::Duration::from_secs(180)))
            // a frame is a few megabytes: bound the upload on slow links
            .timeout_send_body(Some(std::time::Duration::from_secs(120)))
            .http_status_as_error(false)
            .build()
            .into()
    }

    fn read_capped(body: &mut ureq::Body) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        body.as_reader().take(MAX_IMAGE_BYTES as u64 + 1).read_to_end(&mut out).map_err(|e| GenError::Network(e.to_string()))?;
        if out.len() > MAX_IMAGE_BYTES {
            return Err(GenError::BadResponse("reply too large".into()));
        }
        Ok(out)
    }

    pub fn post_json(url: &str, key: &str, body: &Value) -> Result<Value> {
        let mut resp =
            agent().post(url).header("Authorization", &format!("Key {key}")).header("Content-Type", "application/json").send(body.to_string()).map_err(
                |e| {
                    let msg = e.to_string();
                    // fal answers a refused key before reading the upload and closes the connection,
                    // which the write sees as a broken pipe, a reset, or (macOS) EINVAL
                    if msg.contains("Broken pipe") || msg.contains("Connection reset") || msg.contains("os error 22") {
                        GenError::Provider("fal.ai closed the connection during the upload; check the API key and account credit".into())
                    } else {
                        GenError::Network(msg)
                    }
                },
            )?;
        let status = resp.status().as_u16();
        let bytes = read_capped(resp.body_mut())?;
        let text = String::from_utf8_lossy(&bytes);
        if !(200..300).contains(&status) {
            return Err(GenError::Provider(super::error_message(status, &text)));
        }
        serde_json::from_str(&text).map_err(|e| GenError::BadResponse(e.to_string()))
    }

    pub fn get_bytes(url: &str) -> Result<Vec<u8>> {
        let mut resp = agent().get(url).call().map_err(|e| GenError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(GenError::Provider(format!("downloading the image: HTTP {status}")));
        }
        read_capped(resp.body_mut())
    }
}

#[cfg(not(feature = "cloud"))]
mod http {
    use serde_json::Value;

    use crate::{GenError, Result};

    pub fn post_json(_: &str, _: &str, _: &Value) -> Result<Value> {
        Err(GenError::Unsupported("this build has no cloud AI (feature `cloud`)".into()))
    }
    pub fn get_bytes(_: &str) -> Result<Vec<u8>> {
        Err(GenError::Unsupported("this build has no cloud AI (feature `cloud`)".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pic() -> Picture {
        Picture { png: vec![1, 2, 3], width: 1920, height: 1080 }
    }

    #[test]
    fn body_carries_the_frame_prompt_and_seed() {
        let req = RestyleRequest { prompt: "ink".into(), negative_prompt: "blurry".into(), strength: 0.4, seed: Some(42) };
        let b = request_body(&pic(), &req);
        assert!(b["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"));
        assert_eq!(b["prompt"], "ink");
        assert_eq!(b["negative_prompt"], "blurry");
        assert_eq!(b["seed"], 42);
        assert_eq!(b["sync_mode"], true);
        let (w, h) = (b["image_size"]["width"].as_u64().unwrap(), b["image_size"]["height"].as_u64().unwrap());
        assert!(w % 8 == 0 && h % 8 == 0 && (w * h) > 900_000 && (w * h) < 1_200_000, "{w}x{h}");
        assert!((w as f64 / h as f64 - 16.0 / 9.0).abs() < 0.05);
    }

    #[test]
    fn sizes_stay_sane_for_odd_inputs() {
        for (w, h) in [(0, 0), (1, 100_000), (u32::MAX, 1), (640, 480)] {
            let (a, b) = output_size(w, h);
            assert!((256..=2048).contains(&a) && (256..=2048).contains(&b), "{w}x{h} → {a}x{b}");
        }
    }

    #[test]
    fn replies_are_parsed_and_errors_explained() {
        let ok = json!({"images": [{"url": "data:image/png;base64,AAEC"}], "has_nsfw_concepts": [false]});
        assert_eq!(image_location(&ok).unwrap(), "data:image/png;base64,AAEC");
        assert!(image_location(&json!({"images": [{"url": "https://v3.fal.media/x.png"}]})).is_ok());
        assert!(image_location(&json!({"images": []})).is_err());
        assert!(image_location(&json!({"images": [{"url": "file:///etc/passwd"}]})).is_err());
        assert!(matches!(image_location(&json!({"images": [{"url": "data:,"}], "has_nsfw_concepts": [true]})), Err(GenError::Provider(_))));
        assert_eq!(error_message(401, "{}"), "fal.ai refused the API key");
        assert!(error_message(422, r#"{"detail": "strength must be <= 1"}"#).contains("strength must be"));
    }

    /// Network: a frame-sized upload to a server that reads the whole body. Run with
    /// `cargo test -p filmcraft-genai --features cloud -- --ignored`.
    #[cfg(feature = "cloud")]
    #[test]
    #[ignore]
    fn large_uploads_go_through() {
        let body = json!({"image_url": crate::png_data_uri(&vec![7u8; 2_000_000])});
        let r = http::post_json("https://postman-echo.com/post", "unused", &body);
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn endpoint_ids_cannot_escape_the_url() {
        assert!(check_endpoint(DEFAULT_ENDPOINT).is_ok());
        for bad in ["", "fal-ai", "fal-ai/../../x", "https://evil.example/x", "fal-ai/x?y=1", "a//b"] {
            assert!(check_endpoint(bad).is_err(), "{bad}");
        }
    }
}
