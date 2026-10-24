//! Export: render a sequence range and encode it to a file.
//!
//! Frames are rendered in parallel batches (one frame per core, each frame itself row-parallel),
//! then encoded and muxed in order; audio is mixed per batch and interleaved. Progress and cancel
//! are shared atomics so the UI (Export mode, header progress) and MCP can observe/cancel jobs.
//!
//! Retained outputs: H.264 + AAC MP4, ProRes 4444 MOV with straight alpha and stereo PCM,
//! PNG sequences with alpha, and PNG/JPEG still frames. Output geometry and timing follow the
//! sequence; there are no delivery presets, overlays or encoder choices.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

use std::io::Write;

mod audio_out;
mod job;
mod pace;
mod pcm;
mod pipeline;
pub mod settings;
pub mod still;
pub use job::{Exporter, Step, stepped};
pub use pcm::image_sequence_path;
pub use settings::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use waltcut_isobmff::SampleEntry;
use waltcut_project::{ItemId, Project};
use waltcut_render::SourceProvider;
use waltcut_time::{FrameRate, Tick, TimeRange};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("no such sequence")]
    NoSequence,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("I/O: {0}")]
    Io(String),
    #[error("encode: {0}")]
    Encode(String),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, ExportError>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Format {
    #[default]
    #[serde(rename = "h264")]
    H264,
    #[serde(rename = "prores")]
    ProRes,
    #[serde(rename = "png")]
    PngSequence,
}

impl Format {
    pub const ALL: [Self; 3] = [Self::H264, Self::ProRes, Self::PngSequence];

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "h264" | "mp4" => Some(Self::H264),
            "prores" | "prores4444" | "mov" => Some(Self::ProRes),
            "png" => Some(Self::PngSequence),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::ProRes => "prores",
            Self::PngSequence => "png",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::H264 => "mp4",
            Self::ProRes => "mov",
            Self::PngSequence => "png",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264 + AAC MP4",
            Self::ProRes => "ProRes 4444 MOV (alpha)",
            Self::PngSequence => "PNG sequence (alpha)",
        }
    }

    pub fn is_image_sequence(self) -> bool {
        self == Self::PngSequence
    }
}

/// Destination, retained format and exact timeline range. Other fields belong to internal
/// render-preview jobs or the registered compositor, never to command parameters.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportSettings {
    pub format: Format,
    pub path: String,
    pub range: Option<TimeRange>,
    #[serde(skip)]
    pub scale: f32,
    #[serde(skip)]
    pub include_audio: bool,
    #[serde(skip)]
    pub part_of_batch: bool,
    #[serde(skip)]
    pub sdr: bool,
    #[serde(skip)]
    pub gpu_rendering: GpuRendering,
    #[serde(skip)]
    pub signal: ColorSignal,
    /// Draw guide clips. Off: an export never carries a guide unless asked to.
    #[serde(skip)]
    pub include_guides: bool,
}

/// Buffered disk output. User exports write inside the engine's owned staging directory.
struct Out(std::io::BufWriter<std::fs::File>);

impl Out {
    fn create(settings: &ExportSettings) -> Result<Self> {
        let file = std::fs::File::create(&settings.path).map_err(|e| ExportError::Io(format!("{}: {e}", settings.path)))?;
        Ok(Self(std::io::BufWriter::new(file)))
    }

    fn finish(mut self) -> Result<u64> {
        self.0.flush().map_err(|e| ExportError::Io(e.to_string()))?;
        self.0.get_ref().sync_all().map_err(|e| ExportError::Io(e.to_string()))?;
        self.0.get_ref().metadata().map(|m| m.len()).map_err(|e| ExportError::Io(e.to_string()))
    }
}

fn write_output(path: &str, data: Vec<u8>) -> Result<u64> {
    let n = data.len() as u64;
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(&data)?;
        file.sync_all()
    };
    write().map_err(|e| ExportError::Io(format!("{path}: {e}")))?;
    Ok(n)
}

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl std::io::Seek for Out {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

/// Colour description of the encoded stream (ITU-T H.273 code points).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorSignal {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
}

impl Default for ColorSignal {
    fn default() -> Self {
        ColorSignal { primaries: 1, transfer: 1, matrix: 1 }
    }
}

