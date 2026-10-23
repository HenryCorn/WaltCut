//! Oracle-backed tests (skipped when ffmpeg is unavailable).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use waltcut_media::FrameRequest;
use waltcut_time::{TICKS_PER_SECOND, Tick};

pub(crate) fn ffmpeg() -> Option<&'static str> {
    ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"].into_iter().find(|p| std::path::Path::new(p).exists())
}

fn fixture_dir() -> PathBuf {
    // The source includes all recipe arguments and inline inputs; edits invalidate old media.
    waltcut_testkit::fixtures::recipe_dir("codecs", include_str!("tests.rs"))
}

fn fixture(name: &str, args: &[&str]) -> Option<Arc<[u8]>> {
    let ff = ffmpeg().or_else(|| {
        eprintln!("ffmpeg not found; skipping");
        None
    })?;
    let dir = fixture_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let out = dir.join(name);
    waltcut_testkit::fixtures::generate(&out, |tmp| Command::new(ff).args(["-y", "-v", "error"]).args(args).arg(tmp).status().is_ok_and(|s| s.success()))?;
    Some(std::fs::read(out).ok()?.into())
}

fn check_mkv(name: &str, video_args: &[&str], audio_args: &[&str]) {
    let Some(ffmpeg) = ffmpeg() else {
        eprintln!("ffmpeg not found; skipping");
        return;
    };
    let path = fixture_dir().join(name);
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        eprintln!("could not create fixture directory; skipping");
        return;
    }
    let generated = waltcut_testkit::fixtures::generate(&path, |tmp| {
        let status = Command::new(ffmpeg)
            .args(["-y", "-v", "error", "-f", "lavfi", "-i", "testsrc2=s=160x120:r=25:d=1", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:d=1"])
            .args(video_args)
            .args(audio_args)
            .args(["-shortest"])
            .arg(tmp)
            .status();
        status.is_ok_and(|status| status.success())
    });
    if generated.is_none() {
        eprintln!("fixture {name} failed; skipping");
        return;
    }
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("fixture {name} could not be read; skipping");
        return;
    };
    let Ok(source) = crate::open_bytes(name, bytes.into()) else {
        panic!("generated Matroska fixture {name} did not open");
    };
    assert!(source.info().video.is_some(), "{name} has no video");
    assert!(!source.info().audio_streams.is_empty(), "{name} has no audio");
    let frame = source.video_frame(FrameRequest::full(Tick(0))).unwrap();
    assert_eq!((frame.width, frame.height), (160, 120));
    let audio = source.audio(0, 4800, 48_000).unwrap();
    assert!(audio.peaks()[0] > 0.01, "{name} has silent audio");
}

#[test]
fn mjpeg_mov_with_pcm() {
    let Some(b) = fixture(
        "red_mjpeg.mov",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=320x240:r=24:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=48000:d=2",
            "-c:v",
            "mjpeg",
            "-q:v",
            "2",
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ],
    ) else {
        return;
    };
    let src = crate::open_bytes("red_mjpeg.mov", b).unwrap();
    let info = src.info().clone();
    assert_eq!(info.video.as_ref().unwrap().width, 320);
    assert_eq!(info.video.as_ref().unwrap().frame_rate, waltcut_time::FrameRate::FPS_24);
    let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND))).unwrap();
    let px = f.to_rgba8();
    let c = &px[(120 * 320 + 160) * 4..][..3];
    assert!(c[0] > 230 && c[1] < 30 && c[2] < 30, "{c:?}");
    let a = src.audio(0, 4800, 48_000).unwrap();
    let peak = a.peaks()[0];
    assert!(peak > 0.1, "{peak}");
}

#[test]
fn aac_mp4_decodes() {
    let Some(b) = fixture("tone_aac.mp4", &["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:d=3", "-c:a", "aac", "-b:a", "128k"]) else { return };
    let src = crate::open_bytes("tone_aac.mp4", b).unwrap();
    assert!(src.info().audio().unwrap().codec.contains("AAC"));
    // read in the middle (random access) and sequentially
    let a = src.audio(48_000, 4800, 48_000).unwrap();
    let p = a.peaks()[0];
    assert!(p > 0.05 && p < 1.0, "{p}");
    let b2 = src.audio(48_000 + 4800, 4800, 44_100).unwrap();
    assert!(b2.peaks()[0] > 0.05);
}

