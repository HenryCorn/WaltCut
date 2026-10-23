//! Golden-image tests: procedural projects rendered through the CPU compositor and compared with
//! committed reference PNGs (`goldens/*.png`, made by this test with `WALTCUT_BLESS=1`), plus
//! GPU-vs-CPU parity on the same scenes when a GPU adapter is available.
//!
//! Tolerances (8-bit sRGB levels, see `waltcut_testkit::golden`):
//! - CPU vs golden: `Tolerance::RENDER` — PSNR ≥ 45 dB, max abs ≤ 12, 99th percentile ≤ 2.
//! - GPU vs CPU: 99th percentile of the per-pixel max channel difference ≤ 6 and mean < 1.5
//!   (antialiased edges and half-float textures differ slightly; the CPU is the reference).

use std::path::PathBuf;
use std::sync::Arc;

use waltcut_color::{ColorInfo, ColorSpace, Gamut, Primaries, Transfer};
use waltcut_geom::Vec2;
use waltcut_media::generators::GeneratorSource;
use waltcut_media::{DemoScene, Generator, MediaSource, SharedSource};
use waltcut_project::{ClipId, EffectInstance, ItemId, ItemKind, Label, MediaClip, MediaRef, ParamValue, Project, SequenceSettings, TrackItem, TrackKind};
use waltcut_render::{RenderOptions, SourceMap, render_sequence};
use waltcut_testkit::golden::{Rgba8, Tolerance, assert_golden, diff};
use waltcut_time::{FrameRate, TICKS_PER_SECOND, Tick, TimeRange};

const W: u32 = 320;
const H: u32 = 180;
const RATE: FrameRate = FrameRate::FPS_24;

/// A procedurally generated project and the frame to render.
struct Scene {
    project: Project,
    seq: ItemId,
    sources: SourceMap,
    frame: i64,
}

struct Builder {
    p: Project,
    map: SourceMap,
    seq: ItemId,
}

impl Builder {
    fn new(video_tracks: usize) -> Self {
        let mut p = Project::new("golden");
        let seq = p.new_sequence("golden", SequenceSettings { width: W, height: H, frame_rate: RATE, ..Default::default() }, video_tracks, 0, None);
        Builder { p, map: SourceMap::default(), seq }
    }

    /// A generated media item (640×360, 24 fps, 10 s).
    fn media(&mut self, g: Generator) -> ItemId {
        let src = GeneratorSource::new(g, 640, 360, RATE, Tick(10 * TICKS_PER_SECOND));
        let info = src.info().clone();
        let id = self.p.add_item(
            &info.name.clone(),
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(src.generator.clone()),
                info,
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                identity: None,
            }),
            None,
        );
        self.map.0.insert(id, Arc::new(src) as SharedSource);
        id
    }

    fn demo(&mut self, scene: DemoScene) -> ItemId {
        self.media(Generator::Demo(scene))
    }

    /// Demo footage re-encoded as camera log / HDR (see [`Encoded`]), with an optional Interpret
    /// Footage colour-space override.
    fn encoded(&mut self, scene: DemoScene, target: ColorSpace, gain: f32, flag: Option<ColorInfo>, interpret: Option<ColorSpace>) -> ItemId {
        let id = self.demo(scene);
        let inner = self.map.0.remove(&id).unwrap();
        self.map.0.insert(id, Arc::new(Encoded { inner, target, gain, flag }) as SharedSource);
        if let ItemKind::Media(m) = &mut self.p.item_mut(id).unwrap().kind {
            m.interpret.color_space = interpret;
        }
        id
    }

    /// Place `item` on video track `track` (frames), scaled to the frame size.
    fn place(&mut self, track: usize, item: ItemId, start: i64, dur: i64) -> ClipId {
        let mut ti = self.p.make_track_item(item, TrackKind::Video, RATE.tick_of(start), TimeRange::new(Tick::ZERO, RATE.tick_of(dur)), RATE).unwrap();
        ti.scale_to_frame = true;
        let id = ti.id;
        let s = self.p.sequence_mut(self.seq).unwrap();
        s.video_tracks[track].items.push(ti);
        s.video_tracks[track].sort();
        id
    }

    fn clip(&mut self, id: ClipId) -> &mut TrackItem {
        self.p.sequence_mut(self.seq).unwrap().find_item_mut(id).unwrap().1
    }

    /// Set intrinsic-effect parameters (`motion`, `opacity`).
    fn fixed(&mut self, id: ClipId, effect: &str, params: &[(&str, ParamValue)]) {
        let e = self.clip(id).effect_mut(effect).unwrap();
        set(e, params);
    }

    fn at(self, frame: i64) -> Scene {
        Scene { project: self.p, seq: self.seq, sources: self.map, frame }
    }
}