impl ColorSignal {
    pub const PQ: ColorSignal = ColorSignal { primaries: 9, transfer: 16, matrix: 9 };
    pub const HLG: ColorSignal = ColorSignal { primaries: 9, transfer: 18, matrix: 9 };
    pub fn is_hdr(&self) -> bool {
        matches!(self.transfer, 16 | 18)
    }
    /// (Kr, Kb) of the matrix.
    pub fn kr_kb(&self) -> (f32, f32) {
        if self.matrix == 9 { (0.2627, 0.0593) } else { (0.2126, 0.0722) }
    }
    /// Sample-entry boxes: `colr` (nclx for MP4, nclc for MOV) plus `mdcv`/`clli` for PQ
    /// (mastering display BT.2020/D65, 1000/0.0001 cd/m²; MaxCLL/MaxFALL 0 = unknown). SDR gets
    /// its `colr` too: Apple's media stack (QuickTime Player, Final Cut Pro) reads the colour of
    /// ProRes only from this box, not from its frame headers (#2142).
    pub fn apply_to(&self, e: &mut SampleEntry, mov: bool) {
        if let Some(v) = e.video.as_mut() {
            let (p, t, m) = (self.primaries as u16, self.transfer as u16, self.matrix as u16);
            v.color = Some(if mov {
                waltcut_isobmff::ColorInfo::Nclc { primaries: p, transfer: t, matrix: m }
            } else {
                waltcut_isobmff::ColorInfo::Nclx { primaries: p, transfer: t, matrix: m, full_range: false }
            });
            if let Some((md, cll)) = self.static_metadata() {
                v.mastering_display = Some(md);
                v.content_light = Some(cll);
            }
        }
    }
    /// HDR10 static metadata of a PQ signal: the mastering display (BT.2020/D65, 1000 / 0.0001 cd/m²)
    /// and (MaxCLL, MaxFALL) = (0, 0), unknown. `None` for HLG and SDR. The one definition of these
    /// values: the `mdcv` / `clli` boxes and the SEI messages of every encoder come from here.
    pub fn static_metadata(&self) -> Option<(waltcut_isobmff::MasteringDisplay, (u16, u16))> {
        (self.transfer == 16).then(|| (waltcut_isobmff::MasteringDisplay::bt2020(1000.0, 0.0001), (0, 0)))
    }
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: Format::H264,
            path: String::new(),
            range: None,
            scale: 1.0,
            include_audio: true,
            part_of_batch: false,
            sdr: false,
            gpu_rendering: GpuRendering::Off,
            signal: ColorSignal::default(),
            include_guides: false,
        }
    }
}

impl ExportSettings {
    pub fn validate(&self) -> Result<()> {
        if let Some(range) = self.range {
            validate_range(range)?;
        }
        if !self.scale.is_finite() || !(0.01..=4.0).contains(&self.scale) {
            return Err(ExportError::Unsupported("render scale must be finite and between 0.01 and 4".into()));
        }
        Ok(())
    }
}

/// Bound working images before any compositor or encoder allocation (up to 4K square).
pub fn validate_output_size(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > 8192 || height > 8192 || u64::from(width) * u64::from(height) > 16_777_216 {
        return Err(ExportError::Unsupported("export dimensions must be positive, at most 8192 per side and 16777216 pixels total".into()));
    }
    Ok(())
}

/// Shared progress/cancel state of an export job.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    pub finished: AtomicBool,
    pub status: Mutex<String>,
    pub error: Mutex<Option<String>>,
    /// Readings of `done` over time, for [`Progress::eta`].
    pace: Mutex<pace::Pace>,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Ordering::Relaxed).max(1);
        self.done.load(Ordering::Relaxed) as f32 / t as f32
    }

    /// The estimated time left, from the job's speed over the last 15 seconds. Each call is also a
    /// reading of that speed, so ask regularly (the UI does on every frame it draws the job).
    /// `None` until a second of readings shows progress, and once the job is done or finished.
    pub fn eta(&self) -> Option<std::time::Duration> {
        self.eta_at(std::time::Instant::now())
    }

    /// [`Progress::eta`] at a given time (tests, and hosts with a clock of their own).
    pub fn eta_at(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        let (done, total) = (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed));
        let mut pace = self.pace.lock().unwrap_or_else(|e| e.into_inner());
        pace.observe(now, done);
        if self.finished.load(Ordering::Relaxed) || done >= total {
            return None;
        }
        pace.eta(total - done)
    }
    fn set_status(&self, s: impl Into<String>) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = s.into();
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub path: String,
    pub frames: u64,
    pub seconds: f64,
    pub bytes: u64,
    pub render_fps: f64,
}

/// A packet produced by a video encoder.
struct EncodedPacket {
    pub data: Vec<u8>,
    pub key: bool,
    /// Duration in encoder timescale units.
    pub duration: u32,
    /// pts − dts in encoder timescale units.
    pub composition_offset: i32,
}

/// Input picture for encoders: straight sRGB RGBA8 (encoders convert to their own YUV), or for
/// HDR exports the encoded R'G'B' (PQ/HLG, BT.2020; 3 floats per pixel, 0..1) in `hdr`.
struct EncoderFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
    pub hdr: Option<&'a [f32]>,
    pub index: u64,
}

fn validate_encoder_geometry(width: u32, height: u32, rate: FrameRate) -> Result<()> {
    validate_output_size(width, height)?;
    waltcut_project::SequenceSettings { width, height, frame_rate: rate, ..Default::default() }.validate().map_err(ExportError::Unsupported)
}

