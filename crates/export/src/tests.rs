use super::*;
use waltcut_media::generators::GeneratorSource;
use waltcut_media::{FrameRequest, Generator, MediaSource};
use waltcut_project::{ItemKind, Label, MediaClip, MediaRef, SequenceSettings, TrackKind};
use waltcut_render::SourceMap;
use waltcut_time::TICKS_PER_SECOND;

pub(crate) fn project() -> (Arc<Project>, ItemId, SourceMap) {
    let mut p = Project::new("x");
    let g = GeneratorSource::new(Generator::ColorMatte { color: [1.0, 0.0, 0.0, 1.0] }, 320, 180, FrameRate::FPS_24, Tick(2 * TICKS_PER_SECOND));
    let tone = GeneratorSource::new(Generator::Tone { hz: 440.0, db: -6.0 }, 320, 180, FrameRate::FPS_24, Tick(2 * TICKS_PER_SECOND));
    let add = |p: &mut Project, g: &GeneratorSource| {
        let info = g.info().clone();
        p.add_item(
            &info.name.clone(),
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(g.generator.clone()),
                info,
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                identity: None,
            }),
            None,
        )
    };
    let red = add(&mut p, &g);
    let t = add(&mut p, &tone);
    let seq = p.new_sequence("s", SequenceSettings { width: 320, height: 180, frame_rate: FrameRate::FPS_24, ..Default::default() }, 1, 1, None);
    let r = FrameRate::FPS_24;
    let v = p.make_track_item(red, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, r.tick_of(24)), r).unwrap();
    let a = p.make_track_item(t, TrackKind::Audio, Tick::ZERO, TimeRange::new(Tick::ZERO, r.tick_of(24)), r).unwrap();
    p.sequence_mut(seq).unwrap().video_tracks[0].items.push(v);
    p.sequence_mut(seq).unwrap().audio_tracks[0].items.push(a);
    let mut m = SourceMap::default();
    m.0.insert(red, Arc::new(g));
    m.0.insert(t, Arc::new(tone));
    (Arc::new(p), seq, m)
}

fn tmp(name: &str) -> String {
    let d = waltcut_testkit::fixtures::workspace_root().join("target").join(format!("fc-export-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

#[test]
fn h264_aac_mp4_roundtrip() {
    let (p, seq, m) = project();
    let path = tmp("out.mp4");
    let prog = Progress::default();
    let r = export(&p, seq, &ExportSettings { format: Format::H264, path: path.clone(), ..Default::default() }, &m, &prog).unwrap();
    assert_eq!(r.frames, 24);
    assert!(prog.finished.load(Ordering::Relaxed));
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let file = waltcut_isobmff::open(bytes.clone()).unwrap();
    let vt = file.track_of_kind(waltcut_isobmff::TrackKind::Video).unwrap();
    let at = file.track_of_kind(waltcut_isobmff::TrackKind::Audio).unwrap();
    assert_eq!(file.tracks[vt].samples.len(), 24);
    assert_eq!(file.tracks[vt].entries[0].format.0, *b"avc1");
    assert_eq!(file.tracks[at].entries[0].format.0, *b"mp4a");
    let src = waltcut_codecs::open_bytes("out.mp4", bytes).unwrap();
    let info = src.info();
    assert_eq!(info.video.as_ref().unwrap().width, 320);
    assert!((info.duration.seconds() - 1.0).abs() < 0.05, "{}", info.duration.seconds());
    let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 2))).unwrap().to_rgba8();
    assert!(f[0] > 230 && f[1] < 30, "{:?}", &f[..4]);
    let a = src.audio(0, 24_000, 48_000).unwrap();
    let pk = a.peaks()[0];
    assert!((pk - 0.5).abs() < 0.05, "tone at -6 dB: {pk}");
    if let Some(ffprobe) = waltcut_testkit::ffprobe_or_skip("export H.264 frame count") {
        let out = std::process::Command::new(ffprobe)
            .args(["-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0", &path])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "24");
    }
}

