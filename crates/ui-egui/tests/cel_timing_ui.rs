use std::sync::mpsc::{Sender, channel};

use egui::{Pos2, pos2};
use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
    clip: u64,
}

impl Driver {
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.newSequence", json!({"name": "Timing", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 1})).unwrap();
        let clip = session.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
        session.execute("drawing.newCel", json!({"clip": clip, "frame": 0})).unwrap();
        session.execute("drawing.newCel", json!({"clip": clip, "frame": 4})).unwrap();
        let (tx, rx) = channel();
        let app = WaltCutApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| app);
        let mut driver = Self { harness, tx, clip };
        driver.frames(4);
        driver.call("ui.set", json!({"timeline": {"pps": 300.0, "scroll": 0.0}}));
        driver.frames(30);
        driver
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
            if let Ok(value) = reply.try_recv() {
                assert_eq!(value["ok"], true, "{method} {params}: {value}");
                return value["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn element(&mut self, id: &str) -> [f32; 4] {
        let elements = self.call("ui.elements", json!({"prefix": id}));
        let element = elements.as_array().unwrap().iter().find(|element| element["id"] == id).unwrap_or_else(|| panic!("missing {id}: {elements}"));
        let rect = element["rect"].as_array().unwrap();
        [0, 1, 2, 3].map(|i| rect[i].as_f64().unwrap() as f32)
    }

    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.harness.input_mut().events.push(egui::Event::PointerMoved(from));
        self.frames(1);
        self.harness.input_mut().events.push(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        self.frames(1);
        for step in 1..=4 {
            self.harness.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (step as f32 / 4.0)));
            self.frames(1);
        }
        self.harness.input_mut().events.push(egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        });
        self.frames(2);
    }
}

#[test]
fn timeline_exposure_handles_retime_a_pose_and_adjust_its_hold() {
    let mut driver = Driver::new();
    let clip_rect = driver.element(&format!("timeline.clip.{}", driver.clip));
    let start_rect = driver.element(&format!("timeline.cel.{}.4.start", driver.clip));
    let target_x = |frame: f32| clip_rect[0] + clip_rect[2] * (frame / 48.0);
    let start = pos2(start_rect[0] + start_rect[2] / 2.0, start_rect[1] + start_rect[3] / 2.0);
    let to_eight = pos2(target_x(8.0), start.y);
    driver.drag(start, to_eight);
    let cels = driver.call("engine.execute", json!({"command": "drawing.inspect", "params": {"clip": driver.clip}}));
    assert_eq!(cels["cels"][1]["frame"], 8);

    let hold_rect = driver.element(&format!("timeline.cel.{}.0.end", driver.clip));
    let hold = pos2(hold_rect[0] + hold_rect[2] / 2.0, hold_rect[1] + hold_rect[3] / 2.0);
    driver.drag(hold, pos2(target_x(5.0), hold.y));
    let cels = driver.call("engine.execute", json!({"command": "drawing.inspect", "params": {"clip": driver.clip}}));
    assert_eq!(cels["cels"][1]["frame"], 5);
    assert_eq!(cels["exposures"][0]["endFrame"], 5);
}