fn set(e: &mut EffectInstance, params: &[(&str, ParamValue)]) {
    let id = e.effect.clone();
    for (k, v) in params {
        e.param_mut(k).unwrap_or_else(|| panic!("{id}: no param {k}")).value = v.clone();
    }
}

fn fl(v: f64) -> ParamValue {
    ParamValue::Float(v)
}

fn pt(x: f64, y: f64) -> ParamValue {
    ParamValue::Vec2(Vec2::new(x, y))
}

fn blend(name: &str) -> ParamValue {
    ParamValue::Choice(waltcut_project::effect::BLEND_MODES.iter().position(|b| *b == name).unwrap_or_else(|| panic!("blend {name}")) as u32)
}

/// A source that re-encodes another source's (sRGB) frames into a camera log or HDR colour space:
/// linear BT.709 × `gain` → `target` gamut → `target` curve, stored as full-range RGBA8. `flag`
/// replaces the frame's colour metadata (e.g. PQ/BT.2020, so auto-detection kicks in).
struct Encoded {
    inner: SharedSource,
    target: ColorSpace,
    gain: f32,
    flag: Option<ColorInfo>,
}

impl MediaSource for Encoded {
    fn info(&self) -> &waltcut_media::MediaInfo {
        self.inner.info()
    }
    fn video_frame(&self, req: waltcut_media::FrameRequest) -> waltcut_media::Result<Arc<waltcut_frame::VideoFrame>> {
        let f = self.inner.video_frame(req)?;
        let rgba = f.to_rgba8();
        let m = waltcut_color::spaces::to_f32(&waltcut_color::spaces::gamut_matrix(Gamut::Bt709, self.target.gamut()));
        let curve = self.target.curve();
        let out: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let lin = [0, 1, 2].map(|k| waltcut_color::srgb_to_linear(p[k] as f32 / 255.0) * self.gain);
                let c = waltcut_color::spaces::apply3(&m, lin);
                let e =
                    c.map(|v| (waltcut_color::transform::encode_channel(v as f64, curve, waltcut_color::Range::Full).clamp(0.0, 1.0) * 255.0).round() as u8);
                [e[0], e[1], e[2], p[3]]
            })
            .collect();
        let mut nf = waltcut_frame::VideoFrame::rgba8(f.width, f.height, out);
        if let Some(c) = self.flag {
            nf.color = c;
        }
        Ok(Arc::new(nf))
    }
    fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> waltcut_media::Result<waltcut_frame::AudioBuffer> {
        self.inner.audio(start, frames, sample_rate)
    }
}

// ---- scenes ----

/// Motion (scale, rotation, position) and 70 % opacity over a full-frame background.
fn transform_opacity() -> Scene {
    let mut b = Builder::new(2);
    let bg = b.demo(DemoScene::OceanSunset);
    let fg = b.demo(DemoScene::Aurora);
    b.place(0, bg, 0, 48);
    let top = b.place(1, fg, 0, 48);
    b.fixed(top, "motion", &[("scale", fl(45.0)), ("rotation", fl(20.0)), ("position", pt(215.0, 70.0))]);
    b.fixed(top, "opacity", &[("opacity", fl(70.0))]);
    b.at(12)
}

/// Four quadrant layers over a background in Multiply, Screen, Overlay and Difference.
fn blend_modes() -> Scene {
    let mut b = Builder::new(5);
    let bg = b.demo(DemoScene::OceanSunset);
    let fg = b.demo(DemoScene::Dunes);
    b.place(0, bg, 0, 48);
    for (i, (mode, x, y)) in [("Multiply", 80.0, 45.0), ("Screen", 240.0, 45.0), ("Overlay", 80.0, 135.0), ("Difference", 240.0, 135.0)].into_iter().enumerate()
    {
        let c = b.place(i + 1, fg, 0, 48);
        b.fixed(c, "motion", &[("scale", fl(50.0)), ("position", pt(x, y))]);
        b.fixed(c, "opacity", &[("blend", blend(mode))]);
    }
    b.at(12)
}

