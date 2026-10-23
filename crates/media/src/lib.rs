//! Media sources for FilmCraft.
//!
//! A [`MediaSource`] is anything that yields video frames and/or audio for a media time: decoded
//! files (containers + codecs), stills, image sequences and synthetic generators (bars & tone,
//! colour mattes, counting leader, procedural demo scenes). Sources are `Send + Sync`; decoder state
//! lives behind interior mutability so monitors, thumbnails, playback and export can share them.
//!
//! Time passed to a source is **media time** (0 = first frame of the media), in ticks.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod animated;
pub mod cache;
pub mod cancel;
pub mod digits;
pub mod dpx;
pub mod generators;
pub mod pending;
pub mod reader;
pub mod sequence;
pub mod still;
pub mod tga;
mod tiff;
pub mod wav;

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use waltcut_color::ColorInfo;
use waltcut_frame::{AudioBuffer, VideoFrame};
use waltcut_time::{FrameRate, Tick};

pub use cache::FrameCache;
pub use generators::{DemoScene, Generator};
pub use reader::{ByteReader, ReaderOpener, SharedReader};

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("unsupported media: {0}")]
    Unsupported(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("no {0} stream")]
    NoStream(&'static str),
    #[error("media offline: {0}")]
    Offline(String),
    /// The request was cancelled (see [`cancel`]).
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, MediaError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    /// Video, possibly with audio.
    Movie,
    AudioOnly,
    Still,
    ImageSequence,
    Synthetic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoStreamInfo {
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub par: (u32, u32),
    pub codec: String,
    pub pixel_format: String,
    pub color: ColorInfo,
    pub has_alpha: bool,
    /// Bitrate in bits/s when known.
    pub bitrate: Option<u64>,
    /// HDR static metadata (mastering display, content light level) when the file carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr: Option<waltcut_color::HdrMetadata>,
}

/// Most audio streams a source reports (the count comes from the file: never trusted).
pub const MAX_AUDIO_STREAMS: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub sample_rate: u32,
    pub channels: u32,
    pub codec: String,
    pub bits_per_sample: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub name: String,
    pub kind: MediaKind,
    /// Media duration. Stills report a default duration (the still-image default preference).
    pub duration: Tick,
    pub video: Option<VideoStreamInfo>,
    /// Every audio stream the container carries, in file order. Stream 0 is the one
    /// [`MediaSource::audio`] reads; the others are read with [`MediaSource::audio_stream`].
    /// Project files written before multi-stream support hold a single `audio` object (or
    /// `null`): both still load.
    #[serde(default, alias = "audio", deserialize_with = "deserialize_audio_streams")]
    pub audio_streams: Vec<AudioStreamInfo>,
    pub container: String,
    /// Timecode of the first frame (in frames at `video.frame_rate`), if the file carries one.
    pub start_timecode: Option<i64>,
    pub file_size: Option<u64>,
}

/// Reads `audio_streams` from a list, or from the single optional stream older project files wrote.
fn deserialize_audio_streams<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<AudioStreamInfo>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Streams {
        Many(Vec<AudioStreamInfo>),
        One(Option<AudioStreamInfo>),
    }
    Ok(match Streams::deserialize(d)? {
        Streams::Many(v) => v,
        Streams::One(o) => o.into_iter().collect(),
    })
}

impl MediaInfo {
    pub fn frame_rate(&self) -> FrameRate {
        self.video.as_ref().map(|v| v.frame_rate).unwrap_or_default()
    }
    pub fn has_video(&self) -> bool {
        self.video.is_some()
    }
    pub fn has_audio(&self) -> bool {
        !self.audio_streams.is_empty()
    }
    /// The primary audio stream (stream 0), the one [`MediaSource::audio`] reads.
    pub fn audio(&self) -> Option<&AudioStreamInfo> {
        self.audio_streams.first()
    }
}

/// A request for a video frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRequest {
    /// Media time.
    pub time: Tick,
    /// Desired scale relative to full resolution (1.0 = full). Sources may return a larger frame.
    pub scale: f32,
}

impl FrameRequest {
    pub fn full(time: Tick) -> Self {
        Self { time, scale: 1.0 }
    }
}

pub trait MediaSource: Send + Sync {
    fn info(&self) -> &MediaInfo;
    /// User-visible import notices, such as a retained picture track whose audio codec is unsupported.
    fn notices(&self) -> &[String] {
        &[]
    }
    /// The frame displayed at media time `req.time`.
    fn video_frame(&self, req: FrameRequest) -> Result<Arc<VideoFrame>>;
    /// `frames` audio frames starting at sample index `start` (at `sample_rate`, media time).
    fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer>;
    /// Like [`audio`](Self::audio) for audio stream `stream` (an index into
    /// [`MediaInfo::audio_streams`]). Sources with a single audio stream need not override this:
    /// stream 0 is `audio`, any other index is [`MediaError::NoStream`].
    fn audio_stream(&self, stream: usize, start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer> {
        if stream == 0 { self.audio(start, frames, sample_rate) } else { Err(MediaError::NoStream("audio")) }
    }
}

pub type SharedSource = Arc<dyn MediaSource>;

/// File extensions we recognise at import (the media browser filters on these).
pub const STILL_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "tif", "tiff", "bmp", "tga", "dpx", "exr", "hdr"];
pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "wave", "bwf", "aif", "aiff", "mp3", "mp2", "m4a", "aac", "opus"];
pub const VIDEO_EXTENSIONS: &[&str] =
    &["mp4", "m4v", "mov", "mkv", "webm", "avi", "mts", "m2ts", "m2t", "ts", "mpg", "mpeg", "vob", "mod", "tod", "m2v", "m1v", "mpv", "3gp", "y4m"];

