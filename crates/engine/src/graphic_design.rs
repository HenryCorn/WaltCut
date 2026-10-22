//! Responsive layout, per-character styles, source graphics and font replacement.

use std::sync::Arc;

use serde_json::{Value, json};
use waltcut_project::graphic::{self, layer_display_name, layer_indices};
use waltcut_project::graphic_design::{
    CharStyle, GraphicMeta, Pin, PinTarget, RollMode, SourceGraphic, apply_char_style, clear_char_style, ensure_uids, layer_uid,
};
use waltcut_project::{ClipId, EffectInstance, ItemId, ItemKind, Label, ParamValue, Project, TrackItem};
use waltcut_render::graphic_clip::{item_layer_specs, layer_bounds};
use waltcut_time::Tick;

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, has_seq, str_p, u64_p};
use crate::graphics::{layer_effect_index, target_clip};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(id: &'static str, label: &'static str, menu: &'static [&'static str], params: &'static str, enabled: Enabled, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut: None, params, enabled, run, journal: true }
}
fn query(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false }
}

const G: &[&str] = &["Graphics and Titles"];

fn has_graphic(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    target_clip(s, &Value::Null).map(|_| ()).ok_or_else(|| "select a graphic clip".into())
}

fn clip_item(s: &Session, clip: ClipId) -> Result<TrackItem> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    q.find_item(clip).map(|(_, it)| it.clone()).ok_or_else(|| bad("graphics", "no such clip"))
}

fn mt_at_playhead(s: &Session, it: &TrackItem) -> Tick {
    it.source_time_at(s.playhead().clamp(it.start, it.end() - Tick(1)))
}

/// Duration parameter: `<key>Frames` (sequence frames), `<key>Seconds`, or `<key>` in ticks.
fn dur_p(s: &Session, p: &Value, key: &str) -> Option<Tick> {
    let rate = s.sequence_rate();
    if let Some(f) = f64_p(p, &format!("{key}Frames")) {
        return Some(rate.tick_of(f.round() as i64));
    }
    if let Some(sec) = f64_p(p, &format!("{key}Seconds")) {
        return Some(Tick::from_seconds_f64(sec.max(0.0)));
    }
    p.get(key).and_then(Value::as_i64).map(Tick)
}

fn edit_meta(s: &mut Session, clip: ClipId, label: &str, f: impl FnOnce(&mut GraphicMeta, &TrackItem) -> Result<()>) -> Result<GraphicMeta> {
    s.edit_sequence(label, |q, _, _| {
        let (_, it) = q.find_item_mut(clip).ok_or(waltcut_edit::EditError::NoItem(clip))?;
        let mut m = it.graphic.as_deref().cloned().unwrap_or_default();
        f(&mut m, it)?;
        it.graphic = (!m.is_empty()).then(|| Box::new(m.clone()));
        Ok(m)
    })
}

fn set_roll(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = target_clip(s, p).ok_or_else(|| bad("graphics.setRoll", "no graphic clip"))?;
    let mode = match str_p(p, "mode") {
        Some(m) => Some(RollMode::from_name(m).ok_or_else(|| bad("graphics.setRoll", format!("unknown mode `{m}` (off, roll, crawlLeft, crawlRight)")))?),
        None => None,
    };
    let (pre, ei, eo, post) = (dur_p(s, p, "preroll"), dur_p(s, p, "easeIn"), dur_p(s, p, "easeOut"), dur_p(s, p, "postroll"));
    let (so, eoff) = (bool_p(p, "startOffScreen"), bool_p(p, "endOffScreen"));
    let m = edit_meta(s, clip, "Roll/Crawl Options", |m, _| {
        if let Some(x) = mode {
            m.roll.mode = x;
        }
        if let Some(x) = so {
            m.roll.start_off_screen = x;
        }
        if let Some(x) = eoff {
            m.roll.end_off_screen = x;
        }
        for (dst, v) in [(&mut m.roll.preroll, pre), (&mut m.roll.ease_in, ei), (&mut m.roll.ease_out, eo), (&mut m.roll.postroll, post)] {
            if let Some(v) = v {
                *dst = v.max(Tick::ZERO);
            }
        }
        Ok(())
    })?;
    Ok(json!({"clip": clip.0, "roll": serde_json::to_value(&m.roll).unwrap_or_default()}))
}

