//! In-between assist (#72) in the interface: the pairing table, a preview that changes nothing,
//! cancel, accept, and the same cels as the headless commands for the same mapping. These are
//! offscreen kittest renders, not a window on screen.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use serde_json::{Value, json};
use waltcut_engine::Session;
use waltcut_ui_egui::WaltCutApp;
use waltcut_ui_egui::control::ControlRequest;

struct Driver {
    harness: Harness<'static, WaltCutApp>,
    tx: Sender<ControlRequest>,
}

fn session() -> Session {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Between", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 0})).unwrap();
    let clip = s.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
    let stroke = |c: &str, a: [f64; 2], b: [f64; 2]| json!({"brush": "Pen", "color": c, "size": 4.0, "points": [[a[0], a[1], 1.0], [b[0], b[1], 1.0]]});
    s.execute("drawing.addStroke", json!({"clip": clip, "frame": 0, "stroke": stroke("#ff0000", [20.0, 20.0], [120.0, 20.0])})).unwrap();
    s.execute("drawing.addStroke", json!({"clip": clip, "frame": 0, "stroke": stroke("#0000ff", [200.0, 20.0], [200.0, 120.0])})).unwrap();
    s.execute("drawing.addStroke", json!({"clip": clip, "frame": 8, "cel": "new", "stroke": stroke("#0000ff", [240.0, 40.0], [240.0, 140.0])})).unwrap();
    s.execute("drawing.addStroke", json!({"clip": clip, "frame": 8, "stroke": stroke("#ff0000", [120.0, 60.0], [20.0, 60.0])})).unwrap();
    s.execute("playhead.set", json!({"frame": 3})).unwrap();
    s.execute("timeline.select", json!({"clips": [clip]})).unwrap();
    s
}

