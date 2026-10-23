//! Retained timing audio through the real program playback callback and a written PCM export.
//! Synthetic WAVs live under target/ and are deleted when the fixture drops. No audio hardware,
//! GPU, ffmpeg or external media is required.

use std::path::PathBuf;

use serde_json::json;
use waltcut_engine::Session;
use waltcut_media::MediaSource;
use waltcut_media::wav::{WavSource, write_wav16};
use waltcut_time::{FrameRate, Tick};
use waltcut_ui_egui::playback_mix;

const SR: u32 = 48_000;
const RANGE_START: i64 = 2_000;
const RANGE_FRAMES: usize = 12_000;
const CLIP_START: i64 = 4_000;
const CLIP_FRAMES: i64 = 8_000;
const SOURCE_IN: i64 = 2_000;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join("audio-timing-tests").join(std::process::id().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            eprintln!("could not remove audio timing fixture {}: {e}", self.0.display());
        }
    }
}

/// Variable block sizes deliberately cut across clip, frame and DSP block boundaries.
fn capture(mix: &mut impl FnMut(i64, &mut [f32], usize)) -> Vec<f32> {
    let mut out = Vec::with_capacity(RANGE_FRAMES * 2);
    let mut pos = 0;
    for block in [1, 17, 64, 613].into_iter().cycle() {
        if pos == RANGE_FRAMES {
            break;
        }
        let n = block.min(RANGE_FRAMES - pos);
        let mut buf = vec![f32::NAN; n * 2];
        mix(RANGE_START + pos as i64, &mut buf, 2);
        assert!(buf.iter().all(|s| s.is_finite()), "playback must write every device sample");
        out.extend(buf);
        pos += n;
    }
    out
}

fn assert_samples(actual: &[f32], expected: &[f32], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}: sample count");
    for (i, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!((actual - expected).abs() <= 1e-6, "{context}: interleaved sample {i}: {actual} != {expected}");
    }
}