fn set_responsive_time(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = target_clip(s, p).ok_or_else(|| bad("graphics.setResponsiveTime", "no graphic clip"))?;
    let (intro, outro) = (dur_p(s, p, "intro"), dur_p(s, p, "outro"));
    if intro.is_none() && outro.is_none() {
        return Err(bad("graphics.setResponsiveTime", "need `introFrames` / `outroFrames` (or seconds / ticks)"));
    }
    let m = edit_meta(s, clip, "Responsive Design - Time", |m, it| {
        if !m.has_responsive_time() {
            m.design_in = it.source_in;
            m.design_duration = Tick((it.duration.0 as f64 * it.speed.abs()).round() as i64);
        }
        if let Some(x) = intro {
            m.intro = x.max(Tick::ZERO);
        }
        if let Some(x) = outro {
            m.outro = x.max(Tick::ZERO);
        }
        if m.intro + m.outro > m.design_duration {
            return Err(bad("graphics.setResponsiveTime", "the intro and outro are longer than the graphic"));
        }
        if m.intro == Tick::ZERO && m.outro == Tick::ZERO {
            m.design_in = Tick::ZERO;
            m.design_duration = Tick::ZERO;
        }
        Ok(())
    })?;
    Ok(json!({"clip": clip.0, "intro": m.intro.0, "outro": m.outro.0, "designDuration": m.design_duration.0}))
}

fn edges_p(p: &Value) -> [bool; 4] {
    match p.get("edges") {
        Some(Value::Array(a)) => {
            let has = |n: &str| a.iter().any(|v| v.as_str().is_some_and(|x| x.eq_ignore_ascii_case(n)));
            [has("left"), has("top"), has("right"), has("bottom")]
        }
        Some(Value::String(x)) if x != "all" => {
            let has = |n: &str| x.split([',', ' ']).any(|v| v.eq_ignore_ascii_case(n));
            [has("left"), has("top"), has("right"), has("bottom")]
        }
        _ => [true; 4],
    }
}

fn pin_layer(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = target_clip(s, p).ok_or_else(|| bad("graphics.pin", "no graphic clip"))?;
    let (l, ei) = layer_effect_index(s, clip, p)?;
    let it = clip_item(s, clip)?;
    let size = match s.project.item(it.item).map(|x| &x.kind) {
        Some(ItemKind::Graphic { width, height, .. }) => (*width, *height),
        _ => (1920, 1080),
    };
    let idx = layer_indices(&it.effects);
    let to = p.get("to").cloned().unwrap_or(json!("frame"));
    let target: Option<Option<usize>> = match &to {
        Value::String(x) if x == "none" || x.is_empty() => None,
        Value::String(x) if x == "frame" || x.eq_ignore_ascii_case("video frame") => Some(None),
        Value::Number(n) => Some(Some(n.as_u64().unwrap_or(0) as usize)),
        Value::String(name) => Some(Some(
            (0..idx.len())
                .find(|&i| layer_display_name(&it.effects[idx[i]], i).eq_ignore_ascii_case(name))
                .ok_or_else(|| bad("graphics.pin", format!("no layer named `{name}`")))?,
        )),
        _ => return Err(bad("graphics.pin", "`to`: \"frame\", \"none\", a layer index or name")),
    };
    if let Some(Some(t)) = target
        && (t >= idx.len() || t == l)
    {
        return Err(bad("graphics.pin", "pin to another layer of the same graphic"));
    }
    let edges = edges_p(p);
    let mt = mt_at_playhead(s, &it);
    // pins are measured at rest (without the roll / crawl offset)
    let mut rest = it.clone();
    if let Some(m) = rest.graphic.as_mut() {
        m.roll.mode = RollMode::Off;
    }
    let shown = item_layer_specs(&rest, mt, size);
    let bounds_of = |e: usize| shown.iter().find(|(x, _)| *x == e).map(|(_, sp)| layer_bounds(sp));
    let own = bounds_of(ei).ok_or_else(|| bad("graphics.pin", "bad layer"))?;
    let pin = match target {
        None => None,
        Some(None) => Some((PinTarget::Frame, [0.0, 0.0, size.0 as f64, size.1 as f64], None)),
        Some(Some(t)) => Some((PinTarget::Layer(0), bounds_of(idx[t]).ok_or_else(|| bad("graphics.pin", "bad target"))?, Some(idx[t]))),
    };
    let r = s.edit_sequence("Responsive Design - Position", move |q, _, _| {
        let (_, it) = q.find_item_mut(clip).ok_or(waltcut_edit::EditError::NoItem(clip))?;
        let li = layer_indices(&it.effects);
        let mut layers: Vec<EffectInstance> = li.iter().map(|&i| it.effects[i].clone()).collect();
        ensure_uids(&mut layers).map_err(|e| bad("graphics.pin", e))?;
        for (k, &i) in li.iter().enumerate() {
            it.effects[i].layer = layers[k].layer.clone();
        }
        let new_pin = pin.map(|(to, tb, teff)| {
            let to = match (to, teff) {
                (PinTarget::Layer(_), Some(te)) => PinTarget::Layer(layer_uid(&it.effects[te])),
                _ => PinTarget::Frame,
            };
            Pin::new(to, edges, own, tb)
        });
        it.effects[ei].layer.get_or_insert_with(Default::default).pin = new_pin;
        let uid = it.effects[ei].layer.as_ref().map_or(0, |x| x.uid);
        Ok(json!({"clip": clip.0, "layer": l, "uid": uid, "pin": it.effects[ei].layer.as_ref().and_then(|x| x.pin.as_ref()).map(|p| serde_json::to_value(p).unwrap_or_default())}))
    })?;
    Ok(r)
}

