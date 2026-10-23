//! The bounded export command surface, real retained files, and single-worker ownership.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use waltcut_media::generators::GeneratorSource;
use waltcut_media::{FrameRequest, Generator, MediaSource, SharedSource};
use waltcut_project::{ItemId, Label};
use waltcut_time::FrameRate;

use crate::Session;

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let directory = waltcut_testkit::workspace_root().join("target").join("export-tests").join(format!("engine-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn session() -> (Session, ItemId) {
    let mut session = Session::default();
    session.execute("file.newSequence", json!({"name":"Scene.with.dots", "width":32, "height":18, "fps":24, "video":1, "audio":1})).unwrap();
    let source = GeneratorSource::new(Generator::ColorMatte { color: [1.0, 0.0, 0.0, 0.5] }, 32, 18, FrameRate::FPS_24, FrameRate::FPS_24.tick_of(24));
    let item = crate::demo::add_generator(Arc::make_mut(&mut session.project), &session.media, source, "Red", Label::Iris, None);
    session.execute("timeline.place", json!({"item":item.0,"track":"V1"})).unwrap();
    (session, item)
}

fn movie_params(path: String) -> Value {
    json!({"path":path,"format":"prores","range":"custom","startFrame":0,"endFrame":2,"wait":true})
}

#[test]
fn only_quick_export_remains_in_the_export_command_namespace() {
    let ids: Vec<_> = crate::command_specs().iter().filter(|c| c.id.starts_with("export.")).map(|c| c.id).collect();
    assert_eq!(ids, ["export.quick"]);
    let (mut session, _) = session();
    let before = session.project.to_json();
    for command in [
        "export.queue.add",
        "export.queue.list",
        "export.queue.start",
        "export.queue.cancel",
        "export.queue.remove",
        "export.queue.retry",
        "export.queue.reorder",
        "export.presets.list",
        "export.presets.save",
        "export.presets.delete",
        "export.presets.get",
        "export.formats",
        "export.resolve",
    ] {
        assert!(crate::find_command(command).is_none(), "{command}");
        assert!(session.execute(command, json!({})).is_err(), "{command}");
    }
    for command in ["file.exportMedia", "file.exportFrame", "export.quick"] {
        assert!(crate::find_command(command).is_some(), "{command}");
    }
    assert_eq!(session.project.to_json(), before);
    assert!(session.jobs.is_empty());
}

#[test]
fn removed_formats_and_parameters_are_errors_without_files_jobs_or_project_edits() {
    let directory = Scratch::new("unsupported");
    let (mut session, _) = session();
    let project = session.project.to_json();
    let revision = session.revision;
    for format in ["hevc", "h265", "mjpeg", "gif", "wav", "aiff", "aac", "m4a", "tiff", "bmp", "tga", "dpx", "exr"] {
        let mut params = movie_params(directory.path("out.mov"));
        params["format"] = json!(format);
        assert!(session.execute("file.exportMedia", params).is_err(), "{format}");
    }
    for key in [
        "preset",
        "settings",
        "queue",
        "scale",
        "quality",
        "includeAudio",
        "includeVideo",
        "audioOnly",
        "bitrateKbps",
        "maxBitrateKbps",
        "bitrateMode",
        "profile",
        "level",
        "keyframeDistance",
        "proresProfile",
        "alpha",
        "audioSampleRate",
        "audioChannels",
        "audioBits",
        "audioFloat",
        "gpuRendering",
        "hardwareEncoding",
        "sdr",
        "video",
        "audio",
        "overlays",
        "metadata",
        "captions",
        "captionSidecar",
        "loudness",
        "watermark",
        "burnIn",
    ] {
        for value in [Value::Null, json!(true), json!({})] {
            let mut params = movie_params(directory.path("out.mov"));
            params[key] = value;
            assert!(session.execute("file.exportMedia", params).is_err(), "{key}");
        }
    }
    assert_eq!(session.project.to_json(), project);
    assert_eq!(session.revision, revision);
    assert!(session.jobs.is_empty() && session.export_worker.is_none());
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 0);
}

