//! Headless UI tests of the M8.9 / M12.6 panels: Metadata (edit a field by typing, undo),
//! Timecode, Events and Progress, each opened from Window ▸ and driven by automation id through
//! the control channel.
//!
//! Set `WALTCUT_UI_SNAPSHOT_DIR=<dir>` to also render offscreen with wgpu and write `panels-*.png`.

#![allow(dead_code)]

use std::sync::Arc;
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
    fn with(session: Session) -> Self {
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

    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        Self::with(session)
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

    fn ids(&mut self, prefix: &str) -> Vec<String> {
        let v = self.ok("ui.elements", json!({"prefix": prefix}));
        v.as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }

    fn has(&mut self, id: &str) -> bool {
        self.ids(id).iter().any(|i| i == id)
    }

    fn label(&mut self, id: &str) -> String {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().find(|e| e["id"] == json!(id)).and_then(|e| e["label"].as_str().map(str::to_string)).unwrap_or_default()
    }

    /// Step frames until `id` is registered (frames render on worker threads).
    fn wait_for(&mut self, id: &str) {
        for _ in 0..400 {
            if self.has(id) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.frames(1);
        }
        panic!("{id} never appeared: {:?}", self.ids(id.split('.').next().unwrap()));
    }

    fn app(&mut self) -> &mut WaltCutApp {
        self.harness.state_mut()
    }

    fn ui_panels(&mut self) -> Value {
        self.ok("ui.inspect", json!({}))["ui"]["panels"].clone()
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

#[test]
fn every_panel_opens_from_the_window_menu() {
    let mut d = Driver::demo();
    let menu = d.ok("ui.menu.list", json!({}));
    let window: Vec<String> =
        menu.as_array().unwrap().iter().filter(|m| m["path"] == json!(["Window"])).filter_map(|m| m["label"].as_str().map(str::to_string)).collect();
    for p in ["Events", "Progress"] {
        assert!(window.iter().any(|w| w == p), "{p} not in Window: {window:?}");
    }
    for (panel, key) in [("Events", "events.clearAll"), ("Progress", "progress.showFinished")] {
        d.ok("ui.menu.invoke", json!({"id": format!("window.panel.{panel}")}));
        d.frames(3);
        assert!(d.has(&format!("panel.tab.{panel}")), "{panel} has no tab");
        assert!(!d.ids(key).is_empty(), "{panel}: no {key} widgets");
        assert_eq!(d.ok("ui.inspect", json!({}))["ui"]["focused"], json!(panel));
    }
}

/// #284 point 3: the panel menu's Maximize Frame keeps the layout (Restore Frame brings it back),
/// and closing every panel still leaves a dock that Window ▸ can reopen panels into.
#[test]
fn panel_menu_maximize_restores_and_an_empty_dock_reopens() {
    use waltcut_ui_egui::dock::{DockNode, PanelKind};
    let mut d = Driver::demo();
    let layout = d.app().ui.dock.clone();
    d.click("panel.menu.Program");
    assert_eq!(d.label("panel.menu.Program.maximize"), "Maximize Frame");
    d.click("panel.menu.Program.maximize");
    assert_eq!(d.app().ui.keys.maximized, Some(PanelKind::Program));
    assert!(!d.has("panel.Timeline"), "only the maximized panel is shown");
    d.click("panel.menu.Program");
    assert_eq!(d.label("panel.menu.Program.maximize"), "Restore Frame");
    d.click("panel.menu.Program.maximize");
    assert_eq!(d.app().ui.keys.maximized, None);
    assert_eq!(d.app().ui.dock, layout, "the layout survives maximize / restore");
    assert!(d.has("panel.Timeline"));

    let mut all = Vec::new();
    layout.panels(&mut all);
    for p in all {
        d.ok("ui.panel.close", json!({"panel": p.id()}));
    }
    assert_eq!(d.app().ui.dock, DockNode::Tabs { panels: vec![], active: 0 });
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Program"}));
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Timeline"}));
    d.frames(3);
    assert!(d.has("panel.tab.Program") && d.has("panel.tab.Timeline"));
}

/// The wheel over a tab strip switches that group's tab: down = next, up = previous, one tab per
/// notch, smaller trackpad steps adding up to a notch, stopping at the ends. Over the panel's
/// content the wheel leaves the tabs alone.
#[test]
fn wheel_over_a_tab_strip_switches_tabs() {
    use waltcut_ui_egui::dock::{DockNode, PanelKind};
    fn shown(d: &mut Driver) -> PanelKind {
        fn find(n: &DockNode) -> Option<PanelKind> {
            match n {
                DockNode::Tabs { panels, active } if panels.contains(&PanelKind::Project) => panels.get(*active).copied(),
                DockNode::Tabs { .. } => None,
                DockNode::Split { a, b, .. } => find(a).or_else(|| find(b)),
            }
        }
        find(&d.app().ui.dock).unwrap()
    }
    fn wheel(d: &mut Driver, dy: f64) {
        let id = format!("panel.tab.{}", shown(d).id());
        d.ok("ui.scroll", json!({"id": id, "dy": dy}));
        d.frames(2);
    }
    let mut d = Driver::demo();
    assert_eq!(shown(&mut d), PanelKind::Project);
    wheel(&mut d, -40.0);
    assert_eq!(shown(&mut d), PanelKind::Effects);
    wheel(&mut d, 40.0);
    assert_eq!(shown(&mut d), PanelKind::Project);
    wheel(&mut d, 120.0);
    assert_eq!(shown(&mut d), PanelKind::Project, "no wrap past the first tab");

    // a trackpad swipe in small steps moves one tab once the steps add up to a notch
    let r = d.ok("ui.elements", json!({"prefix": "panel.tab.Project"}))[0]["rect"].clone();
    let at = egui::pos2((r[0].as_f64().unwrap() + r[2].as_f64().unwrap() / 2.0) as f32, (r[1].as_f64().unwrap() + r[3].as_f64().unwrap() / 2.0) as f32);
    d.harness.input_mut().events.push(egui::Event::PointerMoved(at));
    d.frames(1);
    for step in 0..5 {
        d.harness.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -10.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        d.frames(1);
        let expect = if step < 3 { PanelKind::Project } else { PanelKind::Effects };
        assert_eq!(shown(&mut d), expect, "after {} pt", (step + 1) * 10);
    }

    d.ok("ui.scroll", json!({"id": "panel.Effects", "dy": -40.0}));
    d.frames(2);
    assert_eq!(shown(&mut d), PanelKind::Effects, "the wheel over the content keeps the tab");

    for _ in 0..10 {
        wheel(&mut d, -40.0);
    }
    assert_eq!(shown(&mut d), PanelKind::History, "no wrap past the last tab");
}

#[test]
fn events_and_progress_panels() {
    let mut d = Driver::demo();
    d.exec("events.clear", json!({}));
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Events"}));
    d.frames(2);
    // a failed command shows up as an error row
    let r = d.call("engine.execute", json!({"command": "clip.rename", "params": {}}));
    assert_eq!(r["ok"], json!(false));
    d.frames(2);
    let rows = d.ids("events.row.");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(d.label(&rows[0]).contains("name"), "{}", d.label(&rows[0]));
    d.click(&rows[0]);
    assert!(d.has("events.details"));
    d.snapshot("panels-events", Some("events."));
    // filter: errors only still shows it; Clear All empties the list
    d.ok("ui.set", json!({"panels": {"events": {"level": "error"}}}));
    d.frames(2);
    assert_eq!(d.ids("events.row.").len(), 1);
    d.click("events.clearAll");
    assert!(d.ids("events.row.").is_empty());
    assert!(d.app().session.log.entries.is_empty());

    // Progress: a running job with a Cancel button
    let job = waltcut_engine::Job {
        id: 4242,
        label: "Export demo.mp4".into(),
        progress: Arc::new(Default::default()),
        result: Arc::new(std::sync::Mutex::new(None)),
    };
    job.progress.total.store(10, std::sync::atomic::Ordering::Relaxed);
    job.progress.done.store(4, std::sync::atomic::Ordering::Relaxed);
    d.app().session.jobs.push(job.clone());
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Progress"}));
    d.frames(3);
    assert!(d.label("progress.job.4242").contains("40 %"), "{}", d.label("progress.job.4242"));
    d.snapshot("panels-progress", Some("progress."));
    d.click("progress.cancel.4242");
    assert!(job.progress.cancel.load(std::sync::atomic::Ordering::Relaxed));
    *job.result.lock().unwrap() = Some(Err("cancelled".into()));
    d.frames(3);
    assert!(!d.has("progress.cancel.4242"));
    assert!(d.label("progress.job.4242").contains("Cancelled"));
    // the Events panel logged the job
    let ev = d.exec("events.list", json!({}));
    let msgs: Vec<&str> = ev["entries"].as_array().unwrap().iter().filter_map(|e| e["message"].as_str()).collect();
    assert!(msgs.contains(&"Started: Export demo.mp4") && msgs.contains(&"Cancelled: Export demo.mp4"), "{msgs:?}");
    // hide finished jobs
    d.click("progress.showFinished");
    assert!(!d.has("progress.job.4242"));
}

/// #1075: child backgrounds must not cover the focus outline shared with Program. Check the
/// rendered primitives, including Media Browser's parent group, in both requested themes.
#[test]
fn focused_panel_outline_is_not_covered_by_child_backgrounds() {
    fn rectangles<'a>(shape: &'a egui::Shape, clip: egui::Rect, out: &mut Vec<(&'a egui::epaint::RectShape, egui::Rect)>) {
        match shape {
            egui::Shape::Rect(rect) => out.push((rect, clip)),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| rectangles(shape, clip, out)),
            _ => {}
        }
    }
    let mut d = Driver::demo();
    for theme in ["dark", "light"] {
        d.ok("ui.set", json!({"theme": theme}));
        for panel in ["Program", "EssentialGraphics", "Timeline", "Effects"] {
            d.ok("ui.panel.show", json!({"panel": panel}));
            d.ok("ui.set", json!({"focused": panel}));
            d.frames(3);
            d.snapshot(&format!("focus-{theme}-{panel}"), None);
            let mut rects = Vec::new();
            for shape in &d.harness.output().shapes {
                rectangles(&shape.shape, shape.clip_rect, &mut rects);
            }
            let focus = d.harness.state().tokens.focus;
            let outlines: Vec<_> = rects
                .iter()
                .enumerate()
                .filter(|(_, (r, _))| r.stroke.color == focus && r.stroke.width == 1.0 && r.rect.width() > 100.0 && r.rect.height() > 80.0)
                .collect();
            assert_eq!(outlines.len(), 1, "{theme} {panel}: one focus outline for the whole parent group");
            let (at, (outline, _)) = outlines[0];
            let bounds = outline.rect;
            let strips = [
                egui::Rect::from_min_max(bounds.min, egui::pos2(bounds.max.x, bounds.min.y + 1.0)),
                egui::Rect::from_min_max(egui::pos2(bounds.min.x, bounds.max.y - 1.0), bounds.max),
                egui::Rect::from_min_max(bounds.min, egui::pos2(bounds.min.x + 1.0, bounds.max.y)),
                egui::Rect::from_min_max(egui::pos2(bounds.max.x - 1.0, bounds.min.y), bounds.max),
            ];
            for (child, clip) in &rects[at + 1..] {
                if child.fill.a() != 0 {
                    let painted = child.rect.intersect(*clip);
                    assert!(
                        strips.iter().all(|edge| !edge.intersect(painted).is_positive()),
                        "{theme} {panel}: a later child background {painted:?} covers the focus outline {bounds:?}"
                    );
                }
            }
        }
    }
}

/// Presets remain indented under their retained folder.
#[test]
fn effects_are_indented_under_their_folders() {
    fn text_x(d: &mut Driver, text: &str) -> f32 {
        fn find(shapes: &[egui::Shape], text: &str) -> Option<f32> {
            shapes.iter().find_map(|s| match s {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.pos.x),
                egui::Shape::Vec(v) => find(v, text),
                _ => None,
            })
        }
        let shapes: Vec<egui::Shape> = d.harness.output().shapes.iter().map(|c| c.shape.clone()).collect();
        find(&shapes, text).unwrap_or_else(|| panic!("`{text}` is not drawn"))
    }
    let mut d = Driver::demo();
    d.ok("ui.panel.show", json!({"panel": "Effects"}));
    d.frames(3);
    d.click("effects.folder.Presets");
    d.frames(2);
    let preset = d.app().session.presets.all()[0].name.clone();
    let (a, b) = (text_x(&mut d, "Presets"), text_x(&mut d, &preset));
    assert!(b >= a + 12.0, "`{preset}` ({b}) is not indented under Presets ({a})");
}