fn validate_encoder_frame(frame: &EncoderFrame, width: u32, height: u32, alpha: bool) -> Result<()> {
    validate_output_size(frame.width, frame.height)?;
    if (frame.width, frame.height) != (width, height) || frame.index > 1_000_000 {
        return Err(ExportError::Unsupported("encoder frame dimensions or index do not match the export".into()));
    }
    let pixels = usize::try_from(u64::from(width) * u64::from(height)).map_err(|_| ExportError::Unsupported("encoder frame is too large".into()))?;
    if (alpha || frame.hdr.is_none()) && pixels.checked_mul(4) != Some(frame.rgba.len()) {
        return Err(ExportError::Unsupported("encoder frame RGBA buffer length does not match its dimensions".into()));
    }
    if let Some(rgb) = frame.hdr
        && (pixels.checked_mul(3) != Some(rgb.len()) || rgb.iter().any(|v| !v.is_finite()))
    {
        return Err(ExportError::Unsupported("encoder frame HDR buffer is malformed or non-finite".into()));
    }
    Ok(())
}

trait VideoEncoder: Send {
    /// MP4/MOV sample entry (codec config) — may only be complete after the first frame.
    fn sample_entry(&self) -> SampleEntry;
    fn timescale(&self) -> u32;
    fn encode(&mut self, frame: &EncoderFrame) -> Result<Vec<EncodedPacket>>;
    fn flush(&mut self) -> Result<Vec<EncodedPacket>>;
    /// Media start offset for an edit list (B-frame delay), in the encoder timescale.
    fn media_start(&self) -> Option<i64> {
        None
    }
}

/// Fixed built-in AAC adapter; MOV PCM is written directly.
trait AudioEncoder: Send {
    fn sample_entry(&self) -> SampleEntry;
    fn encode(&mut self, planar: &[Vec<f32>]) -> Result<Vec<Vec<u8>>>;
    fn flush(&mut self) -> Result<Vec<Vec<u8>>>;
    /// Encoder delay (priming) in samples.
    fn priming(&self) -> u32;
    /// Samples per access unit (1024 for AAC).
    fn frame_size(&self) -> u32;
}

/// The stages of an export whose wall time [`stage_times`] adds up (`perf.stats` `export.stages`). The
/// render of one batch overlaps the encoding of the previous one (`Exporter::overlapping`), so
/// [`Stage::Render`] and [`Stage::Encode`] with its neighbours can sum to more than the export took;
/// [`Stage::Wait`] is the time left waiting for the render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// [`Exporter::new`]: settings, pipeline, encoder and audio set-up.
    Setup,
    /// Rendering a batch of frames (parallel; includes decoding the sources, effects, colour).
    Render,
    /// Encoding the rendered frames, colour conversion included (serial).
    Encode,
    /// The part of [`Stage::Encode`] spent converting RGB to the encoder's YUV.
    Convert,
    /// Mixing and encoding audio and writing it to the container.
    Audio,
    /// Writing video samples to the container (and creating it).
    Mux,
    /// Flushing the encoders and finishing the file.
    Finish,
    /// Time the encoding side waited, after encoding a batch, for the next batch to finish rendering
    /// (only with overlap: large means the export is render-bound, near zero encode-bound).
    Wait,
}

const STAGES: usize = 8;

impl Stage {
    pub const ALL: [Stage; STAGES] = [Stage::Setup, Stage::Render, Stage::Encode, Stage::Convert, Stage::Audio, Stage::Mux, Stage::Finish, Stage::Wait];

    /// The name in `perf.stats`.
    pub fn name(self) -> &'static str {
        match self {
            Stage::Setup => "setup",
            Stage::Render => "render",
            Stage::Encode => "encode",
            Stage::Convert => "convert",
            Stage::Audio => "audio",
            Stage::Mux => "mux",
            Stage::Finish => "finish",
            Stage::Wait => "wait",
        }
    }
}

static STAGE_NS: [std::sync::atomic::AtomicU64; STAGES] = [const { std::sync::atomic::AtomicU64::new(0) }; STAGES];

/// Wall time each export stage has taken so far in this process, in nanoseconds, summed over every
/// stepped export (they only grow: diff two readings to measure one export). Measuring costs two
/// clock reads per stage per batch; nothing an export does depends on it.
pub fn stage_times() -> Vec<(Stage, u64)> {
    Stage::ALL.iter().map(|s| (*s, STAGE_NS.get(*s as usize).map_or(0, |a| a.load(std::sync::atomic::Ordering::Relaxed)))).collect()
}

/// Add `d` to a stage's wall time.
fn note_stage(stage: Stage, d: std::time::Duration) {
    if let Some(a) = STAGE_NS.get(stage as usize) {
        a.fetch_add(u64::try_from(d.as_nanos()).unwrap_or(u64::MAX), std::sync::atomic::Ordering::Relaxed);
    }
}

/// Run `f`, adding its wall time to `stage`.
pub fn timed<T>(stage: Stage, f: impl FnOnce() -> T) -> T {
    let t = std::time::Instant::now();
    let r = f();
    note_stage(stage, t.elapsed());
    r
}

