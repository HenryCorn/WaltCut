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

impl Driver {
    fn click(&mut self, at: Pos2, shift: bool) {
        let modifiers = egui::Modifiers { shift, ..Default::default() };
        self.harness.input_mut().events.push(egui::Event::ModifiersChanged(modifiers));
        self.frames(1);
        self.harness.input_mut().events.push(egui::Event::PointerMoved(at));
        self.frames(1);
        self.harness.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers });
        self.frames(1);
        self.harness.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers });
        self.frames(2);
        self.harness.input_mut().events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        self.frames(1);
    }

    fn centre(&mut self, id: &str) -> Pos2 {
        let r = self.element(id);
        pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
    }

    fn frames_of_cels(&mut self) -> Vec<i64> {
        let cels = self.call("engine.execute", json!({"command": "drawing.inspect", "params": {"clip": self.clip}}));
        cels["cels"].as_array().unwrap().iter().map(|c| c["frame"].as_i64().unwrap()).collect()
    }
}

#[test]
fn picked_exposures_retime_on_twos_threes_and_ones_and_the_pick_follows() {
    let mut driver = Driver::new();
    let clip = driver.clip;
    for frame in 1..4 {
        driver.call("engine.execute", json!({"command": "drawing.newCel", "params": {"clip": clip, "frame": frame}}));
    }
    driver.frames(4);
    assert_eq!(driver.frames_of_cels(), vec![0, 1, 2, 3, 4]);

    let first = driver.centre(&format!("timeline.cel.{clip}.1"));
    let third = driver.centre(&format!("timeline.cel.{clip}.3"));
    driver.click(first, false);
    driver.click(third, true);
    driver.call("engine.execute", json!({"command": "drawing.retimeOnTwos", "params": {}}));
    // cels 1..=3 are on twos from frame 1; the last keeps its hold, so the pose after it moves with it
    assert_eq!(driver.frames_of_cels(), vec![0, 1, 3, 5, 6]);

    // the pick followed the cels, so a second command acts on the same three
    driver.call("engine.execute", json!({"command": "drawing.retimeOnThrees", "params": {}}));
    assert_eq!(driver.frames_of_cels(), vec![0, 1, 4, 7, 8]);
    driver.call("engine.execute", json!({"command": "drawing.retimeOnOnes", "params": {}}));
    assert_eq!(driver.frames_of_cels(), vec![0, 1, 2, 3, 4]);

    // one undo step each
    driver.call("engine.execute", json!({"command": "edit.undo", "params": {}}));
    assert_eq!(driver.frames_of_cels(), vec![0, 1, 4, 7, 8]);
}

#[test]
fn retime_without_a_pick_acts_on_every_cel_and_refuses_a_gap_in_the_pick() {
    let mut driver = Driver::new();
    let clip = driver.clip;
    driver.call("engine.execute", json!({"command": "drawing.retimeOnTwos", "params": {}}));
    assert_eq!(driver.frames_of_cels(), vec![0, 2]);
    for frame in [3, 4] {
        driver.call("engine.execute", json!({"command": "drawing.newCel", "params": {"clip": clip, "frame": frame}}));
    }
    // 0, 2 and 4 picked by hand with 3 left out: refused, nothing moves
    let (req, reply) =
        ControlRequest::new("engine.execute", json!({"command": "drawing.retimeExposures", "params": {"clip": clip, "frames": [0, 2, 4], "on": "threes"}}));
    driver.tx.send(req).unwrap();
    let mut answer = None;
    for _ in 0..600 {
        driver.frames(1);
        if let Ok(v) = reply.try_recv() {
            answer = Some(v);
            break;
        }
    }
    assert_eq!(answer.expect("a reply")["ok"], false);
    assert_eq!(driver.frames_of_cels(), vec![0, 2, 3, 4]);
}
