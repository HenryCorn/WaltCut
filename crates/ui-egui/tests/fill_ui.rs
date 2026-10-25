//! Fill mode in the Brush tool (#68): the toggle and tolerance commands, a click that fills, a
//! drag that draws nothing, the options bar that a click cannot reach through, the edits of the
//! newest fill, a locked track, and the keys the mode takes.

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

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

impl Driver {
    /// 128x72 with a closed black outline from (30, 15) to (100, 57).
    fn new() -> Self {
        let mut s = Session::default();
        s.execute("file.newSequence", json!({"name": "Fill", "width": 128, "height": 72, "fps": 24, "video": 2, "audio": 0})).unwrap();
        let clip = s.execute("drawing.new", json!({"seconds": 2})).unwrap()["clip"].as_u64().unwrap();
        let pts = [[30, 15], [100, 15], [100, 57], [30, 57], [30, 15]].map(|p| json!([p[0], p[1], 1.0]));
        let stroke = json!({"brush": "Pen", "color": "#000000", "size": 4.0, "points": pts});
        s.execute("drawing.addStroke", json!({"clip": clip, "frame": 0, "stroke": stroke})).unwrap();
        s.execute("timeline.select", json!({"clips": [clip]})).unwrap();
        let (tx, rx) = channel();
        let app = WaltCutApp::new(s).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, clip };
        d.frames(4);
        d.call("ui.set", json!({"tool": "Brush"}));
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
            if let Ok(r) = reply.try_recv() {
                return r;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn ui(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let r = self.call("ui.menu.invoke", json!({"id": id, "params": params}));
        self.frames(2);
        if r["ok"] == true { Ok(r["result"].clone()) } else { Err(r["error"].as_str().unwrap_or("").to_string()) }
    }

    fn engine(&mut self, id: &str, params: Value) -> Value {
        let r = self.call("engine.execute", json!({"command": id, "params": params}));
        self.frames(2);
        assert_eq!(r["ok"], true, "{id}: {r}");
        r["result"].clone()
    }

    fn brush(&mut self) -> Value {
        self.call("ui.inspect", json!({}))["result"]["ui"]["brush"].clone()
    }

    fn event(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    fn element(&mut self, id: &str) -> egui::Rect {
        let v = self.call("ui.elements", json!({"prefix": "program.brush"}))["result"].clone();
        let r =
            v.as_array().and_then(|a| a.iter().find(|e| e["id"] == id)).and_then(|e| e["rect"].as_array()).unwrap_or_else(|| panic!("no element {id}")).clone();
        let f = |i: usize| r[i].as_f64().unwrap() as f32;
        egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)))
    }

    /// The screen point of canvas pixel (x, y).
    fn at(&mut self, x: f32, y: f32) -> egui::Pos2 {
        let b = self.element("program.brush");
        b.min + egui::vec2(x / 128.0 * b.width(), y / 72.0 * b.height())
    }

    fn click(&mut self, p: egui::Pos2) {
        self.event(egui::Event::PointerMoved(p));
        self.event(button(p, true));
        self.event(button(p, false));
        self.frames(3);
    }

    fn click_element(&mut self, id: &str) {
        let c = self.element(id).center();
        self.click(c);
    }

    fn drag(&mut self, path: &[egui::Pos2]) {
        self.event(egui::Event::PointerMoved(path[0]));
        self.event(button(path[0], true));
        for p in &path[1..] {
            self.event(egui::Event::PointerMoved(*p));
        }
        self.event(button(*path.last().unwrap(), false));
        self.frames(3);
    }

    fn strokes(&self) -> Vec<waltcut_project::drawing::Stroke> {
        self.harness.state().session.project.items.values().find_map(|i| i.as_drawing()).unwrap().cels[0].strokes.to_vec()
    }

    fn untouched(&self) -> (usize, String) {
        let s = &self.harness.state().session;
        (s.history.undo.len(), s.project.to_json())
    }

    fn status(&self) -> String {
        self.harness.state().ui.status.clone()
    }

    fn fills(&self) -> usize {
        self.strokes().iter().filter(|s| s.is_fill()).count()
    }
}