/// Renders one export frame on something other than the CPU reference renderer — today the GPU
/// compositor (`waltcut-gpu`), registered by the app or CLI at startup. `render` returns the
/// frame exactly as [`waltcut_render::render_sequence`] would (same size, premultiplied
/// linear-light RGBA), or `None` to let the CPU render this frame (no adapter, a plan the GPU
/// cannot draw, any internal error): the CPU renderer is the reference and the fallback.
pub trait FrameRenderer: Send {
    fn render(
        &mut self,
        project: &Project,
        seq: ItemId,
        t: Tick,
        opts: waltcut_render::RenderOptions,
        sources: &dyn SourceProvider,
    ) -> Option<waltcut_render::Image>;
}

/// Builds the frame renderer for one export, or `None` when this host cannot (no adapter).
pub type FrameRendererFactory = fn() -> Option<Box<dyn FrameRenderer>>;

fn frame_renderer_factories() -> &'static RwLock<Vec<FrameRendererFactory>> {
    static F: OnceLock<RwLock<Vec<FrameRendererFactory>>> = OnceLock::new();
    F.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register a frame-renderer factory (tried before those registered earlier). Registering the
/// same factory twice is harmless.
pub fn register_frame_renderer(f: FrameRendererFactory) {
    let mut g = frame_renderer_factories().write().unwrap_or_else(|e| e.into_inner());
    if !g.iter().any(|x| std::ptr::fn_addr_eq(*x, f)) {
        g.insert(0, f);
    }
}

/// Whether `f` is among the registered frame-renderer factories (startup diagnostics, tests).
pub fn frame_renderer_registered(f: FrameRendererFactory) -> bool {
    frame_renderer_factories().read().unwrap_or_else(|e| e.into_inner()).iter().any(|x| std::ptr::fn_addr_eq(*x, f))
}

/// Clear the registered frame-renderer factories. Test-only: registration is a process-wide
/// startup step and tests that register their own must not see each other's.
#[cfg(test)]
pub(crate) fn reset_frame_renderers_for_tests() {
    frame_renderer_factories().write().unwrap_or_else(|e| e.into_inner()).clear();
    GPU_FRAMES.store(0, Ordering::Relaxed);
    GPU_FALLBACKS.store(0, Ordering::Relaxed);
}

/// Pick the first registered factory that can build a renderer for this host.
pub(crate) fn build_frame_renderer() -> Option<Box<dyn FrameRenderer>> {
    let g = frame_renderer_factories().read().unwrap_or_else(|e| e.into_inner());
    g.iter().filter_map(|f| f()).next()
}

static GPU_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static GPU_FALLBACKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LOCK_WAIT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A frame was composited by the registered GPU renderer.
pub fn note_gpu_frame() {
    GPU_FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// The GPU renderer declined a frame (the CPU reference renderer took it).
pub fn note_gpu_fallback() {
    GPU_FALLBACKS.fetch_add(1, Ordering::Relaxed);
}

/// Time an export worker waited for a free GPU renderer (process-wide, for the bench).
pub(crate) fn note_lock_wait(d: std::time::Duration) {
    LOCK_WAIT_NS.fetch_add(u64::try_from(d.as_nanos()).unwrap_or(u64::MAX), Ordering::Relaxed);
}

/// Total time export workers waited for a GPU renderer since [`reset_export_lock_wait`].
pub fn export_lock_wait() -> std::time::Duration {
    std::time::Duration::from_nanos(LOCK_WAIT_NS.load(Ordering::Relaxed))
}

/// Start [`export_lock_wait`] again from zero.
pub fn reset_export_lock_wait() {
    LOCK_WAIT_NS.store(0, Ordering::Relaxed);
}

/// GPU-vs-CPU frame counters of the export renderer (per-process, like the hardware ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuRenderStats {
    pub frames: u64,
    pub fallbacks: u64,
}

pub fn gpu_render_stats() -> GpuRenderStats {
    GpuRenderStats { frames: GPU_FRAMES.load(Ordering::Relaxed), fallbacks: GPU_FALLBACKS.load(Ordering::Relaxed) }
}
/// AAC-LC (our encoder), 320 kbps stereo by default.
struct AacEncoder {
    enc: waltcut_aac::Encoder,
    rate: u32,
    channels: u32,
}

impl AudioEncoder for AacEncoder {
    fn sample_entry(&self) -> SampleEntry {
        SampleEntry::aac(self.enc.audio_specific_config(), self.channels, self.rate)
    }
    fn encode(&mut self, planar: &[Vec<f32>]) -> Result<Vec<Vec<u8>>> {
        let refs: Vec<&[f32]> = planar.iter().map(Vec::as_slice).collect();
        Ok(self.enc.encode(&refs))
    }
    fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        Ok(self.enc.flush())
    }
    fn priming(&self) -> u32 {
        self.enc.priming_samples()
    }
    fn frame_size(&self) -> u32 {
        1024
    }
}