#[test]
fn png_sequence_has_exact_exclusive_end_frame_count() {
    let (p, seq, m) = project();
    let s = ExportSettings {
        format: Format::PngSequence,
        path: tmp("seq.png"),
        range: Some(TimeRange::new(FrameRate::FPS_24.tick_of(3), FrameRate::FPS_24.tick_of(6))),
        ..Default::default()
    };
    let r = export(&p, seq, &s, &m, &Progress::default()).unwrap();
    assert_eq!(r.frames, 6);
    assert!(r.bytes > 0);
    for frame in 0..6 {
        let bytes = std::fs::read(image_sequence_path(&s.path, frame, 6)).unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (320, 180));
    }
    assert!(std::path::Path::new(&tmp("seq005.png")).exists());
    assert!(!std::path::Path::new(&tmp("seq006.png")).exists());
}

#[test]
fn prores_export_roundtrip() {
    let (p, seq, m) = project();
    let path = tmp("pr.mov");
    export(&p, seq, &ExportSettings { format: Format::ProRes, path: path.clone(), ..Default::default() }, &m, &Progress::default()).unwrap();
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let src = waltcut_codecs::open_bytes("pr.mov", bytes).unwrap();
    assert!(src.info().video.as_ref().unwrap().codec.contains("ProRes 4444"));
    let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 3))).unwrap().to_rgba8();
    assert!(f[0] > 240 && f[1] < 15 && f[2] < 15, "{:?}", &f[..4]);
}

/// #2142: SDR ProRes (MOV) and H.264 (MP4) exports carry a `colr` box with the
/// export's BT.709 colour: Apple's media stack reads the colour of the MOV codecs only from there.
#[test]
fn sdr_exports_carry_a_colr_box() {
    use waltcut_isobmff::ColorInfo;
    let (p, seq, m) = project();
    let nclc = ColorInfo::Nclc { primaries: 1, transfer: 1, matrix: 1 };
    let nclx = ColorInfo::Nclx { primaries: 1, transfer: 1, matrix: 1, full_range: false };
    for (format, name, want) in [(Format::ProRes, "colr-prores.mov", nclc.clone()), (Format::H264, "colr-h264.mp4", nclx)] {
        let path = tmp(name);
        export(&p, seq, &ExportSettings { format, path: path.clone(), include_audio: false, ..Default::default() }, &m, &Progress::default()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let file = waltcut_isobmff::open(bytes).unwrap();
        let t = file.track_of_kind(waltcut_isobmff::TrackKind::Video).unwrap();
        let v = file.tracks[t].entries[0].video.as_ref().unwrap();
        assert_eq!(v.color.as_ref(), Some(&want), "{name}");
        assert!(v.mastering_display.is_none() && v.content_light.is_none(), "{name}: no HDR metadata on SDR");
    }
}

/// Both retained alpha outputs always preserve straight alpha, without a delivery toggle.
#[test]
fn prores_4444_export_roundtrip() {
    let (p, seq, m) = project();
    let mut p = (*p).clone();
    let half = GeneratorSource::new(Generator::ColorMatte { color: [1.0, 0.0, 0.0, 0.5] }, 320, 180, FrameRate::FPS_24, Tick(2 * TICKS_PER_SECOND));
    let item = p.sequence(seq).unwrap().video_tracks[0].items[0].item;
    if let ItemKind::Media(mc) = &mut p.item_mut(item).unwrap().kind {
        mc.media = MediaRef::Generator(half.generator.clone());
    }
    let mut m = m;
    m.0.insert(item, Arc::new(half));
    let p = Arc::new(p);
    for (format, name) in [(Format::ProRes, "half-alpha.mov"), (Format::PngSequence, "half-alpha.png")] {
        let s = ExportSettings { format, path: tmp(name), range: Some(TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(6))), ..Default::default() };
        assert!(s.keeps_alpha());
        let result = export(&p, seq, &s, &m, &Progress::default()).unwrap();
        assert_eq!(result.frames, 6);
        if format == Format::ProRes {
            let bytes: Arc<[u8]> = std::fs::read(&s.path).unwrap().into();
            let file = waltcut_isobmff::open(bytes.clone()).unwrap();
            let track = &file.tracks[file.track_of_kind(waltcut_isobmff::TrackKind::Video).unwrap()];
            assert_eq!(track.entries[0].format.0, *b"ap4h");
            assert_eq!(track.samples.len(), 6);
            let src = waltcut_codecs::open_bytes("half-alpha.mov", bytes).unwrap();
            for i in 0..6 {
                let frame = src.video_frame(FrameRequest::full(FrameRate::FPS_24.tick_of(i))).unwrap();
                assert!(frame.format_label().contains("4:4:4"));
                let px = frame.to_rgba8();
                assert!(px[0] > 240 && px[1] < 15 && (i32::from(px[3]) - 128).abs() <= 3, "{:?}", &px[..4]);
            }
        } else {
            for i in 0..6 {
                let bytes = std::fs::read(image_sequence_path(&s.path, i, 6)).unwrap();
                let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
                let px = image.get_pixel(0, 0).0;
                assert!(px[..3] == [255, 0, 0] && px[3].abs_diff(128) <= 1, "half alpha kept: {px:?}");
            }
            assert!(!std::path::Path::new(&image_sequence_path(&s.path, 6, 6)).exists());
        }
    }
}

