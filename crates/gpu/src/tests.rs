use super::*;
use waltcut_geom::{Affine, Vec2};
use waltcut_render::Blend;
use waltcut_render::plan::execute_cpu;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

#[test]
fn yuv_code_range_uniforms() {
    let mut mismatches = Vec::new();
    for bits in [8, 10, 12, 16] {
        let levels = (1u32 << bits) as f32;
        for range in [Range::Full, Range::Limited] {
            let src = SrcInfo {
                kind: 2,
                code_scale: if bits == 8 { 255.0 } else { levels },
                code_levels: levels,
                alpha: 0.0,
                chroma: (1, 1),
                size: (1, 1),
                color: waltcut_color::ColorInfo { range, ..waltcut_color::ColorInfo::REC709 },
                input: None,
            };
            let u = GpuCompositor::uniforms(&src, &Affine::IDENTITY, 1.0, Blend::Normal, (1, 1));
            // Full: 0..2^bits-1, neutral 2^(bits-1). Limited: BT.709 8-bit codes * 2^(bits-8).
            let expected = match range {
                Range::Full => [0.0, levels - 1.0, levels / 2.0, levels - 1.0],
                Range::Limited => [16.0, 219.0, 128.0, 224.0].map(|v| v * (1u32 << (bits - 8)) as f32),
            };
            if u[16..20] != expected {
                mismatches.push(format!("{bits}-bit {range:?}: {:?} != {expected:?}", &u[16..20]));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn yuv_code_range_gpu_matches_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let mut prepared_compositor = GpuCompositor::new(&dev, &q);
    for bits in [8, 10, 12, 16] {
        let scale = 1u32 << (bits - 8);
        for range in [Range::Full, Range::Limited] {
            // Constant chroma isolates range normalization from CPU/GPU chroma reconstruction.
            for (u, v) in [(128, 128), (64, 192), (192, 64)] {
                let y = (0..=255).map(|code| (code * scale) as u16).collect::<Vec<_>>();
                let planes = [Arc::new(y), Arc::new(vec![(u * scale) as u16; 256]), Arc::new(vec![(v * scale) as u16; 256])];
                let data = if bits == 8 {
                    PixelData::Yuv8 { planes: planes.map(|p| Arc::new(p.iter().map(|v| *v as u8).collect())), chroma: Chroma::C444, alpha: None }
                } else {
                    PixelData::Yuv16 { planes, chroma: Chroma::C444, bits, alpha: None }
                };
                let frame = Arc::new(VideoFrame {
                    width: 256,
                    height: 1,
                    data,
                    color: waltcut_color::ColorInfo { range, ..waltcut_color::ColorInfo::REC709 },
                    par: (1, 1),
                    pts: Default::default(),
                });
                let cpu = frame.to_rgba8();
                let plan = FramePlan::Layers { width: 256, height: 1, layers: vec![PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal)] };
                c.composite(&plan);
                let (w, h, gpu) = c.read_output().expect("readback");
                assert_eq!((w, h, gpu.len()), (256, 1, cpu.len()));
                let error = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().expect("pixels");
                // One output-code tolerance: CPU f32 vs GPU f16 accumulator quantization.
                assert!(error <= 1, "{bits}-bit {range:?} U={u} V={v}: max error {error}");
                if u == 128 && v == 128 {
                    assert!(gpu.as_chunks::<4>().0.iter().all(|p| p[0] == p[1] && p[1] == p[2]), "neutral tint: {bits}-bit {range:?}");
                }
                let prepared = prepare(&plan);
                prepared_compositor.composite_prepared(&plan, Some(&prepared));
                assert_eq!(prepared_compositor.read_output().expect("prepared readback").2, gpu, "prepared: {bits}-bit {range:?}");
                eprintln!("range gate: {bits}-bit {range:?} U={u} V={v}, max error={error}, prepared=inline");
            }
        }
    }
}

fn yuv_frame(w: u32, h: u32) -> Arc<VideoFrame> {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y: Vec<u8> = (0..w * h).map(|i| (16 + ((i % w) * 219 / w)) as u8).collect();
    let u: Vec<u8> = (0..cw * ch).map(|i| (64 + (i / cw) * 128 / ch) as u8).collect();
    let v: Vec<u8> = (0..cw * ch).map(|i| (200 - (i % cw) * 100 / cw) as u8).collect();
    Arc::new(VideoFrame {
        width: w,
        height: h,
        data: PixelData::Yuv8 { planes: [Arc::new(y), Arc::new(u), Arc::new(v)], chroma: Chroma::C420, alpha: None },
        color: waltcut_color::ColorInfo::REC709,
        par: (1, 1),
        pts: Default::default(),
    })
}

#[test]
fn gpu_matches_cpu_plan() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let pixels = waltcut_media::generators::render(
        &waltcut_media::Generator::Demo(waltcut_media::DemoScene::OceanSunset),
        320,
        180,
        1.0,
        24,
        waltcut_time::FrameRate::FPS_24,
    );
    let rgba = Arc::new(VideoFrame::rgba8(320, 180, pixels));
    let (w, h) = (320usize, 180usize);
    let plan = FramePlan::Layers {
        width: w,
        height: h,
        layers: vec![
            PlanLayer { frame: yuv_frame(640, 360), matrix: Affine::scale(0.5, 0.5), opacity: 1.0, blend: Blend::Normal, input: None },
            PlanLayer {
                frame: rgba,
                matrix: Affine::motion(Vec2::new(200.0, 100.0), Vec2::new(0.4, 0.4), 12.0, Vec2::new(160.0, 90.0)),
                opacity: 0.7,
                blend: Blend::Normal,
                input: None,
            },
        ],
    };
    let cpu = execute_cpu(&plan).over_black_rgba8();
    c.composite(&plan);
    let (gw, gh, gpu) = c.read_output().expect("readback");
    assert_eq!((gw as usize, gh as usize), (w, h));
    // compare away from antialiased edges: mean abs error and 99th percentile
    let mut diffs: Vec<u32> =
        cpu.chunks(4).zip(gpu.chunks(4)).map(|(a, b)| (0..3).map(|k| (a[k] as i32 - b[k] as i32).unsigned_abs()).max().unwrap_or(0)).collect();
    diffs.sort_unstable();
    let p99 = diffs[diffs.len() * 99 / 100];
    let mean = diffs.iter().sum::<u32>() as f64 / diffs.len() as f64;
    assert!(p99 <= 6 && mean < 1.5, "p99 {p99}, mean {mean}");
    // cache: compositing the same plan again uploads nothing
    let before = c.uploaded_bytes;
    c.composite(&plan);
    assert_eq!(c.uploaded_bytes, before);
}

#[test]
fn half_float_conversion() {
    for v in [0.0f32, 1.0, 0.5, 0.123, 65504.0, -2.0, 1e-5] {
        let h = f32_to_f16(v);
        // decode
        let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
        let e = ((h >> 10) & 0x1f) as i32;
        let m = (h & 0x3ff) as f32;
        let d = if e == 0 { s * m * 2f32.powi(-24) } else { s * (1.0 + m / 1024.0) * 2f32.powi(e - 15) };
        assert!((d - v).abs() <= v.abs() * 1e-3 + 1e-6, "{v} → {d}");
    }
}

#[test]
fn prepared_upload_matches_inline_conversion() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (w, h) = (64u32, 36u32);
    let yuv16 = Arc::new(VideoFrame {
        width: w,
        height: h,
        data: PixelData::Yuv16 {
            planes: [
                Arc::new((0..w * h).map(|i| (64 + (i * 7) % 876) as u16).collect()),
                Arc::new((0..w * h / 2).map(|i| (64 + (i * 13) % 896) as u16).collect()),
                Arc::new((0..w * h / 2).map(|i| (960 - (i * 5) % 896) as u16).collect()),
            ],
            chroma: Chroma::C422,
            bits: 10,
            alpha: None,
        },
        color: waltcut_color::ColorInfo::REC709,
        par: (1, 1),
        pts: Default::default(),
    });
    let f32_layer: Vec<f32> = (0..w * h * 4).map(|i| ((i * 37) % 1000) as f32 / 1000.0 * if i % 4 == 3 { 1.0 } else { 0.6 }).collect();
    let rgbaf = Arc::new(VideoFrame::rgba_f32(w, h, f32_layer.clone()));
    let layers = FramePlan::Layers {
        width: w as usize,
        height: h as usize,
        layers: vec![
            PlanLayer { frame: yuv16, matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, input: None },
            PlanLayer { frame: rgbaf, matrix: Affine::scale(0.5, 0.5), opacity: 0.8, blend: Blend::Normal, input: None },
        ],
    };
    let image = FramePlan::Image(waltcut_render::Image { w: w as usize, h: h as usize, px: f32_layer });
    for plan in [layers, image] {
        let mut a = GpuCompositor::new(&dev, &q);
        a.composite(&plan);
        let inline = a.read_output().expect("readback");
        let mut b = GpuCompositor::new(&dev, &q);
        let prep = prepare(&plan);
        assert!(prep.bytes() > 0);
        b.composite_prepared(&plan, Some(&prep));
        let prepared = b.read_output().expect("readback");
        assert!(inline == prepared, "prepared upload differs");
    }
}