/// Character style from command JSON (friendly names accepted).
fn char_style_p(v: &Value) -> Result<CharStyle> {
    let o = v.as_object().ok_or_else(|| bad("graphics.setCharStyle", "`style` must be an object"))?;
    let mut c = CharStyle::default();
    for (k, x) in o {
        match k.as_str() {
            "font" | "family" => c.font = x.as_str().map(str::to_string),
            "fontStyle" | "font_style" | "style" => c.font_style = x.as_str().map(str::to_string),
            "size" | "fontSize" => c.size = x.as_f64().map(|v| v as f32),
            "fill" | "color" | "fillColor" | "fill_color" => {
                c.fill = Some(match crate::commands::json_to_param(&ParamValue::Color([0.0; 4]), x) {
                    Some(ParamValue::Color(col)) => col,
                    _ => return Err(bad("graphics.setCharStyle", "`color`: \"#rrggbb\" or [r,g,b,a]")),
                })
            }
            "bold" | "fauxBold" | "faux_bold" => c.faux_bold = x.as_bool(),
            "italic" | "fauxItalic" | "faux_italic" => c.faux_italic = x.as_bool(),
            "underline" => c.underline = x.as_bool(),
            "tracking" => c.tracking = x.as_f64().map(|v| v as f32),
            "baselineShift" | "baseline_shift" => c.baseline_shift = x.as_f64().map(|v| v as f32),
            "caps" => {
                c.caps = match x {
                    Value::Number(n) => n.as_u64().map(|v| v.min(2) as u32),
                    Value::String(s) => graphic::CAPS_OPTS.iter().position(|o| o.eq_ignore_ascii_case(s.replace('_', " ").as_str())).map(|i| i as u32),
                    _ => None,
                }
            }
            other => return Err(bad("graphics.setCharStyle", format!("unknown style property `{other}`"))),
        }
    }
    Ok(c)
}

