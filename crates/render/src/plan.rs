use std::sync::Arc;

use waltcut_frame::VideoFrame;
use waltcut_geom::Affine;
use waltcut_media::FrameRequest;
use waltcut_project::{ItemId, ItemKind, Project, Sequence, TrackItem};
use waltcut_time::Tick;

use crate::{Blend, RenderOptions, SourceProvider, motion_matrix, output_size};

/// One layer for the GPU, bottom to top.
#[derive(Clone)]
pub struct PlanLayer {
    pub frame: Arc<VideoFrame>,
    /// Maps frame pixels (0..w, 0..h) to output pixels.
    pub matrix: Affine,
    pub opacity: f32,
    /// How the layer combines with what is under it ([`crate::blend::composite`]).
    pub blend: Blend,
    /// Colour management of a Y'CbCr frame that needs more than the default decode (HDR, BT.2020,
    /// P3 media on a Rec. 709 sequence; [`crate::colorman::gpu_input`]): run on the GPU as on the
    /// CPU. None: the default decode.
    pub input: Option<Arc<waltcut_color::InputTransform>>,
}

impl PlanLayer {
    /// A layer drawing `frame` directly.
    pub fn new(frame: Arc<VideoFrame>, matrix: Affine, opacity: f32, blend: Blend) -> Self {
        Self { frame, matrix, opacity, blend, input: None }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.frame.width, self.frame.height)
    }
}

#[derive(Clone)]
pub enum FramePlan {
    /// Draw these layers over black.
    Layers { width: usize, height: usize, layers: Vec<PlanLayer> },
    /// The CPU produced the final image (fallback).
    Image(crate::Image),
}

impl FramePlan {
    /// Source storage retained by the plan, before GPU preparation.
    pub fn source_bytes(&self) -> usize {
        let layers = |ls: &[PlanLayer]| ls.iter().map(|l| l.frame.byte_size()).sum::<usize>();
        match self {
            Self::Image(img) => img.px.len() * size_of::<f32>(),
            Self::Layers { layers: ls, .. } => layers(ls),
        }
    }

    /// Largest output/source texture side required by the plan.
    pub fn max_side(&self) -> usize {
        let layer_side = |ls: &[PlanLayer]| ls.iter().map(|l| l.frame.width.max(l.frame.height) as usize).max().unwrap_or(0);
        match self {
            Self::Image(img) => img.w.max(img.h),
            Self::Layers { width, height, layers } => (*width).max(*height).max(layer_side(layers)),
        }
    }
}

fn cpu_frame(img: crate::Image) -> Arc<VideoFrame> {
    Arc::new(VideoFrame::rgba_f32(img.w as u32, img.h as u32, img.px))
}

/// Plan the frame at timeline `t`.
pub fn plan_frame(project: &Project, seq_id: ItemId, t: Tick, opts: RenderOptions, sources: &dyn SourceProvider) -> FramePlan {
    let Some(seq) = project.sequence(seq_id) else { return FramePlan::Image(crate::Image::new(1, 1)) };
    let (w, h) = output_size(seq, opts.scale);
    if whole_frame_on_cpu(seq) {
        return FramePlan::Image(crate::render_sequence(project, seq_id, t, opts, sources));
    }
    let mut layers = Vec::new();
    push_tracks(project, seq, t, opts, sources, 0, &mut layers);
    FramePlan::Layers { width: w, height: h, layers }
}

/// Whether [`plan_frame`] hands the frame at `t` back as one CPU image ([`FramePlan::Image`]). A
/// GPU renderer gains nothing on such a frame, so a caller with only a few of them (the export's
/// pool) can render it on the CPU without taking one.
pub fn is_cpu_frame(project: &Project, seq_id: ItemId) -> bool {
    project.sequence(seq_id).is_none_or(whole_frame_on_cpu)
}

/// The frames [`plan_frame`] does not plan: an HDR / wide-gamut sequence composites and converts
/// on the CPU (whole-frame fallback).
fn whole_frame_on_cpu(seq: &Sequence) -> bool {
    !seq.settings.color.is_plain()
}

