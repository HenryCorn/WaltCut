//! Headless UI tests of colour management: Interpret Footage ▸ Color Management and the sequence
//! colour settings, driven by automation id through the control channel.
//!
//! Set `WALTCUT_UI_SNAPSHOT_DIR=<dir>` to also render offscreen with wgpu and write `color-*.png`.

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
    snapshots: Option<std::path::PathBuf>,
}

impl Driver {
    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = WaltCutApp::new(session).with_control(rx);
        let snapshots = std::env::var_os("WALTCUT_UI_SNAPSHOT_DIR").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        if snapshots.is_some() {
            b = b.wgpu();
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, snapshots };
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
        let v = self.call(method, params.clone());
        assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
        v["result"].clone()
    }

    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(2);
    }

    fn element(&mut self, id: &str) -> Option<[f32; 4]> {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array()?.iter().find(|e| e["id"] == id).map(|e| {
            let r = &e["rect"];
            [r[0].as_f64().unwrap() as f32, r[1].as_f64().unwrap() as f32, r[2].as_f64().unwrap() as f32, r[3].as_f64().unwrap() as f32]
        })
    }

    fn ids(&mut self, prefix: &str) -> Vec<String> {
        let v = self.ok("ui.elements", json!({"prefix": prefix}));
        v.as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }

    /// Offscreen render of the window (or the bounding box of the elements under `prefix`).
    fn snapshot(&mut self, name: &str, prefix: Option<&str>) {
        let Some(dir) = self.snapshots.clone() else { return };
        let crop = prefix.map(|p| {
            let v = self.ok("ui.elements", json!({"prefix": p}));
            let mut bb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for e in v.as_array().unwrap() {
                let r = &e["rect"];
                let (x, y, w, h) = (r[0].as_f64().unwrap() as f32, r[1].as_f64().unwrap() as f32, r[2].as_f64().unwrap() as f32, r[3].as_f64().unwrap() as f32);
                bb = [bb[0].min(x), bb[1].min(y), bb[2].max(x + w), bb[3].max(y + h)];
            }
            bb
        });
        self.frames(2);
        let img = match self.harness.render() {
            Ok(i) => i,
            Err(e) => {
                eprintln!("snapshot {name} skipped: {e}");
                return;
            }
        };
        let ppp = img.width() as f32 / 1600.0;
        let img = match crop {
            Some(bb) => {
                let x0 = ((bb[0] - 24.0) * ppp).max(0.0) as u32;
                let y0 = ((bb[1] - 30.0) * ppp).max(0.0) as u32;
                let x1 = (((bb[2] + 16.0) * ppp) as u32).min(img.width());
                let y1 = (((bb[3] + 46.0) * ppp) as u32).min(img.height());
                image::imageops::crop_imm(&img, x0, y0, x1 - x0, y1 - y0).to_image()
            }
            None => img,
        };
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.png"));
        img.save(&path).unwrap();
        eprintln!("snapshot: {}", path.display());
    }
}

/// Select the first clip on V1 and park the playhead on it.
fn select_first_clip(d: &mut Driver) -> u64 {
    let seq = d.exec("sequence.inspect", json!({}));
    let first = seq["video"][0]["items"][0].clone();
    let clip = first["clip"].as_u64().unwrap_or_else(|| panic!("no clip: {seq}"));
    d.exec("timeline.select", json!({"clips": [clip]}));
    clip
}

#[test]
fn interpret_footage_and_sequence_color_dialogs() {
    let mut d = Driver::demo();
    let clip = select_first_clip(&mut d);
    let seq = d.exec("sequence.inspect", json!({}));
    let item = seq["video"][0]["items"].as_array().unwrap().iter().find(|i| i["clip"] == clip).unwrap()["item"].clone();
    // Clip ▸ Modify ▸ Interpret Footage… opens the dialog (no params)
    let r = d.ok("ui.menu.invoke", json!({"id": "clip.interpretFootage"}));
    assert_eq!(r["dialog"], "interpretFootage");
    d.frames(3);
    d.click("colorDialog.space.slog3-sgamut3cine");
    d.snapshot("color-interpret-footage", None);
    d.click("colorDialog.ok");
    let info = d.exec("media.colorInfo", json!({"item": item}));
    assert_eq!(info["override"], "slog3-sgamut3cine", "{info}");
    // Sequence ▸ Color Management…: Rec. 2100 PQ
    let r = d.ok("ui.menu.invoke", json!({"id": "sequence.colorSettings"}));
    assert_eq!(r["dialog"], "sequenceColor");
    d.frames(3);
    d.click("colorDialog.working.rec2100-pq");
    d.snapshot("color-sequence-settings", None);
    d.click("colorDialog.ok");
    let st = d.exec("sequence.colorSettings", json!({"autoToneMap": true}));
    assert_eq!(st["workingSpace"], "rec2100-pq");
    // undo the working space and the interpretation
    d.exec("edit.undo", json!({}));
    d.exec("edit.undo", json!({}));
    let info = d.exec("media.colorInfo", json!({"item": item}));
    assert_eq!(info["override"], Value::Null, "{info}");
}
