use super::*;
use waltcut_media::generators::GeneratorSource;
use waltcut_media::{DemoScene, Generator, MediaSource};
use waltcut_project::{Label, MediaClip, MediaRef, SequenceSettings, TrackKind};
use waltcut_time::{FrameRate, TICKS_PER_SECOND, TimeRange};

fn setup() -> (Project, ItemId, ItemId, ItemId, SourceMap) {
    let mut p = Project::new("r");
    let mut map = SourceMap::default();
    let mut add = |p: &mut Project, g: GeneratorSource| {
        let info = g.info().clone();
        let generator = g.generator.clone();
        let id = p.add_item(
            &info.name.clone(),
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(generator),
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
        map.0.insert(id, Arc::new(g) as SharedSource);
        id
    };
    let red =
        add(&mut p, GeneratorSource::new(Generator::ColorMatte { color: [1.0, 0.0, 0.0, 1.0] }, 320, 180, FrameRate::FPS_24, Tick(10 * TICKS_PER_SECOND)));
    let ocean = add(&mut p, GeneratorSource::demo(DemoScene::OceanSunset));
    let seq = p.new_sequence("s", SequenceSettings { width: 320, height: 180, frame_rate: FrameRate::FPS_24, ..Default::default() }, 2, 2, None);
    (p, red, ocean, seq, map)
}

#[test]
fn intrinsic_volume_uses_source_time_speed_and_reverse_without_bypassing_clip_gain() {
    let (mut project, _, ocean, _, _) = setup();
    let r = FrameRate::FPS_24;
    let mut item = project.make_track_item(ocean, TrackKind::Audio, r.tick_of(10), TimeRange::new(r.tick_of(5), r.tick_of(4)), r).unwrap();
    item.speed = 2.0;
    item.gain_db = -6.0;
    let level = item.effect_mut("volume").unwrap().param_mut("level").unwrap();
    level.put_keyframe(r.tick_of(5), ParamValue::Float(-12.0));
    level.put_keyframe(r.tick_of(13), ParamValue::Float(0.0));
    for (reverse, levels) in [(false, [-12.0, -6.0, 0.0]), (true, [0.0, -6.0, -12.0])] {
        item.reverse = reverse;
        for (frame, level) in [10, 12, 14].into_iter().zip(levels) {
            let want = 10f32.powf((-6.0 + level) / 20.0);
            assert!((audio::clip_gain_at(&item, r.tick_of(frame)) - want).abs() < 1e-6, "reverse={reverse}, frame={frame}");
        }
        item.effect_mut("volume").unwrap().enabled = false;
        assert!((audio::clip_gain_at(&item, r.tick_of(10)) - 10f32.powf(-6.0 / 20.0)).abs() < 1e-6);
        item.effect_mut("volume").unwrap().enabled = true;
        item.effect_mut("volume").unwrap().param_mut("bypass").unwrap().value = ParamValue::Bool(true);
        assert!((audio::clip_gain_at(&item, r.tick_of(10)) - 10f32.powf(-6.0 / 20.0)).abs() < 1e-6);
        item.effect_mut("volume").unwrap().param_mut("bypass").unwrap().value = ParamValue::Bool(false);
    }
}

fn place(p: &mut Project, seq: ItemId, track: usize, item: ItemId, start_f: i64, dur_f: i64) -> waltcut_project::ClipId {
    let r = FrameRate::FPS_24;
    let ti = p.make_track_item(item, TrackKind::Video, r.tick_of(start_f), TimeRange::new(Tick::ZERO, r.tick_of(dur_f)), r).unwrap();
    let id = ti.id;
    let mut ti = ti;
    ti.scale_to_frame = true;
    p.sequence_mut(seq).unwrap().video_tracks[track].items.push(ti);
    p.sequence_mut(seq).unwrap().video_tracks[track].sort();
    id
}

#[test]
fn composites_tracks_and_opacity() {
    let (mut p, red, ocean, seq, map) = setup();
    place(&mut p, seq, 0, ocean, 0, 48);
    let top = place(&mut p, seq, 1, red, 0, 48);
    let img = render_sequence(&p, seq, Tick(1000), RenderOptions::default(), &map);
    assert_eq!((img.w, img.h), (320, 180));
    let c = img.get(160, 90);
    assert!((c[0] - 1.0).abs() < 1e-4 && c[1] < 1e-4, "red on top: {c:?}");
    // 50% opacity shows the bottom layer through
    let s = p.sequence_mut(seq).unwrap();
    let (_, it) = s.find_item_mut(top).unwrap();
    it.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap().value = ParamValue::Float(50.0);
    let img = render_sequence(&p, seq, Tick(1000), RenderOptions::default(), &map);
    let c = img.get(160, 20);
    assert!(c[0] > 0.5 && c[0] < 1.0 && c[2] > 0.0, "{c:?}");
}

#[test]
fn animated_opacity_composites_whole_media_layers() {
    let (mut p, red, ocean, seq, map) = setup();
    place(&mut p, seq, 0, ocean, 0, 72);
    let top = place(&mut p, seq, 1, red, 0, 72);
    let rate = FrameRate::FPS_24;
    let it = p.sequence_mut(seq).unwrap().find_item_mut(top).unwrap().1;
    let opacity = it.effect_mut("opacity").unwrap().param_mut("opacity").unwrap();
    opacity.put_keyframe(rate.tick_of(0), ParamValue::Float(0.0));
    opacity.put_keyframe(rate.tick_of(48), ParamValue::Float(100.0));
    {
        for frame in [0, 12, 24, 36, 48] {
            let t = rate.tick_of(frame);
            let opts = RenderOptions::default();
            let img = render_sequence(&p, seq, t, opts, &map);
            let mut bottom = p.clone();
            bottom.sequence_mut(seq).unwrap().video_tracks[1].items.clear();
            let background = render_sequence(&bottom, seq, t, opts, &map);
            let alpha = frame as f32 / 48.0;
            for (x, y) in [(0, 0), (160, 90), (319, 179)] {
                let bg = background.get(x, y);
                let expected = [alpha + bg[0] * (1.0 - alpha), bg[1] * (1.0 - alpha), bg[2] * (1.0 - alpha), 1.0];
                let actual = img.get(x, y);
                for (a, e) in actual.into_iter().zip(expected) {
                    assert!((a - e).abs() < 1e-5, "frame {frame}: {actual:?} != {expected:?}");
                }
            }
        }
    }
}

#[test]
fn half_resolution_matches_full_downsampled() {
    let (mut p, _red, ocean, seq, map) = setup();
    place(&mut p, seq, 0, ocean, 0, 48);
    let full = render_sequence(&p, seq, Tick(1000), RenderOptions::default(), &map);
    let half = render_sequence(&p, seq, Tick(1000), RenderOptions { scale: 0.5, ..Default::default() }, &map);
    assert_eq!((half.w, half.h), (160, 90));
    let a = full.get(200, 60);
    let b = half.get(100, 30);
    for k in 0..3 {
        assert!((a[k] - b[k]).abs() < 0.06, "{a:?} vs {b:?}");
    }
}

#[test]
fn motion_scale_and_position() {
    let (mut p, red, _o, seq, map) = setup();
    let id = place(&mut p, seq, 0, red, 0, 48);
    let s = p.sequence_mut(seq).unwrap();
    let (_, it) = s.find_item_mut(id).unwrap();
    let m = it.effect_mut("motion").unwrap();
    m.params.get_mut("scale").unwrap().value = ParamValue::Float(50.0);
    m.params.get_mut("position").unwrap().value = ParamValue::Vec2(waltcut_geom::Vec2::new(80.0, 45.0));
    let img = render_sequence(&p, seq, Tick(0), RenderOptions::default(), &map);
    assert!(img.get(80, 45)[3] > 0.99);
    assert!(img.get(10, 10)[3] > 0.99, "top-left quadrant covered");
    assert_eq!(img.get(200, 120)[3], 0.0, "rest transparent");
}

#[test]
fn audio_mix_bars_tone() {
    let mut p = Project::new("a");
    let g = GeneratorSource::new(Generator::BarsAndTone, 64, 36, FrameRate::FPS_24, Tick(5 * TICKS_PER_SECOND));
    let info = g.info().clone();
    let id = p.add_item(
        "bars",
        Label::Iris,
        ItemKind::Media(MediaClip {
            media: MediaRef::Generator(Generator::BarsAndTone),
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
    let mut map = SourceMap::default();
    map.0.insert(id, Arc::new(g));
    let seq = p.new_sequence("s", SequenceSettings::default(), 1, 1, None);
    let r = FrameRate::FPS_23_976;
    let mut ti = p.make_track_item(id, TrackKind::Audio, Tick::ZERO, TimeRange::new(Tick::ZERO, Tick(TICKS_PER_SECOND)), r).unwrap();
    ti.effect_mut("volume").unwrap().params.get_mut("level").unwrap().value = ParamValue::Float(-6.0);
    p.sequence_mut(seq).unwrap().audio_tracks[0].items.push(ti);
    let s = p.sequence(seq).unwrap();
    let buf = audio::mix_sequence(&p, s, 0, 4800, &map);
    let peak = buf.peaks()[0];
    let expect = 10f32.powf(-26.0 / 20.0);
    assert!((peak - expect).abs() < 0.01, "{peak} vs {expect}");
}

#[test]
fn plan_matches_reference_renderer() {
    let (mut p, red, ocean, seq, map) = setup();
    place(&mut p, seq, 0, ocean, 0, 48);
    let top = place(&mut p, seq, 1, red, 10, 20);
    {
        let s = p.sequence_mut(seq).unwrap();
        let (_, it) = s.find_item_mut(top).unwrap();
        let m = it.effect_mut("motion").unwrap();
        m.params.get_mut("scale").unwrap().value = ParamValue::Float(40.0);
        m.params.get_mut("rotation").unwrap().value = ParamValue::Float(15.0);
        it.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap().value = ParamValue::Float(70.0);
    }
    let r = FrameRate::FPS_24;
    for f in [0, 12, 20, 40] {
        let opts = RenderOptions { scale: 0.5, ..Default::default() };
        let reference = render_sequence(&p, seq, r.tick_of(f), opts, &map);
        let plan = plan::plan_frame(&p, seq, r.tick_of(f), opts, &map);
        assert!(matches!(plan, plan::FramePlan::Layers { .. }), "simple frame should be GPU-drawable");
        let got = plan::execute_cpu(&plan);
        assert_eq!((got.w, got.h), (reference.w, reference.h));
        let max = got.px.iter().zip(&reference.px).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(max < 0.02, "frame {f}: max diff {max}");
    }
}

#[test]
fn offline_slate_is_deterministic_and_marked() {
    use crate::offline::{FIELD, OfflineReason, slate_rgba8};
    let a = slate_rgba8(640, 360, "A001_C002.mov", OfflineReason::Missing);
    assert_eq!(a, slate_rgba8(640, 360, "A001_C002.mov", OfflineReason::Missing));
    assert_ne!(a, slate_rgba8(640, 360, "A001_C002.mov", OfflineReason::MadeOffline), "the headline names the reason");
    // corners are the red field / stripes, opaque
    let px = |x: usize, y: usize| &a[(y * 640 + x) * 4..(y * 640 + x) * 4 + 4];
    assert_eq!(px(0, 359)[3], 255);
    assert!(px(0, 359)[0] >= FIELD[0] && px(0, 359)[1] <= 0x20, "{:?}", px(0, 359));
    // tiny sizes work (thumbnails)
    assert_eq!(slate_rgba8(8, 4, "x", OfflineReason::Unreadable).len(), 8 * 4 * 4);
    if let Some(p) = std::env::var_os("FILMCRAFT_SLATE_PNG") {
        ::image::save_buffer(p, &a, 640, 360, ::image::ExtendedColorType::Rgba8).unwrap();
    }
}

/// A 320×180 4:2:0 source with smooth gradients (what decoders hand the renderer).
struct YuvSource(GeneratorSource);

impl MediaSource for YuvSource {
    fn info(&self) -> &waltcut_media::MediaInfo {
        self.0.info()
    }
    fn video_frame(&self, _req: FrameRequest) -> waltcut_media::Result<Arc<waltcut_frame::VideoFrame>> {
        let (w, h) = (320usize, 180usize);
        let y = (0..w * h).map(|i| (16 + (i % w) * 200 / w + (i / w) * 20 / h) as u8).collect();
        let u = (0..w * h / 4).map(|i| (90 + (i % (w / 2)) * 60 / (w / 2)) as u8).collect();
        let v = (0..w * h / 4).map(|i| (100 + (i / (w / 2)) * 50 / (h / 2)) as u8).collect();
        Ok(Arc::new(waltcut_frame::VideoFrame {
            width: w as u32,
            height: h as u32,
            data: waltcut_frame::PixelData::Yuv8 { planes: [Arc::new(y), Arc::new(u), Arc::new(v)], chroma: waltcut_frame::Chroma::C420, alpha: None },
            color: waltcut_color::ColorInfo::REC709,
            par: (1, 1),
            pts: Tick::ZERO,
        }))
    }
    fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> waltcut_media::Result<waltcut_frame::AudioBuffer> {
        self.0.audio(start, frames, sample_rate)
    }
}

#[test]
fn draft_reduced_resolution_plans_carry_decimated_yuv() {
    let (mut p, red, _ocean, seq, mut map) = setup();
    let matte = GeneratorSource::new(Generator::ColorMatte { color: [0.0, 0.0, 0.0, 1.0] }, 320, 180, FrameRate::FPS_24, Tick(10 * TICKS_PER_SECOND));
    map.0.insert(red, Arc::new(YuvSource(matte)) as SharedSource);
    place(&mut p, seq, 0, red, 0, 48);
    let t = FrameRate::FPS_24.tick_of(3);
    for draft in [false, true] {
        for (scale, small_w) in [(1.0f32, 320u32), (0.5, 160), (0.25, 80)] {
            let opts = RenderOptions { scale, ..Default::default() };
            let plan = waltcut_media::cancel::with_draft(draft, || plan::plan_frame(&p, seq, t, opts, &map));
            let plan::FramePlan::Layers { layers, .. } = &plan else { panic!("GPU-drawable") };
            // exact playback hands over the decoded picture; draft playback planes at the drawn size
            assert_eq!(layers[0].frame.width, if draft { small_w } else { 320 }, "scale {scale}, draft {draft}");
            assert!(matches!(layers[0].frame.data, waltcut_frame::PixelData::Yuv8 { .. }));
            let got = plan::execute_cpu(&plan);
            let reference = render_sequence(&p, seq, t, opts, &map);
            assert_eq!((got.w, got.h), (reference.w, reference.h));
            let max = got.px.iter().zip(&reference.px).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
            // the Y'CbCr-code mean differs from a linear-light mean by a little on gradients
            assert!(max < if draft { 0.03 } else { 1e-5 }, "scale {scale}, draft {draft}: max diff {max}");
        }
    }
}

/// A 24 fps source that picks frames like the MP4 reader: the requested time is converted to the
/// track timescale rounding to the nearest unit, then the frame at or before it is shown. It
/// records the frames it was asked for.
struct NearestUnitSource {
    inner: GeneratorSource,
    asked: std::sync::Mutex<Vec<i64>>,
}

impl MediaSource for NearestUnitSource {
    fn info(&self) -> &waltcut_media::MediaInfo {
        self.inner.info()
    }
    fn video_frame(&self, req: FrameRequest) -> waltcut_media::Result<Arc<waltcut_frame::VideoFrame>> {
        // 12288 units a second: 512 a frame at 24 fps
        let units = req.time.to_rational_round(1, 12_288);
        self.asked.lock().unwrap().push(units.div_euclid(512));
        self.inner.video_frame(req)
    }
    fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> waltcut_media::Result<waltcut_frame::AudioBuffer> {
        self.inner.audio(start, frames, sample_rate)
    }
}

/// #318: a trimmed clip (source frames 10–29) played in reverse shows 29 down to 10. Its source
/// time is the instant just before the mirrored position; a reader that rounds to the nearest
/// timescale unit used to land on the next frame, showing frame 30 (outside the clip) and never 10.
#[test]
fn a_reversed_clip_shows_exactly_its_source_range() {
    let r = FrameRate::FPS_24;
    let mut p = Project::new("r");
    let g = GeneratorSource::new(Generator::ColorMatte { color: [0.2, 0.4, 0.6, 1.0] }, 64, 64, r, Tick(4 * TICKS_PER_SECOND));
    let info = g.info().clone();
    let item = p.add_item(
        "ids",
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
    let src = Arc::new(NearestUnitSource { inner: g, asked: Default::default() });
    let mut map = SourceMap::default();
    map.0.insert(item, src.clone() as SharedSource);
    let seq = p.new_sequence("s", SequenceSettings { width: 64, height: 64, frame_rate: r, ..Default::default() }, 1, 0, None);
    let ti = p.make_track_item(item, TrackKind::Video, Tick::ZERO, TimeRange::new(r.tick_of(10), r.tick_of(20)), r).unwrap();
    let id = ti.id;
    p.sequence_mut(seq).unwrap().video_tracks[0].items.push(ti);
    let opts = RenderOptions { scale: 1.0, ..Default::default() };
    let shown = |p: &Project, f: i64, planned: bool| -> Vec<i64> {
        src.asked.lock().unwrap().clear();
        if planned {
            plan::execute_cpu(&plan::plan_frame(p, seq, r.tick_of(f), opts, &map));
        } else {
            render_sequence(p, seq, r.tick_of(f), opts, &map);
        }
        let mut asked = src.asked.lock().unwrap().clone();
        asked.dedup();
        asked
    };
    // forward control: 10, 17, 29
    for (f, want) in [(0, 10), (7, 17), (19, 29)] {
        for planned in [false, true] {
            assert_eq!(shown(&p, f, planned), [want], "forward, timeline frame {f}, planned {planned}");
        }
    }
    p.sequence_mut(seq).unwrap().find_item_mut(id).unwrap().1.reverse = true;
    for (f, want) in [(0, 29), (7, 22), (19, 10)] {
        for planned in [false, true] {
            assert_eq!(shown(&p, f, planned), [want], "reversed, timeline frame {f}, planned {planned}");
        }
    }
    // every timeline frame stays inside the clip's source range, each source frame shown once
    let all: Vec<i64> = (0..20).flat_map(|f| shown(&p, f, false)).collect();
    assert_eq!(all, (10..=29).rev().collect::<Vec<_>>());
}