/// A graphic clip on V2 over `bg` on V1, with `layers` (graphic layer effect instances).
fn graphic_scene(bg: DemoScene, layers: Vec<EffectInstance>) -> Scene {
    let mut b = Builder::new(2);
    let bgi = b.demo(bg);
    b.place(0, bgi, 0, 48);
    let g = b.p.add_item("Graphic", Label::Rose, ItemKind::Graphic { width: W, height: H, rate: RATE }, None);
    let c = b.place(1, g, 0, 48);
    for mut l in layers {
        waltcut_project::resolve_auto_points(&mut l, (W, H), (W, H));
        b.clip(c).effects.push(l);
    }
    b.at(12)
}

/// Per-character styles (a bold, bigger, coloured word and an underlined one) and a rounded box
/// pinned around the text on all four edges.
fn graphic_rich_text() -> Scene {
    use waltcut_project::graphic::{new_shape_layer, new_text_layer};
    use waltcut_project::graphic_design::{CharStyle, LayerExtra, Pin, PinTarget, StyleRun};
    let mut text = new_text_layer("Mixed STYLES in one layer", Vec2::new(28.0, 100.0), 16.0);
    let runs = vec![
        StyleRun { start: 6, end: 12, style: CharStyle { size: Some(26.0), faux_bold: Some(true), fill: Some([1.0, 0.8, 0.1, 1.0]), ..Default::default() } },
        StyleRun {
            start: 16,
            end: 19,
            style: CharStyle { underline: Some(true), font: Some("Noto Serif".into()), faux_italic: Some(true), ..Default::default() },
        },
    ];
    text.layer = Some(Box::new(LayerExtra { uid: 1, runs, ..Default::default() }));
    let mut bx = new_shape_layer(0, Vec2::new(150.0, 90.0), Vec2::new(10.0, 10.0), vec![]);
    set(&mut bx, &[("corner_radius", fl(6.0)), ("fill_color", ParamValue::Color([0.1, 0.12, 0.3, 1.0])), ("opacity", fl(80.0))]);
    let pin = Pin { to: PinTarget::Layer(1), left: true, top: true, right: true, bottom: true, offsets: [-10.0, -8.0, 10.0, 8.0] };
    bx.layer = Some(Box::new(LayerExtra { uid: 2, pin: Some(pin), ..Default::default() }));
    graphic_scene(DemoScene::Dunes, vec![bx, text])
}

/// A title: bold centred text with an outer stroke and a soft drop shadow, a lower-third bar with
/// rounded corners and a background-boxed caption line.
fn graphic_title() -> Scene {
    use waltcut_project::graphic::{new_shape_layer, new_text_layer};
    let mut bar = new_shape_layer(0, Vec2::new(160.0, 150.0), Vec2::new(280.0, 34.0), vec![]);
    set(&mut bar, &[("corner_radius", fl(8.0)), ("fill_color", ParamValue::Color([0.12, 0.35, 0.8, 1.0])), ("opacity", fl(85.0))]);
    let mut title = new_text_layer("Night Drive", Vec2::new(160.0, 80.0), 44.0);
    set(
        &mut title,
        &[
            ("font_style", ParamValue::Text("Bold".into())),
            ("align", ParamValue::Choice(1)),
            ("stroke", ParamValue::Bool(true)),
            ("stroke_width", fl(2.5)),
            ("stroke_color", ParamValue::Color([0.05, 0.05, 0.1, 1.0])),
            ("shadow", ParamValue::Bool(true)),
            ("shadow_distance", fl(5.0)),
            ("shadow_blur", fl(8.0)),
            ("tracking", fl(40.0)),
        ],
    );
    let mut sub = new_text_layer("Directed by Nobody", Vec2::new(160.0, 156.0), 16.0);
    set(
        &mut sub,
        &[
            ("align", ParamValue::Choice(1)),
            ("font", ParamValue::Text("Noto Serif".into())),
            ("faux_italic", ParamValue::Bool(true)),
            ("caps", ParamValue::Choice(2)),
            ("fill_color", ParamValue::Color([1.0, 0.92, 0.6, 1.0])),
        ],
    );
    graphic_scene(DemoScene::CityNight, vec![bar, title, sub])
}