/// Push the layers of every video track of `seq` at `t`, bottom track first.
/// `nest` counts the nested sequences already followed to reach `seq`.
fn push_tracks(project: &Project, seq: &Sequence, t: Tick, opts: RenderOptions, sources: &dyn SourceProvider, nest: u32, layers: &mut Vec<PlanLayer>) {
    for tr in &seq.video_tracks {
        if !tr.enabled {
            continue;
        }
        let Some(item) = tr.item_at(t) else { continue };
        if !item.enabled || (item.guide && !opts.guides) {
            continue;
        }
        push_item(project, seq, item, t, opts, sources, nest, layers);
    }
}

/// Whether a nested sequence can be drawn by pushing its own layers into the plan of the sequence
/// it is in, instead of being rendered to an image on the CPU first. Compositing its layers one
/// by one over what is below gives the same picture as compositing them together first only when
/// they all blend Normal, and the frame must be one that plans as layers in the same colour
/// pipeline.
fn nest_is_plain(seq: &Sequence, nested: &Sequence, ft: Tick, guides: bool) -> bool {
    nested.settings.color == seq.settings.color
        && (nested.settings.width, nested.settings.height) == (seq.settings.width, seq.settings.height)
        && nested.video_tracks.iter().filter(|tr| tr.enabled).all(|tr| {
            tr.item_at(ft).filter(|i| i.enabled && (guides || !i.guide)).is_none_or(|i| crate::opacity_blend(i, i.effect_time_at(ft)).1 == Blend::Normal)
        })
}

/// Push the layer(s) of `item`. `nest` counts the nested sequences already followed to reach
/// `item` (see [`crate::MAX_NEST_DEPTH`]).
#[allow(clippy::too_many_arguments)]
fn push_item(
    project: &Project,
    seq: &Sequence,
    item: &TrackItem,
    t: Tick,
    opts: RenderOptions,
    sources: &dyn SourceProvider,
    nest: u32,
    out: &mut Vec<PlanLayer>,
) {
    // frame time (`ft`) vs. effect time (`mt`): they differ inside a frame hold without Hold Filters
    let ft = item.source_time_at(t);
    let mt = item.effect_time_at(t);
    let (op, bl) = crate::opacity_blend(item, mt);
    // A nested sequence that is only shown (no effects, untransformed, fully opaque, blending
    // Normal) contributes its own layers: the compositor draws them like the clips of this
    // sequence, with no CPU pass over the nested sequence.
    if let Some(ItemKind::Sequence(nested)) = project.item(item.item).map(|p| &p.kind)
        && nest < crate::MAX_NEST_DEPTH
        && bl == Blend::Normal
        && op >= 1.0 - 1e-6
        && near_identity(&motion_matrix(seq, item, (nested.settings.width, nested.settings.height), Some(nested.settings.par), mt))
        && nest_is_plain(seq, nested, ft, opts.guides)
    {
        push_tracks(project, nested, ft, opts, sources, nest + 1, out);
        return;
    }
    // Graphic clips: their layers are rasterised (cached) into one tight
    // image the GPU places as a layer.
    if project.item(item.item).is_some_and(|p| matches!(p.kind, ItemKind::Graphic { .. })) {
        let Some(size) = crate::source_size(project, item.item) else { return };
        let (w, h) = output_size(seq, opts.scale);
        let m = Affine::scale(opts.scale as f64, opts.scale as f64).then_apply(&motion_matrix(seq, item, size, None, mt));
        if let Some((img, x, y)) = crate::graphic_clip::render_graphic_tight(item, mt, size, &m, w, h) {
            out.push(PlanLayer::new(cpu_frame(img), Affine::translate(x as f64, y as f64), op, bl));
        }
        return;
    }
    if project.item(item.item).is_some_and(|p| matches!(p.kind, ItemKind::Media(_) | ItemKind::Subclip { .. })) {
        let Some(src) = sources.source(item.item) else { return };
        let Some(size) = crate::source_size(project, item.item) else { return };
        let motion = motion_matrix(seq, item, size, crate::source_par(project, item.item), mt);
        let lin = ((motion.a * motion.a + motion.b * motion.b).sqrt()).max((motion.c * motion.c + motion.d * motion.d).sqrt());
        let want = (lin * opts.scale as f64).clamp(1.0 / 64.0, 1.0) as f32;
        let time = crate::video_source_time(item, t, src.info().frame_rate());
        let Ok(frame) = src.video_frame(FrameRequest { time, scale: want }) else { return };
        let cs = crate::colorman::source_space(project, item.item, &frame);
        // HDR and wide-gamut media on a Rec. 709 sequence is converted on the GPU too (#965); log
        // media and other pipelines are converted on the CPU (below), and so are blended
        // in-between frames (Frame Blending / Optical Flow on speed-changed clips)
        let pipe = &seq.settings.color;
        let input = if crate::colorman::needs_management(pipe, cs, &frame) { crate::colorman::gpu_input(project, item.item, pipe, cs, &frame) } else { None };
        let plain =
            (input.is_some() || !crate::colorman::needs_management(pipe, cs, &frame)) && crate::interpolation_blend(item, t, src.info().frame_rate()).is_none();
        if plain {
            // Draft playback at reduced resolution: hand over planes box-filtered to the size
            // drawn instead of the full picture (a quarter at 1/2, a sixteenth at 1/4 of the
            // upload and sampling). The mean is taken over Y'CbCr codes, not linear light as
            // the shader's supersampling does, so this stays limited to the opt-in draft mode.
            let n = if waltcut_media::cancel::draft() { crate::decimation(frame.width as f32, size.0 as f32 * want) } else { 1 };
            let frame = match frame.box_decimated(n) {
                Some(small) => Arc::new(small),
                None => frame,
            };
            let px_scale = frame.width as f64 / size.0.max(1) as f64;
            let m = Affine::scale(opts.scale as f64, opts.scale as f64).then_apply(&motion).then_apply(&Affine::scale(1.0 / px_scale, 1.0 / px_scale));
            out.push(PlanLayer { input, ..PlanLayer::new(frame, m, op, bl) });
            return;
        }
    }
    // Drawing, managed-colour and interpolated layers fall back to a CPU-rendered image.
    if let Some((img, op2, _)) = crate::item_layer(project, seq, item, t, opts, sources) {
        out.push(PlanLayer::new(cpu_frame(img), Affine::IDENTITY, op2, bl));
    }
}

