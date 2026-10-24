//! Pose review (#61): previous/next drawing and hold-to-flip. Every exit path puts the playhead
//! and selection back, and a review never edits anything.

use std::sync::mpsc::{Sender, channel};

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

fn key(k: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers }
}

impl Driver {
    /// Cels at frames 0 (drawn), 4 (blank) and 8 (drawn) on a two-second drawing; the playhead
    /// rests on frame 6, inside the hold of the blank cel.
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.newSequence", json!({"name": "Review", "fps": 24, "width": 320, "height": 180, "video": 1, "audio": 0})).unwrap();
        let clip = session.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
        for (frame, draw) in [(0, true), (4, false), (8, true)] {
            session.execute("drawing.newCel", json!({"clip": clip, "frame": frame})).unwrap();
            if draw {
                let stroke = json!({"brush": "Pen", "color": "#000000", "size": 4.0, "points": [[10.0, 10.0, 1.0], [40.0 + frame as f64, 30.0, 1.0]]});
                session.execute("drawing.addStroke", json!({"clip": clip, "frame": frame, "stroke": stroke})).unwrap();
            }
        }
        session.execute("playhead.set", json!({"frame": 6})).unwrap();
        session.execute("timeline.select", json!({"clips": [clip]})).unwrap();
        let (tx, rx) = channel();
        let app = WaltCutApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, clip };
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

    fn event(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    fn inspect(&mut self) -> Value {
        self.call("ui.inspect", json!({}))["result"].clone()
    }

    fn frame(&self) -> i64 {
        let s = &self.harness.state().session;
        s.sequence_rate().frame_at(s.playhead())
    }

    fn review(&mut self) -> Value {
        self.inspect()["review"].clone()
    }

    fn selection(&self) -> Vec<u64> {
        self.harness.state().session.state.selection.iter().map(|c| c.0).collect()
    }

    /// What a review must not change: the undo history and the revision counter.
    fn untouched(&self) -> (usize, u64) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.revision)
    }
}

#[test]
fn a_held_review_shows_the_neighbouring_pose_and_restores_everything_on_release() {
    let mut d = Driver::new();
    let (before, selection) = (d.untouched(), d.selection());
    let alt_cmd = egui::Modifiers::ALT | egui::Modifiers::COMMAND | egui::Modifiers::MAC_CMD;
    d.event(egui::Event::ModifiersChanged(alt_cmd));
    d.event(key(egui::Key::ArrowUp, true, alt_cmd));
    assert_eq!(d.frame(), 0, "previous pose of the blank cel 4 is cel 0");
    let review = d.review();
    assert_eq!((review["active"].clone(), review["pose"].clone(), review["key"].clone()), (json!(true), json!("previous"), json!(true)));
    d.frames(3);
    assert_eq!(d.frame(), 0, "the key is still down");
    // flip to the other side while the first is let go and the second pressed: relative to frame 6
    d.event(key(egui::Key::ArrowUp, false, alt_cmd));
    assert_eq!(d.frame(), 6, "released: back where it was");
    assert_eq!(d.review()["active"], false);
    d.event(key(egui::Key::ArrowDown, true, alt_cmd));
    assert_eq!(d.frame(), 8);
    d.event(key(egui::Key::ArrowDown, false, alt_cmd));
    d.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    d.frames(2);
    assert_eq!(d.frame(), 6);
    assert_eq!(d.selection(), selection);
    assert_eq!(d.untouched(), before, "no undo step, no revision");
}

#[test]
fn a_command_review_lasts_until_ended_and_switching_stays_relative_to_the_origin() {
    let mut d = Driver::new();
    let before = d.untouched();
    d.command("drawing.reviewNext", json!({})).unwrap();
    assert_eq!(d.frame(), 8);
    // from the next pose to the previous one: still relative to frame 6, not to frame 8
    d.command("drawing.reviewPrevious", json!({})).unwrap();
    assert_eq!(d.frame(), 0);
    assert_eq!(d.review()["origin"], d.harness.state().session.sequence_rate().tick_of(6).0);
    d.frames(5);
    assert_eq!(d.review()["active"], true, "a command hold does not time out");
    assert_eq!(d.command("drawing.reviewEnd", json!({})).unwrap()["ended"], true);
    assert_eq!(d.frame(), 6);
    assert_eq!(d.command("drawing.reviewEnd", json!({})).unwrap()["ended"], false);
    // active:false lets go of the pose being shown
    d.command("drawing.reviewNext", json!({})).unwrap();
    d.command("drawing.reviewNext", json!({"active": false})).unwrap();
    assert_eq!((d.frame(), d.review()["active"].clone()), (6, json!(false)));
    assert_eq!(d.untouched(), before);
}