fn set_char_style(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = target_clip(s, p).ok_or_else(|| bad("graphics.setCharStyle", "no graphic clip"))?;
    let (l, ei) = layer_effect_index(s, clip, p)?;
    let clear = bool_p(p, "clear").unwrap_or(false);
    let style = match p.get("style") {
        Some(v) => char_style_p(v)?,
        None if clear => CharStyle::default(),
        None => return Err(bad("graphics.setCharStyle", "need `style` (or `clear`)")),
    };
    let tl = s.playhead();
    let (start, end) = (u64_p(p, "start").map(|v| v as usize), u64_p(p, "end").map(|v| v as usize));
    // A pointer gesture shares one undo step only within the same clip, layer and range.
    let merge = bool_p(p, "merge").unwrap_or(false).then(|| format!("graphics.setCharStyle:{}:{ei}:{start:?}:{end:?}", clip.0));
    if bool_p(p, "begin").unwrap_or(false) {
        s.history.merge_key = None;
    }
    let runs = s.edit_sequence_as("Character Style", merge.as_deref(), move |q, _, _| {
        let (_, it) = q.find_item_mut(clip).ok_or(waltcut_edit::EditError::NoItem(clip))?;
        let mt = it.source_time_at(tl.clamp(it.start, it.end() - Tick(1)));
        let e = &mut it.effects[ei];
        if e.effect != graphic::TEXT_LAYER {
            return Err(bad("graphics.setCharStyle", "the layer is not a text layer"));
        }
        let n = graphic::text_chars(e, mt);
        let (a, b) = (start.unwrap_or(0).min(n), end.unwrap_or(n).min(n));
        if a >= b {
            return Err(bad("graphics.setCharStyle", "empty character range"));
        }
        let x = e.layer.get_or_insert_with(Default::default);
        x.runs = if clear { clear_char_style(&x.runs, n, a, b) } else { apply_char_style(&x.runs, n, a, b, &style) };
        let runs = serde_json::to_value(&x.runs).unwrap_or_default();
        if x.is_empty() {
            e.layer = None;
        }
        Ok(runs)
    })?;
    Ok(json!({"clip": clip.0, "layer": l, "styles": runs}))
}

// ------------------------------------------------------------------------------------------------
// Upgrade to Source Graphic
// ------------------------------------------------------------------------------------------------

fn upgrade_to_source(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = target_clip(s, p).ok_or_else(|| bad("graphics.upgradeToSourceGraphic", "select a graphic clip"))?;
    let it = clip_item(s, clip)?;
    if s.project.source_graphics.contains_key(&it.item) {
        return Err(bad("graphics.upgradeToSourceGraphic", "the graphic already is a source graphic"));
    }
    let kind = s.project.item(it.item).map(|x| x.kind.clone()).ok_or_else(|| bad("graphics.upgradeToSourceGraphic", "no item"))?;
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let name = it.name.clone();
    let item = s.edit("Upgrade to Source Graphic", move |pr, st| {
        let item = pr.add_item(&name, Label::Rose, kind, None);
        let q = pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
        let (_, ti) = q.find_item_mut(clip).ok_or_else(|| bad("graphics.upgradeToSourceGraphic", "no such clip"))?;
        ti.item = item;
        let layers: Vec<EffectInstance> = ti.effects.iter().filter(|e| graphic::is_layer(e)).cloned().collect();
        let meta = ti.graphic.as_deref().cloned();
        pr.source_graphics.insert(item, SourceGraphic { layers, meta });
        st.project_selection = vec![item];
        Ok(item)
    })?;
    Ok(json!({"clip": clip.0, "item": item.0}))
}