/// Shapes: ellipse with centre stroke, rotated polygon with inner stroke, a pen path, rotated
/// boxed text with a background and 2 strokes.
fn graphic_shapes() -> Scene {
    use waltcut_project::graphic::{new_shape_layer, new_text_layer};
    let mut ell = new_shape_layer(1, Vec2::new(70.0, 60.0), Vec2::new(100.0, 70.0), vec![]);
    set(
        &mut ell,
        &[
            ("stroke", ParamValue::Bool(true)),
            ("stroke_width", fl(6.0)),
            ("stroke_type", ParamValue::Choice(1)),
            ("stroke_color", ParamValue::Color([1.0, 1.0, 1.0, 1.0])),
        ],
    );
    let mut poly = new_shape_layer(2, Vec2::new(250.0, 60.0), Vec2::new(80.0, 80.0), vec![]);
    set(
        &mut poly,
        &[
            ("sides", fl(5.0)),
            ("rotation", fl(18.0)),
            ("fill_color", ParamValue::Color([0.2, 0.8, 0.4, 1.0])),
            ("stroke", ParamValue::Bool(true)),
            ("stroke_type", ParamValue::Choice(2)),
            ("stroke_width", fl(5.0)),
        ],
    );
    let path = new_shape_layer(3, Vec2::new(70.0, 140.0), Vec2::new(0.0, 0.0), vec![[-40.0, 20.0], [0.0, -25.0], [40.0, 20.0], [0.0, 5.0]]);
    let mut txt = new_text_layer("Rotated\nTwo lines", Vec2::new(215.0, 140.0), 20.0);
    set(
        &mut txt,
        &[
            ("rotation", fl(-12.0)),
            ("align", ParamValue::Choice(1)),
            ("background", ParamValue::Bool(true)),
            ("background_radius", fl(6.0)),
            ("background_size", fl(6.0)),
            ("stroke", ParamValue::Bool(true)),
            ("stroke_width", fl(1.5)),
            ("stroke2", ParamValue::Bool(true)),
            ("stroke2_width", fl(3.5)),
        ],
    );
    graphic_scene(DemoScene::Aurora, vec![ell, poly, path, txt])
}

/// Camera log → Rec. 709: Ocean Sunset encoded as S-Log3 / S-Gamut3.Cine one stop over, then
/// interpreted as such in a Rec. 709 sequence (decode, gamut compression, BT.2390 tone mapping).
fn log_to_rec709() -> Scene {
    let mut b = Builder::new(1);
    let src = b.encoded(DemoScene::OceanSunset, ColorSpace::SLog3SGamut3Cine, 2.0, None, Some(ColorSpace::SLog3SGamut3Cine));
    b.place(0, src, 0, 48);
    b.at(12)
}

/// HDR → SDR: City Night as Rec. 2100 PQ with highlights up to ~1200 cd/m² (×6 over reference
/// white), detected from the frame's metadata and tone mapped into a Rec. 709 sequence.
fn hdr_tone_map() -> Scene {
    let mut b = Builder::new(1);
    let pq = ColorInfo { transfer: Transfer::Pq, primaries: Primaries::Bt2020, ..ColorInfo::SRGB_FULL };
    let src = b.encoded(DemoScene::CityNight, ColorSpace::Rec2100Pq, 6.0, Some(pq), None);
    b.place(0, src, 0, 48);
    b.at(12)
}

/// (name, title, scene).
fn scenes() -> Vec<(&'static str, &'static str, fn() -> Scene)> {
    vec![
        ("transform_opacity", "Motion transform and opacity", transform_opacity),
        ("blend_modes", "Blend modes (Multiply, Screen, Overlay, Difference)", blend_modes),
        ("graphic_title", "Graphic clip: title text with stroke and shadow, lower-third bar", graphic_title),
        ("graphic_shapes", "Graphic clip: ellipse, polygon, path and rotated text with background", graphic_shapes),
        ("graphic_rich_text", "Per-character text styles and a box pinned around the text", graphic_rich_text),
        ("log_to_rec709", "Colour management: S-Log3/S-Gamut3.Cine footage to Rec. 709 (tone mapped)", log_to_rec709),
        ("hdr_tone_map", "Colour management: Rec. 2100 PQ footage tone mapped into a Rec. 709 sequence", hdr_tone_map),
    ]
}

fn render_cpu(s: &Scene) -> Rgba8 {
    let img = render_sequence(&s.project, s.seq, RATE.tick_of(s.frame), RenderOptions::default(), &s.sources);
    Rgba8::new(img.w as u32, img.h as u32, img.over_black_rgba8())
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("goldens").join(format!("{name}.png"))
}

