//! Guide clips (#65) in the app: the Clip menu command, the export page's switch with its
//! automation id, and a guide staying out of the default export parameters.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_media::Generator;
use waltcut_media::generators::GeneratorSource;
use waltcut_project::Label;
use waltcut_time::FrameRate;
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
    reference: u64,
}

impl Driver {
    fn new() -> Self {
        let mut s = Session::default();
        s.execute("file.newSequence", json!({"name": "G", "width": 32, "height": 18, "fps": 24, "video": 2, "audio": 0})).unwrap();
        for (name, color, track) in [("Green", [0.0, 1.0, 0.0, 1.0], "V1"), ("Red", [1.0, 0.0, 0.0, 1.0], "V2")] {
            let source = GeneratorSource::new(Generator::ColorMatte { color }, 32, 18, FrameRate::FPS_24, FrameRate::FPS_24.tick_of(24));
            let item = waltcut_engine::demo::add_generator(Arc::make_mut(&mut s.project), &s.media, source, name, Label::Iris, None);
            s.execute("timeline.place", json!({"item": item.0, "track": track})).unwrap();
        }
        let reference = s.active_sequence().unwrap().video_tracks[1].items[0].id.0;
        s.execute("timeline.select", json!({"clips": [reference]})).unwrap();
        let (tx, rx) = channel();
        let app = WaltCutApp::new(s).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, reference };
        d.frames(5);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn guide(&self) -> bool {
        self.harness.state().session.active_sequence().unwrap().find_item(waltcut_project::ClipId(self.reference)).unwrap().1.guide
    }
}

#[test]
fn the_clip_menu_marks_the_selected_clip_and_it_is_one_undo_step() {
    let mut d = Driver::new();
    let items = d.call("ui.menu.list", json!({}))["result"].clone();
    let item = items.as_array().unwrap().iter().find(|i| i["id"] == "clip.setGuide").expect("a Clip menu item").clone();
    assert_eq!(item["enabled"], json!(true), "{item}");
    assert!(item.to_string().contains("Clip"), "{item}");
    d.call("ui.menu.invoke", json!({"id": "clip.setGuide"}));
    d.frames(3);
    assert!(d.guide());
    d.call("ui.menu.invoke", json!({"id": "edit.undo"}));
    d.frames(2);
    assert!(!d.guide());
}

#[test]
fn the_export_page_has_an_include_guides_switch_that_defaults_off_and_is_not_remembered() {
    let mut d = Driver::new();
    d.call("ui.menu.invoke", json!({"id": "file.exportMedia"}));
    d.frames(3);
    let ids = |d: &mut Driver| -> Vec<String> {
        d.call("ui.elements", json!({"prefix": "export."}))["result"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap().to_string()).collect()
    };
    assert!(ids(&mut d).contains(&"export.includeGuides".to_string()), "{:?}", ids(&mut d));
    assert!(!waltcut_ui_egui::panels::export_mode::export_params(d.harness.state()).get("includeGuides").is_some_and(|v| v == true));
    d.call("ui.click", json!({"id": "export.includeGuides"}));
    d.frames(3);
    assert_eq!(waltcut_ui_egui::panels::export_mode::export_params(d.harness.state())["includeGuides"], json!(true));
    // saved UI state neither writes nor loads it, and the whole saved state still loads
    let saved = serde_json::to_value(&d.harness.state().ui).unwrap();
    assert!(!saved.to_string().contains("includeGuides"));
    let loaded: waltcut_ui_egui::state::UiState = serde_json::from_value(saved).unwrap();
    assert!(!loaded.export.include_guides);
}