#[test]
fn hostile_ranges_types_and_paths_are_rejected_before_work() {
    let directory = Scratch::new("hostile");
    let (mut session, _) = session();
    let before = session.project.to_json();
    for invalid in [
        json!({"range":"custom","startTime":i64::MIN,"endTime":i64::MAX}),
        json!({"range":"custom","startFrame":u64::MAX,"endFrame":u64::MAX}),
        json!({"range":"custom","startFrame":0,"endFrame":1_000_001}),
        json!({"range":"custom","startFrame":0.5,"endFrame":7}),
        json!({"range":"custom","startSeconds":-1,"endSeconds":1}),
        json!({"range":"custom","startSeconds":0,"endSeconds":1e300}),
        json!({"range":"custom","startFrame":7,"endFrame":7}),
        json!({"range":"custom","startFrame":7,"endFrame":6}),
        json!({"range":"custom","startFrame":0}),
        json!({"range":"custom","endFrame":7}),
        json!({"range":"custom","startFrame":0,"startTime":0,"endFrame":7}),
        json!({"range":"custom","startSeconds":0,"endFrame":7}),
        json!({"range":"custom","startFrame":0,"endTime":7}),
        json!({"range":"custom","startTime":0,"endSeconds":1}),
        json!({"range":"entire","startFrame":0,"endFrame":7}),
        json!({"range":"unknown"}),
        json!({"range":null}),
        json!({"wait":"yes"}),
        json!({"overwrite":1}),
        json!({"format":false}),
        json!({"format":null}),
        json!({"sequence":u64::MAX}),
        json!({"sequence":1.5}),
        json!({"path":null}),
        json!({"path":""}),
        json!({"path":"a\0.mov"}),
        json!({"path":"x".repeat(4097)}),
        json!({"path":directory.path("missing/../source.mov")}),
        json!({"path":directory.path("wrong.mp4")}),
    ] {
        let mut params = json!({"path":directory.path("out.mov"),"format":"prores","wait":true});
        params.as_object_mut().unwrap().extend(invalid.as_object().unwrap().clone());
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.execute("file.exportMedia", params.clone())));
        assert!(outcome.unwrap().is_err(), "{params}");
    }
    for params in [Value::Null, json!([]), json!(true)] {
        assert!(session.execute("file.exportMedia", params).is_err());
    }
    assert_eq!(session.project.to_json(), before);
    assert!(session.jobs.is_empty());
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 0);
}

#[test]
fn retained_ranges_write_the_exact_movie_frame_counts_without_editing_the_project() {
    let directory = Scratch::new("ranges");
    let (mut session, _) = session();
    let seq = session.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut session.project).sequence_mut(seq).unwrap();
    q.mark_in = Some(FrameRate::FPS_24.tick_of(3));
    q.mark_out = Some(FrameRate::FPS_24.tick_of(7));
    q.work_area = Some(waltcut_time::TimeRange::new(FrameRate::FPS_24.tick_of(2), FrameRate::FPS_24.tick_of(3)));
    let before = session.project.to_json();
    for (range, frames) in [("entire", 24), ("inOut", 5), ("workArea", 3), ("custom", 4)] {
        let path = directory.path(&format!("{range}.mov"));
        let mut params = json!({"path":path,"format":"prores","range":range,"wait":true});
        if range == "custom" {
            params["startFrame"] = json!(3);
            params["endFrame"] = json!(7);
        }
        let result = session.execute("file.exportMedia", params).unwrap();
        assert_eq!(result["result"]["frames"], frames, "{range}: {result}");
        let file = waltcut_isobmff::open(std::fs::read(path).unwrap()).unwrap();
        let track = &file.tracks[file.track_of_kind(waltcut_isobmff::TrackKind::Video).unwrap()];
        assert_eq!(track.samples.len(), frames as usize, "{range}");
        assert_eq!(track.entries[0].format.0, *b"ap4h");
        assert_eq!(session.project.to_json(), before);
    }
}

