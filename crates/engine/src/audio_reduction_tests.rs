use super::*;
use serde_json::json;

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
