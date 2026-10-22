//! The single animation layout saves, resets and survives restart.

use egui_kittest::Harness;
use serde_json::json;
use waltcut_engine::Session;
use waltcut_ui_egui::FilmcraftApp;
use waltcut_ui_egui::dock::{self, PanelKind};
use waltcut_ui_egui::menus;

#[test]
fn one_layout_saves_resets_and_rejects_removed_commands() {
    let dir = std::env::temp_dir().join(format!("waltcut-layout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = Session::default();
    session.prefs_path = Some(dir.join("preferences.json"));
    let mut app = FilmcraftApp::new(session);
    let ctx = egui::Context::default();
    assert_eq!(app.ui.workspace, "WaltCut");
    assert_eq!(dock::names(&app.workspaces), ["WaltCut"]);
    assert!(app.ui.dock.contains(PanelKind::History));
    let before = app.ui.dock.clone();
    assert!(app.set_workspace("Editing").is_err());
    assert_eq!(app.ui.workspace, "WaltCut");
    assert_eq!(app.ui.dock, before);
    for removed in ["Libraries", "Info", "Metadata", "Timecode", "ProjectNotes"] {
        assert!(PanelKind::from_name(removed).is_none(), "{removed}");
        assert!(menus::invoke(&mut app, &ctx, &format!("window.panel.{removed}"), json!({})).is_err(), "{removed}");
    }

    for removed in ["assembly", "audio", "color", "learning", "review", "saveAs", "rename", "delete", "edit"] {
        assert!(menus::invoke(&mut app, &ctx, &format!("window.workspace.{removed}"), json!({})).is_err(), "{removed}");
    }
    app.ui.dock.close(PanelKind::History);
    menus::invoke(&mut app, &ctx, "window.workspace.saveChanges", json!({})).unwrap();
    app.ui.dock.close(PanelKind::Project);
    menus::invoke(&mut app, &ctx, "window.workspace.reset", json!({})).unwrap();
    assert!(app.ui.dock.contains(PanelKind::Project));
    assert!(!app.ui.dock.contains(PanelKind::History));

    let mut session = Session::default();
    session.prefs_path = Some(dir.join("preferences.json"));
    let restarted = FilmcraftApp::new(session);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| restarted);
    for _ in 0..4 {
        let ctx = h.ctx.clone();
        let mut input = std::mem::take(h.input_mut());
        eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut input);
        *h.input_mut() = input;
        h.step();
    }
    assert_eq!(h.state().ui.workspace, "WaltCut");
    assert!(h.state().ui.dock.contains(PanelKind::Project));
    assert!(!h.state().ui.dock.contains(PanelKind::History));
    let app = h.state_mut();
    menus::invoke(app, &ctx, "window.workspace.restoreOriginal", json!({})).unwrap();
    assert!(app.ui.dock.contains(PanelKind::History));
    drop(h);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_layout_save_preserves_the_previous_saved_state() {
    let dir = std::env::temp_dir().join(format!("waltcut-layout-save-error-{}", std::process::id()));
    // A directory at the file destination makes the atomic rename fail on every supported host.
    std::fs::create_dir_all(dir.join(dock::WORKSPACES_FILE)).unwrap();
    let mut session = Session::default();
    session.prefs_path = Some(dir.join("preferences.json"));
    let mut app = FilmcraftApp::new(session);
    let previous = serde_json::to_value(&app.workspaces).unwrap();
    app.ui.dock.close(PanelKind::History);
    let current = app.ui.dock.clone();
    let error = menus::invoke(&mut app, &egui::Context::default(), "window.workspace.saveChanges", json!({})).unwrap_err();
    assert!(error.contains("saving workspaces"), "{error}");
    assert_eq!(serde_json::to_value(&app.workspaces).unwrap(), previous);
    assert_eq!(app.ui.dock, current);
    std::fs::remove_dir_all(dir).unwrap();
}