/// After a `graphics.*` command: copy an edited instance of a source graphic to the shared
/// layers and every other instance (in the same undo step).
pub fn sync_source_graphics(s: &mut Session) {
    if s.project.source_graphics.is_empty() {
        return;
    }
    let p = Arc::make_mut(&mut s.project);
    let items: Vec<ItemId> = p.source_graphics.keys().copied().collect();
    for item in items {
        let master = p.source_graphics[&item].clone();
        let layers_of = |t: &TrackItem| t.effects.iter().filter(|e| graphic::is_layer(e)).cloned().collect::<Vec<_>>();
        // the first instance that differs from the master is the edited one
        let mut changed: Option<SourceGraphic> = None;
        for q in p.sequences() {
            if let ItemKind::Sequence(sq) = &q.kind {
                for tr in &sq.video_tracks {
                    for t in tr.items.iter().filter(|t| t.item == item) {
                        let l = layers_of(t);
                        let m = t.graphic.as_deref().cloned();
                        if l != master.layers || m != master.meta {
                            changed = Some(SourceGraphic { layers: l, meta: m });
                        }
                    }
                }
            }
            if changed.is_some() {
                break;
            }
        }
        let Some(new) = changed else { continue };
        let ids: Vec<ItemId> = p.sequences().map(|q| q.id).collect();
        for id in ids {
            if let Some(sq) = p.sequence_mut(id) {
                for tr in &mut sq.video_tracks {
                    for t in tr.items.iter_mut().filter(|t| t.item == item) {
                        let mut fx: Vec<EffectInstance> = t.effects.iter().filter(|e| !graphic::is_layer(e)).cloned().collect();
                        let at = fx.iter().position(|e| !e.def().is_some_and(|d| d.intrinsic)).unwrap_or(fx.len());
                        for (k, l) in new.layers.iter().enumerate() {
                            fx.insert(at + k, l.clone());
                        }
                        t.effects = fx;
                        t.graphic = new.meta.clone().map(Box::new);
                    }
                }
            }
        }
        p.source_graphics.insert(item, new);
    }
}

// ------------------------------------------------------------------------------------------------
// Fonts
// ------------------------------------------------------------------------------------------------

/// Every graphic layer (all sequences and source graphics), mutably.
fn for_each_layer(p: &mut Project, mut f: impl FnMut(&mut EffectInstance)) {
    let ids: Vec<ItemId> = p.sequences().map(|q| q.id).collect();
    for id in ids {
        if let Some(sq) = p.sequence_mut(id) {
            for tr in &mut sq.video_tracks {
                for t in &mut tr.items {
                    t.effects.iter_mut().filter(|e| e.effect == graphic::TEXT_LAYER).for_each(&mut f);
                }
            }
        }
    }
    for sg in p.source_graphics.values_mut() {
        sg.layers.iter_mut().filter(|e| e.effect == graphic::TEXT_LAYER).for_each(&mut f);
    }
}

