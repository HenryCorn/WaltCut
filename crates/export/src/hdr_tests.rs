//! Colour signalling and alpha shared by the retained built-in movie outputs.
use super::*;

#[test]
fn working_hdr_is_retained_unless_an_internal_preview_requests_sdr() {
    assert!(crate::job::hdr_output(true, false));
    assert!(!crate::job::hdr_output(false, false));
    assert!(!crate::job::hdr_output(true, true));
}

#[test]
fn hdr_colour_floats_keep_the_separate_rgba_alpha() {
    let rgba = [255, 0, 0, 128, 0, 255, 0, 0];
    let rgb = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let (mut y, mut cb, mut cr, mut alpha) = ([0; 2], [0; 2], [0; 2], [0; 2]);
    to_yuv444_10(&rgba, Some(&rgb), 2, 1, 0.2627, 0.0593, &mut y, &mut cb, &mut cr, Some(&mut alpha));
    assert_eq!(alpha, [514, 0]);
    assert!(y.iter().all(|v| *v > 64));
}

#[test]
fn hdr_prores_keeps_straight_colour_at_partial_and_zero_alpha() {
    use waltcut_color::{ColorPipeline, WorkingSpace};
    use waltcut_media::generators::GeneratorSource;
    use waltcut_media::{FrameRequest, Generator};
    use waltcut_project::{ItemKind, MediaRef};
    use waltcut_render::Image;

    let directory = waltcut_testkit::fixtures::workspace_root().join("target").join(format!("export-hdr-alpha-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for working in [WorkingSpace::Rec2100Pq, WorkingSpace::Rec2100Hlg] {
        let (mut encoded_reference, mut decoded_reference) = (None, None);
        for alpha in [1.0, 0.5, 0.0] {
            let (mut project, seq, mut sources) = crate::tests::project();
            let p = Arc::make_mut(&mut project);
            p.sequence_mut(seq).unwrap().settings.color = ColorPipeline { working, ..ColorPipeline::REC709 };
            let item = p.sequence(seq).unwrap().video_tracks[0].items[0].item;
            let source =
                GeneratorSource::new(Generator::ColorMatte { color: [1.0, 0.0, 0.0, alpha] }, 320, 180, FrameRate::FPS_24, FrameRate::FPS_24.tick_of(2));
            if let ItemKind::Media(media) = &mut p.item_mut(item).unwrap().kind {
                media.media = MediaRef::Generator(source.generator.clone());
            }
            sources.0.insert(item, Arc::new(source));
            let path = directory.join(format!("{}-{alpha}.mov", working.id()));
            let settings = ExportSettings {
                format: Format::ProRes,
                path: path.to_string_lossy().into_owned(),
                range: Some(TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(1))),
                include_audio: false,
                ..Default::default()
            };
            let pipe = crate::pipeline::Pipeline::new(project.clone(), seq, &settings, true).unwrap();
            let (rgba, rgb) = pipe.frame(0, &sources);
            assert!(rgba[3].abs_diff((alpha * 255.0).round() as u8) <= 1, "alpha {alpha} -> {}", rgba[3]);
            if alpha > 0.0 {
                let reference = encoded_reference.get_or_insert([rgb[0], rgb[1], rgb[2]]);
                for (actual, expected) in rgb[..3].iter().zip(reference.iter()) {
                    assert!((actual - expected).abs() < 1e-6, "{} alpha={alpha}: HDR colour must be straight before encoding", working.id());
                }
            } else {
                assert!(rgb[..3].iter().all(|v| v.abs() < 1e-6), "transparent HDR pixels must not divide by zero");
            }
            let report = export(&project, seq, &settings, &sources, &Progress::default()).unwrap();
            assert_eq!(report.frames, 1);
            let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
            let file = waltcut_isobmff::open(bytes.clone()).unwrap();
            let track = &file.tracks[file.track_of_kind(waltcut_isobmff::TrackKind::Video).unwrap()];
            assert_eq!(track.samples.len(), 1);
            assert_eq!(track.entries[0].format.0, *b"ap4h");
            let source = waltcut_codecs::open_bytes("alpha.mov", bytes).unwrap();
            let frame = source.video_frame(FrameRequest::full(Tick::ZERO)).unwrap();
            let decoded = waltcut_render::colorman::decode(&Project::new("roundtrip"), ItemId(0), &frame, 1, &ColorPipeline::REC709);
            let premul = decoded.get(160, 90);
            let pixel = Image::unpremul(premul);
            assert!((premul[3] - alpha).abs() < 0.01, "{}: {premul:?}", working.id());
            if alpha > 0.0 {
                let reference = decoded_reference.get_or_insert(pixel);
                for (actual, expected) in pixel[..3].iter().zip(&reference[..3]) {
                    assert!((actual - expected).abs() < 0.02, "{} alpha={alpha}: {pixel:?} != {reference:?}", working.id());
                }
            }
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn only_pq_has_static_metadata_and_it_is_what_the_container_writes() {
    assert!(ColorSignal::HLG.static_metadata().is_none());
    assert!(ColorSignal::default().static_metadata().is_none());
    let (md, cll) = ColorSignal::PQ.static_metadata().unwrap();
    assert_eq!(cll, (0, 0));
    assert_eq!(md, waltcut_isobmff::MasteringDisplay::bt2020(1000.0, 0.0001));
    assert_eq!(md.to_bytes().len(), 24);
    let mut entry = waltcut_isobmff::SampleEntry::prores(waltcut_isobmff::FourCc(*b"ap4h"), 64, 64);
    ColorSignal::PQ.apply_to(&mut entry, true);
    let video = entry.video.as_ref().unwrap();
    assert_eq!((video.mastering_display, video.content_light), (Some(md), Some((0, 0))));
    let mut entry = waltcut_isobmff::SampleEntry::prores(waltcut_isobmff::FourCc(*b"ap4h"), 64, 64);
    ColorSignal::HLG.apply_to(&mut entry, true);
    let video = entry.video.as_ref().unwrap();
    assert!(video.color.is_some() && video.mastering_display.is_none() && video.content_light.is_none());
}

fn unescape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut zeros = 0;
    for &byte in bytes {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        out.push(byte);
    }
    out
}

#[test]
fn software_h264_sei_contains_the_same_static_metadata() {
    let settings = ExportSettings { format: Format::H264, signal: ColorSignal::PQ, ..Default::default() };
    let mut encoder = h264_factory(64, 64, FrameRate::FPS_24, &settings).unwrap();
    let rgb = vec![0.5f32; 64 * 64 * 3];
    let mut packets = encoder.encode(&EncoderFrame { width: 64, height: 64, rgba: &[], hdr: Some(&rgb), index: 0 }).unwrap();
    packets.extend(encoder.flush().unwrap());
    let data = &packets[0].data;
    let (mut pos, mut sei) = (0usize, None);
    while let Some(len) = data.get(pos..pos + 4) {
        let n = u32::from_be_bytes([len[0], len[1], len[2], len[3]]) as usize;
        let nal = &data[pos + 4..pos + 4 + n];
        if nal[0] & 0x1f == 6 {
            sei = Some(unescape(&nal[1..]));
        }
        pos += 4 + n;
    }
    let (md, (cll, fall)) = ColorSignal::PQ.static_metadata().unwrap();
    let mut expected = vec![137u8, 24];
    expected.extend(md.to_bytes());
    expected.extend([144u8, 4]);
    expected.extend(cll.to_be_bytes());
    expected.extend(fall.to_be_bytes());
    expected.push(0x80);
    assert_eq!(sei.expect("SEI NAL in the first access unit"), expected);
}

#[test]
fn sdr_gets_a_colr_box_and_no_hdr_metadata() {
    let mut entry = waltcut_isobmff::SampleEntry::prores(waltcut_isobmff::FourCc(*b"ap4h"), 64, 64);
    ColorSignal::default().apply_to(&mut entry, true);
    let video = entry.video.as_ref().unwrap();
    assert_eq!(video.color, Some(waltcut_isobmff::ColorInfo::Nclc { primaries: 1, transfer: 1, matrix: 1 }));
    assert!(video.mastering_display.is_none() && video.content_light.is_none());
    let avcc = waltcut_isobmff::AvcConfig::new(Vec::new(), Vec::new(), 4);
    let mut entry = waltcut_isobmff::SampleEntry::avc(avcc, 64, 64);
    ColorSignal::default().apply_to(&mut entry, false);
    assert_eq!(entry.video.unwrap().color, Some(waltcut_isobmff::ColorInfo::Nclx { primaries: 1, transfer: 1, matrix: 1, full_range: false }));
}