#[test]
fn the_fill_mode_command_toggles_sets_the_tolerance_and_refuses_bad_values_whole() {
    let mut d = Driver::new();
    assert_eq!((d.brush()["fill"].clone(), d.brush()["fill_tolerance"].clone()), (json!(false), json!(32)));
    let state = |fill: bool, tolerance: u32| json!({"fill": fill, "tolerance": tolerance, "gap": 0, "grow": 0, "reference": null, "preview": false});
    assert_eq!(d.ui("drawing.fillMode", json!({})).unwrap(), state(true, 32));
    assert_eq!(d.ui("drawing.fillMode", json!({"tolerance": 90})).unwrap(), state(true, 90), "a tolerance alone keeps the mode");
    assert_eq!(d.ui("drawing.fillMode", json!({"enabled": false, "tolerance": 5})).unwrap(), state(false, 5));
    for bad in [
        json!({"tolerance": 256}),
        json!({"tolerance": -1}),
        json!({"tolerance": 1.5}),
        json!({"tolerance": "x"}),
        json!({"enabled": "yes"}),
        json!({"colour": 1}),
    ] {
        let before = d.brush();
        assert!(d.ui("drawing.fillMode", bad.clone()).is_err(), "{bad}");
        assert_eq!(d.brush(), before, "{bad} changes nothing");
    }
    // Lasso and Fill exclude each other
    d.ui("drawing.fillMode", json!({"enabled": true})).unwrap();
    d.ui("drawing.lassoMode", json!({"enabled": true})).unwrap();
    assert_eq!((d.brush()["fill"].clone(), d.brush()["lasso"].clone()), (json!(false), json!(true)));
    d.ui("drawing.fillMode", json!({"enabled": true})).unwrap();
    assert_eq!((d.brush()["fill"].clone(), d.brush()["lasso"].clone()), (json!(true), json!(false)));
    // the mode is never loaded back from saved interface state
    let saved = serde_json::to_value(&d.harness.state().ui.brush).unwrap();
    assert_eq!(saved["fill"], json!(true));
    let loaded: waltcut_ui_egui::panels::drawing::BrushSettings = serde_json::from_value(saved).unwrap();
    assert!(!loaded.fill, "a session starts drawing");
}

#[test]
fn a_click_inside_a_closed_shape_fills_it_as_one_undo_step() {
    let mut d = Driver::new();
    d.ui("drawing.fillMode", json!({"enabled": true})).unwrap();
    let before = d.untouched().0;
    let p = d.at(64.0, 36.0);
    d.click(p);
    assert_eq!(d.fills(), 1, "{}", d.status());
    assert_eq!(d.untouched().0, before + 1, "one undo step");
    assert!(d.status().starts_with("Filled "), "{}", d.status());
    let fill = d.strokes().pop().unwrap();
    let region = fill.fill.as_deref().unwrap();
    assert_eq!(region.seed, [64, 36]);
    assert!(region.filled_pixels() > 2000 && region.filled_pixels() < 128 * 72 / 2, "only the inside: {}", region.filled_pixels());
    assert_eq!(fill.color, d.harness.state().ui.brush.color, "the brush colour");
    // undo takes it away, the outline stays
    d.engine("edit.undo", json!({}));
    assert_eq!(d.strokes().len(), 1);
    // outside the shape fills the surround, and tolerance reaches the command
    d.ui("drawing.fillMode", json!({"tolerance": 7})).unwrap();
    // (the options bar covers the picture's top left corner, so a point down in the corner)
    let outside = d.at(6.0, 66.0);
    d.click(outside);
    let f = d.strokes().pop().unwrap();
    assert_eq!((f.fill.as_deref().unwrap().tolerance, f.fill.as_deref().unwrap().seed), (7, [6, 66]));
}