#[test]
fn explicit_sequence_uses_its_rate_not_the_active_sequences_rate() {
    let directory = Scratch::new("sequence-rate");
    let (mut session, _) = session();
    let selected = session.state.active_sequence.unwrap();
    session.execute("file.newSequence", json!({"name":"Other","fps":30,"width":32,"height":18})).unwrap();
    let path = directory.path("selected.mov");
    let result = session
        .execute(
            "file.exportMedia",
            json!({
                "sequence":selected.0,"path":path,"format":"prores","range":"custom","startFrame":1,"endFrame":7,"wait":true
            }),
        )
        .unwrap();
    assert_eq!(result["result"]["frames"], 6);
    let file = waltcut_isobmff::open(std::fs::read(path).unwrap()).unwrap();
    let audio = &file.tracks[file.track_of_kind(waltcut_isobmff::TrackKind::Audio).unwrap()];
    assert_eq!(audio.duration, 12_000, "six frames at 24 fps, not the active 30 fps");
}

#[test]
fn quick_export_has_a_real_default_destination_and_handles_sequence_names_with_dots() {
    let directory = Scratch::new("quick");
    let (mut session, _) = session();
    session.path = Some(directory.path("project.waltcut"));
    std::fs::write(session.path.as_ref().unwrap(), b"project remains unchanged").unwrap();
    let before = session.project.to_json();
    let result = session.execute("export.quick", json!({"format":"prores","range":"custom","startFrame":0,"endFrame":2,"wait":true})).unwrap();
    assert_eq!(result["path"], directory.path("Scene.with.dots.mov"));
    assert!(std::fs::metadata(result["path"].as_str().unwrap()).unwrap().len() > 0);
    assert_eq!(std::fs::read(session.path.as_ref().unwrap()).unwrap(), b"project remains unchanged");
    assert_eq!(session.project.to_json(), before);
}

#[test]
fn overwrite_and_source_protection_are_explicit_for_movies_sequences_and_stills() {
    let directory = Scratch::new("source-protection");
    let (mut session, item) = session();
    let movie = directory.path("existing.mov");
    std::fs::write(&movie, b"original").unwrap();
    assert!(session.execute("file.exportMedia", movie_params(movie.clone())).is_err());
    assert_eq!(std::fs::read(&movie).unwrap(), b"original");
    let mut params = movie_params(movie.clone());
    params["overwrite"] = json!(true);
    session.execute("file.exportMedia", params).unwrap();
    assert_ne!(std::fs::read(&movie).unwrap(), b"original");

    let png = directory.path("source000.png");
    session.execute("file.exportFrame", json!({"path":png})).unwrap();
    let original = std::fs::read(&png).unwrap();
    assert!(session.execute("file.exportFrame", json!({"path":png})).is_err());
    session.execute("file.exportFrame", json!({"path":png,"overwrite":true})).unwrap();
    let imported = session.execute("file.import", json!({"paths":[png]})).unwrap();
    assert_eq!(imported["errors"], json!([]));
    assert!(session.execute("file.exportFrame", json!({"path":png,"overwrite":true})).is_err());
    assert_eq!(std::fs::read(&png).unwrap(), original);

    // Even an offline imported still must not be replaced by a numbered export frame.
    std::fs::remove_file(&png).unwrap();
    assert!(
        session
            .execute(
                "file.exportMedia",
                json!({
                    "path":directory.path("source.png"),"format":"png","range":"custom","startFrame":0,"endFrame":2,"wait":true
                })
            )
            .is_err()
    );
    assert!(!std::path::Path::new(&png).exists());
    assert!(
        session
            .execute(
                "file.exportMedia",
                json!({
                    "path":directory.path("new.png"),"format":"png","range":"custom","startFrame":0,"endFrame":2,"overwrite":true
                })
            )
            .is_err()
    );

    // Movie source protection is independent of whether the imported source is currently readable.
    let media = Arc::make_mut(&mut session.project).item_mut(item).unwrap().as_media_mut().unwrap();
    media.media = waltcut_project::MediaRef::File { path: movie.clone() };
    let source_bytes = std::fs::read(&movie).unwrap();
    let before = session.project.to_json();
    let mut params = movie_params(movie.clone());
    params["overwrite"] = json!(true);
    assert!(session.execute("file.exportMedia", params).is_err());
    assert_eq!(std::fs::read(movie).unwrap(), source_bytes);
    assert_eq!(session.project.to_json(), before);
}