/// The upload cache is keyed by pixel-buffer address, so it must keep the buffer alive: otherwise
/// a new frame allocated at a freed frame's address is drawn with the stale texture (seen as
/// whole frames from a previous composite in `crates/golden` GPU parity).
#[test]
fn upload_cache_keeps_buffers_alive() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let px = Arc::new(vec![200u8; 16 * 8 * 4]);
    let frame = Arc::new(VideoFrame { width: 16, height: 8, data: PixelData::Rgba8(px.clone()), ..(*yuv_frame(16, 8)).clone() });
    let plan = FramePlan::Layers {
        width: 16,
        height: 8,
        layers: vec![PlanLayer { frame, matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, input: None }],
    };
    c.composite(&plan);
    drop(plan);
    assert!(Arc::strong_count(&px) > 1, "cached upload must own its pixel buffer");
}

/// A 4:4:4 grey picture with an alpha plane that ramps from transparent (left) to opaque (right).
fn yuv_alpha_frame(w: u32, h: u32, bits: u32) -> Arc<VideoFrame> {
    let n = (w * h) as usize;
    let max = (1u32 << bits) - 1;
    let ramp = |i: usize| ((i % w as usize) as u32 * max / (w - 1)) as u16;
    let data = if bits == 8 {
        PixelData::Yuv8 {
            planes: [Arc::new(vec![180u8; n]), Arc::new(vec![100u8; n]), Arc::new(vec![170u8; n])],
            chroma: Chroma::C444,
            alpha: Some(Arc::new((0..n).map(|i| ramp(i) as u8).collect())),
        }
    } else {
        let s = |v: u32| (v << (bits - 8)) as u16;
        PixelData::Yuv16 {
            planes: [Arc::new(vec![s(180); n]), Arc::new(vec![s(100); n]), Arc::new(vec![s(170); n])],
            chroma: Chroma::C444,
            bits,
            alpha: Some(Arc::new((0..n).map(ramp).collect())),
        }
    };
    Arc::new(VideoFrame { width: w, height: h, data, color: waltcut_color::ColorInfo::REC709, par: (1, 1), pts: Default::default() })
}

