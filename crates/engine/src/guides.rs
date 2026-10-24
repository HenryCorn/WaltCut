//! Guide clips (#65): a reference or tracing clip that shows in the Program preview but is left
//! out of every delivery render unless the export asks for guides.
//!
//! * `clip.setGuide {clips?, guide?}` marks the clips (default: the selection) as guides, or as
//!   ordinary clips with `guide: false`; with no `guide` it flips them (all guides → ordinary,
//!   else → guides). One undo step. A guide is a *video* clip on an unlocked track: audio clips,
//!   locked tracks and missing clips are refused and nothing changes.
//! * Delivery renders, which leave guides out by default: `file.exportFrame`, `file.exportMedia`
//!   and `export.quick` (still, PNG sequence, movie) and the frames `ai.restyle` sends. Each takes
//!   `includeGuides: true` to draw them. The Program monitor, the render cache previews and
//!   `render_frame` (all for looking) always show them.
//! * Guide status is a separate flag from disabled, muted and locked: a disabled clip is drawn
//!   nowhere, a guide is drawn in the preview only.

use serde_json::{Value, json};
use waltcut_project::{ClipId, TrackKind};

use crate::commands::{CommandSpec, bad, clips_p, has_selection};
use crate::{EngineError, Result, Session};

const ID: &str = "clip.setGuide";
/// Most clips one call changes.
const MAX_CLIPS: usize = 4096;

fn set_guide(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(o) = p.as_object()
        && let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "clips" | "clip" | "guide"))
    {
        return Err(bad(ID, format!("unknown parameter `{k}`; expected: clips, guide")));
    }
    let want = match p.get("guide") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(bad(ID, "`guide` must be true or false")),
    };
    if let Some(v) = p.get("clips")
        && !v.as_array().is_some_and(|a| a.iter().all(|c| c.as_u64().is_some()))
    {
        return Err(bad(ID, "`clips` must be a list of clip ids"));
    }
    let mut clips = clips_p(s, p);
    clips.sort_by_key(|c| c.0);
    clips.dedup();
    if clips.is_empty() {
        return Err(bad(ID, "select a video clip"));
    }
    if clips.len() > MAX_CLIPS {
        return Err(bad(ID, format!("at most {MAX_CLIPS} clips at once")));
    }
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let mut all_guides = true;
    for c in &clips {
        let (tid, it) = seq.find_item(*c).ok_or_else(|| bad(ID, format!("no clip {}", c.0)))?;
        let tr = seq.track(tid).ok_or(EngineError::NoSequence)?;
        if tr.kind != TrackKind::Video {
            return Err(bad(ID, format!("clip {} is an audio clip: only video clips can be guides", c.0)));
        }
        if tr.locked {
            return Err(bad(ID, format!("clip {} is on a locked track", c.0)));
        }
        all_guides &= it.guide;
    }
    let guide = want.unwrap_or(!all_guides);
    let label = if guide { "Use as Guide" } else { "Use as Normal Clip" };
    let changed = s.edit_sequence(label, |q, _, _| {
        let mut n = 0usize;
        for c in &clips {
            if let Some((_, it)) = q.find_item_mut(*c)
                && it.guide != guide
            {
                it.guide = guide;
                n += 1;
            }
        }
        Ok(n)
    })?;
    Ok(json!({"clips": clips.iter().map(|c: &ClipId| c.0).collect::<Vec<_>>(), "guide": guide, "changed": changed}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: ID,
        label: "Use as Guide",
        menu: &["Clip"],
        shortcut: None,
        params: r#"{"clips":[id]?,"guide":bool?}  a guide shows in the Program preview but is left out of exports, frames and AI Restyle unless `includeGuides` is set; toggles when `guide` is absent"#,
        enabled: has_selection,
        run: set_guide,
        journal: true,
    }]
}
