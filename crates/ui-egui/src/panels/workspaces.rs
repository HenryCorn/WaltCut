//! Save and restore the single WaltCut layout.

use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::dock::{Layout, SavedWorkspace, WorkspacePrefs};
use crate::menus::MenuItem;

pub fn route(app: &mut FilmcraftApp, id: &str, _params: &Value) -> Option<Result<Value, String>> {
    let action = id.strip_prefix("window.workspace.")?;
    Some(match action {
        "waltcut" | "reset" => app.set_workspace("WaltCut").map(|()| json!({"workspace": "WaltCut"})),
        "saveChanges" => save(app),
        "restoreOriginal" => app.set_workspaces(WorkspacePrefs { current: "WaltCut".into(), saved: vec![] }).and_then(|()| {
            app.set_workspace("WaltCut")?;
            Ok(json!({"restored": "WaltCut"}))
        }),
        _ => Err(format!("unknown layout command `{action}`; WaltCut has one layout")),
    })
}

fn save(app: &mut FilmcraftApp) -> Result<Value, String> {
    Layout { root: app.ui.dock.clone(), floating: app.ui.floating.clone() }.validate()?;
    let layout = serde_json::to_value(&app.ui.dock).map_err(|e| format!("saving layout: {e}"))?;
    let floating = serde_json::to_value(&app.ui.floating).map_err(|e| format!("saving floating panels: {e}"))?;
    app.set_workspaces(WorkspacePrefs { current: "WaltCut".into(), saved: vec![SavedWorkspace { name: "WaltCut".into(), layout, floating }] })?;
    Ok(json!({"saved": "WaltCut"}))
}

pub fn menu(app: &FilmcraftApp, items: &mut Vec<MenuItem>) {
    for item in items {
        if item.id == "window.workspace.waltcut" {
            item.checked = Some(app.ui.workspace == "WaltCut");
        }
    }
}
