//! Step-wise export of H.264 + AAC MP4 and alpha ProRes 4444 MOV.
//!
//! [`Exporter::step`] renders, encodes and muxes one batch of frames (plus the audio up to its
//! end) per call; [`crate::export`] loops it on the engine's owned worker thread.
//!
//! When [`waltcut_media::pending`] reports unavailable video bytes, the batch is rendered again:
//! nothing is encoded from incomplete frames. Unavailable audio is an error, not a successful
//! output with missing samples.
//!
//! The file does not depend on how many frames a step renders: packets go to the muxer in fixed
//! groups of [`INTERLEAVE`] output frames, each followed by
//! the audio up to its end, so the same project and settings give the same bytes on any machine.
//!
//! # Overlapping render and encode
//!
//! Rendering a batch is parallel (rayon); encoding, muxing and the audio are serial and own the
//! encoder, the container writer and the audio state. Run one after the other the wall time was
//! `render + encode + audio + mux`. A step now renders batch *k+1* on the rayon pool
//! ([`rayon::in_place_scope`] + `spawn`) while the *calling* thread encodes, muxes and mixes the
//! audio of batch *k*, and keeps the finished frames in `ahead` for the next step: the wall time
//! is about `max(render, encode + audio + mux)`. Design points:
//!
//! * **Bytes do not change.** Batches are cut exactly as before (`next .. next + batch`), frames
//!   are encoded in order, and the audio / mux interleave still follows [`INTERLEAVE`] groups, so
//!   the file is identical with or without overlap, on any core count and for any step size
//!   (`determinism_tests`).
//! * **The encode side stays on the thread that called [`Exporter::step`].** Only the
//!   render of the next batch moves to the pool. It needs no `&mut` access to the exporter: it
//!   holds an `Arc<Pipeline>` and the source provider.
//! * **Pending media.** [`waltcut_media::pending`] is a thread-local flag. The render of a batch takes the
//!   flag after every frame, on whatever thread drew it, and a batch that saw it is discarded:
//!   a prefetched batch that hit pending is dropped and rendered again by the next step, and
//!   nothing is ever encoded from it. The audio pull checks the flag on the encoding thread and
//!   fails the export explicitly; transactional engine staging protects the destination.
//! * **Cancellation** is checked at the start of every step, which drops the prefetched frames;
//!   the prefetch is not started once it is set.
//! * **Panics.** The prefetch runs under `catch_unwind` and becomes an [`ExportError`]; a panic of
//!   the encode side unwinds the calling thread as before (the job guard turns it into an error).
//! * **Memory.** Two batches are in flight instead of one, so the batch is capped by
//!   [`IN_FLIGHT_BUDGET`]: `2 * batch * frame_bytes <= budget`, at least one frame
//!   ([`overlap_batch`]). A batch never grows over what it was without overlap, so the peak is the
//!   old peak plus at most the finished frames of the batch being encoded (half the budget,
//!   256 MiB); a 4K HDR export (RGBA plus RGB floats, about 127 MiB/frame) runs 2-frame batches
//!   and 4K SDR 8-frame ones instead of 16. Sequential batches respect the same byte budget. Frames are
//!   dropped as soon as they are encoded.
//! * **Timers.** `Render` and `Encode` are wall times of work that now runs at the same time, so
//!   their sum may exceed the export's wall time; `Wait` is the time the encoding side then spent
//!   waiting for the render (large: render-bound; near zero: encode-bound).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use waltcut_isobmff::{Brand, Mp4Writer, PcmConfig, SampleEntry, TrackConfig, WriteSample, WriterOptions};
use waltcut_project::{ItemId, Project};
use waltcut_render::SourceProvider;
use waltcut_time::FrameRate;

use crate::audio_out::AudioOut;
use crate::pipeline::Pipeline;
use crate::{
    AudioEncoder, ColorSignal, EncodedPacket, EncoderFrame, ExportError, ExportSettings, Format, Out, Progress, Report, Result, Stage, VideoEncoder,
    aac_factory, export_range, frame_span, h264_factory, note_stage, prores_factory, timed,
};

/// Output frames per interleaved group: the video packets of these frames, then the audio up to
/// their end, form one chunk of each track. Fixed (not the batch size, which follows the core
/// count) so the file layout is the same on every machine.
pub const INTERLEAVE: i64 = 16;

/// Most bytes of rendered frames an overlapped export keeps in flight (the batch being encoded and
/// the one being rendered), see [`overlap_batch`].
pub const IN_FLIGHT_BUDGET: u64 = 1 << 29;