fn near_identity(m: &Affine) -> bool {
    (m.a - 1.0).abs() < 1e-9 && (m.d - 1.0).abs() < 1e-9 && m.b.abs() < 1e-9 && m.c.abs() < 1e-9 && m.e.abs() < 1e-6 && m.f.abs() < 1e-6
}

/// Execute a plan on the CPU (reference for the GPU compositor).
pub fn execute_cpu(plan: &FramePlan) -> crate::Image {
    match plan {
        FramePlan::Image(img) => img.clone(),
        FramePlan::Layers { width, height, layers } => {
            let mut canvas = crate::Image::new(*width, *height);
            execute_layers(&mut canvas, layers);
            canvas
        }
    }
}

/// A layer's frame (box-decimated by `n`) as a linear working image: the default decode, or
/// `input`'s table and pixel stage as `colorman::decode` runs them.
fn linear_image(frame: &VideoFrame, n: usize, input: Option<&waltcut_color::InputTransform>) -> crate::Image {
    let (w, h, px) = frame.to_linear_f32_decimated_with(n, input.map(|t| &t.table));
    let mut img = crate::Image { w, h, px };
    if let Some(t) = input.filter(|t| t.has_pixel_stage()) {
        img.map_rgb(|c, _, _| t.apply(c));
    }
    img
}

fn execute_layers(canvas: &mut crate::Image, layers: &[PlanLayer]) {
    let (width, height) = (canvas.w, canvas.h);
    for l in layers {
        let src = linear_image(&l.frame, 1, l.input.as_deref());
        let placed = if l.matrix == Affine::scale(width as f64, height as f64) && src.w == 1 && src.h == 1 {
            crate::Image::filled(width, height, src.get(0, 0))
        } else {
            src.transformed(width, height, &l.matrix)
        };
        crate::blend::composite(canvas, &placed, l.opacity, l.blend);
    }
}
