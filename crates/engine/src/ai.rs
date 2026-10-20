//! AI re-style: send rendered frames to an image model and lay the results over the timeline.
//!
//! `ai.restyle` renders the Program frame (picture, graphics and drawings composited) at the
//! playhead, or every `every`-th frame of the In/Out range or of a clip, at about one megapixel,
//! and hands each to the configured [`Restyler`] in a background job. When the job ends (also when
//! it fails or is cancelled part-way), [`poll`] imports the frames it finished and places them on a
//! new top video track, each held until the next one and scaled to the frame.
//!
//! The provider and model come from the preferences (`ai.setup`); the fal.ai key is kept in its
//! own owner-only file next to the preferences, never in them, so `ai.status` and preference
//! queries cannot reveal it.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use filmcraft_genai::{Picture, Provider, ProviderSettings, RestyleRequest, Restyler};
use filmcraft_project::{ItemId, TrackKind};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, has_seq, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// The most frames one request may send (each one is billed by a cloud provider).
pub const DEFAULT_MAX_FRAMES: u64 = 240;
const KEY_FILE: &str = "fal.key";

/// A re-style job whose frames still have to be placed.
pub struct PendingRestyle {
    pub job: u64,
    pub sequence: ItemId,
    /// How long each frame is held on the timeline.
    pub hold: Tick,
    /// (timeline time, image path) of each finished frame.
    pub outputs: Arc<Mutex<Vec<(Tick, String)>>>,
}

fn key_path(s: &Session) -> Option<PathBuf> {
    s.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join(KEY_FILE))
}

/// The provider settings with the saved key filled in.
pub fn settings(s: &Session) -> ProviderSettings {
    let mut st = s.prefs.ai.clone();
    if st.fal_key.is_empty()
        && let Some(k) = key_path(s).and_then(|p| std::fs::read_to_string(p).ok())
    {
        st.fal_key = k.trim().to_string();
    }
    st
}

fn restyler(s: &Session) -> Result<Arc<dyn Restyler>> {
    if let Some(r) = &s.restyler_override {
        return Ok(r.clone());
    }
    settings(s).restyler().map(Arc::from).map_err(|e| EngineError::Other(e.to_string()))
}

fn write_key(path: &std::path::Path, key: &str) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    use std::io::Write;
    o.open(path)?.write_all(key.as_bytes())
}

fn setup(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "ai.setup";
    let mut prefs = s.prefs.clone();
    if let Some(name) = str_p(p, "provider") {
        prefs.ai.provider = Provider::from_name(name).ok_or_else(|| bad(ID, format!("unknown provider `{name}` (fal or local)")))?;
    }
    if let Some(ep) = str_p(p, "endpoint") {
        let ep = ep.trim().trim_matches('/');
        filmcraft_genai::fal::check_endpoint(ep).map_err(|e| bad(ID, e.to_string()))?;
        prefs.ai.fal_endpoint = ep.to_string();
    }
    if let Some(key) = str_p(p, "apiKey") {
        let key = key.trim();
        if key.len() > 512 || key.chars().any(char::is_control) {
            return Err(bad(ID, "that does not look like an API key"));
        }
        match key_path(s) {
            Some(path) if key.is_empty() => {
                let _ = std::fs::remove_file(path);
            }
            Some(path) => write_key(&path, key).map_err(|e| EngineError::Other(format!("saving the API key: {e}")))?,
            // headless sessions without a data directory keep it for this session only
            None => prefs.ai.fal_key = key.to_string(),
        }
    }
    if prefs != s.prefs {
        if s.prefs_path.is_some() {
            s.set_prefs(prefs).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
        } else {
            s.prefs = prefs;
        }
    }
    status(s, &Value::Null)
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let st = settings(s);
    let key = if !st.fal_key.is_empty() {
        "saved"
    } else if std::env::var("FAL_KEY").is_ok_and(|k| !k.trim().is_empty()) {
        "environment"
    } else {
        "missing"
    };
    Ok(json!({
        "provider": st.provider,
        "providerLabel": st.provider.label(),
        "endpoint": st.fal_endpoint,
        "key": key,
        "ready": s.restyler_override.is_some() || st.restyler().is_ok(),
        "cloud": cfg!(feature = "ai-cloud"),
    }))
}