fn aac_factory(sample_rate: u32) -> Result<Box<dyn AudioEncoder>> {
    if !(1..=384_000).contains(&sample_rate) {
        return Err(ExportError::Unsupported("AAC sample rate must be between 1 and 384000 Hz".into()));
    }
    let channels = 2;
    // AAC caps a frame at 6144 bits per channel (ISO/IEC 14496-3 §4.5.3.2): 6 bits per sample, so
    // 264.6 kbps for stereo at 22.05 kHz; a higher setting is capped instead of refused
    let max = (6 * sample_rate as u64 * channels.max(1) as u64).min(u32::MAX as u64) as u32;
    let bps = 320_000.min(max);
    waltcut_aac::Encoder::new(waltcut_aac::EncoderConfig::cbr(sample_rate, channels as usize, bps))
        .map(|enc| Box::new(AacEncoder { enc, rate: sample_rate, channels }) as Box<dyn AudioEncoder>)
        .map_err(|e| ExportError::Encode(e.to_string()))
}

/// ProRes 4444 encoder: straight alpha plus 10-bit limited-range picture.
struct ProResEncoder {
    enc: waltcut_prores::Encoder,
    w: u32,
    h: u32,
    rate: FrameRate,
    signal: ColorSignal,
}

impl VideoEncoder for ProResEncoder {
    fn sample_entry(&self) -> SampleEntry {
        let mut e = SampleEntry::prores(waltcut_isobmff::FourCc(*b"ap4h"), self.w as u16, self.h as u16);
        self.signal.apply_to(&mut e, true);
        e
    }
    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }
    fn encode(&mut self, f: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        validate_encoder_frame(f, self.w, self.h, true)?;
        let mut fr = waltcut_prores::Frame::new(f.width, f.height, waltcut_prores::Profile::P4444.chroma(), 10, true);
        let (w, h) = (f.width as usize, f.height as usize);
        timed(Stage::Convert, || {
            let (kr, kb) = if f.hdr.is_some() { self.signal.kr_kb() } else { (0.2126, 0.0722) };
            to_yuv444_10(f.rgba, f.hdr, w, h, kr, kb, &mut fr.y, &mut fr.cb, &mut fr.cr, fr.alpha.as_deref_mut())
        });
        let data = self.enc.encode(&fr).map_err(|e| ExportError::Encode(e.to_string()))?;
        Ok(vec![EncodedPacket { data, key: true, duration: self.rate.den as u32, composition_offset: 0 }])
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        Ok(Vec::new())
    }
}

/// Limited-range 10-bit 4:4:4 with matrix (Kr, Kb) from straight RGBA8, or from encoded R'G'B'
/// floats (3 per pixel) when `hdr` is given. `alpha` (full range, 0 = transparent) takes the
/// RGBA8 alpha, including for HDR floats, which carry only colour.
#[allow(clippy::too_many_arguments)]
fn to_yuv444_10(
    rgba: &[u8],
    hdr: Option<&[f32]>,
    w: usize,
    h: usize,
    kr: f32,
    kb: f32,
    y: &mut [u16],
    cb: &mut [u16],
    cr: &mut [u16],
    alpha: Option<&mut [u16]>,
) {
    let (w, n) = (w.max(1), w.saturating_mul(h));
    let kg = 1.0 - kr - kb;
    let (sb, sr) = (2.0 * (1.0 - kb), 2.0 * (1.0 - kr));
    let rgb_at = |i: usize| -> [f32; 3] {
        match hdr {
            Some(f) => f.get(i * 3..i * 3 + 3).map_or([0.0; 3], |p| [p[0], p[1], p[2]]),
            None => rgba.get(i * 4..i * 4 + 3).map_or([0.0; 3], |p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]),
        }
    };
    y.par_chunks_mut(w).zip(cb.par_chunks_mut(w).zip(cr.par_chunks_mut(w))).enumerate().for_each(|(row, (yr, (cbr, crr)))| {
        for (x, (yv, (cbv, crv))) in yr.iter_mut().zip(cbr.iter_mut().zip(crr.iter_mut())).enumerate() {
            let [r, g, b] = rgb_at(row * w + x);
            let yy = kr * r + kg * g + kb * b;
            *yv = (64.0 + 876.0 * yy).round().clamp(4.0, 1019.0) as u16;
            *cbv = (512.0 + 896.0 * (b - yy) / sb).round().clamp(4.0, 1019.0) as u16;
            *crv = (512.0 + 896.0 * (r - yy) / sr).round().clamp(4.0, 1019.0) as u16;
        }
    });
    if let Some(a) = alpha {
        for (i, dst) in a.iter_mut().enumerate().take(n) {
            *dst = rgba.get(i * 4 + 3).map_or(1023, |&v| ((v as u32 * 1023 + 127) / 255) as u16);
        }
    }
}

