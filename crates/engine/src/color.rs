//! Colour commands: colour management (`sequence.colorSettings`, `clip.interpretFootage`,
//! `color.spaces`, `media.colorInfo`).

use serde_json::{Value, json};
use waltcut_color::{ColorSpace, WorkingSpace};
use waltcut_project::{ItemId, ItemKind};

use crate::commands::{CommandSpec, always, bad, bool_p, has_seq, item_p, str_p};
use crate::{Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(id: &'static str, label: &'static str, menu: &'static [&'static str], params: &'static str, enabled: Enabled, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut: None, params, enabled, run, journal: true }
}
fn query(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false }
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        spec(
            "sequence.colorSettings",
            "Color Management…",
            &["Sequence"],
            r#"{"workingSpace":"rec709"|"rec2100-pq"|"rec2100-hlg"?,"wideGamut":bool?,"autoToneMap":bool?}"#,
            has_seq,
            color_settings,
        ),
        spec(
            "clip.interpretFootage",
            "Interpret Footage…",
            &["Clip", "Modify"],
            r#"{"items":[id]?,"colorSpace":"auto"|"<color space id>"?,"pixelAspect":[num,den]|"file"?}"#,
            has_footage,
            interpret,
        ),
        query("color.spaces", "List Colour Spaces", "{}", spaces),
        query("media.colorInfo", "Media Colour Info", r#"{"item":id}"#, color_info),
    ]
}

fn color_settings(s: &mut Session, p: &Value) -> Result<Value> {
    let id = s.state.active_sequence.ok_or_else(|| bad("sequence.colorSettings", "no active sequence"))?;
    let ws = match str_p(p, "workingSpace") {
        Some(w) => Some(WorkingSpace::parse(w).ok_or_else(|| bad("sequence.colorSettings", format!("unknown working space `{w}`")))?),
        None => None,
    };
    let (wide, tone) = (bool_p(p, "wideGamut"), bool_p(p, "autoToneMap"));
    let cur = s.project.sequence(id).map(|q| q.settings.color).unwrap_or_default();
    let changed = ws.is_some_and(|w| w != cur.working) || wide.is_some_and(|v| v != cur.wide_gamut) || tone.is_some_and(|v| v != cur.auto_tone_map);
    if changed {
        s.edit("Sequence Color Settings", |pr, _| {
            let q = pr.sequence_mut(id).ok_or_else(|| bad("sequence.colorSettings", "no sequence"))?;
            let c = &mut q.settings.color;
            if let Some(w) = ws {
                c.working = w;
            }
            if let Some(v) = wide {
                c.wide_gamut = v;
            }
            if let Some(v) = tone {
                c.auto_tone_map = v;
            }
            q.settings.working_space = c.working.label().into();
            Ok(())
        })?;
    }
    let c = s.project.sequence(id).map(|q| q.settings.color).unwrap_or_default();
    Ok(json!({"workingSpace": c.working.id(), "label": c.working.label(), "wideGamut": c.wide_gamut, "autoToneMap": c.auto_tone_map}))
}

fn has_footage(s: &Session) -> std::result::Result<(), String> {
    if footage_targets(s, &Value::Null).is_empty() { Err("select footage in the Project panel or clips in the timeline".into()) } else { Ok(()) }
}

/// Media items addressed by `items`, else the Project panel selection, else the media of the
/// selected timeline clips.
pub(crate) fn footage_targets(s: &Session, p: &Value) -> Vec<ItemId> {
    let media = |i: &ItemId| s.project.item(*i).is_some_and(|it| matches!(it.kind, ItemKind::Media(_)));
    if let Some(a) = p.get("items").and_then(Value::as_array) {
        return a.iter().filter_map(Value::as_u64).map(ItemId).filter(media).collect();
    }
    let mut v: Vec<ItemId> = s.state.project_selection.iter().copied().filter(media).collect();
    if v.is_empty()
        && let Some(q) = s.active_sequence()
    {
        for c in &s.state.selection {
            if let Some((_, it)) = q.find_item(*c)
                && media(&it.item)
                && !v.contains(&it.item)
            {
                v.push(it.item);
            }
        }
    }
    v
}