/// One rendered picture: straight RGBA8, or for HDR exports the encoded R'G'B' floats.
type Frame = (Vec<u8>, Vec<f32>);

/// Bytes of one [`Frame`] of a `w` x `h` export (RGBA8, plus three f32 per pixel for HDR).
pub(crate) fn frame_bytes(w: u32, h: u32, hdr: bool) -> u64 {
    u64::from(w).saturating_mul(u64::from(h)).saturating_mul(if hdr { 16 } else { 4 })
}

/// Frames per batch when two batches are in flight: `batch` (what a sequential export would use),
/// lowered so that `2 * frames * frame_bytes` stays within `budget`, but never below one frame.
pub(crate) fn overlap_batch(batch: i64, frame_bytes: u64, budget: u64) -> i64 {
    let want = u64::try_from(batch.max(1)).unwrap_or(1);
    let fit = (budget / 2) / frame_bytes.max(1);
    i64::try_from(fit.min(want)).unwrap_or(1).max(1)
}

/// A batch rendered while the previous one was being encoded.
struct Ahead {
    /// First frame.
    first: i64,
    frames: Vec<Frame>,
}

/// Render frames `f..end` in parallel. `None` when a source was not ready for one of them (the
/// pending flag, taken after every frame because it is per thread): the batch is incomplete and
/// must be rendered again.
fn render_batch(pipe: &Pipeline, f: i64, end: i64, sources: &dyn SourceProvider) -> Option<Vec<Frame>> {
    let pending = AtomicBool::new(false);
    let frames: Vec<Frame> = (f..end)
        .into_par_iter()
        .map(|fi| {
            let frame = pipe.frame(fi, sources);
            if waltcut_media::pending::take() {
                pending.store(true, Ordering::Relaxed);
            }
            frame
        })
        .collect();
    (!pending.load(Ordering::Relaxed)).then_some(frames)
}

/// What one [`Exporter::step`] did.
#[derive(Debug)]
pub enum Step {
    /// A batch was encoded; call again.
    Progress,
    /// Media bytes are still loading: nothing was encoded, call again later.
    Pending,
    /// The file is complete.
    Done(Report),
}

/// An export of an encoded-video format, advanced one batch at a time.
pub struct Exporter {
    pipe: Arc<Pipeline>,
    settings: ExportSettings,
    f0: i64,
    f1: i64,
    /// Next frame to encode.
    next: i64,
    batch: i64,
    /// Render the next batch while this one is encoded (see the module docs).
    overlap: bool,
    /// Bytes of frames the overlap may keep in flight ([`IN_FLIGHT_BUDGET`]; tests lower it).
    budget: u64,
    /// The batch starting at `next`, already rendered.
    ahead: Option<Ahead>,
    /// Steps that rendered a batch alongside the encoding of the previous one.
    overlapped_steps: u64,
    brand: Brand,
    venc: Box<dyn VideoEncoder>,
    aenc: Option<Box<dyn AudioEncoder>>,
    audio: Option<AudioOut>,
    /// Encoded video packets not written yet (the current [`INTERLEAVE`] group).
    queued: Vec<EncodedPacket>,
    /// Created after the first group was encoded (encoders may finalise their codec config then).
    mux: Option<Mp4Writer<Out>>,
    vt: usize,
    at: Option<usize>,
    t0: std::time::Instant,
}

/// Whether [`Exporter`] handles a format.
pub fn stepped(format: Format) -> bool {
    matches!(format, Format::H264 | Format::ProRes)
}

/// Whether an export is HDR: an HDR working space, SDR not asked for, and a format whose encoder
/// writes HDR unless an internal preview explicitly asks for SDR.
pub(crate) fn hdr_output(working_hdr: bool, sdr: bool) -> bool {
    working_hdr && !sdr
}

fn make_venc(settings: &ExportSettings, w: u32, h: u32, rate: FrameRate) -> Result<Box<dyn VideoEncoder>> {
    match settings.format {
        Format::H264 => h264_factory(w, h, rate, settings),
        Format::ProRes => prores_factory(w, h, rate, settings),
        Format::PngSequence => Err(ExportError::Unsupported("PNG is written as an image sequence".into())),
    }
}

impl Exporter {
    /// Set up an export (encoder, output size, colour signalling); sets `progress.total`.
    pub fn new(project: Arc<Project>, seq: ItemId, settings: &ExportSettings, progress: &Progress) -> Result<Self> {
        timed(Stage::Setup, || Self::set_up(project, seq, settings, progress))
    }