/// A Y'CbCr layer with an alpha plane (ProRes 4444) is drawn with its alpha: the GPU result matches
/// the CPU compositor over an opaque background. (It used to draw such a layer opaque.)
#[test]
fn gpu_draws_yuv_alpha_plane_like_the_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (w, h) = (320usize, 180usize);
    let pixels = waltcut_media::generators::render(
        &waltcut_media::Generator::Demo(waltcut_media::DemoScene::OceanSunset),
        320,
        180,
        1.0,
        24,
        waltcut_time::FrameRate::FPS_24,
    );
    let background = Arc::new(VideoFrame::rgba8(320, 180, pixels));
    for bits in [8, 10, 12] {
        let mut c = GpuCompositor::new(&dev, &q);
        let plan = FramePlan::Layers {
            width: w,
            height: h,
            layers: vec![
                PlanLayer { frame: background.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, input: None },
                PlanLayer { frame: yuv_alpha_frame(320, 180, bits), matrix: Affine::IDENTITY, opacity: 0.9, blend: Blend::Normal, input: None },
            ],
        };
        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("readback");
        let mut diffs: Vec<u32> =
            cpu.chunks(4).zip(gpu.chunks(4)).map(|(a, b)| (0..3).map(|k| (a[k] as i32 - b[k] as i32).unsigned_abs()).max().unwrap_or(0)).collect();
        diffs.sort_unstable();
        let (p99, max) = (diffs[diffs.len() * 99 / 100], diffs[diffs.len() - 1]);
        assert!(p99 <= 3 && max <= 8, "{bits}-bit: p99 {p99}, max {max}");
        // the transparent left edge shows the background, the opaque right edge the layer
        let at = |img: &[u8], x: usize| (img[(90 * w + x) * 4], img[(90 * w + x) * 4 + 1], img[(90 * w + x) * 4 + 2]);
        let bg = execute_cpu(&FramePlan::Layers {
            width: w,
            height: h,
            layers: vec![PlanLayer { frame: background.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, input: None }],
        })
        .over_black_rgba8();
        let (l, r, b0) = (at(&gpu, 1), at(&gpu, w - 2), at(&bg, 1));
        assert!((l.0 as i32 - b0.0 as i32).abs() <= 6 && (l.1 as i32 - b0.1 as i32).abs() <= 6, "{bits}-bit: left edge {l:?} vs background {b0:?}");
        assert!(r != at(&bg, w - 2), "{bits}-bit: right edge {r:?} should differ from the background");
    }
}

