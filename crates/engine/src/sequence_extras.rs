//! Sequence / Markers menu long tail (M3.11).
//!
//! | Id | Menu | Notes |
//! |---|---|---|
//! | `sequence.simplify` | Sequence ▸ Simplify Sequence… | a simplified copy of the sequence |
//! | `markers.addFlashCue` | Markers ▸ Add Flash Cue Marker… | `MarkerKind::FlashCue` |
//!
//! **Simplify Sequence** (Premiere 26): makes a copy named "<name> (Simplified)" and opens it;
//! the original is untouched. Options: remove disabled clips, remove empty tracks, close gaps (spans
//! where no track has a clip), move video clips down to the lowest free track, remove video / audio
//! effects (standard effects; Motion, Opacity, Volume… are kept), remove text (graphic clips), keep only video or only audio.

use serde_json::{Value, json};
use waltcut_edit as edit;
use waltcut_project::{ClipId, ItemKind, Label, Marker, MarkerId, MarkerKind, Sequence};
use waltcut_time::{Tick, TimeRange};

use crate::commands::{CommandSpec, bad, bool_p, has_seq, marker_color_p, sequence_time_p, str_p, u64_p};
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
            "sequence.simplify",
            "Simplify Sequence…",
            &["Sequence"],
            None,
            r#"{"name":str?,"removeDisabled":bool=true,"removeEmptyTracks":bool=true,"closeGaps":bool=false,"moveClipsDown":bool=false,"removeText":bool=false,"keep":"both|video|audio"}"#,
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
// Simplify Sequence
// ---------------------------------------------------------------------------------------------

/// What Simplify Sequence does to the copy.
#[derive(Clone, Debug)]
pub struct SimplifyOptions {
    pub remove_disabled: bool,
    pub remove_empty_tracks: bool,
    pub close_gaps: bool,
    pub move_clips_down: bool,
    pub remove_text: bool,
    /// `both`, `video` or `audio`.
    pub keep: String,
}

impl SimplifyOptions {
    pub fn from_params(p: &Value) -> Result<Self> {
        crate::commands::check_parameter_keys(
            p,
            "sequence.simplify",
            &["name", "removeDisabled", "removeEmptyTracks", "closeGaps", "moveClipsDown", "removeText", "keep"],
        )?;
        let keep = str_p(p, "keep").unwrap_or("both").to_ascii_lowercase();
        if !matches!(keep.as_str(), "both" | "video" | "audio") {
            return Err(bad("sequence.simplify", "`keep` is both, video or audio"));
        }
        Ok(SimplifyOptions {
            remove_disabled: bool_p(p, "removeDisabled").unwrap_or(true),
            remove_empty_tracks: bool_p(p, "removeEmptyTracks").unwrap_or(true),
            close_gaps: bool_p(p, "closeGaps").unwrap_or(false),
            move_clips_down: bool_p(p, "moveClipsDown").unwrap_or(false),
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
    json!({"removedClips": removed_clips, "movedClips": moved, "closedGaps": closed.0, "removedTracks": removed_tracks})
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
