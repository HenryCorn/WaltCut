use super::*;
use serde_json::json;

#[test]
fn removed_video_catalogue_commands_and_ids_are_rejected() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    s.state.selection = vec![ClipId(clip)];
    let before = s.project.clone();
    assert!(crate::commands::find("sequence.applyVideoTransition").is_none());
    assert!(s.shortcuts.primary("sequence.applyVideoTransition").is_none());
    assert!(!s.is_enabled("trim.applyDefaultTransition"), "video-only selection has no default transition");
    assert!(s.execute("sequence.applyVideoTransition", json!({})).is_err());
    for id in ["gaussian_blur", "crop", "ultra_key", "warp_stabilizer", "cross_dissolve", "push", "wipe", "time_remap"] {
        assert!(waltcut_project::find_effect(id).is_none(), "{id}");
        assert!(s.execute("effects.apply", json!({"effect": id, "clips": [clip]})).is_err(), "{id}");
        assert!(s.execute("effects.setParam", json!({"clip": clip, "effect": id, "param": "opacity", "value": 10})).is_err(), "{id}");
        assert!(s.execute("effects.addKeyframe", json!({"clip": clip, "effect": id, "param": "opacity"})).is_err(), "{id}");
        assert!(s.execute("effects.setDefaultTransition", json!({"effect": id})).is_err(), "{id}");
    }
    assert_eq!(s.project, before);
    let listing = s.execute("effects.list", json!({})).unwrap();
    assert_eq!(
        listing.as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect::<Vec<_>>(),
        ["motion", "opacity", "volume", "graphic_text", "graphic_shape"]
    );
    assert!(s.execute("effects.list", json!({"kind": "VideoTransition"})).unwrap().as_array().unwrap().is_empty());
}

#[test]
fn removed_effect_preset_imports_fail_without_rewriting_the_source_or_library() {
    let mut s = Session::default();
    let before = serde_json::to_value(s.presets.all()).unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("fixtures")
        .join(format!("removed-video-presets-{}.json", std::process::id()));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let preset = crate::presets::builtin_presets().into_iter().find(|p| p.name == "Fade In").unwrap();
    for removed in
        ["gaussian_blur", "tint", "cross_dissolve", "parametric_eq", "hard_limiter", "channel_volume", "panner", "constant_power", "exponential_fade"]
    {
        let mut source = json!({"format": "filmcraft.effect-presets", "version": 1, "presets": [preset]});
        source["presets"][0]["effects"][0]["effect"] = json!(removed);
        let bytes = serde_json::to_vec(&source).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let error = s.execute("presets.import", json!({"path": path.to_string_lossy()})).unwrap_err();
        assert!(error.to_string().contains("unsupported effect"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(serde_json::to_value(s.presets.all()).unwrap(), before);
    }
    std::fs::remove_file(path).unwrap();
}