fn text_of(e: &EffectInstance, k: &str) -> String {
    match e.params.get(k).map(|x| &x.value) {
        Some(ParamValue::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

/// Fonts used by graphics (layers and character styles).
pub fn fonts_used(s: &Session) -> Vec<(String, String, usize)> {
    let mut v: Vec<(String, String, usize)> = Vec::new();
    let mut add = |fam: String, st: String| match v.iter_mut().find(|x| x.0.eq_ignore_ascii_case(&fam) && x.1.eq_ignore_ascii_case(&st)) {
        Some(x) => x.2 += 1,
        None => v.push((fam, st, 1)),
    };
    let mut p = (*s.project).clone();
    for_each_layer(&mut p, |e| {
        let (fam, st) = (text_of(e, "font"), text_of(e, "font_style"));
        add(if fam.is_empty() { "Inter".into() } else { fam.clone() }, if st.is_empty() { "Regular".into() } else { st.clone() });
        for r in e.layer.as_ref().map(|x| x.runs.clone()).unwrap_or_default() {
            if r.style.font.is_some() || r.style.font_style.is_some() {
                add(r.style.font.clone().unwrap_or_else(|| fam.clone()), r.style.font_style.clone().unwrap_or_else(|| st.clone()));
            }
        }
    });
    v.sort();
    v
}

fn replace_fonts(s: &mut Session, p: &Value) -> Result<Value> {
    let (from, from_style) = match p.get("from") {
        Some(Value::String(f)) => (f.clone(), None),
        Some(Value::Object(o)) => {
            (o.get("family").and_then(Value::as_str).unwrap_or("").to_string(), o.get("style").and_then(Value::as_str).map(str::to_string))
        }
        _ => return Err(bad("file.replaceFonts", "need `from` (a family, or {family, style})")),
    };
    let (to, to_style) = match p.get("to") {
        Some(Value::String(f)) => (f.clone(), str_p(p, "toStyle").map(str::to_string)),
        Some(Value::Object(o)) => {
            (o.get("family").and_then(Value::as_str).unwrap_or("").to_string(), o.get("style").and_then(Value::as_str).map(str::to_string))
        }
        _ => return Err(bad("file.replaceFonts", "need `to` (a family, or {family, style})")),
    };
    if from.is_empty() || to.is_empty() {
        return Err(bad("file.replaceFonts", "font families cannot be empty"));
    }
    let n = s.edit("Replace Fonts", move |pr, _| {
        let mut n = 0usize;
        let matches = |fam: &str, st: &str| fam.eq_ignore_ascii_case(&from) && from_style.as_ref().is_none_or(|x| x.eq_ignore_ascii_case(st));
        for_each_layer(pr, |e| {
            let (fam, st) = (text_of(e, "font"), text_of(e, "font_style"));
            if matches(&fam, &st) {
                e.params.insert("font".into(), waltcut_project::Param::new(ParamValue::Text(to.clone())));
                if let Some(ts) = &to_style {
                    e.params.insert("font_style".into(), waltcut_project::Param::new(ParamValue::Text(ts.clone())));
                }
                n += 1;
            }
            if let Some(x) = e.layer.as_mut() {
                for r in &mut x.runs {
                    let rf = r.style.font.clone().unwrap_or_else(|| fam.clone());
                    let rs = r.style.font_style.clone().unwrap_or_else(|| st.clone());
                    if r.style.font.is_some() && matches(&rf, &rs) {
                        r.style.font = Some(to.clone());
                        if let Some(ts) = &to_style {
                            r.style.font_style = Some(ts.clone());
                        }
                        n += 1;
                    }
                }
            }
        });
        Ok(n)
    })?;
    Ok(json!({"replaced": n}))
}

fn list_fonts_used(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(Value::Array(
        fonts_used(s)
            .into_iter()
            .map(|(f, st, n)| {
                let missing = waltcut_text::resolve(&f, &st).missing;
                json!({"family": f, "style": st, "uses": n, "missing": missing})
            })
            .collect(),
    ))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec(
            "graphics.setRoll",
            "Roll/Crawl Options",
            &[],
            r#"{"clip":id?,"mode":"off|roll|crawlLeft|crawlRight"?,"startOffScreen":bool?,"endOffScreen":bool?,"prerollFrames":n?,"easeInFrames":n?,"easeOutFrames":n?,"postrollFrames":n? (or …Seconds, or ticks as preroll/easeIn/easeOut/postroll)}"#,
            has_graphic,
            set_roll,
        ),
        spec(
            "graphics.setResponsiveTime",
            "Responsive Design - Time",
            &[],
            r#"{"clip":id?,"introFrames":n?,"outroFrames":n? (or introSeconds / outroSeconds, or ticks as intro / outro)}"#,
            has_graphic,
            set_responsive_time,
        ),
        spec(
            "graphics.pin",
            "Responsive Design - Position",
            &[],
            r#"{"clip":id?,"layer":n|name?,"to":"frame"|"none"|layer index|layer name,"edges":["left","top","right","bottom"]|"all"="all"}"#,
            has_graphic,
            pin_layer,
        ),
        spec(
            "graphics.setCharStyle",
            "Character Style",
            &[],
            r##"{"clip":id?,"layer":n|name?,"start":char=0,"end":char=len,"style":{"font":str,"fontStyle":str,"size":px,"color":"#rrggbb","bold":bool,"italic":bool,"underline":bool,"tracking":n,"baselineShift":px,"caps":"normal|all caps|small caps"},"clear":bool? (remove styles from the range),"merge":bool?,"begin":bool? (start a new undo gesture)}"##,
            has_graphic,
            set_char_style,
        ),
        spec("graphics.upgradeToSourceGraphic", "Upgrade to Source Graphic", G, r#"{"clip":id?}"#, has_graphic, upgrade_to_source),
        spec(
            "file.replaceFonts",
            "Replace Fonts in Projects…",
            G,
            r#"{"from":family|{"family","style"?},"to":family|{"family","style"?},"toStyle":str?}"#,
            always,
            replace_fonts,
        ),
        query("graphics.fonts.used", "List Fonts Used", "{}", list_fonts_used),
    ]
}

#[cfg(test)]
#[path = "graphic_design_tests.rs"]
mod tests;
