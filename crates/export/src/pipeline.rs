//! Sequence-sized export composition, alpha and colour conversion.

use std::sync::Arc;

use rayon::prelude::*;
use waltcut_project::{ItemId, Project};
use waltcut_render::{Image, RenderOptions, SourceProvider};
use waltcut_time::FrameRate;

use crate::{ExportError, ExportSettings, FrameRenderer, GpuRendering, Result, build_frame_renderer, note_gpu_fallback, note_gpu_frame};

/// GPU renderers shared by the export workers: a worker takes a free one for a frame and puts it
/// back (a single renderer behind a mutex made the workers queue, GPU export ~20 % slower than CPU).
struct RendererPool {
    /// Initial capacity, also the maximum number of frame tasks admitted at once.
    capacity: usize,
    renderers: std::sync::Mutex<Vec<Box<dyn FrameRenderer>>>,
    available: std::sync::Condvar,
    /// A renderer panicked: the pool is retired and the rest of the export renders on the CPU.
    failed: std::sync::atomic::AtomicBool,
}

impl RendererPool {
    /// `None` for an empty list (a pool nobody could take from).
    fn new(renderers: Vec<Box<dyn FrameRenderer>>) -> Option<Self> {
        (!renderers.is_empty()).then(|| Self {
            capacity: renderers.len(),
            renderers: std::sync::Mutex::new(renderers),
            available: std::sync::Condvar::new(),
            failed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Run `f` with a free renderer. Frame batches must respect `Pipeline::render_batch_limit`:
    /// a renderer can run nested Rayon work, which may steal another frame on the same thread.
    /// Admitting more frame tasks than renderers could then deadlock this condition variable
    /// against the suspended outer frame's renderer. `None` once a renderer has
    /// panicked: the panicking renderer is dropped (its state can't be trusted) and so are the
    /// others as they come back, so the caller falls back to the CPU for the rest of the export.
    fn with<R>(&self, f: impl FnOnce(&mut dyn FrameRenderer) -> R) -> Option<R> {
        use std::sync::atomic::Ordering;
        let mut free = self.renderers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut r = loop {
            if self.failed.load(Ordering::SeqCst) {
                return None;
            }
            match free.pop() {
                Some(r) => break r,
                None => free = self.available.wait(free).unwrap_or_else(std::sync::PoisonError::into_inner),
            }
        };
        drop(free);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut *r)));
        let mut free = self.renderers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if res.is_err() {
            // (the panic hook has logged the panic itself)
            self.failed.store(true, Ordering::SeqCst);
            free.clear();
            drop(free);
            drop(r);
        } else if self.failed.load(Ordering::SeqCst) {
            drop(free);
            drop(r);
        } else {
            free.push(r);
            drop(free);
        }
        // waiters re-check `failed` (or take the renderer that came back)
        self.available.notify_all();
        res.ok()
    }
}

/// The immutable part of an export: everything needed to produce one output frame.
pub(crate) struct Pipeline {
    pub project: Arc<Project>,
    /// The registered frame renderers (GPU compositors) when the setting allows and factories
    /// provide them; pooled because `frame` runs on several export threads at once.
    renderer: Option<RendererPool>,
    pub seq: ItemId,
    /// Output frame rate (frame `f` is at `rate.tick_of(f)`).
    pub rate: FrameRate,
    pub w: u32,
    pub h: u32,
    opts: RenderOptions,
    pub hdr_out: bool,
    /// Keep straight alpha in the RGBA8 output instead of flattening over black.
    alpha: bool,
    out_tf: Option<waltcut_color::OutputTransform>,
    /// Free the pool's float images when the export ends: a standalone export, not one part of a
    /// batch (`ExportSettings::part_of_batch`).
    trim_pool: bool,
}

impl Drop for Pipeline {
    /// The export is over, however it ended (done, failed, cancelled, dropped half way). The float
    /// images its frames left on the `waltcut_frame::pool` float shelf are the size of its
    /// frames, which nothing else asks for, so free them instead of holding up to 320 MiB idle. A
    /// job still rendering at that moment allocates a few images again; the pool is only a cache.
    /// Parts of a larger job (render-preview segments, proxies) keep them for the next part.
    fn drop(&mut self) {
        if self.trim_pool {
            waltcut_frame::pool::trim_f32();
        }
    }
}

impl Pipeline {
    /// Bound outer frame tasks before spawning them, not inside a Rayon job. A renderer can
    /// invoke nested parallel decoding/conversion; those joins may steal another frame task.
    /// With at most one active batch and no more tasks than the initial renderer capacity,
    /// every stolen frame can acquire a renderer without waiting for an ancestor to return it.
    /// Keep the immutable capacity even after a failed renderer retires the pool.
    pub(crate) fn render_batch_limit(&self, requested: i64) -> i64 {
        let requested = requested.max(1);
        self.renderer.as_ref().map_or(requested, |pool| requested.min(i64::try_from(pool.capacity).unwrap_or(i64::MAX)))
    }

