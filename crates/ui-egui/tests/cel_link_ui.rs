//! Linked exposures (#67) in the interface: the lane and the options bar say what is shared and
//! what an edit reaches, Link and Make Independent work from the bar and the exposure menu, a stroke
//! drawn with the pen lands on every linked exposure, and a refusal changes nothing.

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

fn button(pos: Pos2, b: egui::PointerButton, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: b, pressed, modifiers: Default::default() }
}

impl Driver {
    /// A 320x180 drawing: open eyes on frame 0, closed eyes on frame 4, the open eyes linked to 8.
    fn new() -> Self {
        let mut s = Session::default();
        s.execute("file.newSequence", json!({"name": "Link", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 0})).unwrap();
        let clip = s.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
        let dash = |y: f64, c: &str| json!({"brush": "Pen", "color": c, "size": 4.0, "points": [[40.0, y], [280.0, y]]});
        s.execute("drawing.addStroke", json!({"clip": clip, "frame": 0, "stroke": dash(40.0, "#000000")})).unwrap();
        s.execute("drawing.addStroke", json!({"clip": clip, "frame": 4, "cel": "new", "stroke": dash(100.0, "#0000ff")})).unwrap();
        s.execute("drawing.linkCel", json!({"clip": clip, "from": 0, "frame": 8})).unwrap();
        s.execute("timeline.select", json!({"clips": [clip]})).unwrap();
        let (tx, rx) = channel();
        let app = WaltCutApp::new(s).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_step_dt(1.0 / 60.0).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, clip };
        d.frames(4);
        d.call("ui.set", json!({"tool": "Brush"}));
        d.call("ui.set", json!({"timeline": {"pps": 300.0, "scroll": 0.0}}));
        d.frames(30);
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