#[test]
fn a_drag_in_fill_mode_draws_nothing_and_the_options_bar_is_not_clicked_through() {
    let mut d = Driver::new();
    d.ui("drawing.fillMode", json!({"enabled": true})).unwrap();
    let before = d.untouched();
    let (a, b) = (d.at(40.0, 30.0), d.at(90.0, 45.0));
    d.drag(&[a, b]);
    assert_eq!(d.untouched(), before, "a drag neither draws nor fills");
    // the tolerance slider sits inside the picture's rectangle: using it fills nothing
    d.click_element("program.brush.fill.tolerance");
    assert_eq!(d.untouched().0, before.0, "a click on the options bar is not a click on the picture");
    assert_eq!(d.fills(), 0);
}

#[test]
fn the_controls_are_registered_and_the_newest_fill_can_be_recoloured_and_removed() {
    let mut d = Driver::new();
    // the toggle is there before the mode is on; the rest appear with it
    d.element("program.brush.fill");
    d.click_element("program.brush.fill");
    assert_eq!(d.brush()["fill"], json!(true), "the button turns the mode on");
    for id in ["program.brush.fill.tolerance", "program.brush.fill.recolor", "program.brush.fill.remove"] {
        d.element(id);
    }
    let p = d.at(64.0, 36.0);
    d.click(p);
    assert_eq!(d.fills(), 1);
    // recolour with the brush colour
    d.harness.state_mut().ui.brush.color = [0.1, 0.8, 0.2, 1.0];
    d.frames(2);
    let undo = d.untouched().0;
    d.click_element("program.brush.fill.recolor");
    let color = d.strokes().pop().unwrap().color;
    assert!((color[1] - 0.8).abs() < 1e-6, "{color:?}");
    assert_eq!(d.untouched().0, undo + 1, "one undo step");
    // remove the newest fill
    d.click_element("program.brush.fill.remove");
    assert_eq!(d.fills(), 0);
    assert_eq!(d.untouched().0, undo + 2);
    // with no fill left both are disabled: a click changes nothing
    let before = d.untouched();
    d.click_element("program.brush.fill.remove");
    d.click_element("program.brush.fill.recolor");
    assert_eq!(d.untouched(), before);
}

#[test]
fn a_locked_track_and_a_click_outside_the_canvas_say_why_and_change_nothing() {
    let mut d = Driver::new();
    d.ui("drawing.fillMode", json!({"enabled": true})).unwrap();
    let track = {
        let s = &d.harness.state().session;
        s.active_sequence().and_then(|q| q.find_item(waltcut_project::ClipId(d.clip))).map(|(t, _)| t.0).unwrap()
    };
    d.engine("timeline.setTrack", json!({"track": track, "locked": true}));
    let before = d.untouched();
    let p = d.at(64.0, 36.0);
    d.click(p);
    assert_eq!(d.untouched(), before, "nothing changed");
    assert!(d.status().contains("locked"), "{}", d.status());
    d.engine("timeline.setTrack", json!({"track": track, "locked": false}));
    // a point past the canvas is refused by the engine with its reason
    let before = d.untouched();
    let r = d.harness.state_mut().session.execute("drawing.fill", json!({"point": [500.0, 5.0]}));
    assert!(r.unwrap_err().to_string().contains("outside"));
    assert_eq!(d.untouched(), before);
}

#[test]
fn the_mode_has_a_key_that_nothing_else_uses() {
    let mut d = Driver::new();
    let items = d.call("ui.menu.list", json!({}))["result"].clone();
    let all = items.as_array().unwrap();
    let mine = all.iter().find(|m| m["id"] == "drawing.fillMode").expect("in the menu");
    assert_eq!(mine["shortcut"], json!("Alt+F"));
    assert_eq!(mine["enabled"], json!(true));
    let same: Vec<&str> = all.iter().filter(|m| m["shortcut"] == mine["shortcut"]).filter_map(|m| m["id"].as_str()).collect();
    assert_eq!(same, vec!["drawing.fillMode"], "no other command has Alt+F");
    // not available with another tool selected
    d.call("ui.set", json!({"tool": "Selection"}));
    d.frames(3);
    assert!(d.ui("drawing.fillMode", json!({})).is_err());
}