#[test]
fn mp3_file_decodes() {
    let Some(b) = fixture("tone.mp3", &["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100:d=2", "-c:a", "libmp3lame", "-b:a", "192k"]) else { return };
    let src = crate::open_bytes("tone.mp3", b).unwrap();
    assert!((src.info().duration.seconds() - 2.0).abs() < 0.2, "{}", src.info().duration.seconds());
    let a = src.audio(44_100, 4410, 44_100).unwrap();
    assert!(a.peaks()[0] > 0.1);
}

#[test]
fn aiff_file_decodes() {
    // 0.1 s of 48 kHz 16-bit stereo PCM: left a ramp, right silent (AIFF, big-endian samples)
    let frames = 4800u32;
    let mut b = b"FORM".to_vec();
    b.extend((4 + 26 + 16 + frames * 4).to_be_bytes());
    b.extend(b"AIFFCOMM");
    b.extend(18u32.to_be_bytes());
    b.extend(2u16.to_be_bytes());
    b.extend(frames.to_be_bytes());
    b.extend(16u16.to_be_bytes());
    b.extend([0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]); // 48000 as an 80-bit extended float
    b.extend(b"SSND");
    b.extend((8 + frames * 4).to_be_bytes());
    b.extend([0; 8]);
    for i in 0..frames {
        b.extend(((i as i16) * 4).to_be_bytes());
        b.extend(0i16.to_be_bytes());
    }
    let src = crate::open_bytes("tone.aif", b.into()).unwrap();
    assert!((src.info().duration.seconds() - 0.1).abs() < 1e-3, "{}", src.info().duration.seconds());
    let a = src.audio(0, 4800, 48_000).unwrap();
    assert_eq!(a.channels.len(), 2);
    assert!((a.channels[0][1000] - 4000.0 / 32768.0).abs() < 1e-4, "{}", a.channels[0][1000]);
    assert_eq!(a.channels[1][1000], 0.0);
}

#[test]
fn seek_backwards_and_forwards_mjpeg() {
    let Some(b) = fixture("counter_mjpeg.mov", &["-f", "lavfi", "-i", "testsrc2=s=160x120:r=25:d=3", "-c:v", "mjpeg"]) else { return };
    let src = crate::open_bytes("counter_mjpeg.mov", b).unwrap();
    let rate = src.info().frame_rate();
    let a = src.video_frame(FrameRequest::full(rate.tick_of(60))).unwrap();
    let b = src.video_frame(FrameRequest::full(rate.tick_of(10))).unwrap();
    let c = src.video_frame(FrameRequest::full(rate.tick_of(60))).unwrap();
    assert_eq!(a.to_rgba8(), c.to_rgba8());
    assert_ne!(a.to_rgba8(), b.to_rgba8());
}

#[test]
fn h264_mp4_decodes_and_seeks() {
    let Some(b) = fixture(
        "mandel_h264.mp4",
        &["-f", "lavfi", "-i", "mandelbrot=s=640x360:r=25", "-t", "4", "-c:v", "libx264", "-preset", "fast", "-bf", "3", "-g", "25", "-pix_fmt", "yuv420p"],
    ) else {
        return;
    };
    let src = crate::open_bytes("mandel_h264.mp4", b).unwrap();
    assert!(src.info().video.as_ref().unwrap().codec.contains("H.264"));
    let rate = src.info().frame_rate();
    let late = src.video_frame(FrameRequest::full(rate.tick_of(70))).unwrap();
    assert_eq!((late.width, late.height), (640, 360));
    let early = src.video_frame(FrameRequest::full(rate.tick_of(3))).unwrap();
    let again = src.video_frame(FrameRequest::full(rate.tick_of(70))).unwrap();
    assert_eq!(late.to_rgba8(), again.to_rgba8(), "random access is deterministic");
    assert_ne!(late.to_rgba8(), early.to_rgba8());
    // sequential access after a seek
    for f in 71..90 {
        src.video_frame(FrameRequest::full(rate.tick_of(f))).unwrap();
    }
}