    fn raw(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(value) = reply.try_recv() {
                return value;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let v = self.raw(method, params.clone());
        assert_eq!(v["ok"], true, "{method} {params}: {v}");
        v["result"].clone()
    }

    fn engine(&mut self, id: &str, params: Value) -> Value {
        let r = self.call("engine.execute", json!({"command": id, "params": params}));
        self.frames(3);
        r
    }

    fn playhead(&mut self, frame: i64) {
        self.engine("playhead.set", json!({"frame": frame}));
        self.frames(3);
    }

    /// The registered element `id`, if it is on screen: `(rect, label, enabled)`.
    fn find(&mut self, id: &str) -> Option<([f32; 4], String, Option<bool>)> {
        let prefix = id.split('.').take(2).collect::<Vec<_>>().join(".");
        let all = self.call("ui.elements", json!({"prefix": prefix}));
        let e = all.as_array()?.iter().find(|e| e["id"] == id)?.clone();
        let r = e["rect"].as_array()?;
        Some(([0, 1, 2, 3].map(|i| r[i].as_f64().unwrap() as f32), e["label"].as_str().unwrap_or("").to_string(), e["enabled"].as_bool()))
    }

    fn element(&mut self, id: &str) -> ([f32; 4], String, Option<bool>) {
        self.find(id).unwrap_or_else(|| panic!("missing {id}"))
    }

    fn centre(&mut self, id: &str) -> Pos2 {
        let (r, ..) = self.element(id);
        pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
    }

    fn move_to(&mut self, p: Pos2) {
        self.harness.input_mut().events.push(egui::Event::PointerMoved(p));
        self.frames(1);
    }

    fn click(&mut self, p: Pos2) {
        self.move_to(p);
        for pressed in [true, false] {
            self.harness.input_mut().events.push(button(p, egui::PointerButton::Primary, pressed));
            self.frames(1);
        }
        self.frames(3);
    }

    fn right_click(&mut self, p: Pos2) {
        self.move_to(p);
        for pressed in [true, false] {
            self.harness.input_mut().events.push(button(p, egui::PointerButton::Secondary, pressed));
            self.frames(1);
        }
        self.frames(4);
    }

    fn drag(&mut self, path: &[Pos2]) {
        self.move_to(path[0]);
        self.harness.input_mut().events.push(button(path[0], egui::PointerButton::Primary, true));
        self.frames(1);
        for p in &path[1..] {
            self.move_to(*p);
        }
        let last = *path.last().unwrap();
        self.harness.input_mut().events.push(button(last, egui::PointerButton::Primary, false));
        self.frames(4);
    }

    fn cels(&mut self) -> Vec<(i64, Option<u64>, u64)> {
        let r = self.engine("drawing.inspect", json!({"clip": self.clip}));
        r["cels"].as_array().unwrap().iter().map(|c| (c["frame"].as_i64().unwrap(), c["link"].as_u64(), c["strokes"].as_u64().unwrap())).collect()
    }

    fn undo_len(&self) -> usize {
        self.harness.state().session.history.undo.len()
    }

    /// With `WALTCUT_SHOTS=<dir>` set, a PNG of the whole window rendered offscreen (the control
    /// channel's `ui.screenshot` needs a visible window); otherwise nothing.
    fn shot(&mut self, name: &str) {
        let Some(dir) = std::env::var_os("WALTCUT_SHOTS").map(std::path::PathBuf::from) else { return };
        self.frames(10);
        let img = self.harness.render().expect("wgpu render");
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }

    fn status(&self) -> String {
        self.harness.state().ui.status.clone()
    }
}

#[test]
fn the_lane_and_the_options_bar_say_what_is_shared_and_what_an_edit_reaches() {
    let mut d = Driver::new();
    let clip = d.clip;
    let linked = d.element(&format!("timeline.cel.{clip}.0")).1;
    assert!(linked.contains("linked: 2 exposures share this artwork") && linked.contains("editing one edits all"), "{linked}");
    assert!(d.element(&format!("timeline.cel.{clip}.8")).1.contains("linked: 2 exposures"));
    let own = d.element(&format!("timeline.cel.{clip}.4")).1;
    assert!(!own.contains("linked"), "an unlinked cel is not marked: {own}");
    // on the linked exposure the bar states the scope; on the plain one it does not
    d.playhead(8);
    d.shot("linked-exposures");
    let scope = d.element("program.brush.linked").1;
    assert!(scope.contains("2 exposures share this artwork") && scope.contains("edits change all"), "{scope}");
    assert!(d.find("program.brush.makeIndependent").is_some_and(|e| e.2 == Some(true)));
    d.playhead(5);
    assert!(d.find("program.brush.linked").is_none() && d.find("program.brush.makeIndependent").is_none());
    // Link needs a frame without a cel: refused (disabled) on a frame that starts one
    d.playhead(4);
    assert_eq!(d.element("program.brush.link").2, Some(false));
}

#[test]
fn make_independent_from_the_bar_is_one_undo_step_and_keeps_the_picture() {
    let mut d = Driver::new();
    d.playhead(8);
    let undo = d.undo_len();
    let c = d.centre("program.brush.makeIndependent");
    d.click(c);
    assert_eq!(d.cels(), vec![(0, None, 1), (4, None, 1), (8, None, 1)], "the group of two is dissolved and both keep their strokes");
    assert_eq!(d.undo_len(), undo + 1);
    assert!(d.status().contains("independent"), "{}", d.status());
    assert!(d.find("program.brush.linked").is_none(), "the cel shown is no longer linked");
    d.engine("edit.undo", json!({}));
    let cels = d.cels();
    assert!(cels[0].1.is_some() && cels[0].1 == cels[2].1, "{cels:?}");
    d.frames(3);
    assert!(d.find("program.brush.linked").is_some());
}

#[test]
fn link_starts_a_shared_exposure_from_the_picked_cel_and_a_duplicate_stays_independent() {
    let mut d = Driver::new();
    let clip = d.clip;
    // pick the exposure at frame 4 (closed eyes), move to a free frame, press Link
    let at4 = d.centre(&format!("timeline.cel.{clip}.4"));
    d.click(at4);
    d.playhead(14);
    assert_eq!(d.element("program.brush.link").2, Some(true));
    let c = d.centre("program.brush.link");
    d.click(c);
    let cels = d.cels();
    assert_eq!(cels.iter().map(|c| c.0).collect::<Vec<_>>(), vec![0, 4, 8, 14]);
    assert!(cels[1].1.is_some() && cels[1].1 == cels[3].1 && cels[0].1 == cels[2].1 && cels[0].1 != cels[1].1, "{cels:?}");
    assert!(d.status().contains("Linked frame 14 to frame 4"), "{}", d.status());
    // Duplicate on a later frame is an independent copy, even of a linked exposure
    d.playhead(20);
    let dup = d.engine("drawing.newCel", json!({"clip": clip, "frame": 20, "copy": true}));
    assert_eq!(dup["cel"], 20);
    assert_eq!(d.cels().last().map(|c| c.1), Some(None));
    // pressing Link where a cel already starts changes nothing
    d.playhead(14);
    let before = (d.undo_len(), d.cels());
    let c = d.centre("program.brush.link");
    d.click(c);
    assert_eq!((d.undo_len(), d.cels()), before);
}

#[test]
fn the_exposure_menu_links_at_the_playhead_and_makes_independent() {
    let mut d = Driver::new();
    let clip = d.clip;
    d.playhead(16);
    let at0 = d.centre(&format!("timeline.cel.{clip}.0"));
    d.click(at0);
    d.right_click(at0);
    d.shot("link-menu");
    assert_eq!(d.element("timeline.celMenu.link").2, Some(true));
    assert_eq!(d.element("timeline.celMenu.independent").2, Some(true), "the picked exposure is linked");
    let c = d.centre("timeline.celMenu.link");
    d.click(c);
    let cels = d.cels();
    assert_eq!(cels.iter().map(|c| (c.0, c.1.is_some())).collect::<Vec<_>>(), vec![(0, true), (4, false), (8, true), (16, true)]);
    assert!(d.status().contains("3 exposures"), "{}", d.status());
    // an unlinked exposure: Make Independent is disabled
    let at4 = d.centre(&format!("timeline.cel.{clip}.4"));
    d.click(at4);
    d.right_click(at4);
    assert_eq!(d.element("timeline.celMenu.independent").2, Some(false));
    d.engine("edit.undo", json!({}));
    d.frames(2);
    // on a linked one it detaches just that exposure
    let at8 = d.centre(&format!("timeline.cel.{clip}.8"));
    d.click(at8);
    d.right_click(at8);
    let c = d.centre("timeline.celMenu.independent");
    d.click(c);
    assert_eq!(d.cels().iter().map(|c| c.1.is_some()).collect::<Vec<_>>(), vec![false, false, false], "a group of two loses its second member and dissolves");
}

#[test]
fn a_stroke_drawn_with_the_pen_on_a_linked_exposure_lands_on_all_of_them() {
    let mut d = Driver::new();
    d.playhead(8);
    let (r, ..) = d.element("program.brush");
    let at = |x: f32, y: f32| pos2(r[0] + x / 320.0 * r[2], r[1] + y / 180.0 * r[3]);
    let before = d.cels();
    d.drag(&[at(60.0, 140.0), at(120.0, 150.0), at(200.0, 140.0), at(260.0, 145.0)]);
    let after = d.cels();
    assert_eq!(
        after.iter().map(|c| c.2).collect::<Vec<_>>(),
        vec![before[0].2 + 1, before[1].2, before[2].2 + 1],
        "frames 0 and 8 got the stroke, 4 did not: {after:?}"
    );
    assert!(d.element("program.brush.linked").1.contains("2 exposures"));
    // the picture the pen drew on frame 8 is the one at frame 0
    d.playhead(0);
    let a = d.harness.state().session.render_program(1.0).unwrap().to_rgba8();
    d.playhead(8);
    let b = d.harness.state().session.render_program(1.0).unwrap().to_rgba8();
    assert_eq!(a, b);
}