// ---- reference drawing, gap closure, edge expansion and preview (#69) ----

impl Driver {
    /// A second drawing clip, "Lines", with a closed square outline from (20, 8) to (110, 64) that is
    /// not the colour clip's own outline; the colour clip stays the target.
    fn with_lines(open_by: Option<u32>) -> (Self, u64) {
        let mut d = Driver::new();
        let art = d.engine("drawing.new", json!({"seconds": 2, "name": "Lines"}))["clip"].as_u64().unwrap();
        let pen = |pts: Vec<[u32; 2]>| json!({"brush": "Pen", "color": "#000000", "size": 4.0, "points": pts.iter().map(|p| json!([p[0], p[1], 1.0])).collect::<Vec<_>>()});
        let top = |a: u32, b: u32| (pen(vec![[20, 8], [a, 8]]), pen(vec![[b, 8], [110, 8]]));
        let (l, r) = match open_by {
            Some(g) => top(60, 64 + g),
            None => top(60, 60),
        };
        for st in [l, r, pen(vec![[110, 8], [110, 64], [20, 64], [20, 8]])] {
            d.engine("drawing.addStroke", json!({"clip": art, "frame": 0, "stroke": st}));
        }
        d.engine("timeline.select", json!({"clips": [d.clip]}));
        d.frames(3);
        (d, art)
    }

    fn colour_strokes(&self) -> Vec<waltcut_project::drawing::Stroke> {
        let s = &self.harness.state().session;
        let seq = s.active_sequence().unwrap();
        waltcut_engine::drawing::drawing_of(s, seq, waltcut_project::ClipId(self.clip)).unwrap().1.cels[0].strokes.to_vec()
    }

    fn pending(&mut self) -> Value {
        self.brush()["fill_pending"].clone()
    }
}

#[test]
fn the_new_options_are_commands_with_whole_value_checking_and_only_the_numbers_are_saved() {
    let mut d = Driver::new();
    let r = d.ui("drawing.fillMode", json!({"enabled": true, "gap": 3, "grow": 2, "preview": true})).unwrap();
    assert_eq!((r["gap"].clone(), r["grow"].clone(), r["preview"].clone(), r["reference"].clone()), (json!(3), json!(2), json!(true), Value::Null));
    let r = d.ui("drawing.fillMode", json!({"reference": 42})).unwrap();
    assert_eq!(r["reference"], json!(42));
    assert_eq!(d.ui("drawing.fillMode", json!({"reference": null})).unwrap()["reference"], Value::Null);
    for bad in [
        json!({"gap": 9}),
        json!({"gap": -1}),
        json!({"gap": 2.5}),
        json!({"grow": 9}),
        json!({"grow": "x"}),
        json!({"reference": 0}),
        json!({"reference": -4}),
        json!({"reference": "clip"}),
        json!({"preview": "yes"}),
        json!({"gap": 2, "grow": 99}),
    ] {
        let before = d.brush();
        assert!(d.ui("drawing.fillMode", bad.clone()).is_err(), "{bad}");
        assert_eq!(d.brush(), before, "{bad} changes nothing, not even the valid half");
    }
    d.ui("drawing.fillMode", json!({"reference": 7})).unwrap();
    let saved = serde_json::to_value(&d.harness.state().ui.brush).unwrap();
    let loaded: waltcut_ui_egui::panels::drawing::BrushSettings = serde_json::from_value(saved).unwrap();
    assert_eq!((loaded.fill_gap, loaded.fill_grow), (3, 2), "the numbers are interface state");
    assert_eq!((loaded.fill_reference, loaded.fill_preview, loaded.fill_pending.is_some(), loaded.fill), (None, false, false, false), "the rest is not");
    // no preview to apply or cancel
    assert!(d.ui("drawing.fillApply", json!({})).is_err());
    assert!(d.ui("drawing.fillCancel", json!({})).is_err());
    assert!(d.ui("drawing.fillCancel", json!({"x": 1})).is_err());
}