#[test]
fn prores_mov_decodes() {
    let Some(b) = fixture("bars_prores.mov", &["-f", "lavfi", "-i", "smptehdbars=s=640x360:r=24:d=1", "-c:v", "prores_ks", "-profile:v", "3"]) else { return };
    let src = crate::open_bytes("bars_prores.mov", b).unwrap();
    assert!(src.info().video.as_ref().unwrap().codec.contains("ProRes"));
    let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 2))).unwrap();
    assert!(f.format_label().contains("4:2:2"));
    let px = f.to_rgba8();
    // leftmost bars area is 40% grey
    let c = &px[(100 * 640 + 20) * 4..][..3];
    assert!((c[0] as i32 - 104).abs() < 6, "{c:?}");
}

#[test]
fn matroska_h264_aac() {
    check_mkv("blue_h264_aac.mkv", &["-c:v", "libx264", "-bf", "2", "-pix_fmt", "yuv420p"], &["-c:a", "aac"]);
}

#[test]
fn matroska_prores_pcm() {
    check_mkv("blue_prores_pcm.mkv", &["-c:v", "prores_ks", "-profile:v", "2"], &["-c:a", "pcm_s16le"]);
}

#[test]
fn removed_video_codecs_in_retained_containers_are_rejected_actionably() {
    for (name, args, token) in [
        ("unsupported_vp8.webm", &["-f", "lavfi", "-i", "testsrc2=s=64x48:r=25:d=0.2", "-c:v", "libvpx", "-deadline", "realtime"][..], "vp8"),
        (
            "unsupported_dnx.mov",
            &["-f", "lavfi", "-i", "testsrc2=s=1280x720:r=25:d=0.2", "-c:v", "dnxhd", "-profile:v", "dnxhr_lb", "-pix_fmt", "yuv422p"][..],
            "dnx",
        ),
    ] {
        let Some(bytes) = fixture(name, args) else { continue };
        let err = crate::open_bytes(name, bytes).err().expect("removed codec must fail during import");
        let message = err.to_string().to_ascii_lowercase();
        assert!(message.contains(token) && message.contains("convert"), "{name}: {err}");
    }
}

/// `bytes` with its first `DefaultDuration` element (0x23E383) overwritten by an EBML Void of the
/// same length, as a muxer that leaves the optional element out would write the track.
fn without_default_duration(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let at = out.windows(3).position(|w| w == [0x23, 0xE3, 0x83]).expect("DefaultDuration");
    // ffmpeg writes a one-byte size (0x80 | n)
    let total = 4 + (out[at + 3] & 0x7F) as usize;
    out[at] = 0xEC;
    out[at + 1] = 0x80 | (total - 2) as u8;
    out[at + 2..at + total].fill(0);
    out
}

