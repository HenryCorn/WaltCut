//! Retained graphic design controls driven through the control channel.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::FilmcraftApp;
use waltcut_ui_egui::control::ControlRequest;

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    shots: Option<std::path::PathBuf>,
}

impl Driver {
    fn demo(data: &str) -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("fixtures")
            .join(format!("gfx-ui-{data}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        session.prefs_path = Some(dir.join("prefs.json"));
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let shots = std::env::var_os("FILMCRAFT_UI_SHOTS").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        if shots.is_some() {
            b = b.wgpu().with_pixels_per_point(1.0);
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, shots };
        d.frames(4);
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
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params.clone());
        assert_eq!(r["ok"], true, "{method} {params}: {r}");
        r["result"].clone()
    }
    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }
    fn shot(&mut self, name: &str) {
        let Some(dir) = self.shots.clone() else { return };
        self.frames(8);
        let img = self.harness.render().expect("wgpu render");
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
    fn ids(&mut self, prefix: &str) -> Vec<String> {
        let v = self.ok("ui.elements", json!({"prefix": prefix}));
        v.as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }
}

#[test]
fn graphic_editor_and_font_dialog_work_without_a_library() {
    let mut d = Driver::demo("layout");
    d.exec("playhead.set", json!({"seconds": 300}));
    let r = d.exec("graphics.newText", json!({"text": "Editable title", "font": "Inter"}));
    let clip = r["clip"].as_u64().unwrap();
    d.ok("ui.set", json!({"workspace": "WaltCut"}));
    d.ok("ui.panel.show", json!({"panel": "EssentialGraphics"}));
    d.ok("ui.menu.invoke", json!({"id": "window.panel.EssentialGraphics"}));
    d.exec("graphics.selectLayer", json!({"clip": clip, "layers": []}));
    d.frames(3);
    assert!(d.ids("graphics.time.").contains(&"graphics.time.intro".to_string()));
    d.shot("graphic-editor");

    let v = d.exec("file.replaceFonts", json!({}));
    assert_eq!(v["dialog"], "replaceFonts");
    d.frames(3);
    assert!(d.ids("replaceFonts.").contains(&"replaceFonts.ok".to_string()));
    d.ok("ui.set", json!({"replaceFonts": {"from": "Inter", "to": "Noto Serif"}}));
    d.frames(2);
    d.shot("graphic-replace-fonts");
    d.ok("ui.click", json!({"id": "replaceFonts.ok"}));
    let used = d.exec("graphics.fonts.used", json!({}));
    assert!(used.as_array().unwrap().iter().any(|f| f["family"] == "Noto Serif"), "{used}");
    d.exec("edit.undo", json!({}));
    let used = d.exec("graphics.fonts.used", json!({}));
    assert!(used.as_array().unwrap().iter().any(|f| f["family"] == "Inter"), "{used}");
}

#[test]
fn removed_template_commands_are_absent_from_control_catalog_and_menus() {
    let mut d = Driver::demo("catalog");
    for method in ["engine.commands", "ui.menu.list"] {
        let catalog = d.ok(method, json!({}));
        let catalog = catalog.to_string();
        assert!(!catalog.contains("graphics.template."), "{method}");
        assert!(!catalog.contains("file.exportGraphicsTemplate"), "{method}");
    }
    let before = d.harness.state().session.project.clone();
    for id in ["graphics.template.apply", "graphics.template.install", "graphics.template.export", "file.exportGraphicsTemplate"] {
        let r = d.call("engine.execute", json!({"command": id}));
        assert_eq!(r["ok"], false, "{r}");
    }
    assert_eq!(*d.harness.state().session.project, *before);
}

#[test]
fn type_tool_selection_styles_characters() {
    let mut d = Driver::demo("chars");
    d.exec("playhead.set", json!({"seconds": 300}));
    let r = d.exec("graphics.newText", json!({"text": "Hello World"}));
    let clip = r["clip"].as_u64().unwrap();
    d.ok("ui.set", json!({"workspace": "WaltCut"}));
    d.ok("ui.panel.show", json!({"panel": "EssentialGraphics"}));
    d.exec("graphics.selectLayer", json!({"clip": clip, "layers": [0]}));
    // a Type-tool selection of "World" (bytes 6..11)
    d.ok("ui.set", json!({"tool": "Type", "gfxEdit": {"clip": clip, "layer": 0, "caret": 11, "anchor": 6}}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "graphics.prop.faux_bold"}));
    d.frames(2);
    let l = d.exec("graphics.list", json!({"clip": clip}));
    assert_eq!(l["layers"][0]["styles"], json!([{"start": 6, "end": 11, "fauxBold": true}]), "{l}");
    let hist = d.exec("history.list", json!({}));
    assert_eq!(hist["undo"].as_array().unwrap().last().unwrap(), "Character Style");
}