#[test]
fn the_reference_gap_grow_and_preview_controls_are_registered() {
    let mut d = Driver::new();
    d.click_element("program.brush.fill");
    for id in ["program.brush.fill.reference", "program.brush.fill.gap", "program.brush.fill.grow", "program.brush.fill.preview"] {
        d.element(id);
    }
    d.click_element("program.brush.fill.preview");
    assert_eq!(d.brush()["fill_preview"], json!(true));
    for id in ["program.brush.fill.apply", "program.brush.fill.cancel"] {
        d.element(id);
    }
    // using the options bar neither fills nor previews
    assert_eq!((d.fills(), d.pending()), (0, Value::Null));
}

#[test]
fn a_click_fills_from_the_reference_clip_and_not_from_the_cels_own_lines() {
    let (mut d, art) = Driver::with_lines(None);
    d.ui("drawing.fillMode", json!({"enabled": true, "reference": art})).unwrap();
    let art_before = serde_json::to_string(&d.harness.state().session.project.items.values().filter_map(|i| i.as_drawing()).collect::<Vec<_>>()).unwrap();
    let undo = d.untouched().0;
    // (64, 36) is inside both outlines: the reference's is the bigger one on the left
    let p = d.at(64.0, 36.0);
    d.click(p);
    let strokes = d.colour_strokes();
    assert_eq!(strokes.len(), 2, "the outline and the fill: {}", d.status());
    let f = strokes[1].fill.as_deref().unwrap();
    assert_eq!(f.reference, Some(art));
    // reference interior (20..110 x 8..64, 4 px lines) is wider than the cel's own (30..100)
    let [x, _, w, _] = f.rect;
    assert!(x < 28 && x + w > 104, "the region comes from the reference's lines: {:?}", f.rect);
    assert_eq!(d.untouched().0, undo + 1);
    let art_after = serde_json::to_string(&d.harness.state().session.project.items.values().filter_map(|i| i.as_drawing()).collect::<Vec<_>>()).unwrap();
    assert!(art_after.contains("Fill"), "the colour cel holds it");
    assert_eq!(art_before.matches("Pen").count(), art_after.matches("Pen").count(), "no line art was copied or changed");
}

#[test]
fn gap_closing_from_the_slider_command_stops_a_leak() {
    let (mut d, art) = Driver::with_lines(Some(4));
    d.ui("drawing.fillMode", json!({"enabled": true, "reference": art})).unwrap();
    let p = d.at(64.0, 36.0);
    d.click(p);
    let leaked = d.colour_strokes().pop().unwrap().fill.unwrap().filled_pixels();
    assert!(leaked > 128 * 72 * 6 / 10, "an opening of 4 pixels leaks with no radius: {leaked}");
    d.engine("edit.undo", json!({}));
    d.ui("drawing.fillMode", json!({"gap": 2})).unwrap();
    d.click(p);
    let closed = d.colour_strokes().pop().unwrap().fill.unwrap();
    assert!(closed.filled_pixels() < 128 * 72 / 2, "a radius of 2 closes it: {}", closed.filled_pixels());
    assert_eq!((closed.gap, closed.grow), (2, 0));
    // a seed beside a line starts from the nearest pixel away from it and still fills the shape
    d.ui("drawing.fillMode", json!({"gap": 8})).unwrap();
    d.engine("edit.undo", json!({}));
    let near = d.at(23.0, 36.0);
    d.click(near);
    let near_fill = d.colour_strokes().pop().unwrap().fill.unwrap();
    near_fill.validate(128, 72).unwrap();
    assert_ne!(near_fill.seed, [23, 36], "it started from the nearest open pixel");
    // a seed on a thin line has nowhere to start when closing gaps: it says why and changes nothing
    d.engine("edit.undo", json!({}));
    let before = d.untouched();
    let on_line = d.at(20.0, 36.0);
    d.click(on_line);
    assert_eq!(d.untouched(), before);
    assert!(d.status().contains("too close to a line"), "{}", d.status());
}

