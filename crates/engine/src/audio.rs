//! Minimal retained clip-level audio gain command.

use serde_json::{Value, json};

use crate::commands::{bad, bool_p, f64_p, str_p, with_links};
use crate::{Result, Session};
use waltcut_project::ClipId;

pub fn channels_from(value: &str) -> Option<waltcut_project::AudioChannels> {
    match value.trim().to_ascii_lowercase().as_str() {
        "mono" => Some(waltcut_project::AudioChannels::Mono),
        "stereo" | "standard" => Some(waltcut_project::AudioChannels::Stereo),
        _ => None,
    }
}

pub fn set_clip_gain(session: &mut Session, params: &Value) -> Result<Value> {
    let mode = str_p(params, "mode")
        .or_else(|| params.get("relative").and_then(Value::as_bool).map(|relative| if relative { "adjust" } else { "set" }))
        .unwrap_or("set");
    if !matches!(mode, "set" | "adjust") {
        return Err(bad("clip.audioGain", format!("unsupported mode `{mode}` (set or adjust)")));
    }
    let Some(db) = f64_p(params, "db") else {
        return Err(bad("clip.audioGain", "need a finite `db` value"));
    };
    if !db.is_finite() {
        return Err(bad("clip.audioGain", "`db` must be finite"));
    }
    let requested: Vec<ClipId> = match params.get("clips").and_then(Value::as_array) {
        Some(clips) => clips.iter().filter_map(|value| value.as_u64().map(ClipId)).collect(),
        None => session.state.selection.clone(),
    };
    let mut targets = with_links(session, &requested);
    targets.sort_by_key(|clip| clip.0);
    targets.dedup();
    let mut changed = Vec::new();
    if bool_p(params, "begin").unwrap_or(false) {
        session.history.merge_key = None;
    }
    let merge =
        bool_p(params, "merge").unwrap_or(false).then(|| format!("audioGain:{}", targets.iter().map(|clip| clip.0.to_string()).collect::<Vec<_>>().join(",")));
    session.edit_sequence_as("Audio Gain", merge.as_deref(), |seq, _, _| {
        for track in &mut seq.audio_tracks {
            for clip in &mut track.items {
                if targets.contains(&clip.id) {
                    let gain = if mode == "adjust" { clip.gain_db + db } else { db };
                    if !gain.is_finite() {
                        return Err(crate::EngineError::Other("resulting clip gain is not finite".into()));
                    }

                    clip.gain_db = gain.clamp(-96.0, 96.0);
                    changed.push((clip.id, clip.gain_db));
                }
            }
        }
        Ok(())
    })?;
    Ok(json!({
        "mode": mode,
        "clips": changed.len(),
        "gainDb": changed.iter().map(|(clip, gain)| json!({"clip": clip.0, "gainDb": gain})).collect::<Vec<_>>()
    }))
}