fn prores_factory(w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Result<Box<dyn VideoEncoder>> {
    validate_encoder_geometry(w, h, rate)?;
    let profile = waltcut_prores::Profile::P4444;
    let mut cfg = waltcut_prores::EncoderConfig::new(profile, w, h);
    if s.signal.is_hdr() {
        cfg.color = waltcut_prores::ColorInfo { primaries: s.signal.primaries, transfer: s.signal.transfer, matrix: s.signal.matrix };
    }
    cfg.encode_alpha = true;
    let enc = waltcut_prores::Encoder::with_config(cfg);
    Ok(Box::new(ProResEncoder { enc, w, h, rate, signal: s.signal }) as Box<dyn VideoEncoder>)
}

/// H.264 High (our encoder): sRGB/709 RGBA8 → 8-bit limited-range BT.709 4:2:0, VBR at the
/// fixed delivery bitrate, length-prefixed samples with the `avcC` in the sample entry.
struct H264Encoder {
    enc: waltcut_h264enc::Encoder,
    w: u32,
    h: u32,
    rate: FrameRate,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    signal: ColorSignal,
}

impl H264Encoder {
    fn packets(&self, ps: Vec<waltcut_h264enc::Packet>) -> Result<Vec<EncodedPacket>> {
        ps.into_iter()
            .map(|p| {
                let composition_offset = p
                    .pts
                    .checked_sub(p.dts)
                    .and_then(|offset| i32::try_from(offset).ok())
                    .ok_or_else(|| ExportError::Unsupported("encoded presentation offset exceeds the container range".into()))?;
                Ok(EncodedPacket { data: p.data, key: p.keyframe, duration: self.rate.den as u32, composition_offset })
            })
            .collect()
    }
}

impl VideoEncoder for H264Encoder {
    fn sample_entry(&self) -> SampleEntry {
        let cfg = waltcut_isobmff::AvcConfig::parse(&self.enc.avcc()).unwrap_or_else(|_| {
            let (sps, pps) = self.enc.sps_pps();
            waltcut_isobmff::AvcConfig::new(vec![sps], vec![pps], 4)
        });
        let mut e = SampleEntry::avc(cfg, self.w as u16, self.h as u16);
        self.signal.apply_to(&mut e, false);
        e
    }
    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }
    fn encode(&mut self, f: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        validate_encoder_frame(f, self.w, self.h, false)?;
        timed(Stage::Convert, || match f.hdr {
            Some(rgb) => {
                let (kr, kb) = self.signal.kr_kb();
                rgbf_to_yuv420_8(rgb, f.width as usize, f.height as usize, kr, kb, &mut self.y, &mut self.u, &mut self.v)
            }
            None => rgba_to_yuv420_8(f.rgba, f.width as usize, f.height as usize, &mut self.y, &mut self.u, &mut self.v),
        });
        let cw = (f.width as usize).div_ceil(2);
        let frame = waltcut_h264enc::YuvFrame { y: &self.y, u: &self.u, v: &self.v, y_stride: f.width as usize, uv_stride: cw };
        let timestamp = i64::try_from(f.index)
            .ok()
            .and_then(|i| i.checked_mul(self.rate.den))
            .ok_or_else(|| ExportError::Unsupported("encoder frame timestamp overflows".into()))?;
        let ps = self.enc.try_encode(&frame, timestamp).map_err(|e| ExportError::Encode(e.to_string()))?;
        self.packets(ps)
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        let ps = self.enc.flush();
        self.packets(ps)
    }
    fn media_start(&self) -> Option<i64> {
        // With B-frames the first DTS is one frame before the first PTS.
        (self.enc.delay() > 0).then_some(self.rate.den)
    }
}

/// BT.709 limited-range 8-bit 4:2:0 from straight RGBA8 (2×2 chroma average).
fn rgba_to_yuv420_8(rgba: &[u8], w: usize, h: usize, y: &mut Vec<u8>, u: &mut Vec<u8>, v: &mut Vec<u8>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    y.resize(w * h, 0);
    u.resize(cw * ch, 0);
    v.resize(cw * ch, 0);
    y.par_chunks_mut(w * 2).zip(u.par_chunks_mut(cw).zip(v.par_chunks_mut(cw))).enumerate().for_each(|(cy, (yr, (ur, vr)))| {
        let rows = yr.len() / w;
        let mut us = vec![0f32; cw];
        let mut vs = vec![0f32; cw];
        let mut cnt = vec![0f32; cw];
        for dy in 0..rows {
            let row = cy * 2 + dy;
            let src = &rgba[row * w * 4..(row + 1) * w * 4];
            for x in 0..w {
                let (r, g, b) = (src[x * 4] as f32 / 255.0, src[x * 4 + 1] as f32 / 255.0, src[x * 4 + 2] as f32 / 255.0);
                let yy = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                yr[dy * w + x] = (16.0 + 219.0 * yy).round().clamp(1.0, 254.0) as u8;
                us[x / 2] += (b - yy) / 1.8556;
                vs[x / 2] += (r - yy) / 1.5748;
                cnt[x / 2] += 1.0;
            }
        }
        for cx in 0..cw {
            ur[cx] = (128.0 + 224.0 * us[cx] / cnt[cx]).round().clamp(1.0, 254.0) as u8;
            vr[cx] = (128.0 + 224.0 * vs[cx] / cnt[cx]).round().clamp(1.0, 254.0) as u8;
        }
    });
}