#[test]
fn a_preview_shows_the_region_and_apply_or_cancel_ends_it_without_surprises() {
    let (mut d, art) = Driver::with_lines(None);
    d.ui("drawing.fillMode", json!({"enabled": true, "reference": art, "preview": true, "grow": 1})).unwrap();
    let before = d.untouched();
    let p = d.at(64.0, 36.0);
    d.click(p);
    assert_eq!(d.untouched(), before, "a preview changes neither the cel nor the history nor the project");
    let pending = d.pending();
    assert!(pending["pixels"].as_u64().unwrap() > 2000 && pending["runs"].as_u64().unwrap() >= 1, "{pending}");
    assert!(d.status().starts_with("Preview: "), "{}", d.status());
    d.frames(2);
    // Cancel (button): nothing was done
    d.click_element("program.brush.fill.cancel");
    assert_eq!((d.pending(), d.untouched()), (Value::Null, before.clone()));
    // Escape cancels too
    d.click(p);
    assert!(d.pending().is_object());
    d.event(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    d.frames(2);
    assert_eq!((d.pending(), d.untouched()), (Value::Null, before.clone()));
    // Apply: one undo step, the previewed region exactly
    d.click(p);
    let shown = d.pending()["pixels"].clone();
    d.click_element("program.brush.fill.apply");
    assert_eq!((d.pending(), d.fills(), d.untouched().0), (Value::Null, 1, before.0 + 1));
    let f = d.colour_strokes().pop().unwrap().fill.unwrap();
    assert_eq!(json!(f.filled_pixels()), shown, "applying paints what was previewed");
    assert_eq!((f.grow, f.reference), (1, Some(art)));
    // a change after the preview drops it: Apply would not be what was shown
    d.click(p);
    assert!(d.pending().is_object());
    d.engine("drawing.addStroke", json!({"frame": 0, "stroke": {"brush": "Pen", "color": "#ff0000", "size": 2.0, "points": [[5, 5, 1.0], [9, 9, 1.0]]}}));
    d.frames(3);
    assert_eq!(d.pending(), Value::Null, "an edit drops the preview");
    assert!(d.ui("drawing.fillApply", json!({})).is_err());
    // turning the mode off drops it as well
    d.click(p);
    assert!(d.pending().is_object());
    d.ui("drawing.fillMode", json!({"enabled": false})).unwrap();
    assert_eq!(d.pending(), Value::Null);
}

#[test]
fn a_preview_on_a_locked_track_or_a_missing_reference_says_why_and_shows_nothing() {
    let (mut d, art) = Driver::with_lines(None);
    d.ui("drawing.fillMode", json!({"enabled": true, "reference": art, "preview": true})).unwrap();
    let track = {
        let s = &d.harness.state().session;
        s.active_sequence().and_then(|q| q.find_item(waltcut_project::ClipId(d.clip))).map(|(t, _)| t.0).unwrap()
    };
    d.engine("timeline.setTrack", json!({"track": track, "locked": true}));
    let before = d.untouched();
    let p = d.at(64.0, 36.0);
    d.click(p);
    assert!(d.status().contains("locked"), "{}", d.status());
    assert_eq!((d.pending(), d.untouched()), (Value::Null, before));
    d.engine("timeline.setTrack", json!({"track": track, "locked": false}));
    d.ui("drawing.fillMode", json!({"reference": 987_654})).unwrap();
    d.click(p);
    assert!(d.status().contains("no longer exists"), "{}", d.status());
    assert_eq!(d.pending(), Value::Null);
    assert_eq!(d.fills(), 0);
}