#[test]
fn fast_half_float_conversion_matches_the_reference_for_every_value() {
    for h in 0..=u16::MAX {
        let (fast, slow) = (f16_to_f32_fast(h), f16_to_f32(h));
        assert!(fast.to_bits() == slow.to_bits() || (fast.is_nan() && slow.is_nan()), "{h:#06x}: {fast} vs {slow}");
    }
}

/// HDR and wide-gamut Y'CbCr on a Rec. 709 sequence (#965): the shader's colour management
/// (curve, HLG OOTF, BT.2020 → 709 matrix, BT.2390 tone mapping, desaturation) matches the CPU
/// running the same `InputTransform`.
#[test]
fn hdr_input_gpu_matches_cpu() {
    use waltcut_color::{ColorInfo, ColorPipeline, ColorSpace, InputTransform, Matrix, Primaries, Transfer};
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    // a 10-bit luma ramp (to the top code) under neutral, saturated and extreme chroma, 4:4:4
    let (w, h) = (256u32, 6u32);
    let chroma = [(512, 512), (300, 760), (760, 300), (64, 512), (960, 960), (512, 64)];
    let y: Vec<u16> = (0..h).flat_map(|_| (0..w).map(|x| (64 + x * 876 / 255) as u16)).collect();
    let cb: Vec<u16> = (0..h).flat_map(|r| std::iter::repeat_n(chroma[r as usize].0, w as usize)).collect();
    let cr: Vec<u16> = (0..h).flat_map(|r| std::iter::repeat_n(chroma[r as usize].1, w as usize)).collect();
    let data = PixelData::Yuv16 { planes: [Arc::new(y), Arc::new(cb), Arc::new(cr)], chroma: Chroma::C444, bits: 10, alpha: None };
    let hdr = |transfer| ColorInfo { matrix: Matrix::Bt2020Ncl, transfer, primaries: Primaries::Bt2020, range: Range::Limited };
    let cases = [
        ("HLG", hdr(Transfer::Hlg), None),
        ("PQ", hdr(Transfer::Pq), None),
        ("PQ 4000 cd/m²", hdr(Transfer::Pq), Some(4000.0)),
        ("BT.2020 SDR", ColorInfo { primaries: Primaries::Bt2020, ..ColorInfo::REC709 }, None),
    ];
    for (name, color, peak) in cases {
        let frame = Arc::new(VideoFrame { width: w, height: h, data: data.clone(), color, par: (1, 1), pts: Default::default() });
        let cs = ColorSpace::from_info(&color);
        let t = Arc::new(InputTransform::new(cs, color.range, &ColorPipeline::REC709, peak));
        assert!(t.gpu_params().is_some(), "{name}");
        let layer = PlanLayer { input: Some(t), ..PlanLayer::new(frame.clone(), Affine::IDENTITY, 1.0, Blend::Normal) };
        let plan = FramePlan::Layers { width: w as usize, height: h as usize, layers: vec![layer] };
        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("readback");
        let error = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        // and the stage does something: the default decode of the same frame is far off
        let plain = FramePlan::Layers { width: w as usize, height: h as usize, layers: vec![PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal)] };
        c.composite(&plain);
        let (_, _, unmanaged) = c.read_output().expect("readback");
        let apart = cpu.iter().zip(&unmanaged).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        eprintln!("{name}: max error {error}, unmanaged differs by up to {apart}");
        assert!(error <= 2, "{name}: GPU differs from the CPU by up to {error} codes");
        assert!(apart > 20, "{name}: colour management changed nothing ({apart})");
    }
}