/// Timeline times to re-style: the playhead, or every `every`-th frame of a range.
fn frame_times(s: &Session, cmd: &str, p: &Value) -> Result<(Vec<Tick>, Tick)> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = seq.settings.frame_rate;
    let fd = rate.frame_duration();
    if fd <= Tick::ZERO {
        return Err(bad(cmd, "the sequence has no frame rate"));
    }
    let every = u64_p(p, "every").unwrap_or(2).clamp(1, 1000) as i64;
    let hold = Tick(fd.0.saturating_mul(every));
    let range = match str_p(p, "range").unwrap_or("playhead") {
        "playhead" | "frame" => return Ok((vec![rate.snap(s.playhead())], hold)),
        "inout" | "in-out" => {
            let (Some(i), Some(o)) = (seq.mark_in, seq.mark_out) else {
                return Err(bad(cmd, "set In and Out points on the sequence first"));
            };
            TimeRange::new(i, o - i)
        }
        "clip" => {
            let c = s.state.selection.first().copied().ok_or_else(|| bad(cmd, "select a clip"))?;
            seq.find_item(c).map(|(_, it)| it.range()).ok_or_else(|| bad(cmd, "no such clip"))?
        }
        other => return Err(bad(cmd, format!("`range` is playhead, inout or clip, not `{other}`"))),
    };
    let max = u64_p(p, "maxFrames").unwrap_or(DEFAULT_MAX_FRAMES).clamp(1, 10_000);
    let mut out = Vec::new();
    let mut t = rate.snap(range.start);
    while t < range.end() {
        if out.len() as u64 >= max {
            return Err(bad(cmd, format!("that is more than {max} frames; raise `every` or `maxFrames`")));
        }
        out.push(t);
        t += hold;
    }
    if out.is_empty() {
        return Err(bad(cmd, "the range is empty"));
    }
    Ok((out, hold))
}

/// Folder for a job's frames: next to the saved project, else in the data directory.
fn output_dir(s: &Session, job: u64) -> PathBuf {
    let stamp = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let base = match (&s.path, &s.prefs_path) {
        (Some(p), _) => std::path::Path::new(p).with_extension("").with_file_name(format!(
            "{} AI Frames",
            std::path::Path::new(p).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Project".into())
        )),
        (None, Some(pp)) => pp.parent().map(|d| d.join("AI Frames")).unwrap_or_else(|| std::env::temp_dir().join("WaltCut AI Frames")),
        (None, None) => std::env::temp_dir().join("WaltCut AI Frames"),
    };
    base.join(format!("restyle-{stamp}-{job}"))
}

fn restyle(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "ai.restyle";
    let req = RestyleRequest {
        prompt: str_p(p, "prompt").unwrap_or_default().to_string(),
        negative_prompt: str_p(p, "negativePrompt").unwrap_or_default().to_string(),
        strength: f64_p(p, "strength").unwrap_or(0.55) as f32,
        seed: u64_p(p, "seed").or(Some(1234)),
    };
    req.validate().map_err(|e| bad(ID, e.to_string()))?;
    let (times, hold) = frame_times(s, ID, p)?;
    let backend = restyler(s)?;
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let (w, h) = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).ok_or(EngineError::NoSequence)?;
    // about one megapixel: what the models work at, and less to upload
    let scale = ((1_100_000.0 / (w.max(1) as f64 * h.max(1) as f64)).sqrt()).min(1.0) as f32;
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let n = times.len();
    let job = crate::Job {
        id,
        label: format!("AI Restyle ({n} frame{}, {})", if n == 1 { "" } else { "s" }, backend.name()),
        progress: Default::default(),
        result: Default::default(),
    };
    job.progress.total.store(n as u64, Ordering::Relaxed);
    let dir = output_dir(s, id);
    let outputs: Arc<Mutex<Vec<(Tick, String)>>> = Default::default();
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    let project = s.project.clone();
    let (prog, res, outs) = (job.progress.clone(), job.result.clone(), outputs.clone());
    let t0 = web_time::Instant::now();
    let run = move || {
        let r = (|| -> std::result::Result<usize, String> {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            for (i, t) in times.iter().enumerate() {
                if prog.cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("Frame {} of {n}", i + 1);
                let opts = filmcraft_render::RenderOptions { scale, ..Default::default() };
                let img = filmcraft_render::render_sequence(&project, seq_id, *t, opts, &provider);
                let png = filmcraft_export::encode_png(img.over_black_rgba8(), img.w as u32, img.h as u32).map_err(|e| e.to_string())?;
                let out = backend.restyle(&Picture { png, width: img.w as u32, height: img.h as u32 }, &req).map_err(|e| format!("frame {}: {e}", i + 1))?;
                let ext = if out.starts_with(b"\x89PNG") { "png" } else { "jpg" };
                let path = dir.join(format!("frame_{:05}.{ext}", i + 1));
                std::fs::write(&path, &out).map_err(|e| format!("{}: {e}", path.display()))?;
                outs.lock().unwrap_or_else(PoisonError::into_inner).push((*t, path.to_string_lossy().to_string()));
                prog.done.fetch_add(1, Ordering::Relaxed);
            }
            Ok(n)
        })();
        let secs = t0.elapsed().as_secs_f64();
        let report =
            filmcraft_export::Report { path: dir.to_string_lossy().to_string(), frames: 0, seconds: secs, bytes: 0, render_fps: 0.0, extra_files: Vec::new() };
        let r = match r {
            Ok(done) => {
                *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("Restyled {done} frame(s)");
                Ok(report)
            }
            Err(e) => {
                *prog.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(e.clone());
                Err(e)
            }
        };
        prog.finished.store(true, Ordering::Relaxed);
        *res.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
    };
    let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), run);
    s.jobs.push(job);
    s.ai_jobs.push(PendingRestyle { job: id, sequence: seq_id, hold, outputs });
    if bool_p(p, "wait").unwrap_or(false) || cfg!(target_arch = "wasm32") {
        run();
        poll(s);
    } else {
        std::thread::Builder::new().name("ai-restyle".into()).spawn(run).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    Ok(json!({"job": id, "frames": n}))
}

