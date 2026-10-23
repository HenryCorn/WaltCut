use super::*;
use serde_json::json;

#[test]
fn removed_essential_sound_commands_cannot_mutate_the_project() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let project = s.project.clone();
    let prefs = s.prefs.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    let redo = s.history.redo.len();
    let jobs = s.jobs.len();
    for id in [
        "essentialSound.inspect",
        "essentialSound.setType",
        "essentialSound.clearType",
        "essentialSound.set",
        "essentialSound.applyPreset",
        "essentialSound.savePreset",
        "essentialSound.deletePreset",
        "essentialSound.autoMatch",
        "essentialSound.generateDucking",
    ] {
        assert!(crate::commands::find(id).is_none(), "{id}");
        assert!(!s.is_enabled(id), "{id}");
        for params in [
            json!({}),
            json!({"clips": [u64::MAX, -1, null], "type": "dialogue", "target": -1e300,
                "values": {"mute": true, "volume.levelDb": 1e300, "ducking.fadeS": -1},
                "name": "..", "preset": "(Default)", "begin": true}),
            json!({"clip": -1, "key": "repair.noise.amount", "value": {"unexpected": []}}),
            json!(null),
            json!([]),
        ] {
            assert!(matches!(s.execute(id, params), Err(EngineError::UnknownCommand(ref missing)) if missing == id), "{id}");
            assert!(Arc::ptr_eq(&s.project, &project));
            assert_eq!(s.prefs, prefs);
            assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
            assert_eq!(s.history.undo.len(), undo);
            assert_eq!(s.history.redo.len(), redo);
            assert_eq!(s.jobs.len(), jobs);
        }
    }
    assert!(!crate::commands::command_specs().iter().any(|c| c.id.starts_with("essentialSound.")));
}

#[test]
fn essential_sound_preferences_are_absent_and_reject_writes() {
    let mut prefs = autosave::Preferences::default();
    let before = prefs.clone();
    assert!(prefs.get("essentialSound").is_none());
    for key in [
        "essentialSound.userPresets",
        "audio.autoTagAudioTypes",
        "audio.alwaysOverrideAudioTags",
        "audio.dialogueTargetLufs",
        "audio.musicTargetLufs",
        "audio.sfxTargetLufs",
        "audio.ambienceTargetLufs",
    ] {
        assert!(prefs.get(key).is_none(), "{key}");
        for value in [json!(null), json!(-1e300), json!({"settings": []})] {
            assert!(prefs.set(key, value).is_err(), "{key}");
            assert_eq!(prefs, before);
        }
    }
}

#[test]
fn removed_capture_commands_cannot_mutate_the_project() {
    let mut s = Session::default();
    let project = s.project.clone();
    let prefs = serde_json::to_value(&s.prefs).unwrap();
    for id in ["audio.voiceover.settings", "audio.voiceover.start", "audio.voiceover.sync", "audio.voiceover.stop"] {
        assert!(crate::commands::find(id).is_none(), "{id}");
        for params in [json!({}), json!({"track": -1, "time": i64::MAX, "preroll": -1, "dir": "..", "inputChannel": u64::MAX})] {
            assert!(s.execute(id, params).is_err(), "{id}");
            assert_eq!(s.project, project);
            assert_eq!(serde_json::to_value(&s.prefs).unwrap(), prefs);
        }
    }
    assert!(prefs.get("voiceOver").is_none());
    let strip = serde_json::to_value(waltcut_project::MixerStrip::default()).unwrap();
    assert!(strip.get("recordArm").is_none());
}