#[test]
fn escape_and_focus_loss_restore_the_playhead_and_selection() {
    let mut d = Driver::new();
    let selection = d.selection();
    d.command("drawing.reviewPrevious", json!({})).unwrap();
    // something selects another thing meanwhile (selection follows the playhead)
    d.harness.state_mut().session.state.selection.clear();
    d.event(key(egui::Key::Escape, true, egui::Modifiers::NONE));
    d.event(key(egui::Key::Escape, false, egui::Modifiers::NONE));
    d.frames(2);
    assert_eq!((d.frame(), d.review()["active"].clone()), (6, json!(false)));
    assert_eq!(d.selection(), selection);

    d.command("drawing.reviewNext", json!({})).unwrap();
    assert_eq!(d.frame(), 8);
    d.event(egui::Event::WindowFocused(false));
    d.frames(2);
    assert_eq!((d.frame(), d.review()["active"].clone()), (6, json!(false)));
}

#[test]
fn a_scrub_during_a_review_stands_and_missing_neighbours_start_nothing() {
    let mut d = Driver::new();
    d.command("drawing.reviewNext", json!({})).unwrap();
    d.command("playhead.set", json!({"frame": 11})).unwrap();
    d.frames(2);
    assert_eq!((d.frame(), d.review()["active"].clone()), (11, json!(false)), "the scrub is kept");

    // frame 11 is on the last cel: no next pose, and no review begins
    let err = d.command("drawing.reviewNext", json!({})).unwrap_err();
    assert!(err.contains("no next cel"), "{err}");
    assert_eq!((d.frame(), d.review()["active"].clone()), (11, json!(false)));
    // no previous pose before the first cel either
    d.command("playhead.set", json!({"frame": 1})).unwrap();
    assert!(d.command("drawing.reviewPrevious", json!({})).unwrap_err().contains("no previous cel"));
    assert_eq!(d.frame(), 1);
    // bad parameters are refused
    assert!(d.command("drawing.reviewNext", json!({"active": "yes"})).is_err());
    assert!(d.command("drawing.reviewNext", json!({"skipBlank": 3})).is_err());
}

#[test]
fn previous_and_next_cel_have_default_keys_and_the_review_keys_do_not_collide() {
    let mut d = Driver::new();
    let s = &d.harness.state().session.shortcuts;
    let primary = |id: &str| s.primary(id);
    for (id, keys) in [
        ("drawing.previousCel", "Cmd+Alt+,"),
        ("drawing.nextCel", "Cmd+Alt+."),
        ("drawing.reviewPrevious", "Cmd+Alt+Up"),
        ("drawing.reviewNext", "Cmd+Alt+Down"),
    ] {
        assert_eq!(primary(id).as_deref(), Some(keys), "{id}");
    }
    let conflicts = s.conflicts(waltcut_engine::shortcuts::Platform::current());
    let ours: Vec<_> = conflicts.iter().filter(|c| c.2.iter().any(|id| id.starts_with("drawing."))).collect();
    assert!(ours.is_empty(), "the drawing keys collide: {ours:?}");
    // and the key navigates
    let alt_cmd = egui::Modifiers::ALT | egui::Modifiers::COMMAND | egui::Modifiers::MAC_CMD;
    d.event(egui::Event::ModifiersChanged(alt_cmd));
    d.event(key(egui::Key::Period, true, alt_cmd));
    d.event(key(egui::Key::Period, false, alt_cmd));
    d.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    assert_eq!(d.frame(), 8);
    let _ = d.clip;
}