fn check(name: &str) {
    let (_, title, make) = scenes().into_iter().find(|(n, _, _)| *n == name).unwrap();
    let img = render_cpu(&make());
    assert_eq!((img.w, img.h), (W, H));
    let d = assert_golden(&golden_path(name), &img, Tolerance::RENDER, title, &format!("crates/golden/tests/golden.rs ({name})"));
    eprintln!("{name}: {d}");
}

macro_rules! goldens {
    ($($name:ident),* $(,)?) => {
        mod cpu {
            $(
                #[test]
                fn $name() {
                    super::check(stringify!($name));
                }
            )*
        }

        #[test]
        fn every_scene_has_a_test() {
            let tested = [$(stringify!($name)),*];
            for (n, _, _) in scenes() {
                assert!(tested.contains(&n), "scene {n} has no golden test");
            }
        }
    };
}

goldens!(transform_opacity, blend_modes, graphic_title, graphic_shapes, graphic_rich_text, log_to_rec709, hdr_tone_map,);

/// Sanity: the scenes are not trivially empty or identical to each other.
#[test]
fn scenes_are_distinct() {
    let imgs: Vec<(&str, Rgba8)> = scenes().into_iter().map(|(n, _, f)| (n, render_cpu(&f()))).collect();
    for (n, img) in &imgs {
        let lit = img.px.as_chunks::<4>().0.iter().filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30).count();
        assert!(lit > img.px.len() / 4 / 4, "{n}: mostly black");
    }
    for i in 0..imgs.len() {
        for j in i + 1..imgs.len() {
            let d = diff(&imgs[i].1, &imgs[j].1).unwrap();
            assert!(d.psnr < 30.0, "{} and {} are near-identical ({d})", imgs[i].0, imgs[j].0);
        }
    }
}

// ---- GPU parity ----

/// A device for the GPU parity tests, or `None` (with a `SKIPPED` note) when the machine has no
/// usable adapter.
fn device(context: &str) -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let Some(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok() else {
        eprintln!("SKIPPED ({context}): no GPU adapter");
        return None;
    };
    let name = adapter.get_info().name;
    match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
        Ok(device) => Some(device),
        Err(_) => {
            eprintln!("SKIPPED ({context}): adapter {name} refused a device");
            None
        }
    }
}

#[test]
fn gpu_matches_cpu_on_golden_scenes() {
    let Some((dev, q)) = device("gpu parity") else {
        return;
    };
    let mut c = waltcut_gpu::GpuCompositor::new(&dev, &q);
    let mut failures = Vec::new();
    for (name, _, make) in scenes() {
        let s = make();
        let t = RATE.tick_of(s.frame);
        let cpu = render_cpu(&s);
        let plan = waltcut_render::plan::plan_frame(&s.project, s.seq, t, RenderOptions::default(), &s.sources);
        let kind = if matches!(plan, waltcut_render::plan::FramePlan::Layers { .. }) { "layers" } else { "cpu image" };
        c.composite(&plan);
        let (gw, gh, mut px) = c.read_output().expect("GPU readback");
        px.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
        let gpu = Rgba8::new(gw, gh, px);
        let d = diff(&cpu, &gpu).unwrap();
        // mean over RGB only (alpha is equal by construction)
        let mean_rgb = d.mean_abs * 4.0 / 3.0;
        eprintln!("{name} ({kind}): GPU vs CPU {d}");
        if d.p99 > 6 || mean_rgb >= 1.5 {
            let dir = waltcut_testkit::golden::failures_dir();
            let _ = waltcut_testkit::golden::write_png(&dir.join(format!("{name}.gpu.png")), &gpu);
            let _ = waltcut_testkit::golden::write_png(&dir.join(format!("{name}.cpu.png")), &cpu);
            failures.push(format!("{name} ({kind}): {d}; images in {}", dir.display()));
        }
    }
    assert!(failures.is_empty(), "GPU differs from the CPU reference:\n{}", failures.join("\n"));
}

