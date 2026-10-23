//! Actual input/control-channel tests of the simple Export dialog and shared Quick Export.
//! Optional FILMCRAFT_UI_SNAPSHOT_DIR renders snapshots; native visual acceptance is separate.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_media::generators::GeneratorSource;
use waltcut_media::{FrameRequest, Generator, MediaSource};
use waltcut_project::{ItemKind, Label, MediaClip, MediaRef};
use waltcut_time::FrameRate;
use waltcut_ui_egui::FilmcraftApp;
use waltcut_ui_egui::control::ControlRequest;

/// Keep a long export running long enough to exercise the real Cancel widget, without hardware.
struct SlowSource(GeneratorSource);
impl MediaSource for SlowSource {
    fn info(&self) -> &waltcut_media::MediaInfo {
        self.0.info()
    }
    fn video_frame(&self, request: FrameRequest) -> waltcut_media::Result<Arc<waltcut_frame::VideoFrame>> {
        std::thread::sleep(std::time::Duration::from_millis(10));
        self.0.video_frame(request)
    }
    fn audio(&self, start: i64, frames: usize, rate: u32) -> waltcut_media::Result<waltcut_frame::AudioBuffer> {
        self.0.audio(start, frames, rate)
    }
}

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    directory: PathBuf,
}
impl Driver {
    fn new(name: &str) -> Self {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("target")
            .join("export-tests")
            .join(format!("ui-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut session = Session::default();
        session.execute("file.newSequence", json!({"name":"Export","width":32,"height":18,"fps":24,"video":1,"audio":1})).unwrap();
        let generator =
            GeneratorSource::new(Generator::ColorMatte { color: [1.0, 0.0, 0.0, 0.5] }, 32, 18, FrameRate::FPS_24, FrameRate::FPS_24.tick_of(100_000));
        let info = generator.info().clone();
        let item = Arc::make_mut(&mut session.project).add_item(
            "Red",
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(generator.generator.clone()),
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
        session.media.insert(item, Arc::new(SlowSource(generator)));
        session.execute("timeline.place", json!({"item":item.0,"track":"V1"})).unwrap();
        let (tx, rx) = channel();
        let mut app = FilmcraftApp::new(session).with_control(rx);
        app.ui.export.path = directory.join("out.mov").to_string_lossy().into_owned();
        app.ui.export.format = waltcut_export::Format::ProRes;
        app.ui.export.range = "custom".into();
        app.ui.export.start_frame = 0;
        app.ui.export.end_frame = 6;
        let mut builder = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_max_steps(10_000);
        if std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").is_some() {
            builder = builder.wgpu();
        }
        let harness = builder.build_eframe(move |_cc| app);
        let mut driver = Self { harness, tx, directory };
        driver.frames(4);
        driver
    }
    fn path(&self, name: &str) -> String {
        self.directory.join(name).to_string_lossy().into_owned()
    }
    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            let ctx = self.harness.ctx.clone();
            let mut input = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut input);
            *self.harness.input_mut() = input;
            self.harness.step();
        }
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        let (request, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(request).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(value) = reply.try_recv() {
                return value;
            }
        }
        panic!("no response to {method} {params}");
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let reply = self.call(method, params.clone());
        assert_eq!(reply["ok"], true, "{method} {params}: {reply}");
        reply["result"].clone()
    }
    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command":command,"params":params}))
    }
    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id":id}));
        self.frames(3);
    }
    fn has(&mut self, id: &str) -> bool {
        self.ok("ui.elements", json!({"prefix":id})).as_array().unwrap().iter().any(|entry| entry["id"] == id)
    }
    fn wait_export(&mut self) -> Value {
        for _ in 0..2000 {
            self.frames(2);
            let jobs = self.exec("jobs.list", json!({}));
            if let Some(job) = jobs.as_array().unwrap().last()
                && job["finished"] == true
            {
                return job.clone();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("export did not complete");
    }
    fn snapshot(&mut self, name: &str) {
        let Some(directory) = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").map(PathBuf::from) else { return };
        self.frames(2);
        let image = self.harness.render().unwrap();
        std::fs::create_dir_all(&directory).unwrap();
        image.save(directory.join(format!("{name}.png"))).unwrap();
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        self.harness.state_mut().session.shutdown();
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn only_destination_format_range_and_start_controls_are_exposed() {
    let mut driver = Driver::new("controls");
    driver.ok("ui.set", json!({"mode":"export"}));
    driver.frames(3);
    for id in ["export.path", "export.browse", "export.format", "export.range", "export.range.start", "export.range.end", "export.button"] {
        assert!(driver.has(id), "{id}");
    }
    for removed in [
        "export.preset",
        "export.preset.more",
        "export.section.video",
        "export.section.audio",
        "export.section.multiplexer",
        "export.video.bitrateMode",
        "export.video.hardwareEncoding",
        "export.audio.sampleRate",
        "export.scaling",
        "export.summary",
        "export.estimate",
        "export.sendToQueue",
        "export.queue.start",
    ] {
        assert!(!driver.has(removed), "{removed}");
    }
    driver.click("export.format");
    for format in ["h264", "prores", "png"] {
        assert!(driver.has(&format!("export.format.{format}")));
    }
    for removed in ["hevc", "mjpeg", "gif", "wav", "aiff", "aac", "tiff", "bmp"] {
        assert!(!driver.has(&format!("export.format.{removed}")));
    }
    driver.click("export.format.png");
    let state = driver.ok("ui.inspect", json!({}))["ui"]["export"].clone();
    assert_eq!(state["format"], "png");
    assert!(state["path"].as_str().unwrap().ends_with(".png"));
    driver.snapshot("export-simple");
}

#[test]
fn removed_export_state_and_hostile_drafts_are_rejected_without_mutating_the_draft() {
    let mut driver = Driver::new("invalid");
    let before = driver.ok("ui.inspect", json!({}))["ui"]["export"].clone();
    for patch in [
        json!({"preset":"old"}),
        json!({"settings":{}}),
        json!({"queue":{}}),
        json!({"format":"wav"}),
        json!({"format":"hevc"}),
        json!({"range":"old"}),
        json!({"startFrame":-1}),
        json!({"endFrame":0}),
        json!({"endFrame":i64::MAX}),
        json!({"endFrame":1_000_001}),
        json!({"endFrame":2.5}),
        json!({"path":"a\0.mov"}),
        json!({"path":"x".repeat(4097)}),
        json!({"alpha":false}),
        json!({"forSequence":1}),
        Value::Null,
        json!([]),
    ] {
        let response = driver.call("ui.set", json!({"export":patch}));
        assert_eq!(response["ok"], false, "{patch}: {response}");
        assert_eq!(driver.ok("ui.inspect", json!({}))["ui"]["export"], before);
    }
}

#[test]
fn export_button_writes_the_retained_movie_and_the_real_cancel_widget_stops_a_job() {
    let mut driver = Driver::new("start-cancel");
    driver.ok("ui.set", json!({"mode":"export"}));
    let before = driver.harness.state_mut().session.project.to_json();
    driver.click("export.button");
    let job = driver.wait_export();
    assert!(job["result"].get("error").is_none(), "{job}");
    assert_eq!(job["result"]["frames"], 6);
    let file = waltcut_isobmff::open(std::fs::read(driver.path("out.mov")).unwrap()).unwrap();
    let video = &file.tracks[file.track_of_kind(waltcut_isobmff::TrackKind::Video).unwrap()];
    assert_eq!(video.samples.len(), 6);
    assert_eq!(video.entries[0].format.0, *b"ap4h");
    assert!(driver.has("export.status"));

    let path = driver.path("cancel.mov");
    driver.ok("ui.set", json!({"export":{"path":path,"endFrame":10_000}}));
    driver.click("export.button");
    for id in ["export.progress", "export.progress.status", "export.cancel"] {
        assert!(driver.has(id), "{id}");
    }
    driver.click("export.cancel");
    let cancelled = driver.wait_export();
    assert!(cancelled["result"]["error"].as_str().unwrap().contains("cancel"), "{cancelled}");
    assert!(!std::path::Path::new(&path).exists());
    assert_eq!(driver.harness.state_mut().session.project.to_json(), before);
}

#[test]
fn replacement_is_explicit_and_png_conflicts_are_visible_without_clobbering_files() {
    let mut driver = Driver::new("overwrite");
    let path = driver.path("out.mov");
    std::fs::write(&path, b"original").unwrap();
    driver.ok("ui.set", json!({"mode":"export"}));
    driver.click("export.button");
    assert!(driver.has("export.replace") && driver.has("export.keep"));
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    driver.click("export.keep");
    assert!(driver.exec("jobs.list", json!({})).as_array().unwrap().is_empty());
    driver.click("export.button");
    driver.click("export.replace");
    let job = driver.wait_export();
    assert!(job["result"].get("error").is_none(), "{job}");
    assert_ne!(std::fs::read(&path).unwrap(), b"original");

    let frame = driver.path("seq000.png");
    std::fs::write(&frame, b"existing frame").unwrap();
    driver.ok("ui.set", json!({"export":{"path":driver.path("seq.png"),"format":"png"}}));
    driver.click("export.button");
    assert!(driver.harness.state_mut().ui.status.contains("already exists"));
    assert_eq!(std::fs::read(frame).unwrap(), b"existing frame");
    assert!(!std::path::Path::new(&driver.path("seq001.png")).exists());
}

#[test]
fn quick_export_uses_the_same_simple_draft_and_can_be_closed_by_automation() {
    let mut driver = Driver::new("quick");
    driver.click("header.quickExport");
    for id in ["quickExport.path", "quickExport.browse", "quickExport.format", "quickExport.range", "quickExport.button", "quickExport.close"] {
        assert!(driver.has(id), "{id}");
    }
    assert!(!driver.has("quickExport.preset"));
    driver.click("quickExport.button");
    let job = driver.wait_export();
    assert!(job["result"].get("error").is_none(), "{job}");
    assert!(std::path::Path::new(&driver.path("out.mov")).exists());
    driver.click("quickExport.close");
    assert_eq!(driver.ok("ui.inspect", json!({}))["ui"]["export"]["quickOpen"], false);
    assert!(!driver.has("quickExport.button"));
}
