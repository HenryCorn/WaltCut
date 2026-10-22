//! Sequence / Markers menu long tail (M3.11).
//!
//! | Id | Menu | Notes |
//! |---|---|---|
//! | `sequence.applyDefaultTransitionsToSelection` | Sequence ▸ Apply Default Transitions to Selection (⇧D) | selected clips' edges; selected edit points |
//! | `sequence.normalizeMixTrack` | Sequence ▸ Normalize Mix Track… | Mix fader so the mix peaks at the target |
//! | `sequence.simplify` | Sequence ▸ Simplify Sequence… | a simplified copy of the sequence |
//! | `markers.addFlashCue` | Markers ▸ Add Flash Cue Marker… | `MarkerKind::FlashCue` |
//!
//! **Simplify Sequence** (Premiere 26): makes a copy named "<name> (Simplified)" and opens it;
//! the original is untouched. Options: remove disabled clips, remove empty tracks, close gaps (spans
//! where no track has a clip), move video clips down to the lowest free track, remove video / audio
//! effects (standard effects; Motion, Opacity, Volume… are kept), remove text (graphic clips), keep only video or only audio.

use serde_json::{Value, json};
use waltcut_edit as edit;
use waltcut_project::{ClipId, ItemKind, Label, Marker, MarkerId, MarkerKind, Sequence, TrackId, TrackKind};
use waltcut_time::{Tick, TimeRange};

use crate::commands::{CommandSpec, bad, bool_p, f64_p, has_seq, marker_color_p, sequence_time_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(
    id: &'static str,
    label: &'static str,
    menu: &'static [&'static str],
    shortcut: Option<&'static str>,
    params: &'static str,
    enabled: Enabled,
    run: Run,
) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut, params, enabled, run, journal: true }
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        spec(
            "sequence.applyDefaultTransitionsToSelection",
            "Apply Default Transitions to Selection",
            &["Sequence"],
            // ⇧D comes from the shortcut audit (`shortcut_presets::EXTRA_KEYS`)
            None,
            r#"{"clips":[id]?}"#,
            has_clips_or_edit_points,
            apply_default_transitions,
        ),
        spec("sequence.normalizeMixTrack", "Normalize Mix Track…", &["Sequence"], None, r#"{"db":f64=0}"#, has_audio_clips, normalize_mix),
        spec(
            "sequence.simplify",
            "Simplify Sequence…",
            &["Sequence"],
            None,
            r#"{"name":str?,"removeDisabled":bool=true,"removeEmptyTracks":bool=true,"closeGaps":bool=false,"moveClipsDown":bool=false,"removeAudioEffects":bool=false,"removeText":bool=false,"keep":"both|video|audio"}"#,
            has_seq,
            simplify,
        ),
        spec(
            "markers.addFlashCue",
            "Add Flash Cue Marker…",
            &["Markers"],
            None,
            r#"{"time":ticks?,"name":str?,"comment":str?,"color":label?}"#,
            has_seq,
            add_flash_cue,
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// enablement
// ---------------------------------------------------------------------------------------------

fn has_clips_or_edit_points(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    let q = s.active_sequence().ok_or_else(|| "no active sequence".to_string())?;
    if q.audio_tracks
        .iter()
        .any(|t| !t.locked && t.items.iter().any(|i| s.state.selection.contains(&i.id) || s.state.edit_points.iter().any(|p| p.clip == i.id)))
    {
        Ok(())
    } else {
        Err("select audio clips or audio edit points".into())
    }
}

fn has_audio_clips(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    let q = s.active_sequence().ok_or("no sequence is open")?;
    if q.audio_tracks.iter().any(|t| !t.items.is_empty()) { Ok(()) } else { Err("the sequence has no audio clips".into()) }
}

// ---------------------------------------------------------------------------------------------
// Apply Default Transitions to Selection
// ---------------------------------------------------------------------------------------------

/// Premiere applies the default audio transition at each edge of the selected clips
/// (centred on cuts between two clips, starting / ending at a clip's free edge). Edges that
/// already have a transition keep it. With edit points selected (trim mode) and no clips, the
/// transitions go at the edit points.
fn apply_default_transitions(s: &mut Session, p: &Value) -> Result<Value> {
    let clips: Vec<ClipId> = match p.get("clips").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(|v| v.as_u64().map(ClipId)).collect(),
        None => s.state.selection.clone(),
    };
    if clips.is_empty() {
        return crate::trim::apply_default_transitions(s);
    }
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let edges = clip_edges(q, &clips);
    apply_at_edges(s, edges, &Value::Null, "Apply Default Transitions")
}

/// Apply Audio Transition (⇧⌘D) with clips or edit points of `kind` selected: the
/// transition goes at the selected clips' edges (as Apply Default Transitions to Selection), or at
/// the selected edit points, instead of at the playhead (#1124). `None` when nothing of `kind` is
/// selected. `p`'s settings (`effect`, `frames`, `params`, `reverse`) apply to every edge.
pub(crate) fn apply_to_selection(s: &mut Session, p: &Value, kind: TrackKind) -> Result<Option<Value>> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let on_kind = |c: ClipId| q.find_item(c).and_then(|(t, _)| q.track(t)).is_some_and(|t| t.kind == kind);
    let clips: Vec<ClipId> = s.state.selection.iter().copied().filter(|c| on_kind(*c)).collect();
    let edges = if clips.is_empty() {
        s.state.edit_points.iter().filter(|e| on_kind(e.clip)).map(|e| (kind, e.clip, if e.out { "out" } else { "in" })).collect()
    } else {
        clip_edges(q, &clips)
    };
    // an explicit `edge` keeps only that side of each selected clip
    let edges: Vec<_> = match p.get("edge").and_then(Value::as_str) {
        Some(only) => edges.into_iter().filter(|e| e.2 == only).collect(),
        None => edges,
    };
    if clips.is_empty() && edges.is_empty() {
        return Ok(None);
    }
    let label = "Apply Audio Transition";
    apply_at_edges(s, edges, p, label).map(Some)
}