/// Per-pixel max RGB difference (99th percentile, mean) over the pixels at least 1.5 px from every
/// plan layer's quad outline: the CPU resampler fades a layer's edge over one more pixel than the
/// GPU rasterises, and with blend modes those edge pixels are a visible share of a 320×180 frame.
fn interior_diff(plan: &waltcut_render::plan::FramePlan, a: &Rgba8, b: &Rgba8) -> (u32, f64) {
    let waltcut_render::plan::FramePlan::Layers { width, layers, .. } = plan else { return (u32::MAX, f64::MAX) };
    let inv: Vec<_> = layers.iter().filter_map(|l| Some((l.matrix.inverse()?, l.size().0 as f64, l.size().1 as f64))).collect();
    let mut d: Vec<u32> = Vec::new();
    for (i, (pa, pb)) in a.px.chunks(4).zip(b.px.chunks(4)).enumerate() {
        let (x, y) = ((i % width) as f64 + 0.5, (i / width) as f64 + 0.5);
        let edge = inv.iter().any(|(m, fw, fh)| {
            let c = [(-1.5, -1.5), (1.5, -1.5), (-1.5, 1.5), (1.5, 1.5)].map(|(dx, dy)| {
                let p = m.apply(Vec2::new(x + dx, y + dy));
                p.x >= 0.0 && p.y >= 0.0 && p.x <= *fw && p.y <= *fh
            });
            c.iter().any(|v| *v != c[0])
        });
        if !edge {
            d.push((0..3).map(|k| (pa[k] as i32 - pb[k] as i32).unsigned_abs()).max().unwrap_or(0));
        }
    }
    d.sort_unstable();
    (d[d.len() * 99 / 100], d.iter().sum::<u32>() as f64 / d.len() as f64)
}

/// Every blend mode composited by the GPU from `plan_frame` (no CPU fallback): two overlapping
/// transformed clips at partial opacity in the mode over footage, plus a third scaled clip in
/// the same mode, match the CPU render with the
/// GPU parity tolerance (p99 ≤ 6, mean < 1.5) away from layer outlines; whole-frame numbers are
/// printed too.
#[test]
fn gpu_blend_modes_match_cpu_render() {
    let Some((dev, q)) = device("gpu blend parity") else {
        return;
    };
    let mut c = waltcut_gpu::GpuCompositor::new(&dev, &q);
    let mut failures = Vec::new();
    for mode in waltcut_project::effect::BLEND_MODES {
        let mut b = Builder::new(4);
        let bg = b.demo(DemoScene::OceanSunset);
        let (dunes, aurora, bars) = (b.demo(DemoScene::Dunes), b.demo(DemoScene::Aurora), b.media(Generator::BarsAndTone));
        b.place(0, bg, 0, 48);
        let v2 = b.place(1, dunes, 0, 48);
        b.fixed(v2, "motion", &[("scale", fl(60.0)), ("rotation", fl(12.0)), ("position", pt(130.0, 80.0))]);
        b.fixed(v2, "opacity", &[("opacity", fl(70.0)), ("blend", blend(mode))]);
        let v3 = b.place(2, aurora, 0, 48);
        b.fixed(v3, "motion", &[("scale", fl(45.0)), ("rotation", fl(-20.0)), ("position", pt(210.0, 110.0))]);
        b.fixed(v3, "opacity", &[("opacity", fl(60.0)), ("blend", blend(mode))]);
        let v4 = b.place(3, bars, 0, 48);
        b.fixed(v4, "motion", &[("scale", fl(30.0)), ("position", pt(250.0, 50.0))]);
        b.fixed(v4, "opacity", &[("opacity", fl(85.0)), ("blend", blend(mode))]);
        let s = b.at(12);
        let t = RATE.tick_of(s.frame);
        let cpu = render_cpu(&s);
        let plan = waltcut_render::plan::plan_frame(&s.project, s.seq, t, RenderOptions::default(), &s.sources);
        let waltcut_render::plan::FramePlan::Layers { layers, .. } = &plan else {
            failures.push(format!("{mode}: planned as a CPU image"));
            continue;
        };
        assert_eq!(layers.len(), 4, "{mode}");
        c.composite(&plan);
        let (gw, gh, mut px) = c.read_output().expect("GPU readback");
        px.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
        let gpu = Rgba8::new(gw, gh, px);
        let d = diff(&cpu, &gpu).unwrap();
        let (p99, mean) = interior_diff(&plan, &cpu, &gpu);
        eprintln!("{mode}: GPU vs CPU interior p99 {p99}, mean {mean:.3}; whole frame {d}");
        if p99 > 6 || mean >= 1.5 {
            failures.push(format!("{mode}: interior p99 {p99}, mean {mean}; {d}"));
        }
    }
    assert!(failures.is_empty(), "GPU blend modes differ from the CPU render:\n{}", failures.join("\n"));
}