    fn set_up(project: Arc<Project>, seq: ItemId, settings: &ExportSettings, progress: &Progress) -> Result<Self> {
        if !stepped(settings.format) {
            return Err(ExportError::Unsupported(format!("{} is not a stepped export", settings.format.label())));
        }
        settings.validate()?;
        let q = project.sequence(seq).ok_or(ExportError::NoSequence)?;
        // HDR sequences retain their working colour; internal render previews request SDR.
        let pipe = q.settings.color;
        let hdr_out = hdr_output(pipe.working.is_hdr(), settings.sdr);
        let mut settings = settings.clone();
        settings.signal = match (hdr_out, pipe.working) {
            (true, waltcut_color::WorkingSpace::Rec2100Pq) => ColorSignal::PQ,
            (true, _) => ColorSignal::HLG,
            _ => ColorSignal::default(),
        };
        let range = export_range(&project, seq, &settings)?;
        let pipe = Arc::new(Pipeline::new(project.clone(), seq, &settings, hdr_out)?);
        let (f0, f1) = frame_span(pipe.rate, range);
        let nframes = u64::try_from(f1 - f0).map_err(|_| ExportError::Unsupported("invalid frame count".into()))?;
        if !(1..=1_000_000).contains(&nframes) {
            return Err(ExportError::Unsupported("export must contain 1..1000000 frames".into()));
        }
        if !settings.part_of_batch {
            progress.total.store(nframes, Ordering::Relaxed);
            progress.set_status(format!("Exporting {} frames ({})", nframes, settings.format.label()));
        }
        let venc = make_venc(&settings, pipe.w, pipe.h, pipe.rate)?;
        let brand = if settings.format == Format::H264 { Brand::Mp4 } else { Brand::Mov };
        let audio = if settings.has_audio() { Some(AudioOut::new(project.clone(), seq, &settings, range)?) } else { None };
        let aenc: Option<Box<dyn AudioEncoder>> = match (&audio, settings.format) {
            (Some(a), Format::H264) => Some(aac_factory(a.sr)?),
            _ => None,
        };
        Ok(Self {
            pipe,
            f0,
            f1,
            next: f0,
            batch: rayon::current_num_threads().clamp(2, 16) as i64,
            overlap: true,
            budget: IN_FLIGHT_BUDGET,
            ahead: None,
            overlapped_steps: 0,
            brand,
            venc,
            aenc,
            audio,
            queued: Vec::new(),
            mux: None,
            vt: 0,
            at: None,
            t0: std::time::Instant::now(),
            settings,
        })
    }

    /// Frames in the export.
    pub fn frames(&self) -> u64 {
        (self.f1 - self.f0).max(0) as u64
    }

    /// Requested frames per [`Self::step`] (default: one per core), capped by GPU renderer
    /// capacity and the memory budget.
    pub fn set_batch(&mut self, n: i64) {
        self.batch = n.max(1);
    }

    /// Render the next batch while encoding the current one. Off gives sequential
    /// render-then-encode; the file is the same.
    pub fn set_overlap(&mut self, on: bool) {
        self.overlap = on;
        if !on {
            self.ahead = None;
        }
    }