struct Gated {
    source: SharedSource,
    entered: AtomicBool,
    release: AtomicBool,
}
impl MediaSource for Gated {
    fn info(&self) -> &waltcut_media::MediaInfo {
        self.source.info()
    }
    fn video_frame(&self, request: FrameRequest) -> waltcut_media::Result<Arc<waltcut_frame::VideoFrame>> {
        self.entered.store(true, Ordering::Release);
        for _ in 0..10_000 {
            if self.release.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.source.video_frame(request)
    }
    fn audio(&self, start: i64, frames: usize, rate: u32) -> waltcut_media::Result<waltcut_frame::AudioBuffer> {
        self.source.audio(start, frames, rate)
    }
}

fn wait_job(session: &mut Session, job: u64) -> Value {
    for _ in 0..2000 {
        crate::export_tools::poll(session);
        let jobs = session.execute("jobs.list", json!({})).unwrap();
        let state = jobs.as_array().unwrap().iter().find(|j| j["id"] == job).unwrap();
        if state["finished"] == true && session.export_worker.is_none() {
            return state.clone();
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("export job {job} did not finish and release its worker");
}

#[test]
fn one_async_worker_can_be_cancelled_without_losing_existing_output_or_project_state() {
    let directory = Scratch::new("cancel");
    let (mut session, item) = session();
    let gate = Arc::new(Gated { source: session.media.cached(item).unwrap(), entered: AtomicBool::new(false), release: AtomicBool::new(false) });
    session.media.insert(item, gate.clone());
    let path = directory.path("existing.mov");
    std::fs::write(&path, b"original").unwrap();
    let before = session.project.to_json();
    let mut params = movie_params(path.clone());
    params["wait"] = json!(false);
    params["overwrite"] = json!(true);
    let started = session.execute("file.exportMedia", params).unwrap();
    assert_eq!((started["poll"].as_str(), started["cancel"].as_str()), (Some("jobs.list"), Some("jobs.cancel")));
    let job = started["job"].as_u64().unwrap();
    for _ in 0..1000 {
        if gate.entered.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(gate.entered.load(Ordering::Acquire), "the background renderer actually started");
    assert!(session.execute("file.exportMedia", movie_params(directory.path("second.mov"))).is_err());
    session.execute("jobs.cancel", json!({"job":job})).unwrap();
    gate.release.store(true, Ordering::Release);
    let state = wait_job(&mut session, job);
    assert!(state["result"]["error"].as_str().unwrap().contains("cancel"), "{state}");
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert_eq!(session.project.to_json(), before);
    assert!(session.export_worker.is_none());
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1, "no staging or unrelated output remains");
}

#[test]
fn shutdown_joins_the_active_export_and_leaves_no_orphan_job() {
    let directory = Scratch::new("shutdown");
    let (mut session, _) = session();
    let before = session.project.to_json();
    let mut params = movie_params(directory.path("cancelled.mov"));
    params["wait"] = json!(false);
    let started = session.execute("file.exportMedia", params).unwrap();
    session.shutdown();
    assert!(session.export_worker.is_none());
    let job = session.jobs.iter().find(|j| Some(j.id) == started["job"].as_u64()).unwrap();
    assert!(job.result.lock().unwrap().is_some(), "shutdown joins the worker even when cancellation races completion");
    assert_eq!(session.project.to_json(), before);
    assert!(!std::fs::read_dir(&directory.0).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".waltcut-export-")));
}