/// HDR sequences export PQ / HLG with colour signalling in the bitstream (VUI, ProRes header) and
/// the container (`colr`, `mdcv`, `clli`); our demuxer and ffprobe both read it back, and the
/// picture survives the round trip.
#[test]
fn hdr_exports_signal_pq_and_hlg() {
    use waltcut_color::{ColorPipeline, Transfer, WorkingSpace};
    for (ws, fmt, ext, transfer, ff_transfer) in [
        (WorkingSpace::Rec2100Pq, Format::ProRes, "mov", Transfer::Pq, "smpte2084"),
        (WorkingSpace::Rec2100Pq, Format::H264, "mp4", Transfer::Pq, "smpte2084"),
        (WorkingSpace::Rec2100Hlg, Format::ProRes, "mov", Transfer::Hlg, "arib-std-b67"),
        (WorkingSpace::Rec2100Hlg, Format::H264, "mp4", Transfer::Hlg, "arib-std-b67"),
    ] {
        let (p, seq, m) = project();
        let mut p = (*p).clone();
        p.sequence_mut(seq).unwrap().settings.color = ColorPipeline { working: ws, ..ColorPipeline::REC709 };
        let p = Arc::new(p);
        let path = tmp(&format!("hdr-{}.{ext}", ws.id()));
        let s = ExportSettings { format: fmt, path: path.clone(), include_audio: false, ..Default::default() };
        export(&p, seq, &s, &m, &Progress::default()).unwrap();
        // our demuxer/decoder sees the signalling
        let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
        let src = waltcut_codecs::open_bytes(&path, bytes).unwrap();
        let v = src.info().video.clone().unwrap();
        assert_eq!(v.color.transfer, transfer, "{path}");
        assert_eq!(v.color.primaries, waltcut_color::Primaries::Bt2020, "{path}");
        assert_eq!(v.color.matrix, waltcut_color::Matrix::Bt2020Ncl, "{path}");
        // PQ exports carry mastering metadata (1000 cd/m² peak; MaxCLL 0 = unknown), which sets
        // the tone-mapping peak when the file is used again
        if transfer == Transfer::Pq {
            let hdr = v.hdr.unwrap_or_else(|| panic!("{path}: no HDR metadata"));
            assert_eq!((hdr.mastering_max_nits, hdr.max_cll, hdr.peak_nits()), (Some(1000.0), None, Some(1000.0)), "{path}");
        }
        // 709 red matte → BT.2020 HDR → decoded back into Rec. 709 (tone mapped) stays red
        let f = src.video_frame(FrameRequest::full(Tick::ZERO)).unwrap();
        let back = waltcut_render::colorman::decode(&Project::new("x"), ItemId(0), &f, 1, &ColorPipeline::REC709);
        let c = waltcut_render::Image::unpremul(back.get(160, 90));
        assert!(c[0] > 0.6 && c[1] < 0.05 && c[2] < 0.05, "{path}: {c:?}");
        // ffprobe agrees
        let Some(ffprobe) = waltcut_testkit::ffprobe_or_skip("hdr export signalling") else { continue };
        let out = std::process::Command::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=color_transfer,color_primaries,color_space:stream_side_data",
                "-of",
                "default=nw=1",
                &path,
            ])
            .output()
            .unwrap();
        let txt = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(txt.contains(&format!("color_transfer={ff_transfer}")), "{path}: {txt}");
        assert!(txt.contains("color_primaries=bt2020"), "{path}: {txt}");
        assert!(txt.contains("color_space=bt2020nc"), "{path}: {txt}");
        if transfer == Transfer::Pq {
            assert!(txt.contains("Mastering display metadata") && txt.contains("max_luminance=10000000/10000"), "{path}: {txt}");
            assert!(txt.contains("Content light level metadata"), "{path}: {txt}");
        }
        eprintln!("{path}:\n{txt}");
    }
}