impl Driver {
    fn new() -> Self {
        let (tx, rx) = channel();
        let app = WaltCutApp::new(session()).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.command("tool.brush", json!({})).unwrap();
        d.frames(6);
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
            if let Ok(r) = reply.try_recv() {
                return r;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn command(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let r = self.call("engine.execute", json!({"command": id, "params": params}));
        if r["ok"] == true { Ok(r["result"].clone()) } else { Err(r["error"].as_str().unwrap_or("").to_string()) }
    }

    fn click(&mut self, id: &str) {
        let r = self.call("ui.click", json!({"id": id}));
        assert!(r["ok"] == true, "click {id}: {r}");
        self.frames(3);
    }

    fn assist(&mut self) -> Value {
        self.call("ui.inspect", json!({}))["result"]["ui"]["brush"]["inbetween"].clone()
    }

    fn ids(&mut self) -> Vec<String> {
        self.call("ui.elements", json!({"prefix": "program.brush.inbetween"}))["result"]
            .as_array()
            .map(|a| a.iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }

    fn untouched(&self) -> (usize, u64, bool, String) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.revision, s.is_dirty(), serde_json::to_string(&s.project).unwrap())
    }

    fn frames_of_cels(&self) -> Vec<i64> {
        let p = &self.harness.state().session.project;
        p.items.values().find_map(|i| i.as_drawing()).map(|d| d.cels.iter().map(|c| c.frame).collect()).unwrap_or_default()
    }

    fn drawing(&self) -> waltcut_project::drawing::Drawing {
        let p = &self.harness.state().session.project;
        p.items.values().find_map(|i| i.as_drawing()).cloned().unwrap()
    }
}

#[test]
fn the_window_pairs_previews_cancels_and_accepts() {
    let mut d = Driver::new();
    assert_eq!(d.ids(), vec!["program.brush.inbetween.open".to_string()], "only the button until the window is opened");
    d.click("program.brush.inbetween.open");
    let ids = d.ids();
    for id in ["add", "inOrder", "clear", "step", "easing", "unmatched", "preview", "accept", "cancel", "status"] {
        assert!(ids.contains(&format!("program.brush.inbetween.{id}")), "{id}: {ids:?}");
    }
    // nothing is paired until someone pairs it
    assert_eq!(d.assist()["rows"], json!([]));
    d.click("program.brush.inbetween.inOrder");
    assert_eq!(d.assist()["rows"], json!([{"from": 0, "to": 0, "reverse": false}, {"from": 1, "to": 1, "reverse": false}]));
    // a row can be changed: reverse the first
    d.click("program.brush.inbetween.row.0.reverse");
    assert_eq!(d.assist()["rows"][0]["reverse"], true);
    d.click("program.brush.inbetween.row.0.reverse");

    let before = d.untouched();
    d.click("program.brush.inbetween.preview");
    let a = d.assist();
    assert_eq!(a["report"]["status"], "ok", "{a}");
    assert_eq!(a["report"]["frames"], json!([2, 4, 6]));
    assert_eq!(d.untouched(), before, "a preview leaves history, revision, dirty flag and project alone");
    assert!(d.harness.state().session.inbetween_preview().is_some());
    d.click("program.brush.inbetween.next");
    assert_eq!(d.assist()["shown"], 1);
    assert_eq!(d.untouched(), before);

    // cancel leaves nothing behind
    d.click("program.brush.inbetween.cancel");
    assert!(d.harness.state().session.inbetween_preview().is_none());
    assert_eq!(d.assist()["report"], Value::Null);
    assert_eq!(d.untouched(), before);

    // preview again and accept
    d.click("program.brush.inbetween.preview");
    d.click("program.brush.inbetween.accept");
    assert_eq!(d.frames_of_cels(), [0, 2, 4, 6, 8]);
    assert_eq!(d.harness.state().session.history.undo.len(), before.0 + 1, "one undo step");
    assert!(d.harness.state().session.inbetween_preview().is_none());

    // the same mapping headless gives the same cels
    let mut headless = session();
    headless
        .execute(
            "drawing.inbetweenAccept",
            json!({"from": 0, "to": 8, "pairs": [{"from": 0, "to": 0}, {"from": 1, "to": 1}], "step": 2, "easing": "linear", "unmatched": "drop"}),
        )
        .unwrap();
    let theirs = headless.project.items.values().find_map(|i| i.as_drawing()).cloned().unwrap();
    assert_eq!(d.drawing(), theirs, "UI and headless make the same cels for the same mapping");

    d.command("edit.undo", json!({})).unwrap();
    assert_eq!(d.frames_of_cels(), [0, 8]);
}

#[test]
fn unsupported_pairings_and_changed_pairs_are_said_and_escape_cancels() {
    let mut d = Driver::new();
    d.command("drawing.addStroke", json!({"frame": 0, "stroke": {"brush": "Eraser", "size": 8, "points": [[1.0, 1.0, 1.0], [9.0, 9.0, 1.0]]}})).unwrap();
    d.click("program.brush.inbetween.open");
    d.click("program.brush.inbetween.inOrder");
    let before = d.untouched();
    d.click("program.brush.inbetween.preview");
    let a = d.assist();
    assert_eq!(a["report"]["status"], "unsupported");
    assert_eq!(a["report"]["problems"][0]["kind"], "eraser");
    assert_eq!(a["previewed"], Value::Null, "nothing to accept");
    assert_eq!(d.untouched(), before);

    // remove the eraser from the picture by redrawing the cel without it, then preview, then change a row
    d.command("edit.undo", json!({})).unwrap();
    d.click("program.brush.inbetween.preview");
    assert_eq!(d.assist()["report"]["status"], "ok");
    d.click("program.brush.inbetween.row.1.reverse");
    d.frames(2);
    assert!(d.assist()["previewed"] != Value::Null, "the old preview is still the one that would be accepted");
    // Escape ends the preview, then closes the window
    d.harness.input_mut().events.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    d.frames(3);
    assert_eq!(d.assist()["previewed"], Value::Null);
    assert!(d.harness.state().session.inbetween_preview().is_none());
    assert_eq!(d.assist()["open"], true);
}