#[test]
fn matroska_reordered_video_without_default_duration_keeps_its_rate() {
    // blocks are in decode order, so B-frames put file neighbours 2–3 frames apart (#923)
    for (bf, name) in [(0, "rate25_bf0.mkv"), (1, "rate25_bf1.mkv"), (2, "rate25_bf2.mkv")] {
        let params = format!("bframes={bf}:b-adapt=0:b-pyramid=0:keyint=250:scenecut=0");
        let args =
            ["-f", "lavfi", "-i", "testsrc2=size=64x48:rate=25", "-frames:v", "25", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-x264-params", &params, "-an"];
        let Some(control) = fixture(name, &args) else { return };
        let stripped = without_default_duration(&control);
        for (label, bytes) in [("with DefaultDuration", control.to_vec()), ("without", stripped)] {
            let src = crate::open_bytes(name, bytes.into()).expect("open");
            let rate = src.info().video.as_ref().expect("video").frame_rate;
            assert_eq!((rate.num, rate.den), (25, 1), "{bf} B-frames, {label}");
            assert_eq!(src.info().duration, Tick::from_rational(1, 1, 1), "{bf} B-frames, {label}");
        }
    }
}

/// Path of a fixture under `target/fixtures/codecs/`, generated with ffmpeg when missing
/// (`None` when ffmpeg or the encoder is unavailable).
pub(crate) fn fixture_path(name: &str, args: &[&str]) -> Option<PathBuf> {
    fixture(name, args)?;
    Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/codecs").join(name))
}

/// ffmpeg's decode of `src` as raw planar video (the oracle).
pub(crate) fn reference_yuv(src: &std::path::Path, pix_fmt: &str) -> Option<Vec<u8>> {
    let name = format!("{}.{pix_fmt}.yuv", src.file_name()?.to_str()?);
    let path = src.to_str()?;
    let b = fixture(&name, &["-i", path, "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", pix_fmt])?;
    Some(b.to_vec())
}

/// Planes of a decoded YUV frame as little-endian bytes (ffmpeg rawvideo layout).
pub(crate) fn yuv_bytes(f: &waltcut_frame::VideoFrame) -> Vec<u8> {
    match &f.data {
        waltcut_frame::PixelData::Yuv8 { planes, .. } => planes.iter().flat_map(|p| p.iter().copied()).collect(),
        waltcut_frame::PixelData::Yuv16 { planes, .. } => planes.iter().flat_map(|p| p.iter().flat_map(|s| s.to_le_bytes())).collect(),
        _ => panic!("not a YUV frame: {}", f.format_label()),
    }
}

/// Random access into a VP9 file: frames requested out of order (seeks back and forth across key
/// frames, then sequential playback) are sample-exact against ffmpeg's decode.
fn check_vp9_seeks(name: &str, container: &str, pix_fmt: &str, extra: &[&str], frames: usize) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/codecs");
    let log = dir.join(format!("{name}.passlog")).to_string_lossy().into_owned();
    let mut args = vec!["-f", "lavfi", "-i", "testsrc2=s=352x288:r=25,noise=alls=8:allf=t", "-frames:v"];
    let n = frames.to_string();
    args.push(&n);
    args.extend_from_slice(&["-c:v", "libvpx-vp9", "-pix_fmt", pix_fmt, "-g", "25", "-b:v", "600k"]);
    args.extend_from_slice(extra);
    if extra.contains(&"-auto-alt-ref") {
        // libvpx only uses alternate reference frames in two-pass mode: run the first pass.
        let Some(ff) = ffmpeg() else { return };
        if !dir.join(name).exists() {
            let _ = std::fs::create_dir_all(&dir);
            let ok = Command::new(ff).args(["-y", "-v", "error"]).args(&args).args(["-pass", "1", "-passlogfile"]).arg(&log).args(["-f", "null", "-"]).status();
            if !ok.is_ok_and(|s| s.success()) {
                eprintln!("first pass for {name} failed; skipping");
                return;
            }
        }
        args.extend_from_slice(&["-pass", "2", "-passlogfile"]);
        args.push(&log);
    }
    let Some(path) = fixture_path(name, &args) else { return };
    let Some(reference) = reference_yuv(&path, pix_fmt) else { return };
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let src = crate::open_bytes(name, bytes).unwrap();
    let info = src.info().clone();
    assert_eq!(info.container, container);
    let v = info.video.as_ref().unwrap();
    assert!(v.codec.contains("VP9"), "{}", v.codec);
    assert_eq!((v.width, v.height), (352, 288));
    let frame_len = reference.len() / frames;
    let rate = info.frame_rate();
    let order: Vec<usize> = [frames - 3, 3, 40, 26, 24, frames - 1, 0, 12].into_iter().chain(30..45).filter(|&k| k < frames).collect();
    for k in order {
        let f = src.video_frame(FrameRequest::full(rate.tick_of(k as i64))).unwrap();
        assert_eq!((f.width, f.height), (352, 288));
        let got = yuv_bytes(&f);
        assert!(got == reference[k * frame_len..(k + 1) * frame_len], "{name}: frame {k} differs from ffmpeg");
    }
}