/// The In and Out edges of `clips`, one per cut; edges that already have a transition keep it.
fn clip_edges(q: &Sequence, clips: &[ClipId]) -> Vec<(TrackKind, ClipId, &'static str)> {
    // (track, clip, edge) per distinct cut
    let mut edges: Vec<(TrackKind, ClipId, &'static str, Tick, TrackId)> = Vec::new();
    for c in clips {
        let Some((tid, it)) = q.find_item(*c) else { continue };
        let Some(tr) = q.track(tid) else { continue };
        if tr.kind != TrackKind::Audio {
            continue;
        }
        for (edge, at) in [("in", it.start), ("out", it.end())] {
            if edges.iter().any(|e| e.4 == tid && e.3 == at) {
                continue;
            }
            // an existing transition at this cut stays
            if tr.transitions.iter().any(|x| x.start <= at && x.end() >= at && (x.from == Some(*c) || x.to == Some(*c))) {
                continue;
            }
            edges.push((tr.kind, *c, edge, at, tid));
        }
    }
    edges.into_iter().map(|(kind, clip, edge, _, _)| (kind, clip, edge)).collect()
}

/// Apply a transition at each (track kind, clip, edge) with `settings`, as one undo step `label`.
fn apply_at_edges(s: &mut Session, edges: Vec<(TrackKind, ClipId, &'static str)>, settings: &Value, label: &str) -> Result<Value> {
    let n0 = s.history.undo.len();
    let mut ids = Vec::new();
    let mut errors = Vec::new();
    for (kind, clip, edge) in edges {
        if kind != TrackKind::Audio {
            continue;
        }
        let cmd = "sequence.applyAudioTransition";
        let mut args = settings.as_object().cloned().unwrap_or_default();
        args.insert("clip".into(), json!(clip.0));
        args.insert("edge".into(), json!(edge));
        match s.execute(cmd, Value::Object(args)) {
            Ok(r) => ids.push(r["transition"].clone()),
            Err(e) => errors.push(e.to_string()),
        }
    }
    crate::clip_ops::collapse_history(s, n0, label);
    if ids.is_empty() {
        return Err(EngineError::Other(errors.into_iter().next().unwrap_or_else(|| "no transition could be applied to the selection".into())));
    }
    Ok(json!({"transitions": ids}))
}

// ---------------------------------------------------------------------------------------------
// Normalize Mix Track
// ---------------------------------------------------------------------------------------------

/// Peak (dBFS) of the active sequence's mix over its whole duration, rendered through the mixer
/// graph exactly like playback and export.
pub fn mix_peak_db(s: &Session) -> Result<f64> {
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let q = s.project.sequence(seq_id).ok_or(EngineError::NoSequence)?;
    let sr = q.settings.sample_rate.max(1);
    let total = q.duration().to_units_floor(sr as i64);
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let mut peak = 0f32;
    let mut pos = 0i64;
    while pos < total {
        let n = (total - pos).min(sr as i64) as usize;
        let b = waltcut_render::audio::mix_sequence(&s.project, q, pos, n, &provider);
        peak = b.peaks().into_iter().fold(peak, f32::max);
        pos += n as i64;
    }
    Ok(if peak > 0.0 { 20.0 * (peak as f64).log10() } else { f64::NEG_INFINITY })
}

/// Sequence ▸ Normalize Mix Track…: set the Mix fader so the mix peaks at `db` (default 0 dBFS).
/// The mix is measured, the fader moved by the difference, and measured again (post-fader
/// effects such as a limiter are not linear), up to three times. One undo step.
fn normalize_mix(s: &mut Session, p: &Value) -> Result<Value> {
    let target = f64_p(p, "db").unwrap_or(0.0);
    if !(-96.0..=24.0).contains(&target) {
        return Err(bad("sequence.normalizeMixTrack", "the target must be between -96 and +24 dB"));
    }
    let before = s.active_sequence().map(|q| q.master_volume_db).unwrap_or(0.0);
    let first = mix_peak_db(s)?;
    if !first.is_finite() {
        return Err(EngineError::Other("the mix is silent: nothing to normalize".into()));
    }
    let n0 = s.history.undo.len();
    let mut peak = first;
    for _ in 0..3 {
        let delta = target - peak;
        if delta.abs() < 0.01 {
            break;
        }
        s.edit_sequence("Normalize Mix Track", |q, _, _| {
            q.master_volume_db = (q.master_volume_db + delta).clamp(-96.0, 48.0);
            Ok(())
        })?;
        peak = mix_peak_db(s)?;
    }
    crate::clip_ops::collapse_history(s, n0, "Normalize Mix Track");
    let after = s.active_sequence().map(|q| q.master_volume_db).unwrap_or(0.0);
    Ok(json!({"peakBeforeDb": first, "peakDb": peak, "gainDb": after - before, "masterVolumeDb": after}))
}

// ---------------------------------------------------------------------------------------------
// Simplify Sequence
// ---------------------------------------------------------------------------------------------

/// What Simplify Sequence does to the copy.
#[derive(Clone, Debug)]
pub struct SimplifyOptions {
    pub remove_disabled: bool,
    pub remove_empty_tracks: bool,
    pub close_gaps: bool,
    pub move_clips_down: bool,
    pub remove_audio_effects: bool,
    pub remove_text: bool,
    /// `both`, `video` or `audio`.
    pub keep: String,
}

impl SimplifyOptions {
    pub fn from_params(p: &Value) -> Result<Self> {
        let keep = str_p(p, "keep").unwrap_or("both").to_ascii_lowercase();
        if !matches!(keep.as_str(), "both" | "video" | "audio") {
            return Err(bad("sequence.simplify", "`keep` is both, video or audio"));
        }
        Ok(SimplifyOptions {
            remove_disabled: bool_p(p, "removeDisabled").unwrap_or(true),
            remove_empty_tracks: bool_p(p, "removeEmptyTracks").unwrap_or(true),
            close_gaps: bool_p(p, "closeGaps").unwrap_or(false),
            move_clips_down: bool_p(p, "moveClipsDown").unwrap_or(false),
            remove_audio_effects: bool_p(p, "removeAudioEffects").unwrap_or(false),
            remove_text: bool_p(p, "removeText").unwrap_or(false),
            keep,
        })
    }
}

fn is_text(p: &waltcut_project::Project, it: &waltcut_project::TrackItem) -> bool {
    p.item(it.item).is_some_and(|x| matches!(x.kind, ItemKind::Graphic { .. }))
}

/// Apply Simplify Sequence to `q` (a copy). Returns what changed.
pub fn simplify_sequence(proj: &waltcut_project::Project, q: &mut Sequence, o: &SimplifyOptions) -> Value {
    let mut removed_clips = 0usize;
    // keep only video / audio
    if o.keep == "video" {
        q.audio_tracks.clear();
    } else if o.keep == "audio" {
        q.video_tracks.clear();
    }
    let mut drop: Vec<ClipId> = Vec::new();
    for t in q.video_tracks.iter().chain(q.audio_tracks.iter()) {
        for it in &t.items {
            if (o.remove_disabled && !it.enabled) || (o.remove_text && is_text(proj, it)) {
                drop.push(it.id);
            }
        }
    }
    removed_clips += edit::delete_items(q, &drop);
    // linked partners removed with "keep" lose their link
    let mut links: std::collections::BTreeMap<u64, usize> = Default::default();
    for t in q.all_tracks() {
        for it in &t.items {
            if let Some(l) = it.link {
                *links.entry(l).or_default() += 1;
            }
        }
    }
    for t in q.video_tracks.iter_mut().chain(q.audio_tracks.iter_mut()) {
        for it in &mut t.items {
            if it.link.is_some_and(|l| links.get(&l).copied().unwrap_or(0) < 2) {
                it.link = None;
            }
        }
        edit::remove_orphan_transitions(t);
    }
    // Audio effects only; intrinsic parameters and Essential Sound stay.
    let mut removed_effects = 0usize;
    if o.remove_audio_effects {
        for t in &mut q.audio_tracks {
            for it in &mut t.items {
                let n = it.effects.len();
                it.effects.retain(|e| e.def().is_none_or(|d| d.intrinsic || waltcut_project::graphic::is_layer_id(d.id)));
                removed_effects += n - it.effects.len();
            }
            let n = t.effects.len();
            t.effects.clear();
            removed_effects += n;
        }
    }
    // Move video clips down to the lowest track where they fit.
    let mut moved = 0usize;
    if o.move_clips_down {
        for ti in 1..q.video_tracks.len() {
            let ids: Vec<ClipId> = q.video_tracks[ti].items.iter().map(|i| i.id).collect();
            for id in ids {
                let Some(pos) = q.video_tracks[ti].items.iter().position(|i| i.id == id) else { continue };
                let it = q.video_tracks[ti].items[pos].clone();
                let Some(dest) = (0..ti).find(|&d| !q.video_tracks[d].locked && edit::track_range_empty(&q.video_tracks[d], it.range())) else { continue };
                q.video_tracks[ti].items.remove(pos);
                q.video_tracks[dest].items.push(it);
                q.video_tracks[dest].sort();
                moved += 1;
            }
        }
    }
    // close gaps: spans where no track has anything
    let mut closed = Tick::ZERO;
    if o.close_gaps {
        let mut spans: Vec<TimeRange> = q.all_tracks().flat_map(|t| t.items.iter().map(|i| i.range())).collect();
        spans.sort_by_key(|r| r.start);
        let mut gaps: Vec<TimeRange> = Vec::new();
        let mut cursor = Tick::ZERO;
        for r in &spans {
            if r.start > cursor {
                gaps.push(TimeRange::from_bounds(cursor, r.start));
            }
            cursor = cursor.max(r.end());
        }
        for g in gaps.iter().rev() {
            for t in q.video_tracks.iter_mut().chain(q.audio_tracks.iter_mut()) {
                edit::shift_track_from(t, g.end(), Tick::ZERO - g.duration);
            }
            crate::sequence_tools::ripple_markers(&mut q.markers, g.end(), Tick::ZERO - g.duration);
            closed += g.duration;
        }
    }
    // empty tracks (one video and one audio track always stay unless "keep" dropped the kind)
    let mut removed_tracks = 0usize;
    if o.remove_empty_tracks {
        for tracks in [&mut q.video_tracks, &mut q.audio_tracks] {
            // all empty: the first track stays so the sequence remains editable
            let all_empty = tracks.iter().all(|t| t.items.is_empty());
            let n = tracks.len();
            let mut first = true;
            tracks.retain(|t| {
                let keep = !t.items.is_empty() || (all_empty && first);
                first = false;
                keep
            });
            removed_tracks += n - tracks.len();
        }
    }
    json!({"removedClips": removed_clips, "removedEffects": removed_effects, "movedClips": moved, "closedGaps": closed.0, "removedTracks": removed_tracks})
}

fn simplify(s: &mut Session, p: &Value) -> Result<Value> {
    let o = SimplifyOptions::from_params(p)?;
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let src = s.project.item(seq_id).cloned().ok_or(EngineError::NoSequence)?;
    let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| format!("{} (Simplified)", src.name));
    let (id, report) = s.edit("Simplify Sequence", |pr, st| {
        let ItemKind::Sequence(q) = &src.kind else { return Err(EngineError::NoSequence) };
        let mut q = (**q).clone();
        let report = simplify_sequence(pr, &mut q, &o);
        q.check().map_err(EngineError::Other)?;
        let bin = pr.root.parent_of(seq_id).filter(|b| b.0 != 0);
        let id = pr.add_item(&name, src.label, ItemKind::Sequence(std::sync::Arc::new(q)), bin);
        st.active_sequence = Some(id);
        if !st.open_sequences.contains(&id) {
            st.open_sequences.push(id);
        }
        st.selection.clear();
        st.project_selection = vec![id];
        Ok((id, report))
    })?;
    s.events.push(crate::Event::OpenSequence(id));
    let mut out = report;
    out["sequence"] = json!(id.0);
    out["name"] = json!(name);
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Markers ▸ Add Flash Cue Marker…
// ---------------------------------------------------------------------------------------------

fn add_flash_cue(s: &mut Session, p: &Value) -> Result<Value> {
    let t = s.sequence_rate().snap(sequence_time_p(s, p, "", "markers.addFlashCue")?.unwrap_or(s.playhead()));
    let n = s.active_sequence().map(|q| q.markers.iter().filter(|m| m.kind == MarkerKind::FlashCue).count()).unwrap_or(0) + 1;
    let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| format!("Cue Point {n}"));
    let comment = str_p(p, "comment").unwrap_or("").to_string();
    let color = marker_color_p(p, "markers.addFlashCue")?.unwrap_or(Label::Green);
    let dur = u64_p(p, "durationFrames").map(|f| s.sequence_rate().tick_of(f as i64)).unwrap_or(Tick::ZERO);
    let id = s.edit_sequence("Add Flash Cue Marker", |q, ctx, _| {
        let id = MarkerId(ctx.alloc());
        q.markers.push(Marker { id, start: t, duration: dur, name, comment, kind: MarkerKind::FlashCue, color });
        q.markers.sort_by_key(|m| m.start);
        Ok(id)
    })?;
    Ok(json!({"marker": id.0}))
}

#[cfg(test)]
#[path = "sequence_extras_tests.rs"]
mod tests;