    /// Lower the in-flight byte budget (tests make small frames hit the cap).
    #[cfg(test)]
    pub(crate) fn set_budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }

    /// Whether steps overlap rendering with encoding now: switched on, a pool with more than one
    /// thread.
    pub fn overlapping(&self) -> bool {
        self.overlap && rayon::current_num_threads() > 1
    }

    /// Steps so far that rendered the next batch alongside the encoding of the current one.
    pub fn overlapped_steps(&self) -> u64 {
        self.overlapped_steps
    }

    /// Frames rendered ahead of the encoder now (memory held by the overlap).
    pub fn frames_ahead(&self) -> usize {
        self.ahead.as_ref().map_or(0, |a| a.frames.len())
    }

    /// Frames in the next batch, bounded before Rayon can steal nested frame tasks.
    fn batch_len(&self) -> i64 {
        let batch = self.pipe.render_batch_limit(self.batch);
        let budget = if self.overlapping() { self.budget } else { self.budget.saturating_mul(2) };
        overlap_batch(batch, frame_bytes(self.pipe.w, self.pipe.h, self.pipe.hdr_out), budget)
    }

    fn sample_at_frame(&self, f: i64, sr: u32) -> i64 {
        self.pipe.rate.tick_of(f).to_units_floor(sr as i64)
    }

    fn write_audio(&mut self, planar: Option<Vec<Vec<f32>>>) -> Result<()> {
        let (Some(at), Some(buf)) = (self.at, planar) else { return Ok(()) };
        let n = buf.first().map_or(0, Vec::len);
        if n == 0 {
            return Ok(());
        }
        let mux = self.mux.as_mut().ok_or_else(|| ExportError::Encode("internal: the container writer was not created".into()))?;
        match self.aenc.as_mut() {
            Some(a) => {
                for au in a.encode(&buf)? {
                    mux.write_sample(at, WriteSample { data: &au, duration: a.frame_size(), composition_offset: 0, is_sync: true })
                        .map_err(|e| ExportError::Io(e.to_string()))?;
                }
            }
            None => {
                // little-endian, like the track's `lpcm` / `sowt` description says
                let mut pcm = Vec::new();
                crate::pcm::encode(&crate::audio_out::interleave(&buf), &mut pcm);
                mux.write_sample(at, WriteSample { data: &pcm, duration: n as u32, composition_offset: 0, is_sync: true })
                    .map_err(|e| ExportError::Io(e.to_string()))?;
            }
        }
        Ok(())
    }

    fn write_audio_until(&mut self, until: i64, sources: &dyn SourceProvider, progress: &Progress) -> Result<()> {
        loop {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(ExportError::Cancelled);
            }
            let planar = self.audio.as_mut().and_then(|a| a.pull(until, sources));
            if waltcut_media::pending::take() {
                return Err(ExportError::Io("audio media is still loading".into()));
            }
            if planar.is_none() {
                return Ok(());
            }
            self.write_audio(planar)?;
        }
    }

    fn write_video(&mut self, packets: Vec<EncodedPacket>) -> Result<()> {
        let mux = self.mux.as_mut().ok_or_else(|| ExportError::Encode("internal: the container writer was not created".into()))?;
        for p in packets {
            mux.write_sample(self.vt, WriteSample { data: &p.data, duration: p.duration, composition_offset: p.composition_offset, is_sync: p.key })
                .map_err(|e| ExportError::Io(e.to_string()))?;
        }
        Ok(())
    }

    /// Create the muxer and its tracks (after the first batch was encoded).
    fn open_mux(&mut self) -> Result<()> {
        let file = Out::create(&self.settings)?;
        let mut opts = WriterOptions::new(self.brand);
        opts.metadata = vec![("©too".into(), format!("FilmCraft {}", env!("CARGO_PKG_VERSION")))];
        let mut mux = Mp4Writer::new(file, opts).map_err(|e| ExportError::Io(e.to_string()))?;
        let mut vcfg = TrackConfig::new(self.venc.sample_entry(), self.venc.timescale());
        vcfg.media_start = self.venc.media_start();
        self.vt = mux.add_track(vcfg).map_err(|e| ExportError::Io(e.to_string()))?;
        self.at = match (&self.audio, &self.aenc) {
            (None, _) => None,
            (Some(a), Some(enc)) => {
                let mut c = TrackConfig::new(enc.sample_entry(), a.sr);
                c.media_start = Some(enc.priming() as i64);
                Some(mux.add_track(c).map_err(|e| ExportError::Io(e.to_string()))?)
            }
            (Some(a), None) => {
                let pcm = PcmConfig { bits: 32, float: true, big_endian: false, signed: true, channels: a.channels as _, sample_rate: a.sr as f64 };
                Some(mux.add_track(TrackConfig::new(SampleEntry::pcm(pcm), a.sr)).map_err(|e| ExportError::Io(e.to_string()))?)
            }
        };
        self.mux = Some(mux);
        Ok(())
    }

    /// Render, encode and mux the next batch (or finish the file).
    pub fn step(&mut self, sources: &dyn SourceProvider, progress: &Progress) -> Result<Step> {
        if progress.cancel.load(Ordering::Relaxed) {
            self.ahead = None;
            return Err(ExportError::Cancelled);
        }
        let _ = waltcut_media::pending::take();
        if self.next < self.f1 {
            return self.step_batch(sources, progress);
        }
        self.ahead = None;
        let finishing = std::time::Instant::now();
        if self.mux.is_none() {
            self.open_mux()?;
        }
        let mut tail = std::mem::take(&mut self.queued);
        tail.extend(self.venc.flush()?);
        self.write_video(tail)?;
        if let Some(end) = self.audio.as_ref().map(|a| a.end) {
            self.write_audio_until(end, sources, progress)?;
        }
        if let (Some(at), Some(a)) = (self.at, self.aenc.as_mut()) {
            let mux = self.mux.as_mut().ok_or_else(|| ExportError::Encode("internal: the container writer was not created".into()))?;
            for au in a.flush()? {
                mux.write_sample(at, WriteSample { data: &au, duration: a.frame_size(), composition_offset: 0, is_sync: true })
                    .map_err(|e| ExportError::Io(e.to_string()))?;
            }
        }
        let w = self
            .mux
            .take()
            .ok_or_else(|| ExportError::Encode("internal: the container writer was not created".into()))?
            .finish()
            .map_err(|e| ExportError::Io(e.to_string()))?;
        let bytes = w.finish()?;
        note_stage(Stage::Finish, finishing.elapsed());
        let secs = self.t0.elapsed().as_secs_f64();
        let nframes = self.frames();
        if !self.settings.part_of_batch {
            progress.finished.store(true, Ordering::Relaxed);
            progress.set_status(format!("Done in {secs:.1}s"));
        }
        Ok(Step::Done(Report { path: self.settings.path.clone(), frames: nframes, seconds: secs, bytes, render_fps: nframes as f64 / secs.max(1e-6) }))
    }

    /// Encode (and mux) the batch starting at `self.next`, rendering the one after it meanwhile.
    fn step_batch(&mut self, sources: &dyn SourceProvider, progress: &Progress) -> Result<Step> {
        let (f, frames) = match self.ahead.take() {
            Some(a) if a.first == self.next => (a.first, a.frames),
            _ => {
                let (f, end) = (self.next, self.next.saturating_add(self.batch_len()).min(self.f1));
                let pipe = &self.pipe;
                match timed(Stage::Render, || render_batch(pipe, f, end, sources)) {
                    Some(frames) => (f, frames),
                    None => return Ok(Step::Pending),
                }
            }
        };
        let end = f.saturating_add(i64::try_from(frames.len()).unwrap_or(0));
        if end <= f {
            return Err(ExportError::Encode("internal: an empty batch was rendered".into()));
        }
        // the batch after this one, when the pool can render it while this one is encoded
        let plan =
            (end < self.f1 && self.overlapping() && !progress.cancel.load(Ordering::Relaxed)).then(|| (end, end.saturating_add(self.batch_len()).min(self.f1)));
        let Some((a, b)) = plan else {
            self.encode_batch(f, frames, sources, progress)?;
            return {
                progress.done.fetch_add((end - f) as u64, Ordering::Relaxed);
                self.next = end;
                Ok(Step::Progress)
            };
        };
        let pipe = self.pipe.clone();
        let mut slot: Option<std::thread::Result<Option<Vec<Frame>>>> = None;
        let mut encode_done: Option<std::time::Instant> = None;
        let encoded = rayon::in_place_scope(|s| {
            s.spawn(|_| {
                slot = Some(catch_unwind(AssertUnwindSafe(|| timed(Stage::Render, || render_batch(&pipe, a, b, sources)))));
            });
            let r = self.encode_batch(f, frames, sources, progress);
            encode_done = Some(std::time::Instant::now());
            r
        });
        // the scope returned only now: what is left since the encoding ended is waiting for the render
        if let Some(t) = encode_done {
            note_stage(Stage::Wait, t.elapsed());
        }
        encoded?;
        {
            self.overlapped_steps += 1;
            progress.done.fetch_add((end - f) as u64, Ordering::Relaxed);
            self.next = end;
            match slot {
                Some(Ok(Some(frames))) => self.ahead = Some(Ahead { first: a, frames }),
                // a source was not ready: the next step renders this batch again
                Some(Ok(None)) | None => {}
                Some(Err(_)) => return Err(ExportError::Encode("internal error while rendering a frame (the panic was logged by the panic hook)".into())),
            }
            Ok(Step::Progress)
        }
    }

    /// Encode the rendered frames `f..`, mux them with their audio, and drop each frame as soon
    /// as it is encoded.
    fn encode_batch(&mut self, f: i64, frames: Vec<Frame>, sources: &dyn SourceProvider, progress: &Progress) -> Result<()> {
        let end = f.saturating_add(i64::try_from(frames.len()).unwrap_or(0));
        let cuts: Vec<i64> = (f + 1..=end).filter(|&g| (g - self.f0) % INTERLEAVE == 0).collect();
        for (k, frame) in frames.into_iter().enumerate() {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(ExportError::Cancelled);
            }
            let fi = f + k as i64;
            let packets = timed(Stage::Encode, || self.encode(std::slice::from_ref(&frame), fi))?;
            self.queued.extend(packets);
            if cuts.contains(&(fi + 1)) {
                timed(Stage::Mux, || -> Result<()> {
                    if self.mux.is_none() {
                        self.open_mux()?;
                    }
                    let packets = std::mem::take(&mut self.queued);
                    self.write_video(packets)
                })?;
                if let Some(sr) = self.audio.as_ref().map(|a| a.sr) {
                    let until = self.sample_at_frame(fi + 1, sr);
                    timed(Stage::Audio, || self.write_audio_until(until, sources, progress))?;
                }
            }
        }
        Ok(())
    }

    fn encode(&mut self, frames: &[(Vec<u8>, Vec<f32>)], f: i64) -> Result<Vec<EncodedPacket>> {
        let mut packets = Vec::new();
        for (k, (rgba, hdr)) in frames.iter().enumerate() {
            let fr = EncoderFrame {
                width: self.pipe.w,
                height: self.pipe.h,
                rgba,
                hdr: self.pipe.hdr_out.then_some(hdr.as_slice()),
                index: (f - self.f0) as u64 + k as u64,
            };
            packets.extend(self.venc.encode(&fr)?);
        }
        Ok(packets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    #[test]
    fn sequential_batches_respect_the_memory_budget_even_for_hostile_requests() {
        let (project, seq, _) = crate::tests::project();
        let mut export = Exporter::new(project, seq, &ExportSettings::default(), &Progress::default()).unwrap();
        export.set_overlap(false);
        let bytes = frame_bytes(export.pipe.w, export.pipe.h, false);
        export.set_budget(2 * bytes);
        for request in [16, i64::MAX] {
            export.set_batch(request);
            assert_eq!(export.batch_len(), 2, "request {request} must not allocate an oversized batch");
        }
        export.set_batch(i64::MIN);
        assert_eq!(export.batch_len(), 1);
    }

    #[test]
    fn frame_bytes_by_format() {
        assert_eq!(frame_bytes(1920, 1080, false), 1920 * 1080 * 4);
        assert_eq!(frame_bytes(3840, 2160, true), 3840 * 2160 * 16);
        assert_eq!(frame_bytes(u32::MAX, u32::MAX, true), u64::MAX, "saturates");
        assert_eq!(frame_bytes(0, 1080, false), 0);
    }

    #[test]
    fn the_overlap_batch_fits_the_memory_budget() {
        let budget = IN_FLIGHT_BUDGET;
        // 1080p keeps the full 16 frames, 4K SDR (33 MB a frame) 8 + 8 in flight (2 x 8 x 33 MB = 512 MiB)
        assert_eq!(overlap_batch(16, frame_bytes(1920, 1080, false), budget), 16);
        assert_eq!(overlap_batch(16, frame_bytes(3840, 2160, false), budget), 8);
        // 4K HDR (RGBA plus RGB floats): 256 MiB per batch is 2 frames, not 16.
        let hdr4k = frame_bytes(3840, 2160, true);
        let n = overlap_batch(16, hdr4k, budget);
        assert_eq!(n, 2);
        assert!(2 * n as u64 * hdr4k <= budget);
        // 8K HDR: one frame at a time, even though two do not fit (never fewer than one)
        assert_eq!(overlap_batch(16, frame_bytes(7680, 4320, true), budget), 1);
        // a smaller batch (web: 1) is never raised, a degenerate one is one frame
        assert_eq!(overlap_batch(1, 4, budget), 1);
        assert_eq!(overlap_batch(0, 4, budget), 1);
        assert_eq!(overlap_batch(-3, 4, budget), 1);
        assert_eq!(overlap_batch(8, 0, budget), 8, "no division by zero");
        assert_eq!(overlap_batch(8, u64::MAX, budget), 1);
        assert_eq!(overlap_batch(8, 100 * MIB, 0), 1);
        // two batches in flight never exceed the budget unless a single frame alone does
        for batch in 1..=16 {
            for fb in [MIB, 8 * MIB, 33 * MIB, 100 * MIB, 400 * MIB, 700 * MIB] {
                let n = overlap_batch(batch, fb, budget) as u64;
                assert!(n >= 1 && n <= batch as u64);
                assert!(n == 1 || 2 * n * fb <= budget, "batch {batch}, frame {fb}: {n}");
            }
        }
    }
}