/// Limited-range 8-bit 4:2:0 from encoded R'G'B' floats with matrix (Kr, Kb).
#[allow(clippy::too_many_arguments)]
fn rgbf_to_yuv420_8(rgb: &[f32], w: usize, h: usize, kr: f32, kb: f32, y: &mut Vec<u8>, u: &mut Vec<u8>, v: &mut Vec<u8>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let kg = 1.0 - kr - kb;
    let (sb, sr) = (2.0 * (1.0 - kb), 2.0 * (1.0 - kr));
    y.resize(w * h, 0);
    u.resize(cw * ch, 0);
    v.resize(cw * ch, 0);
    y.par_chunks_mut(w * 2).zip(u.par_chunks_mut(cw).zip(v.par_chunks_mut(cw))).enumerate().for_each(|(cy, (yr, (ur, vr)))| {
        let rows = yr.len() / w;
        let mut us = vec![0f32; cw];
        let mut vs = vec![0f32; cw];
        let mut cnt = vec![0f32; cw];
        for dy in 0..rows {
            let row = cy * 2 + dy;
            let src = &rgb[row * w * 3..(row + 1) * w * 3];
            for x in 0..w {
                let (r, g, b) = (src[x * 3], src[x * 3 + 1], src[x * 3 + 2]);
                let yy = kr * r + kg * g + kb * b;
                yr[dy * w + x] = (16.0 + 219.0 * yy).round().clamp(1.0, 254.0) as u8;
                us[x / 2] += (b - yy) / sb;
                vs[x / 2] += (r - yy) / sr;
                cnt[x / 2] += 1.0;
            }
        }
        for cx in 0..cw {
            ur[cx] = (128.0 + 224.0 * us[cx] / cnt[cx]).round().clamp(1.0, 254.0) as u8;
            vr[cx] = (128.0 + 224.0 * vs[cx] / cnt[cx]).round().clamp(1.0, 254.0) as u8;
        }
    });
}

/// Slices per H.264 picture: one per four macroblock rows. The encoder's default follows the core
/// count, which would make the stream (and every decoded picture) depend on the machine; this is
/// what it picks on a machine with enough cores.
pub(crate) fn h264_slices(height: u32) -> usize {
    (height.div_ceil(16) as usize).div_ceil(4).max(1)
}

fn h264_factory(w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Result<Box<dyn VideoEncoder>> {
    validate_encoder_geometry(w, h, rate)?;
    let mut cfg = waltcut_h264enc::EncoderConfig::new(w, h, rate.num as u32, rate.den as u32);
    cfg.format = waltcut_h264enc::PacketFormat::LengthPrefixed;
    cfg.aud = false;
    cfg.keyint = (rate.as_f64() * 2.0).round().max(1.0) as u32;
    cfg.slices = h264_slices(h);
    cfg.rate = waltcut_h264enc::RateControl::Vbr { target_kbps: 20_000, max_kbps: 30_000 };
    cfg.profile = waltcut_h264enc::Profile::High;
    if s.signal.is_hdr() {
        cfg.color = waltcut_h264enc::ColorConfig { primaries: s.signal.primaries, transfer: s.signal.transfer, matrix: s.signal.matrix, full_range: false };
        if let Some((md, cll)) = s.signal.static_metadata() {
            let mut b = [0u8; 24];
            b.copy_from_slice(&md.to_bytes());
            cfg.mastering_display = Some(b);
            cfg.content_light = Some(cll);
        }
    }
    waltcut_h264enc::Encoder::new(cfg)
        .map(|enc| Box::new(H264Encoder { enc, w, h, rate, y: Vec::new(), u: Vec::new(), v: Vec::new(), signal: s.signal }) as Box<dyn VideoEncoder>)
        .map_err(|e| ExportError::Encode(e.to_string()))
}

/// The range to export (settings → In/Out → whole sequence).
pub fn export_range(project: &Project, seq: ItemId, settings: &ExportSettings) -> Result<TimeRange> {
    let q = project.sequence(seq).ok_or(ExportError::NoSequence)?;
    q.check_bounds().map_err(ExportError::Unsupported)?;
    if let Some(r) = settings.range {
        validate_range(r)?;
        return Ok(r);
    }
    let fd = q.settings.frame_rate.frame_duration();
    let a = q.mark_in.unwrap_or(Tick::ZERO);
    let minimum_end = a.0.checked_add(fd.0).ok_or_else(|| ExportError::Unsupported("export In point overflows the time range".into()))?;
    let b = match q.mark_out {
        Some(out) => out.0.checked_add(fd.0).ok_or_else(|| ExportError::Unsupported("export Out point overflows the time range".into()))?,
        None => q.duration().0,
    };
    let duration = b.max(minimum_end).checked_sub(a.0).ok_or_else(|| ExportError::Unsupported("export range duration overflows".into()))?;
    let range = TimeRange::new(a, Tick(duration));
    validate_range(range)?;
    Ok(range)
}

/// Validate input time before frame/sample conversion or any `TimeRange::end` arithmetic.
pub fn validate_range(range: TimeRange) -> Result<()> {
    if range.start.0 < 0
        || range.duration.0 <= 0
        || range.duration.0 > 86_400 * waltcut_time::TICKS_PER_SECOND
        || range.start.0.checked_add(range.duration.0).is_none_or(|end| end > Tick::MAX.0)
    {
        return Err(ExportError::Unsupported(
            "export range must start at or after zero, last at most 24 hours with positive duration and end within supported time bounds".into(),
        ));
    }
    Ok(())
}

/// Output frames `[f0, f1)` of `range` at `rate` (frame `f` is at `rate.tick_of(f)`).
pub fn frame_span(rate: FrameRate, range: TimeRange) -> (i64, i64) {
    let f0 = rate.frame_at(range.start);
    let f1 = rate.frame_at(range.end() - Tick(1)) + 1;
    (f0, f1.max(f0))
}

/// Encode straight sRGB RGBA8 pixels as PNG, preserving alpha.
pub fn encode_png(rgba: Vec<u8>, w: u32, h: u32) -> Result<Vec<u8>> {
    validate_output_size(w, h)?;
    let expected = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4));
    if expected != Some(rgba.len()) {
        return Err(ExportError::Encode("PNG pixel buffer does not match its dimensions".into()));
    }
    let mut out = Vec::new();
    image::ImageEncoder::write_image(image::codecs::png::PngEncoder::new(&mut out), &rgba, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| ExportError::Encode(e.to_string()))?;
    Ok(out)
}

