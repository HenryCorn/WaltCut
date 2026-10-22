use super::*;
use serde_json::json;

#[test]
fn removed_audio_catalogue_commands_and_parameters_are_rejected_atomically() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let project = s.project.clone();
    let prefs = s.prefs.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    let redo = s.history.redo.len();
    let jobs = s.jobs.len();
    for id in [
        "effects.apply",
        "sequence.applyAudioTransition",
        "sequence.setTransition",
        "sequence.removeTransition",
        "sequence.applyDefaultTransitionsToSelection",
        "trim.applyDefaultTransition",
        "effects.setDefaultTransition",
    ] {
        assert!(crate::commands::find(id).is_none(), "{id}");
        assert!(s.shortcuts.primary(id).is_none(), "{id}");
        assert!(!s.is_enabled(id), "{id}");
        for params in [json!({}), json!({"effect": "constant_power", "frames": i64::MIN, "transition": u64::MAX}), json!(null), json!([])] {
            assert!(matches!(s.execute(id, params), Err(EngineError::UnknownCommand(ref missing)) if missing == id));
        }
    }
    let clip = s.active_sequence().unwrap().audio_tracks[0].items[0].id;
    for effect in ["parametric_eq", "channel_volume", "panner", "hard_limiter", "constant_gain"] {
        assert!(s.execute("effects.setParam", json!({"clip": clip.0, "effect": effect, "param": "level", "value": 9})).is_err());
    }
    assert!(s.execute("timeline.select", json!({"transitions": [1]})).is_err());
    for key in ["timeline.audioTransitionDuration", "timeline.audioTransitionUnit"] {
        assert!(s.prefs.get(key).is_none(), "{key}");
        assert!(s.execute("prefs.set", json!({"values": {key: 1}})).is_err(), "{key}");
    }
    for (command, params) in [
        ("sequence.settings", json!({"mix": "5.1"})),
        ("file.newSequence", json!({"mix": "Mono"})),
        ("file.newSequence", json!({"trackType": "5.1"})),
        ("sequence.simplify", json!({"removeAudioEffects": true})),
        ("clip.automateToSequence", json!({"audioTransition": true})),
        ("edit.removeAttributes", json!({"channelVolume": true})),
        ("edit.removeAttributes", json!({"panner": true})),
        ("timeline.setTrack", json!({"track": "A1", "pan": -50.0})),
        ("timeline.setTrack", json!({"track": "A1", "volumeDb": -6.0})),
        ("timeline.setTrack", json!({"track": "A1", "solo": true})),
        ("effects.list", json!({"folder": "Audio Effects"})),
    ] {
        assert!(s.execute(command, params).is_err(), "{command}");
    }
    let audio = s.execute("effects.list", json!({"kind": "Audio"})).unwrap();
    assert_eq!(audio.as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect::<Vec<_>>(), ["volume"]);
    assert!(s.execute("effects.list", json!({"kind": "AudioTransition"})).unwrap().as_array().unwrap().is_empty());
    assert!(Arc::ptr_eq(&s.project, &project));
    assert_eq!(s.prefs, prefs);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!((s.history.undo.len(), s.history.redo.len(), s.jobs.len()), (undo, redo, jobs));
}

#[test]
fn clip_channel_mapping_rejects_multichannel_and_hostile_numbers_without_mutation() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let clip = s.active_sequence().unwrap().audio_tracks[0].items[0].id;
    let item = s.active_sequence().unwrap().audio_tracks[0].items[0].item;
    s.state.project_selection.clear();
    s.state.selection = vec![clip];
    let project = s.project.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let history = (s.history.undo.len(), s.history.redo.len());
    for params in [
        json!({"channels": [0, 1, 2]}),
        json!({"channels": [-1]}),
        json!({"channels": [65536]}),
        json!({"channels": [u64::MAX]}),
        json!({"channels": [0.5]}),
        json!({"channels": [2]}),
        json!({"channels": "stereo"}),
        json!({"channels": [null]}),
        json!({"clips": "broken", "channels": [0]}),
        json!({"items": [item.0], "format": "5.1"}),
        json!({"items": [item.0], "format": "adaptive"}),
        json!({"items": [item.0], "format": "stereo", "clips": [[0, 1, 0]]}),
        json!({"items": [item.0], "format": "stereo", "clips": [[65536]]}),
        json!({"items": [item.0], "format": "mono", "clips": [[0, 1]]}),
    ] {
        assert!(s.execute("clip.audioChannels", params.clone()).is_err(), "{params}");
        assert!(Arc::ptr_eq(&s.project, &project), "{params}");
        assert_eq!(serde_json::to_value(&s.state).unwrap(), state, "{params}");
        assert_eq!((s.history.undo.len(), s.history.redo.len()), history, "{params}");
    }
}

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
fn removed_surround_layout_preferences_are_absent_and_reject_writes() {
    let mut prefs = autosave::Preferences::default();
    let before = prefs.clone();
    let serialized = serde_json::to_value(&prefs).unwrap();
    assert!(serialized["audio"].get("mixdownType").is_none());
    assert!(serialized["audio"].get("sumToMonoInSource").is_none());
    assert!(serialized["timeline"].get("surroundMediaTracks").is_none());
    for key in ["audio.mixdownType", "audio.sumToMonoInSource", "timeline.surroundMediaTracks"] {
        assert!(prefs.get(key).is_none(), "{key}");
        for value in [json!(null), json!("frontRear"), json!({"channels": [0, 1, 2]})] {
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
    for id in ["mixer.open", "mixer.addSubmix", "mixer.addSend", "mixer.addInsert", "mixer.setStrip", "mixer.removeInsert", "timeline.toggleSoloTargetedAudio"]
    {
        assert!(crate::commands::find(id).is_none(), "{id}");
        assert!(matches!(s.execute(id, json!({"strip": "A1", "value": f64::NAN})), Err(EngineError::UnknownCommand(ref missing)) if missing == id));
        assert!(Arc::ptr_eq(&s.project, &project), "{id} changed the project");
    }
}