/// Actionable errors for formats deliberately outside the codec/container allowlist.
pub fn unsupported_format_extension(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mxf" => "MXF is not supported; export or convert to MP4/MOV, Matroska/WebM, AVI, or MPEG-TS/PS.",
        "ogg" | "oga" | "ogv" | "ogx" => "Ogg is not supported; convert audio to AAC, MP2, or PCM, or video to a supported container.",
        "flac" => "FLAC is not supported; convert audio to AAC, MP2, or PCM.",
        "dng" => "DNG still images are not supported; export the image as TIFF, PNG, or JPEG.",
        "dnx" => "DNx video is not supported; convert it to H.264, HEVC, ProRes, VP9, or AV1.",
        "apv" => "APV video is not supported; convert it to H.264, HEVC, ProRes, VP9, or AV1.",
        "vp8" => "VP8 video is not supported; convert it to VP9, H.264, HEVC, ProRes, or AV1.",
        "ac3" | "eac3" => "AC-3/E-AC-3 audio is not supported; convert it to AAC, MP2, or PCM.",
        _ => return None,
    })
}

pub fn is_importable(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    STILL_EXTENSIONS.contains(&ext.as_str()) || AUDIO_EXTENSIONS.contains(&ext.as_str()) || VIDEO_EXTENSIONS.contains(&ext.as_str())
}

/// A pluggable opener (containers/codecs register themselves here from higher-level crates).
pub type Opener = fn(name: &str, bytes: Arc<[u8]>) -> Option<Result<SharedSource>>;

/// Open media from bytes, trying `extra` openers first, then the built-in ones (WAV, animated
/// images, stills).
pub fn open_bytes(name: &str, bytes: Arc<[u8]>, extra: &[Opener]) -> Result<SharedSource> {
    if let Some(message) = unsupported_format_extension(name) {
        return Err(MediaError::Unsupported(format!("{name}: {message}")));
    }
    for o in extra {
        if let Some(r) = o(name, bytes.clone()) {
            return r;
        }
    }
    if still::exr::sniff(&bytes) {
        return Ok(Arc::new(still::exr::still(name, &bytes)?));
    }
    if still::hdr::sniff(&bytes) {
        return Ok(Arc::new(still::hdr::still(name, &bytes)?));
    }
    if wav::sniff(&bytes) {
        return Ok(Arc::new(wav::WavSource::parse(name, bytes)?));
    }
    // APNG and animated GIF play as movies (#402); single-frame files fall through to the stills
    if let Some(r) = animated::open(name, &bytes) {
        return Ok(Arc::new(r?));
    }
    if still::sniff(name, &bytes) {
        return Ok(Arc::new(still::StillSource::decode(name, &bytes)?));
    }
    Err(MediaError::Unsupported(format!("{name}: unrecognised format")))
}

/// Open a file from disk (native only).
pub fn open_path(path: &Path, extra: &[Opener]) -> Result<SharedSource> {
    let bytes = std::fs::read(path).map_err(|e| MediaError::Io(format!("{}: {e}", path.display())))?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    open_bytes(&name, bytes.into(), extra)
}

#[cfg(test)]
mod unsupported_format_tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[test]
    fn removed_formats_reject_empty_and_truncated_inputs_actionably() {
        for (ext, token) in [
            ("mxf", "mxf"),
            ("ogg", "ogg"),
            ("oga", "ogg"),
            ("ogv", "ogg"),
            ("ogx", "ogg"),
            ("dnx", "dnx"),
            ("apv", "apv"),
            ("vp8", "vp8"),
            ("dng", "dng"),
            ("ac3", "ac-3"),
            ("eac3", "e-ac-3"),
            ("flac", "flac"),
        ] {
            for bytes in [Vec::new(), vec![0], vec![0, 1, 2, 3]] {
                let name = format!("hostile.{ext}");
                let result = catch_unwind(AssertUnwindSafe(|| open_bytes(&name, bytes.into(), &[])));
                let Ok(result) = result else {
                    panic!("{name}: opening hostile input panicked");
                };
                let Err(MediaError::Unsupported(message)) = result else {
                    panic!("{name}: expected an actionable unsupported-format error");
                };
                let lower = message.to_ascii_lowercase();
                assert!(lower.contains(token), "{name}: {message}");
                assert!(lower.contains("convert") || lower.contains("export"), "{name}: missing conversion guidance: {message}");
            }
        }
    }
}

/// A source that renders nothing (used for offline media): the "Media Offline" slate.
pub struct OfflineSource {
    pub info: MediaInfo,
}

impl MediaSource for OfflineSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }
    fn video_frame(&self, req: FrameRequest) -> Result<Arc<VideoFrame>> {
        let v = self.info.video.as_ref().ok_or(MediaError::NoStream("video"))?;
        let s = req.scale.clamp(0.02, 1.0);
        let (w, h) = (((v.width as f32 * s) as u32).max(2), ((v.height as f32 * s) as u32).max(2));
        let mut px = vec![0u8; (w * h * 4) as usize];
        for p in px.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&[140, 16, 16, 255]);
        }
        Ok(Arc::new(VideoFrame::rgba8(w, h, px).with_pts(req.time)))
    }
    fn audio(&self, _start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer> {
        Ok(AudioBuffer::silence(sample_rate, self.info.audio().map_or(2, |a| a.channels as usize), frames))
    }
}

#[cfg(test)]
mod importable_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn camera_extensions_match_regardless_of_case() {
        for name in ["clip.mov", "clip.MOV", "clip.Mov", "shot.MP4", "IMG_0001.JPG", "sound.WAV", "take.M2TS"] {
            assert!(is_importable(Path::new(name)), "{name}");
        }
        assert!(!is_importable(Path::new("notes.txt")));
    }
}
