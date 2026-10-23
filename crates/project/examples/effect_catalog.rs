use std::process::ExitCode;

use serde_json::json;
use waltcut_project::effect::EFFECT_TOP_FOLDERS;

fn main() -> ExitCode {
    let items: Vec<serde_json::Value> = waltcut_project::effect_defs()
        .iter()
        .filter(|d| d.category.first().is_some_and(|top| EFFECT_TOP_FOLDERS.contains(top)))
        .map(|d| json!({"id": d.id, "name": d.name, "kind": format!("{:?}", d.kind), "path": d.category}))
        .collect();
    match serde_json::to_string_pretty(&items) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("effect_catalog: {e}");
            ExitCode::FAILURE
        }
    }
}