/// Scenario runner for the GPU hook: everything in one test because the renderer registry is
/// process-global and parallel tests would see each other's factories.
#[test]
fn gpu_hook_scenarios() {
    // 1. Declining renderer: byte-equal to Off, and an empty registry too.
    crate::reset_frame_renderers_for_tests();
    crate::register_frame_renderer(|| None);
    let (p, seq, m) = project();
    let mk = |path: &String, gpu: Option<bool>| {
        let mut s = ExportSettings { format: Format::PngSequence, path: path.clone(), ..Default::default() };
        s.range = Some(TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(2)));
        s.gpu_rendering = match gpu {
            Some(true) | None => crate::GpuRendering::Auto,
            Some(false) => crate::GpuRendering::Off,
        };
        export(&p, seq, &s, &m, &Progress::default()).unwrap()
    };
    mk(&tmp("off.png"), Some(false));
    mk(&tmp("auto-declined.png"), Some(true));
    assert_eq!(std::fs::read(tmp("off000.png")).unwrap(), std::fs::read(tmp("auto-declined000.png")).unwrap(), "declined GPU export must be byte-equal to Off");
    crate::reset_frame_renderers_for_tests();
    mk(&tmp("auto-empty.png"), Some(true));
    assert_eq!(std::fs::read(tmp("off000.png")).unwrap(), std::fs::read(tmp("auto-empty000.png")).unwrap(), "empty registry must behave like Off");

    // 2. Producing renderer: counters up and its picture reaches the export.
    crate::reset_frame_renderers_for_tests();
    crate::register_frame_renderer(|| Some(Box::new(FakeRenderer) as Box<dyn FrameRenderer>));
    mk(&tmp("fake-gpu.png"), Some(true));
    assert!(gpu_render_stats().frames >= 2, "gpu frames counted: {:?}", gpu_render_stats());
    assert_eq!(gpu_render_stats().fallbacks, 0);
    mk(&tmp("fake-off.png"), Some(false));
    assert_eq!(gpu_render_stats().fallbacks, 0, "Off must not count as fallback");
    assert_ne!(std::fs::read(tmp("fake-gpu000.png")).unwrap(), std::fs::read(tmp("fake-off000.png")).unwrap(), "the fake GPU picture must reach the export");

    // 3. Hostile and degenerate frame sizes must not panic with a renderer attached.
    crate::reset_frame_renderers_for_tests();
    crate::register_frame_renderer(|| None);
    for (w, h) in [(1, 1), (17, 13), (4096, 1)] {
        let mut p = Project::new("x");
        let g = GeneratorSource::new(Generator::ColorMatte { color: [0.5, 0.5, 0.5, 1.0] }, 320, 180, FrameRate::FPS_24, Tick(2 * TICKS_PER_SECOND));
        let info = g.info().clone();
        let red = p.add_item(
            &info.name.clone(),
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(g.generator.clone()),
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
        let seq = p.new_sequence("s", SequenceSettings { width: w, height: h, frame_rate: FrameRate::FPS_24, ..Default::default() }, 1, 1, None);
        let v = p.make_track_item(red, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(2)), FrameRate::FPS_24).unwrap();
        p.sequence_mut(seq).unwrap().video_tracks[0].items.push(v);
        let mut m = SourceMap::default();
        m.0.insert(red, Arc::new(g));
        let mut s = ExportSettings { format: Format::PngSequence, path: tmp("hostile.png"), ..Default::default() };
        s.range = Some(TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(1)));
        s.gpu_rendering = crate::GpuRendering::Auto;
        let r = export(&Arc::new(p), seq, &s, &m, &Progress::default());
        if r.is_err() {
            continue; // sizes the exporter rejects are fine too — as long as nothing panics
        }
        let bytes = std::fs::read(tmp("hostile000.png")).unwrap_or_default();
        assert!(!bytes.is_empty(), "{w}x{h}: PNG written");
    }

    // 4. A renderer that panics is dropped, not put back: the export still completes, every
    //    frame comes from the CPU (byte-equal to Off), and no renderer is used after its panic.
    crate::reset_frame_renderers_for_tests();
    PANICS.store(0, std::sync::atomic::Ordering::SeqCst);
    crate::register_frame_renderer(|| Some(Box::new(PanickingRenderer) as Box<dyn FrameRenderer>));
    let (p, seq, m) = project();
    let mk12 = |path: &String, gpu: crate::GpuRendering| {
        let mut s = ExportSettings { format: Format::PngSequence, path: path.clone(), ..Default::default() };
        s.range = Some(TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(12)));
        s.gpu_rendering = gpu;
        export(&p, seq, &s, &m, &Progress::default()).unwrap()
    };
    mk12(&tmp("panic-off.png"), crate::GpuRendering::Off);
    mk12(&tmp("panic-auto.png"), crate::GpuRendering::Auto);
    for f in ["000", "005", "011"] {
        assert_eq!(
            std::fs::read(tmp(&format!("panic-off{f}.png"))).unwrap(),
            std::fs::read(tmp(&format!("panic-auto{f}.png"))).unwrap(),
            "frame {f}: a panicking GPU renderer falls back to the CPU"
        );
    }
    // at most one panic per pooled renderer (4): none is used again after it panicked
    assert!(PANICS.load(std::sync::atomic::Ordering::SeqCst) <= 4, "renderer reused after a panic: {}", PANICS.load(std::sync::atomic::Ordering::SeqCst));

    crate::reset_frame_renderers_for_tests();
}

static PANICS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct PanickingRenderer;

impl FrameRenderer for PanickingRenderer {
    fn render(
        &mut self,
        _project: &Project,
        _seq: ItemId,
        _t: Tick,
        _opts: waltcut_render::RenderOptions,
        _sources: &dyn SourceProvider,
    ) -> Option<waltcut_render::Image> {
        PANICS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        panic!("GPU renderer failure (test)");
    }
}

struct FakeRenderer;

impl FrameRenderer for FakeRenderer {
    fn render(
        &mut self,
        _project: &Project,
        _seq: ItemId,
        _t: Tick,
        _opts: waltcut_render::RenderOptions,
        _sources: &dyn SourceProvider,
    ) -> Option<waltcut_render::Image> {
        Some(waltcut_render::Image::new(320, 180))
    }
}