/// AV1 (libsvtav1) in MP4 / WebM: random access is sample-exact against libdav1d. `keyint`
/// "1" is all-intra; larger values give hierarchical inter GOPs (hidden ALTREFs, shown-existing
/// frames), so seeks decode forward from the previous key frame.
fn check_av1_seeks(name: &str, container: &str, pix_fmt: &str, frames: usize, keyint: &str) {
    let params = format!("keyint={keyint}");
    let n = frames.to_string();
    let args = [
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=320x240:r=25,noise=alls=8:allf=t",
        "-frames:v",
        &n,
        "-c:v",
        "libsvtav1",
        "-pix_fmt",
        pix_fmt,
        "-preset",
        "8",
        "-crf",
        "40",
        "-svtav1-params",
        &params,
    ];
    let Some(path) = fixture_path(name, &args) else { return };
    let ref_name = format!("{name}.{pix_fmt}.yuv");
    let p = path.to_str().unwrap();
    let Some(reference) = fixture(&ref_name, &["-c:v", "libdav1d", "-i", p, "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", pix_fmt]) else { return };
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let src = crate::open_bytes(name, bytes).unwrap();
    let info = src.info().clone();
    assert_eq!(info.container, container);
    let v = info.video.as_ref().unwrap();
    assert!(v.codec.contains("AV1"), "{}", v.codec);
    assert_eq!((v.width, v.height), (320, 240));
    let frame_len = reference.len() / frames;
    let rate = info.frame_rate();
    for k in [frames - 1, 2, 7, 0, 5, 6, 3, frames / 2 + 1, frames - 2] {
        let f = src.video_frame(FrameRequest::full(rate.tick_of(k as i64))).unwrap();
        assert_eq!((f.width, f.height), (320, 240));
        assert!(yuv_bytes(&f) == reference[k * frame_len..(k + 1) * frame_len], "{name}: frame {k} differs from libdav1d");
    }
}

#[test]
fn mp4_av1_intra_seeks_bit_exact() {
    check_av1_seeks("noise_av1_intra.mp4", "MPEG-4", "yuv420p", 10, "1");
}

#[test]
fn mp4_av1_inter_gop_seeks_bit_exact() {
    check_av1_seeks("noise_av1_gop16.mp4", "MPEG-4", "yuv420p", 40, "16");
}

#[test]
fn webm_av1_inter_gop_10bit_seeks_bit_exact() {
    check_av1_seeks("noise_av1_gop16_10bit.webm", "WebM", "yuv420p10le", 24, "16");
}

#[test]
fn webm_av1_intra_10bit_seeks_bit_exact() {
    check_av1_seeks("noise_av1_intra_10bit.webm", "WebM", "yuv420p10le", 10, "1");
}

#[test]
fn webm_vp9_seeks_bit_exact() {
    check_vp9_seeks("noise_vp9.webm", "WebM", "yuv420p", &["-deadline", "realtime", "-speed", "8"], 60);
}

#[test]
fn webm_vp9_altref_superframes_seek_bit_exact() {
    // Good-quality encode with alternate reference frames: hidden frames packed into superframes.
    check_vp9_seeks("noise_vp9_altref.webm", "WebM", "yuv420p", &["-speed", "4", "-auto-alt-ref", "1", "-lag-in-frames", "16"], 60);
}

#[test]
fn mp4_vp9_10bit_seeks_bit_exact() {
    check_vp9_seeks("noise_vp9_10bit.mp4", "MPEG-4", "yuv420p10le", &["-profile:v", "2", "-deadline", "realtime", "-speed", "8"], 50);
}

#[test]
fn mkv_vp9_444_12bit_seeks_bit_exact() {
    check_vp9_seeks("noise_vp9_444_12bit.mkv", "Matroska", "yuv444p12le", &["-profile:v", "3", "-deadline", "realtime", "-speed", "8"], 30);
}

#[test]
fn mkv_vp9_422_seeks_bit_exact() {
    check_vp9_seeks("noise_vp9_422.mkv", "Matroska", "yuv422p", &["-profile:v", "1", "-deadline", "realtime", "-speed", "8"], 30);
}

