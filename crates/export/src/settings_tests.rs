//! Fixed retained-output contract and hostile-input regressions.
use super::*;
use serde_json::json;
use waltcut_time::TICKS_PER_SECOND;

#[test]
fn only_retained_formats_and_serialized_settings_are_accepted() {
    assert_eq!(Format::ALL, [Format::H264, Format::ProRes, Format::PngSequence]);
    for name in ["h264", "mp4", "prores", "prores4444", "mov", "png"] {
        assert!(Format::from_name(name).is_some(), "{name}");
    }
    for removed in ["hevc", "h265", "mjpeg", "gif", "wav", "aiff", "aac", "m4a", "tiff", "bmp", "tga", "dpx", "exr"] {
        assert!(Format::from_name(removed).is_none(), "{removed}");
    }
    let minimal = json!({"format":"prores","path":"out.mov","range":null});
    assert!(serde_json::from_value::<ExportSettings>(minimal.clone()).is_ok());
    for key in [
        "preset",
        "settings",
        "queue",
        "scale",
        "include_audio",
        "quality",
        "bitrate_kbps",
        "alpha",
        "prores_profile",
        "audio",
        "overlays",
        "metadata",
        "sdr",
        "gpu_rendering",
    ] {
        let mut value = minimal.clone();
        value[key] = Value::Null;
        assert!(serde_json::from_value::<ExportSettings>(value).is_err(), "{key}: even null is unsupported");
    }
}

#[test]
fn output_properties_follow_the_sequence_without_delivery_overrides() {
    let png = ExportSettings { format: Format::PngSequence, ..Default::default() };
    let resolved = png.resolve(17, 13, FrameRate::FPS_24, 44_100).unwrap();
    assert_eq!((resolved.width, resolved.height, resolved.sample_rate, resolved.channels), (17, 13, 44_100, 2));
    assert!(png.keeps_alpha());
    assert!(!png.has_audio());
    assert_eq!(png.resolve(1, 1, FrameRate::FPS_24, 48_000).unwrap().width, 1);
    let h264 = ExportSettings { format: Format::H264, ..Default::default() };
    let resolved = h264.resolve(17, 13, FrameRate { num: 48, den: 2 }, 48_000).unwrap();
    assert_eq!((resolved.width, resolved.height, resolved.rate), (16, 12, FrameRate::FPS_24));
    assert!(!h264.keeps_alpha());
    assert!(h264.has_audio());
    let prores = ExportSettings { format: Format::ProRes, ..Default::default() };
    assert!(prores.keeps_alpha() && prores.has_audio());
}

#[test]
fn hostile_geometry_rates_and_time_ranges_are_errors_not_panics() {
    let settings = ExportSettings::default();
    for (w, h, rate, sr) in [
        (0, 1, FrameRate::FPS_24, 48_000),
        (1, 0, FrameRate::FPS_24, 48_000),
        (u32::MAX, u32::MAX, FrameRate::FPS_24, 48_000),
        (8192, 8192, FrameRate::FPS_24, 48_000),
        (32, 32, FrameRate { num: 0, den: 0 }, 48_000),
        (32, 32, FrameRate { num: i64::MAX, den: 1 }, 48_000),
        (32, 32, FrameRate { num: 1, den: i64::MAX }, 48_000),
        (32, 32, FrameRate::FPS_24, 0),
        (32, 32, FrameRate::FPS_24, u32::MAX),
    ] {
        assert!(std::panic::catch_unwind(|| settings.resolve(w, h, rate, sr)).unwrap().is_err());
    }
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::MAX] {
        let settings = ExportSettings { scale, ..Default::default() };
        assert!(std::panic::catch_unwind(|| settings.resolve(32, 32, FrameRate::FPS_24, 48_000)).unwrap().is_err());
    }
    for range in [
        TimeRange::new(Tick(-1), Tick(1)),
        TimeRange::new(Tick::ZERO, Tick::ZERO),
        TimeRange::new(Tick::ZERO, Tick(-1)),
        TimeRange::new(Tick(i64::MAX), Tick(1)),
        TimeRange::new(Tick::ZERO, Tick(86_401 * TICKS_PER_SECOND)),
    ] {
        assert!(std::panic::catch_unwind(|| validate_range(range)).unwrap().is_err(), "{range:?}");
    }
}

#[test]
fn malformed_encoder_frames_are_rejected_before_conversion_or_allocation() {
    for format in [Format::ProRes, Format::H264] {
        let settings = ExportSettings { format, ..Default::default() };
        let factory = if format == Format::ProRes { prores_factory } else { h264_factory };
        assert!(factory(u32::MAX, u32::MAX, FrameRate::FPS_24, &settings).is_err());
        assert!(factory(16, 16, FrameRate { num: 0, den: 1 }, &settings).is_err());
        let mut encoder = factory(16, 16, FrameRate::FPS_24, &settings).unwrap();
        let rgba = vec![255; 16 * 16 * 4];
        let malformed_hdr = vec![f32::NAN; 16 * 16 * 3];
        for frame in [
            EncoderFrame { width: 16, height: 16, rgba: &[], hdr: None, index: 0 },
            EncoderFrame { width: 16, height: 16, rgba: &rgba, hdr: Some(&[0.0]), index: 0 },
            EncoderFrame { width: 16, height: 16, rgba: &rgba, hdr: Some(&malformed_hdr), index: 0 },
            EncoderFrame { width: 32, height: 16, rgba: &rgba, hdr: None, index: 0 },
            EncoderFrame { width: 16, height: 16, rgba: &rgba, hdr: None, index: u64::MAX },
        ] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| encoder.encode(&frame)));
            assert!(result.unwrap().is_err(), "{format:?}");
        }
    }
}

#[test]
fn png_rejects_hostile_dimensions_and_buffer_lengths() {
    for (bytes, w, h) in [(vec![], 0, 0), (vec![0; 3], 1, 1), (vec![0; 5], 1, 1), (vec![], u32::MAX, u32::MAX)] {
        assert!(std::panic::catch_unwind(|| encode_png(bytes, w, h)).unwrap().is_err());
    }
    let bytes = encode_png(vec![255, 0, 0, 128], 1, 1).unwrap();
    assert_eq!(image::load_from_memory(&bytes).unwrap().to_rgba8().as_raw(), &[255, 0, 0, 128]);
}

#[test]
fn movie_pcm_is_float32_stereo_and_non_finite_samples_are_sanitized() {
    let mut bytes = Vec::new();
    crate::pcm::encode(&[0.25, -0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY], &mut bytes);
    let samples: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    assert_eq!(&samples[..3], &[0.25, -0.5, 0.0]);
    assert!(samples.iter().all(|s| s.is_finite()));
}

#[test]
fn image_sequence_names_and_progress_formatters_are_bounded() {
    assert_eq!(image_sequence_path("Shot.png", 0, 12), "Shot000.png");
    assert_eq!(image_sequence_path("Shot.png", 9, 1001), "Shot0009.png");
    assert!(image_sequence_path("Shot.png", u64::MAX, u64::MAX).ends_with(".png"));
    assert_eq!(format_eta(std::time::Duration::from_millis(120_001)), "2:01");
    assert_eq!(format_eta(std::time::Duration::MAX), "5124095576030431:00:15");
    assert_eq!(GpuRendering::default(), GpuRendering::Off);
}

use serde_json::Value;