/// `clip.interpretFootage`: the colour space (`colorSpace`) and / or the pixel aspect ratio
/// (`pixelAspect`) the footage is interpreted with. `"auto"` / `"file"` go back to the file's
/// metadata; a parameter left out keeps its current interpretation.
fn interpret(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "clip.interpretFootage";
    let items = footage_targets(s, p);
    if items.is_empty() {
        return Err(bad(CMD, "no footage selected"));
    }
    let cs = match str_p(p, "colorSpace") {
        None => None,
        Some("auto") | Some("") => Some(None),
        Some(c) => Some(Some(ColorSpace::parse(c).ok_or_else(|| bad(CMD, format!("unknown colour space `{c}`")))?)),
    };
    let par = match p.get("pixelAspect") {
        None | Some(Value::Null) => None,
        Some(Value::String(f)) if f == "file" || f == "auto" => Some(None),
        Some(v) => Some(Some(
            pixel_aspect_p(v)
                .ok_or_else(|| bad(CMD, format!("pixelAspect must be [num, den] between 1:{0} and {0}:1, or \"file\"", waltcut_project::MAX_PIXEL_ASPECT)))?,
        )),
    };
    if cs.is_none() && par.is_none() {
        return Err(bad(CMD, "need `colorSpace` (auto or a colour space id) or `pixelAspect`"));
    }
    s.edit("Interpret Footage", |pr, _| {
        for i in &items {
            if let Some(ItemKind::Media(m)) = pr.item_mut(*i).map(|it| &mut it.kind) {
                if let Some(cs) = cs {
                    m.interpret.color_space = cs;
                }
                if let Some(par) = par {
                    m.interpret.par = par;
                }
            }
        }
        Ok(())
    })?;
    let first = items.first().and_then(|i| s.project.item(*i)).and_then(|i| i.as_media());
    Ok(json!({
        "items": items.iter().map(|i| i.0).collect::<Vec<_>>(),
        "colorSpace": first.and_then(|m| m.interpret.color_space).map(|c| c.id()).unwrap_or("auto"),
        "pixelAspect": first.map(|m| m.pixel_aspect()),
    }))
}

/// `[num, den]` as a pixel aspect ratio WaltCut can use ([`waltcut_project::checked_par`]).
fn pixel_aspect_p(v: &Value) -> Option<(u32, u32)> {
    let [n, d] = v.as_array()?.as_slice() else { return None };
    let (n, d) = (u32::try_from(n.as_u64()?).ok()?, u32::try_from(d.as_u64()?).ok()?);
    waltcut_project::checked_par((n, d))
}

fn spaces(_: &mut Session, _: &Value) -> Result<Value> {
    let cs: Vec<Value> = ColorSpace::ALL
        .iter()
        .map(|c| json!({"id": c.id(), "label": c.label(), "hdr": c.is_hdr(), "log": c.is_log(), "gamut": format!("{:?}", c.gamut())}))
        .collect();
    let ws: Vec<Value> = WorkingSpace::ALL.iter().map(|w| json!({"id": w.id(), "label": w.label(), "hdr": w.is_hdr()})).collect();
    Ok(json!({"colorSpaces": cs, "workingSpaces": ws}))
}

fn color_info(s: &mut Session, p: &Value) -> Result<Value> {
    let id = item_p(p, "item").ok_or_else(|| bad("media.colorInfo", "need `item`"))?;
    let Some(ItemKind::Media(m)) = s.project.item(id).map(|i| &i.kind) else { return Err(bad("media.colorInfo", "not a media item")) };
    let detected = m.info.video.as_ref().map(|v| ColorSpace::from_info(&v.color));
    let effective = m.interpret.color_space.or(detected);
    Ok(json!({
        "detected": detected.map(|c| c.id()),
        "detectedLabel": detected.map(|c| c.label()),
        "metadata": m.info.video.as_ref().map(|v| json!({"transfer": format!("{:?}", v.color.transfer), "primaries": format!("{:?}", v.color.primaries), "matrix": format!("{:?}", v.color.matrix), "range": format!("{:?}", v.color.range)})),
        "override": m.interpret.color_space.map(|c| c.id()),
        "effective": effective.map(|c| c.id()),
        "effectiveLabel": effective.map(|c| c.label()),
        "hdr": effective.is_some_and(|c| c.is_hdr()),
        // mastering display / content light level and the peak tone mapping uses
        "hdrMetadata": m.info.video.as_ref().and_then(|v| v.hdr),
        "toneMapPeakNits": m.info.video.as_ref().and_then(|v| v.hdr.and_then(|h| h.peak_nits())),
    }))
}