/// Run an export (blocking; call from a worker thread).
pub fn export(project: &Arc<Project>, seq: ItemId, settings: &ExportSettings, sources: &dyn SourceProvider, progress: &Progress) -> Result<Report> {
    settings.validate()?;
    if stepped(settings.format) {
        let mut ex = Exporter::new(project.clone(), seq, settings, progress)?;
        loop {
            match ex.step(sources, progress)? {
                Step::Progress => {}
                Step::Done(r) => return Ok(r),
                // A blocking export cannot wait for asynchronous source data.
                Step::Pending => return Err(ExportError::Io("media data is still loading; run the export as a stepped job".into())),
            }
        }
    }
    let t0 = std::time::Instant::now();
    let range = export_range(project, seq, settings)?;
    let cancelled = || progress.cancel.load(Ordering::Relaxed);
    let batch = rayon::current_num_threads().clamp(2, 16) as i64;
    let pipe = pipeline::Pipeline::new(project.clone(), seq, settings, false)?;
    let batch = pipe.render_batch_limit(job::overlap_batch(batch, job::frame_bytes(pipe.w, pipe.h, false), job::IN_FLIGHT_BUDGET));
    let (f0, f1) = frame_span(pipe.rate, range);
    let count = u64::try_from(f1 - f0).map_err(|_| ExportError::Unsupported("invalid frame count".into()))?;
    if !(1..=1_000_000).contains(&count) {
        return Err(ExportError::Unsupported("export must contain 1..1000000 frames".into()));
    }
    if !settings.part_of_batch {
        progress.total.store(count, Ordering::Relaxed);
        progress.set_status(format!("Exporting {count} frames ({})", settings.format.label()));
    }
    let mut bytes = 0u64;
    let mut f = f0;
    while f < f1 {
        if cancelled() {
            return Err(ExportError::Cancelled);
        }
        let end = f.saturating_add(batch).min(f1);
        let written: Vec<Result<u64>> = (f..end)
            .into_par_iter()
            .map(|fi| {
                if cancelled() {
                    return Err(ExportError::Cancelled);
                }
                let _ = waltcut_media::pending::take();
                let (rgba, _) = pipe.frame(fi, sources);
                if waltcut_media::pending::take() {
                    return Err(ExportError::Io("video media is still loading".into()));
                }
                let data = encode_png(rgba, pipe.w, pipe.h)?;
                if cancelled() {
                    return Err(ExportError::Cancelled);
                }
                write_output(&image_sequence_path(&settings.path, (fi - f0) as u64, count), data)
            })
            .collect();
        for result in written {
            bytes = bytes.checked_add(result?).ok_or_else(|| ExportError::Io("output byte count overflow".into()))?;
        }
        progress.done.fetch_add((end - f) as u64, Ordering::Relaxed);
        f = end;
    }
    let nframes = count;
    let secs = t0.elapsed().as_secs_f64();
    if !settings.part_of_batch {
        progress.finished.store(true, Ordering::Relaxed);
        progress.set_status(format!("Done in {secs:.1}s"));
    }
    Ok(Report { path: settings.path.clone(), frames: nframes, seconds: secs, bytes, render_fps: nframes as f64 / secs.max(1e-6) })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod settings_tests;

#[cfg(test)]
mod determinism_tests;

#[cfg(test)]
mod hdr_tests;