    pub fn new(project: Arc<Project>, seq: ItemId, settings: &ExportSettings, hdr_out: bool) -> Result<Self> {
        let q = project.sequence(seq).ok_or(ExportError::NoSequence)?;
        q.settings.validate().map_err(ExportError::Unsupported)?;
        settings.validate()?;
        let r = settings.resolve(q.settings.width, q.settings.height, q.settings.frame_rate, q.settings.sample_rate)?;
        crate::validate_output_size(r.width, r.height)?;
        let (w, h) = waltcut_render::output_size(q, settings.scale);
        let w = u32::try_from(w).map_err(|_| ExportError::Unsupported("rendered picture width is too large".into()))?;
        let h = u32::try_from(h).map_err(|_| ExportError::Unsupported("rendered picture height is too large".into()))?;
        crate::validate_output_size(w, h)?;
        let pipe = q.settings.color;
        let out_tf = hdr_out.then(|| waltcut_color::OutputTransform::new(&pipe, pipe.working.output_space()));
        let opts = RenderOptions { scale: settings.scale, working_output: hdr_out, ..Default::default() };
        let renderer = if settings.gpu_rendering == GpuRendering::Off {
            None
        } else {
            // Four renderers: as fast as one per worker (measured on 16 threads, where more only
            // contend for memory bandwidth in the read-back), with a quarter of the GPU memory.
            let count = rayon::current_num_threads().clamp(2, 4);
            let mut list = Vec::with_capacity(count);
            for _ in 0..count {
                if let Some(r) = build_frame_renderer() {
                    list.push(r);
                } else {
                    break;
                }
            }
            RendererPool::new(list)
        };
        Ok(Pipeline {
            renderer,
            trim_pool: !settings.part_of_batch,
            project: project.clone(),
            seq,
            rate: r.rate,
            w: r.width,
            h: r.height,
            opts,
            hdr_out,
            alpha: settings.keeps_alpha(),
            out_tf,
        })
    }

    /// Output frame `f` as straight sRGB RGBA8 at the output size, or for HDR exports the encoded
    /// R'G'B' floats (3 per pixel).
    pub fn frame(&self, f: i64, sources: &dyn SourceProvider) -> (Vec<u8>, Vec<f32>) {
        let t = self.rate.tick_of(f);
        // The registered GPU renderer goes first; `None` (no adapter, a plan the GPU cannot
        // draw, any internal error) falls back to the CPU reference renderer below. A frame the
        // planner hands back as one CPU image anyway (a complex transition,
        // HDR) does not take a pooled renderer: it would render on the CPU inside the pool, as
        // many frames at a time as there are renderers instead of one per export thread.
        let pool = self.renderer.as_ref().filter(|_| !waltcut_render::plan::is_cpu_frame(&self.project, self.seq));
        let gpu = match pool {
            Some(pool) => {
                let asked = std::time::Instant::now();
                pool.with(|r| {
                    crate::note_lock_wait(asked.elapsed());
                    r.render(&self.project, self.seq, t, self.opts, sources)
                })
                .flatten()
            }
            None => None,
        };
        let img = match gpu {
            Some(img) => {
                note_gpu_frame();
                img
            }
            None => {
                if self.renderer.is_some() {
                    note_gpu_fallback();
                }
                waltcut_render::render_sequence(&self.project, self.seq, t, self.opts, sources)
            }
        };
        let img = self.place(img);
        if let Some(tf) = self.out_tf.as_ref().filter(|_| self.hdr_out) {
            let mut out = vec![0f32; img.w * img.h * 3];
            out.par_chunks_mut(img.w * 3).zip(img.px.par_chunks(img.w * 4)).for_each(|(o, s)| {
                for (o, p) in o.as_chunks_mut::<3>().0.iter_mut().zip(s.as_chunks::<4>().0) {
                    let rgb = if self.alpha { Image::unpremul(*p) } else { [p[0], p[1], p[2]] };
                    o.copy_from_slice(&tf.encode(rgb));
                }
            });
            // the float image is not needed any more: its buffer serves the next frame's layers
            let alpha = if self.alpha { img.to_rgba8() } else { Vec::new() };
            waltcut_frame::pool::recycle_f32(img.px);
            return (alpha, out);
        }
        let rgba = if self.alpha { img.to_rgba8() } else { img.over_black_rgba8() };
        // the float image is not needed any more: its buffer serves the next frame's layers
        waltcut_frame::pool::recycle_f32(img.px);
        (rgba, Vec::new())
    }

    /// Fit the rendered picture into the output frame.
    fn place(&self, img: Image) -> Image {
        let (w, h) = (self.w as usize, self.h as usize);
        if img.w == w && img.h == h {
            return img;
        }
        let mut out = Image::new(w, h);
        let (ox, oy) = (0i64, 0i64);
        let x0 = ox.max(0) as usize;
        let x1 = (ox + img.w as i64).clamp(0, w as i64) as usize;
        if x1 <= x0 {
            return out;
        }
        out.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            let sy = y as i64 - oy;
            if sy < 0 || sy >= img.h as i64 {
                return;
            }
            let sx0 = (x0 as i64 - ox) as usize;
            let src = &img.px[(sy as usize * img.w + sx0) * 4..(sy as usize * img.w + sx0 + (x1 - x0)) * 4];
            row[x0 * 4..x1 * 4].copy_from_slice(src);
        });
        out
    }
}