/// Place the frames of finished re-style jobs on a new top video track.
pub fn poll(s: &mut Session) {
    let finished: Vec<usize> = s
        .ai_jobs
        .iter()
        .enumerate()
        .filter(|(_, pj)| s.jobs.iter().find(|j| j.id == pj.job).is_none_or(|j| j.progress.finished.load(Ordering::Relaxed)))
        .map(|(i, _)| i)
        .collect();
    for i in finished.into_iter().rev() {
        let pj = s.ai_jobs.remove(i);
        let outs = std::mem::take(&mut *pj.outputs.lock().unwrap_or_else(PoisonError::into_inner));
        if outs.is_empty() {
            continue;
        }
        if let Err(e) = place(s, &pj, &outs) {
            s.error_toast("AI Restyle", e.to_string());
        }
    }
}

fn place(s: &mut Session, pj: &PendingRestyle, outs: &[(Tick, String)]) -> Result<()> {
    let paths: Vec<&str> = outs.iter().map(|(_, p)| p.as_str()).collect();
    let r = s.execute("file.import", json!({"paths": paths}))?;
    let items: Vec<ItemId> = r.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(ItemId).collect()).unwrap_or_default();
    if items.len() != outs.len() {
        return Err(EngineError::Other(format!("imported {} of {} restyled frames", items.len(), outs.len())));
    }
    let seq_id = pj.sequence;
    let hold = pj.hold;
    s.edit("AI Restyle", |pr, st| {
        let rate = pr.sequence(seq_id).ok_or(EngineError::NoSequence)?.settings.frame_rate;
        let mut clips = Vec::new();
        for (k, ((t, _), item)) in outs.iter().zip(&items).enumerate() {
            // held until the next frame (the last one for `hold`)
            let next = outs.get(k + 1).map(|(n, _)| *n);
            let dur = next.map_or(hold, |n| n - *t).max(rate.frame_duration());
            let mut ti = pr.make_track_item(*item, TrackKind::Video, *t, TimeRange::new(Tick::ZERO, dur), rate).ok_or_else(|| bad("ai.restyle", "bad item"))?;
            ti.scale_to_frame = true;
            clips.push(ti);
        }
        let track_id = filmcraft_project::TrackId(pr.alloc_id());
        let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let mut track = filmcraft_project::Track::new(track_id, TrackKind::Video, format!("AI Restyle {}", q.video_tracks.len() + 1));
        st.selection = clips.iter().map(|c| c.id).collect();
        track.items = clips;
        track.sort();
        q.video_tracks.push(track);
        q.check().map_err(EngineError::Other)?;
        Ok(())
    })
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "ai.restyle",
            label: "AI Restyle…",
            menu: &[],
            shortcut: None,
            params: r#"{"prompt":str,"negativePrompt":str?,"strength":0..1?,"seed":int?,"range":"playhead|inout|clip"?,"every":frames?,"maxFrames":int?,"wait":bool?}"#,
            enabled: has_seq,
            run: restyle,
            // never replayed by crash recovery: each run is billed by a cloud provider
            journal: false,
        },
        CommandSpec {
            id: "ai.setup",
            label: "AI Provider Setup",
            menu: &[],
            shortcut: None,
            params: r#"{"provider":"fal|local"?,"endpoint":str?,"apiKey":str?}"#,
            enabled: always,
            run: setup,
            journal: false,
        },
        CommandSpec { id: "ai.status", label: "AI Status", menu: &[], shortcut: None, params: "{}", enabled: always, run: status, journal: false },
    ]
}