/// #402: a WebM whose VP9 track carries an alpha layer (`AlphaMode` 1, a second VP9 stream in
/// each block's `BlockAdditional`) decodes with that alpha. Frames requested out of order (seeks
/// across key frames, then forward) match libvpx's decode exactly, colour and alpha.
#[test]
fn vp9_webm_alpha_layer_matches_libvpx() {
    let (w, h, frames) = (176usize, 144usize, 30usize);
    // the alpha changes every frame, so a picture paired with the wrong alpha frame shows
    let Some(path) = fixture_path(
        "alpha_vp9.webm",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=176x144:r=25,format=yuva420p,geq=lum='lum(X,Y)':cb='cb(X,Y)':cr='cr(X,Y)':a='mod(X*2+N*7\\,256)'",
            "-frames:v",
            "30",
            "-c:v",
            "libvpx-vp9",
            "-pix_fmt",
            "yuva420p",
            "-g",
            "10",
            "-deadline",
            "realtime",
            "-speed",
            "8",
            "-b:v",
            "600k",
        ],
    ) else {
        return;
    };
    // the reference: ffmpeg's libvpx decoder (its native VP9 decoder ignores the alpha layer)
    let src_path = path.to_string_lossy().into_owned();
    let Some(reference) =
        fixture("alpha_vp9.webm.yuva420p.yuv", &["-c:v", "libvpx-vp9", "-i", &src_path, "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "yuva420p"])
    else {
        return;
    };
    let (luma, chroma) = (w * h, (w / 2) * (h / 2));
    let frame_len = luma * 2 + chroma * 2;
    if reference.len() != frame_len * frames {
        eprintln!("libvpx reference has {} bytes, not {}; skipping", reference.len(), frame_len * frames);
        return;
    }
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let src = crate::open_bytes("alpha_vp9.webm", bytes).unwrap();
    let info = src.info().clone();
    assert_eq!(info.container, "WebM");
    assert!(info.video.as_ref().unwrap().has_alpha);
    let rate = info.frame_rate();
    let order: Vec<usize> = [29, 3, 17, 12, 0, 25, 21, 9].into_iter().chain(10..frames).collect();
    for k in order {
        let f = src.video_frame(FrameRequest::full(rate.tick_of(k as i64))).unwrap();
        let r = &reference[k * frame_len..(k + 1) * frame_len];
        let waltcut_frame::PixelData::Yuv8 { planes, alpha, .. } = &f.data else { panic!("frame {k}: not 8-bit YUV: {}", f.format_label()) };
        assert_eq!(planes[0].as_slice(), &r[..luma], "frame {k}: luma");
        let alpha = alpha.as_ref().unwrap_or_else(|| panic!("frame {k}: no alpha plane"));
        assert_eq!(alpha.as_slice(), &r[luma + 2 * chroma..], "frame {k}: alpha");
    }
}

/// VP9 random access restarts at genuine key frames even when a container marks every sample sync.
#[test]
fn vp9_random_access_ignores_bogus_sync_flags() {
    use crate::gop::{GopCache, VideoSamples};
    let Some(path) = fixture_path(
        "noise_vp9_g20.ivf",
        &["-f", "lavfi", "-i", "testsrc2=s=176x144:r=25", "-frames:v", "50", "-c:v", "libvpx-vp9", "-g", "20", "-deadline", "realtime", "-speed", "8"],
    ) else {
        return;
    };
    let Some(reference) = reference_yuv(&path, "yuv420p") else { return };
    let data = std::fs::read(&path).unwrap();
    let mut chunks = Vec::new();
    let mut p = 32;
    while p + 12 <= data.len() {
        let sz = u32::from_le_bytes(data[p..p + 4].try_into().unwrap()) as usize;
        chunks.push(data[p + 12..p + 12 + sz].to_vec());
        p += 12 + sz;
    }
    struct AllSync(Vec<Vec<u8>>);
    impl VideoSamples for AllSync {
        fn count(&self) -> usize {
            self.0.len()
        }
        fn pts(&self, i: usize) -> i64 {
            i as i64
        }
        fn sync_before(&self, i: usize) -> usize {
            i
        }
        fn sample_at(&self, t: i64) -> Option<usize> {
            (t >= 0 && (t as usize) < self.0.len()).then_some(t as usize)
        }
        fn read(&self, i: usize) -> crate::Result<Vec<u8>> {
            Ok(self.0[i].clone())
        }
        fn make_decoder(&self) -> crate::Result<Box<dyn crate::VideoDecoder>> {
            Ok(Box::new(crate::video::Vp9Decoder::new(None)))
        }
    }
    let s = AllSync(chunks);
    let cache = GopCache::new(None);
    let frame_len = 176 * 144 * 3 / 2;
    for k in [37usize, 5, 49, 21, 20, 19] {
        let f = cache.frame(&s, k as i64).unwrap();
        assert!(yuv_bytes(&f) == reference[k * frame_len..(k + 1) * frame_len], "frame {k} differs from ffmpeg");
    }
}

/// VP9 RGB (profile 1, colour space sRGB; planes G, B, R) decodes to RGBA in the right order.
#[test]
fn mkv_vp9_rgb() {
    let Some(b) = fixture(
        "orange_vp9_gbrp.mkv",
        &["-f", "lavfi", "-i", "color=c=0xe08020:s=128x96:r=25:d=0.4", "-c:v", "libvpx-vp9", "-pix_fmt", "gbrp", "-deadline", "realtime", "-lossless", "1"],
    ) else {
        return;
    };
    let src = crate::open_bytes("orange_vp9_gbrp.mkv", b).unwrap();
    let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 5))).unwrap();
    let px = f.to_rgba8();
    let c = &px[(48 * 128 + 64) * 4..][..3];
    // (the lavfi colour source is converted to RGB by ffmpeg, which rounds by a level)
    for (got, want) in c.iter().zip([0xe0u8, 0x80, 0x20]) {
        assert!((*got as i32 - want as i32).abs() <= 2, "{c:?}");
    }
}