#[test]
fn clip_volume_and_track_mute_affect_playback_and_exported_audio() {
    let fixture = Fixture::new();
    for channels in [1u16, 2] {
        // Distinct channels and non-periodic clip/frame boundaries expose offsets and dropped samples.
        let input: Vec<f32> =
            (0..16_000).flat_map(|i| [(i % 251 - 125) as f32 / 512.0, (i % 197 - 98) as f32 / 512.0].into_iter().take(usize::from(channels))).collect();
        let bytes = write_wav16(&input, channels, SR);
        let original = WavSource::parse("source.wav", bytes.clone().into()).unwrap().audio(0, 16_000, SR).unwrap();
        let input_path = fixture.path(&format!("source-{channels}.wav"));
        std::fs::write(&input_path, bytes).unwrap();

        let mut session = Session::default();
        session.execute("file.newSequence", json!({"name": "Timing", "width": 32, "height": 32, "fps": 24, "sampleRate": SR, "video": 1, "audio": 1})).unwrap();
        let imported = session.execute("file.import", json!({"paths": [input_path]})).unwrap();
        assert_eq!(imported["errors"], json!([]));
        let item = imported["items"][0].as_u64().unwrap();
        let placed = session
            .execute(
                "timeline.place",
                json!({
                    "item": item, "track": "A1",
                    "time": Tick::from_units(CLIP_START, i64::from(SR)).0,
                    "sourceIn": Tick::from_units(SOURCE_IN, i64::from(SR)).0,
                    "duration": Tick::from_units(CLIP_FRAMES, i64::from(SR)).0
                }),
            )
            .unwrap();
        let clip = placed["clips"][0].as_u64().unwrap();
        let seq = session.state.active_sequence.unwrap();
        // Keep this callback alive across edits: playback must hear updated project snapshots.
        let mut mix = playback_mix(&session, seq, SR);
        let half_db = 20.0 * 0.5f64.log10();
        for (name, db, muted, level, mode) in [
            ("unity", 0.0, false, 0.0, "static"),
            ("half", half_db, false, 0.0, "static"),
            ("muted", half_db, true, 0.0, "static"),
            ("unmuted", half_db, false, 0.0, "static"),
            ("restored", 0.0, false, 0.0, "static"),
            ("volume", 0.0, false, half_db, "static"),
            ("gain_plus_volume", half_db, false, half_db, "static"),
            ("volume_disabled", half_db, false, half_db, "disabled"),
            ("volume_bypassed", half_db, false, half_db, "bypassed"),
            ("volume_floor", 0.0, false, -96.0, "static"),
            ("selected_mono", half_db, false, 0.0, "selected_mono"),
            ("volume_animated", half_db, false, -12.0, "animated"),
        ] {
            let selected = if mode == "selected_mono" { vec![channels - 1] } else { vec![] };
            session.execute("clip.audioChannels", json!({"clips": [clip], "channels": selected})).unwrap();
            let enabled = session.active_sequence().unwrap().find_item(waltcut_engine::project::ClipId(clip)).unwrap().1.effect("volume").unwrap().enabled;
            if enabled != (mode != "disabled") {
                session.execute("effects.toggleEnabled", json!({"clip": clip, "index": 0})).unwrap();
            }
            session.execute("effects.setParam", json!({"clip": clip, "effect": "volume", "param": "level", "value": level})).unwrap();
            session.execute("effects.setParam", json!({"clip": clip, "effect": "volume", "param": "bypass", "value": mode == "bypassed"})).unwrap();
            if mode == "animated" {
                for (sample, value) in [(CLIP_START, level), (CLIP_START + CLIP_FRAMES, 0.0)] {
                    session
                        .execute(
                            "effects.setParam",
                            json!({"clip": clip, "effect": "volume", "param": "level", "value": value,
                            "time": Tick::from_units(sample, i64::from(SR)).0, "keyframe": true}),
                        )
                        .unwrap();
                }
            }
            session.execute("clip.audioGain", json!({"clips": [clip], "mode": "set", "db": db})).unwrap();
            session.execute("timeline.setTrack", json!({"track": "A1", "muted": muted})).unwrap();
            let expected: Vec<f32> = (0..RANGE_FRAMES)
                .flat_map(|i| {
                    let timeline = RANGE_START + i as i64;
                    if (CLIP_START..CLIP_START + CLIP_FRAMES).contains(&timeline) {
                        let source = (SOURCE_IN + timeline - CLIP_START) as usize;
                        let volume_db = match mode {
                            "disabled" | "bypassed" => 0.0,
                            "animated" => level * (1.0 - (timeline - CLIP_START) as f64 / CLIP_FRAMES as f64),
                            _ => level,
                        };
                        let gain = if muted || db + volume_db <= -96.0 { 0.0 } else { 10f64.powf((db + volume_db) / 20.0) as f32 };
                        let left = if mode == "selected_mono" { usize::from(channels - 1) } else { 0 };
                        let right = usize::from(channels - 1);
                        [original.channels[left][source] * gain, original.channels[right][source] * gain]
                    } else {
                        [0.0, 0.0]
                    }
                })
                .collect();
            assert_eq!(expected.iter().any(|s| *s != 0.0), !muted && level > -96.0, "{name}: non-silent fixture");
            assert_samples(&capture(&mut mix), &expected, &format!("{name} playback"));

            let path = fixture.path(&format!("{channels}-{name}.wav"));
            let result = session
                .execute(
                    "file.exportMedia",
                    json!({
                        "path": path, "format": "wav", "range": "custom", "startFrame": 1, "endFrame": 7, "wait": true,
                        "settings": {"audio": {"sample_rate": SR, "bits": 32, "float": true}}
                    }),
                )
                .unwrap();
            assert_eq!(result["offlineMedia"], json!([]), "{name}: no offline silence");
            let exported = WavSource::parse("export.wav", std::fs::read(path).unwrap().into()).unwrap();
            assert_eq!(exported.info().audio().unwrap().sample_rate, SR);
            assert_eq!(exported.info().audio().unwrap().channels, 2);
            assert_eq!(exported.info().duration.to_units_floor(i64::from(SR)), RANGE_FRAMES as i64, "{name}: exact export duration");
            let audio = exported.audio(0, RANGE_FRAMES, SR).unwrap();
            let samples: Vec<f32> = audio.channels[0].iter().zip(&audio.channels[1]).flat_map(|(&l, &r)| [l, r]).collect();
            assert_samples(&samples, &expected, &format!("{name} exported WAV"));
        }
        assert_eq!(FrameRate::FPS_24.tick_of(1).to_units_floor(i64::from(SR)), RANGE_START);
    }
}

#[test]
fn removed_music_retiming_tool_is_not_a_ui_command_or_serializable_tool() {
    assert!(!waltcut_ui_egui::menus::external_commands().iter().any(|c| c.id == "tool.remix"));
    assert!(waltcut_ui_egui::state::Tool::from_name("Remix").is_none());
    assert!(serde_json::from_value::<waltcut_ui_egui::state::Tool>(json!("Remix")).is_err());
}