/// ffmpeg + libopus decode of a container fixture (pre-skip / codec delay / edit list applied) as
/// interleaved f32 at 48 kHz: the reference for our Opus path.
fn opus_reference(name: &str) -> Option<Vec<f32>> {
    let ff = ffmpeg()?;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/codecs");
    let out = dir.join(format!("{name}.ref.f32"));
    let st = Command::new(ff)
        .args(["-y", "-v", "error", "-c:a", "libopus", "-i"])
        .arg(dir.join(name))
        .args(["-f", "f32le", "-ar", "48000"])
        .arg(&out)
        .status()
        .ok()?;
    if !st.success() {
        return None;
    }
    Some(std::fs::read(out).ok()?.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn snr_db(reference: &[f32], test: &[f32]) -> f64 {
    let (mut s, mut e) = (0f64, 0f64);
    for (r, t) in reference.iter().zip(test) {
        s += (*r as f64).powi(2);
        e += (*r as f64 - *t as f64).powi(2);
    }
    if e == 0.0 { 200.0 } else { 10.0 * (s.max(1e-20) / e).log10() }
}

/// Interleaves `frames` of `src.audio` from `start` (48 kHz).
fn read_interleaved(src: &waltcut_media::SharedSource, start: i64, frames: usize) -> Vec<f32> {
    let a = src.audio(start, frames, 48_000).expect("audio");
    let ch = a.channels.len();
    let mut v = vec![0f32; frames * ch];
    for (c, chan) in a.channels.iter().enumerate() {
        for (i, s) in chan.iter().enumerate() {
            v[i * ch + c] = *s;
        }
    }
    v
}

/// Opus in a container: metadata, sample-exact alignment (pre-skip honoured) against libopus for a
/// sequential read, and the same samples after random access (pre-roll).
fn check_opus(name: &str, channels: usize, expr: &str, extra: &[&str], container: &str) {
    let mut args: Vec<&str> = vec!["-f", "lavfi", "-i", expr, "-t", "3", "-c:a", "libopus"];
    args.extend_from_slice(extra);
    let Some(b) = fixture(name, &args) else { return };
    let Some(reference) = opus_reference(name) else {
        eprintln!("libopus decoder unavailable; skipping {name}");
        return;
    };
    let src = crate::open_bytes(name, b).expect("open");
    let info = src.info().clone();
    assert_eq!(info.container, container);
    let a = info.audio().expect("audio");
    assert_eq!((a.codec.as_str(), a.sample_rate, a.channels as usize), ("Opus", 48_000, channels));
    let d = info.duration.seconds();
    assert!((d - 3.0).abs() < 0.03, "{name}: duration {d}");
    let total = reference.len() / channels;
    assert!((total as i64 - 144_000).abs() < 960, "{name}: reference length {total}");

    // Sequential read in 100 ms blocks.
    let mut ours = Vec::with_capacity(reference.len());
    let mut pos = 0;
    while pos < total {
        let n = 4800.min(total - pos);
        ours.extend(read_interleaved(&src, pos as i64, n));
        pos += n;
    }
    let snr = snr_db(&reference, &ours);
    eprintln!("{name}: sequential SNR vs libopus {snr:.1} dB");
    assert!(snr > 40.0, "{name}: sequential SNR {snr:.1} dB");

    // Random access on a fresh source (cold decoder): 100 ms at 1.7 s, then back to 0.5 s.
    let src = crate::open_bytes(name, fixture(name, &args).expect("fixture")).expect("open");
    for start in [81_600usize, 24_000] {
        let got = read_interleaved(&src, start as i64, 4800);
        let want = &reference[start * channels..(start + 4800) * channels];
        let snr = snr_db(want, &got);
        eprintln!("{name}: random access at {start}: SNR {snr:.1} dB");
        assert!(snr > 40.0, "{name}: random access at {start}: SNR {snr:.1} dB");
    }
}

#[test]
fn opus_webm_stereo() {
    check_opus(
        "tones_opus.webm",
        2,
        "aevalsrc=0.4*sin(2*PI*440*t)+0.1*sin(2*PI*5000*t)|0.3*sin(2*PI*660*t)+0.1*sin(2*PI*3100*t):s=48000",
        &["-b:a", "128k"],
        "WebM",
    );
}

#[test]
fn opus_mkv_mono_speechlike_16k() {
    // Low bitrate VoIP mode exercises SILK/hybrid; the input rate is 16 kHz but Opus still outputs 48 kHz.
    check_opus("tones_opus_voip.mkv", 1, "aevalsrc=0.4*sin(2*PI*220*t)*(0.6+0.4*sin(2*PI*3*t)):s=16000", &["-b:a", "16k", "-application", "voip"], "Matroska");
}

#[test]
fn opus_mkv_surround_51() {
    check_opus(
        "tones_opus_51.mkv",
        6,
        "aevalsrc=0.3*sin(2*PI*300*t)|0.3*sin(2*PI*400*t)|0.3*sin(2*PI*500*t)|0.3*sin(2*PI*60*t)|0.3*sin(2*PI*700*t)|0.3*sin(2*PI*800*t):s=48000:c=5.1",
        &["-b:a", "256k"],
        "Matroska",
    );
}

#[test]
fn opus_mp4_stereo() {
    check_opus(
        "tones_opus.mp4",
        2,
        "aevalsrc=0.4*sin(2*PI*440*t)+0.1*sin(2*PI*5000*t)|0.3*sin(2*PI*660*t)+0.1*sin(2*PI*3100*t):s=48000",
        &["-b:a", "128k"],
        "MPEG-4",
    );
}

/// ffmpeg writes MP4 without a `colr` box unless asked: the colour comes from the x265 / x264
/// SPS VUI, and matches the same stream remuxed with `colr`.
#[test]
fn mp4_colour_comes_from_the_sps_without_colr() {
    use waltcut_color::{ColorSpace, Primaries, Range};
    let src = |name: &str, args: &[&str]| fixture(name, args).map(|b| crate::open_bytes(name, b).expect("open"));
    let color = |s: &waltcut_media::SharedSource| s.info().video.as_ref().expect("video").color;
    let lavfi = |size: &'static str| ["-f", "lavfi", "-i", size, "-t", "0.2"];
    let x265 = "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:log-level=error";
    let mut pq = lavfi("testsrc2=s=640x360:r=25").to_vec();
    pq.extend(["-c:v", "libx265", "-tag:v", "hvc1", "-pix_fmt", "yuv420p10le", "-x265-params", x265]);
    let Some(plain) = src("hevc_pq_no_colr.mp4", &pq) else { return };
    assert_eq!(ColorSpace::from_info(&color(&plain)), ColorSpace::Rec2100Pq);
    let colr_path = fixture_path("hevc_pq_no_colr.mp4", &pq).expect("fixture");
    if let Some(with_colr) = src("hevc_pq_colr.mp4", &["-i", colr_path.to_str().expect("path"), "-c", "copy", "-movflags", "+write_colr"]) {
        assert_eq!(color(&with_colr), color(&plain));
    }
    let mut full = lavfi("testsrc2=s=640x360:r=25").to_vec();
    full.extend(["-c:v", "libx264", "-pix_fmt", "yuvj420p", "-color_range", "pc"]);
    if let Some(s) = src("h264_full_no_colr.mp4", &full) {
        assert_eq!(color(&s).range, Range::Full);
    }
    let mut sd = lavfi("testsrc2=s=720x480:r=30000/1001").to_vec();
    sd.extend(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-x264-params", "colorprim=smpte170m:transfer=smpte170m:colormatrix=smpte170m"]);
    if let Some(s) = src("h264_sd601_no_colr.mp4", &sd) {
        assert_eq!(color(&s).primaries, Primaries::Bt601_525);
    }
}
