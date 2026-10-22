//! Simple, single-job sequence export shared by UI, CLI and MCP.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use waltcut_export::{ExportSettings, Format};
use waltcut_project::{ItemId, ItemKind, MediaRef};
use waltcut_time::{TICKS_PER_SECOND, Tick, TimeRange};

use crate::commands::{CommandSpec, bad, has_seq};
use crate::{EngineError, Result, Session};

pub const EXPORT_PARAMS: &str = r#"{"path":str,"format":"h264|prores|png"?,"sequence":id?,"range":"entire|inOut|workArea|custom"?,"startTime":ticks?,"endTime":ticks?,"startSeconds":f64?,"endSeconds":f64?,"startFrame":int?,"endFrame":int?,"wait":bool=false,"overwrite":bool=false}"#;

pub(crate) fn checked_keys(p: &Value, allowed: &[&str], cmd: &str) -> Result<()> {
    let object = p.as_object().ok_or_else(|| bad(cmd, "parameters must be an object"))?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(bad(cmd, format!("unsupported export parameter `{key}`")));
    }
    Ok(())
}

pub(crate) fn bool_param(p: &Value, key: &str, cmd: &str) -> Result<bool> {
    match p.get(key) {
        None => Ok(false),
        Some(v) => v.as_bool().ok_or_else(|| bad(cmd, format!("`{key}` must be a boolean"))),
    }
}

fn sequence_param(s: &Session, p: &Value, cmd: &str) -> Result<ItemId> {
    let seq = match p.get("sequence") {
        None => s.state.active_sequence.ok_or(EngineError::NoSequence)?,
        Some(v) => ItemId(v.as_u64().ok_or_else(|| bad(cmd, "sequence must be an item id"))?),
    };
    s.project.sequence(seq).ok_or_else(|| bad(cmd, "no such sequence"))?;
    Ok(seq)
}

fn bound(p: &Value, prefix: &str, rate: waltcut_time::FrameRate, cmd: &str) -> Result<Option<Tick>> {
    let mut result = None;
    for unit in ["Time", "Seconds", "Frame"] {
        let key = format!("{prefix}{unit}");
        let Some(v) = p.get(&key) else { continue };
        if result.is_some() {
            return Err(bad(cmd, format!("provide only one {prefix} time")));
        }
        let tick = match unit {
            "Seconds" => {
                let seconds = v
                    .as_f64()
                    .filter(|v| v.is_finite() && *v >= 0.0 && *v <= Tick::MAX.seconds())
                    .ok_or_else(|| bad(cmd, format!("`{key}` must be finite and within supported time bounds")))?;
                Tick::from_seconds_f64(seconds)
            }
            "Frame" => {
                let frame = v.as_u64().ok_or_else(|| bad(cmd, format!("`{key}` must be a non-negative integer")))?;
                let ticks = i128::from(frame)
                    .checked_mul(i128::from(TICKS_PER_SECOND))
                    .and_then(|v| v.checked_mul(i128::from(rate.den)))
                    .map(|v| v / i128::from(rate.num))
                    .filter(|v| *v <= i128::from(Tick::MAX.0))
                    .ok_or_else(|| bad(cmd, "frame time exceeds supported bounds"))?;
                Tick(i64::try_from(ticks).map_err(|_| bad(cmd, "frame time exceeds supported bounds"))?)
            }
            _ => Tick(
                v.as_i64()
                    .filter(|v| *v >= 0 && *v <= Tick::MAX.0)
                    .ok_or_else(|| bad(cmd, format!("`{key}` must be non-negative ticks within supported bounds")))?,
            ),
        };
        result = Some(tick);
    }
    Ok(result)
}

fn range_param(s: &Session, seq: ItemId, p: &Value, cmd: &str) -> Result<TimeRange> {
    let q = s.project.sequence(seq).ok_or(EngineError::NoSequence)?;
    q.check_bounds().map_err(|e| bad(cmd, e))?;
    for unit in ["Time", "Seconds", "Frame"] {
        if p.get(format!("start{unit}")).is_some() != p.get(format!("end{unit}")).is_some() {
            return Err(bad(cmd, "provide paired start/end bounds in the same time unit"));
        }
    }
    let a = bound(p, "start", q.settings.frame_rate, cmd)?;
    let b = bound(p, "end", q.settings.frame_rate, cmd)?;
    let mode = match p.get("range") {
        None if a.is_some() || b.is_some() => "custom",
        None => "inOut",
        Some(v) => v.as_str().ok_or_else(|| bad(cmd, "range must be entire, inOut, workArea or custom"))?,
    };
    if mode != "custom" && (a.is_some() || b.is_some()) {
        return Err(bad(cmd, "start/end parameters require a custom range"));
    }
    let range = match mode {
        "entire" => TimeRange::new(Tick::ZERO, q.duration().max(q.settings.frame_rate.frame_duration())),
        "inOut" => waltcut_export::export_range(&s.project, seq, &ExportSettings::default()).map_err(|e| bad(cmd, e.to_string()))?,
        "workArea" => q.work_area.ok_or_else(|| bad(cmd, "the sequence has no work area"))?,
        "custom" => {
            let a = a.ok_or_else(|| bad(cmd, "custom range needs a start"))?;
            let b = b.ok_or_else(|| bad(cmd, "custom range needs an end"))?;
            let duration = b.0.checked_sub(a.0).filter(|v| *v > 0).ok_or_else(|| bad(cmd, "end must be after start"))?;
            TimeRange::new(a, Tick(duration))
        }
        _ => return Err(bad(cmd, "range must be entire, inOut, workArea or custom")),
    };
    waltcut_export::validate_range(range).map_err(|e| bad(cmd, e.to_string()))?;
    let (a, b) = waltcut_export::frame_span(q.settings.frame_rate, range);
    if b.checked_sub(a).is_none_or(|n| n <= 0 || n > crate::export_output::MAX_FRAMES as i64) {
        return Err(bad(cmd, "export range must contain between 1 and 1000000 frames"));
    }
    Ok(range)
}

/// Where exports go when no path is given: next to the saved project, else ~/Movies, else the
/// home directory, else the current directory.
pub fn default_export_dir(s: &Session) -> PathBuf {
    if let Some(d) = s.path.as_deref().and_then(|p| Path::new(p).parent()).filter(|d| !d.as_os_str().is_empty()) {
        return d.to_path_buf();
    }
    if let Some(h) = crate::std_home_dir() {
        let movies = Path::new(&h).join("Movies");
        return if movies.is_dir() { movies } else { PathBuf::from(h) };
    }
    PathBuf::from(".")
}

fn file_safe(n: &str) -> String {
    let s: String = n.chars().take(200).map(|c| if c.is_alphanumeric() || " -_().".contains(c) { c } else { '_' }).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "Export".into() } else { s }
}

/// `~/` expanded to the home directory.
pub fn expand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), crate::std_home_dir()) {
        (Some(rest), Some(h)) => format!("{h}/{rest}"),
        _ => p.to_string(),
    }
}

fn output_path(s: &Session, seq: ItemId, p: &Value, format: Format, quick: bool, cmd: &str) -> Result<String> {
    let mut path = match p.get("path") {
        Some(v) => expand_home(
            v.as_str().filter(|v| !v.trim().is_empty() && v.len() <= 4096).ok_or_else(|| bad(cmd, "path must be a non-empty string of at most 4096 bytes"))?,
        ),
        None if quick => {
            let name = s.project.item(seq).map(|i| file_safe(&i.name)).unwrap_or_else(|| "Sequence".into());
            default_export_dir(s).join(format!("{name}.{}", format.extension())).to_string_lossy().to_string()
        }
        None => return Err(bad(cmd, "need `path`")),
    };
    match Path::new(&path).extension().and_then(|s| s.to_str()) {
        None => path.push_str(&format!(".{}", format.extension())),
        Some(ext) if ext.eq_ignore_ascii_case(format.extension()) => {}
        Some(_) => return Err(bad(cmd, format!("{} output requires .{}", format.label(), format.extension()))),
    }
    if path.len() > 4096 || path.contains('\0') {
        return Err(bad(cmd, "resolved export path must be at most 4096 bytes and contain no NUL"));
    }
    Ok(path)
}

fn canonical_target(path: &Path) -> std::io::Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let absolute = std::path::absolute(path)?;
            let mut parent = absolute.as_path();
            let mut tail = Vec::new();
            while !parent.exists() {
                tail.push(parent.file_name().ok_or_else(|| std::io::Error::other("invalid output path"))?.to_os_string());
                parent = parent.parent().ok_or_else(|| std::io::Error::other("invalid output parent"))?;
            }
            let mut path = parent.canonicalize()?;
            for component in tail.iter().rev() {
                path.push(component);
            }
            Ok(path)
        }
        Err(e) => Err(e),
    }
}

/// Never replace an imported source or the open project, even with explicit overwrite.
pub(crate) fn protect_source(s: &Session, path: &str) -> Result<()> {
    protect_sources(s, path, None)
}

fn protect_sources(s: &Session, path: &str, sequence: Option<&waltcut_media::sequence::Numbered>) -> Result<()> {
    if Path::new(path).components().any(|c| c == std::path::Component::ParentDir) {
        return Err(EngineError::Other("export destination must not contain parent-directory (`..`) components; use a normalized path".into()));
    }
    let output = canonical_target(Path::new(path)).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let protected = || EngineError::Other("export destination is an imported source or the open project; choose another path".into());
    if let Some(path) = &s.path
        && canonical_target(Path::new(path)).map_err(|e| EngineError::Other(e.to_string()))? == output
    {
        return Err(protected());
    }
    for item in s.project.items.values() {
        if let ItemKind::Media(media) = &item.kind
            && let MediaRef::File { path } = &media.media
        {
            let source = canonical_target(Path::new(path)).map_err(|e| EngineError::Other(e.to_string()))?;
            if source == output {
                return Err(protected());
            }
            if let Some(sequence) = sequence
                && source.parent() == output.parent()
                && source.file_name().and_then(|s| s.to_str()).and_then(|s| sequence.number_of(s)).is_some()
            {
                return Err(protected());
            }
            if media.info.kind == waltcut_media::MediaKind::ImageSequence
                && let Some(numbered) = waltcut_media::sequence::Numbered::parse(path)
                && source.parent() == output.parent()
                && (output.file_name().and_then(|s| s.to_str()).and_then(|s| numbered.number_of(s)).is_some()
                    || sequence.is_some_and(|pattern| pattern.prefix == numbered.prefix && pattern.suffix.eq_ignore_ascii_case(&numbered.suffix)))
            {
                return Err(EngineError::Other("export destination belongs to an imported image sequence; choose another path".into()));
            }
        }
    }
    Ok(())
}

/// Own the thread until completion; dropping a session cancels and joins its export.
pub(crate) struct ExportWorker {
    progress: Arc<waltcut_export::Progress>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ExportWorker {
    fn drop(&mut self) {
        self.progress.cancel.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            log::error!("export worker escaped its panic guard");
        }
    }
}

pub(crate) fn poll(s: &mut Session) {
    if s.export_worker.as_ref().is_some_and(|w| w.handle.as_ref().is_none_or(|h| h.is_finished())) {
        s.export_worker.take();
    }
}

pub fn export_media(s: &mut Session, p: &Value) -> Result<Value> {
    run_export(s, p, false)
}

fn quick(s: &mut Session, p: &Value) -> Result<Value> {
    run_export(s, p, true)
}

fn run_export(s: &mut Session, p: &Value, quick: bool) -> Result<Value> {
    let cmd = if quick { "export.quick" } else { "file.exportMedia" };
    checked_keys(
        p,
        &["path", "format", "sequence", "range", "startTime", "endTime", "startSeconds", "endSeconds", "startFrame", "endFrame", "wait", "overwrite"],
        cmd,
    )?;
    let wait = bool_param(p, "wait", cmd)?;
    let overwrite = bool_param(p, "overwrite", cmd)?;
    let seq = sequence_param(s, p, cmd)?;
    let format = match p.get("format") {
        None => Format::H264,
        Some(v) => v.as_str().and_then(Format::from_name).ok_or_else(|| bad(cmd, "format must be h264, prores or png"))?,
    };
    let range = range_param(s, seq, p, cmd)?;
    let sequence_settings = &s.project.sequence(seq).ok_or(EngineError::NoSequence)?.settings;
    ExportSettings { format, ..Default::default() }
        .resolve(sequence_settings.width, sequence_settings.height, sequence_settings.frame_rate, sequence_settings.sample_rate)
        .map_err(|e| bad(cmd, e.to_string()))?;
    let path = output_path(s, seq, p, format, quick, cmd)?;
    let (start, end) = waltcut_export::frame_span(s.project.sequence(seq).ok_or(EngineError::NoSequence)?.settings.frame_rate, range);
    let count = u64::try_from(end.checked_sub(start).ok_or_else(|| bad(cmd, "invalid frame count"))?).map_err(|_| bad(cmd, "invalid frame count"))?;
    let sequence_pattern =
        if format.is_image_sequence() { waltcut_media::sequence::Numbered::parse(&waltcut_export::image_sequence_path(&path, 0, count)) } else { None };
    protect_sources(s, &path, sequence_pattern.as_ref())?;
    poll(s);
    if s.export_worker.is_some() {
        return Err(bad(cmd, "an export is already active; use jobs.cancel and wait for it to finish"));
    }
    let project = s.project.clone();
    let sequence = format.is_image_sequence().then_some(count);
    if sequence.is_some() && overwrite {
        return Err(bad(cmd, "PNG sequences require an unused filename base; overwrite is not supported"));
    }
    let mut output = crate::export_output::ExportOutput::new(Path::new(&path), sequence, overwrite).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let settings = ExportSettings {
        format,
        path: output.staged_path().to_string_lossy().into_owned(),
        range: Some(range),
        gpu_rendering: waltcut_export::GpuRendering::Auto,
        ..Default::default()
    };
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0).checked_add(1).ok_or_else(|| bad(cmd, "job id space exhausted"))?;
    let job = crate::Job {
        id,
        label: format!("Export {}", Path::new(&path).file_name().unwrap_or_default().to_string_lossy()),
        progress: Arc::default(),
        result: Arc::default(),
    };
    let provider = s.media.provider(project.clone(), s.services.clone()).recording();
    let (progress, result) = (job.progress.clone(), job.result.clone());
    let offline = Arc::new(std::sync::Mutex::new(Vec::new()));
    let used = offline.clone();
    let destination = path.clone();
    let run = guard_job(progress.clone(), result.clone(), move || {
        *progress.status.lock().unwrap_or_else(|e| e.into_inner()) = "Checking destination filenames".into();
        let r = output
            .prepare(&progress.cancel)
            .map_err(|e| e.to_string())
            .and_then(|()| waltcut_export::export(&project, seq, &settings, &provider, &progress).map_err(|e| e.to_string()))
            .and_then(|mut report| {
                *progress.status.lock().unwrap_or_else(|e| e.into_inner()) = "Finalizing output".into();
                output.commit(&progress.cancel).map_err(|e| e.to_string())?;
                report.path = destination;
                Ok(report)
            });
        let cleanup = output.cleanup().map_err(|error| error.to_string());
        let r = match (r, cleanup) {
            (result, Ok(())) => result,
            (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => Err(format!("{error}; {cleanup}")),
        };
        drop(output);
        let offline = provider.offline_used();
        let offline_count = offline.len();
        *used.lock().unwrap_or_else(|e| e.into_inner()) = offline;
        match &r {
            Ok(_) => {
                *progress.status.lock().unwrap_or_else(|e| e.into_inner()) = if offline_count == 0 {
                    "Export complete".into()
                } else {
                    format!("Export complete with {offline_count} offline item(s) rendered as placeholders; relink and export again")
                }
            }
            Err(e) => {
                *progress.status.lock().unwrap_or_else(|e| e.into_inner()) = format!("Export failed: {e}");
                *progress.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.clone());
            }
        }
        *result.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
        progress.finished.store(true, Ordering::Release);
    });
    if wait {
        run();
    } else {
        let handle =
            std::thread::Builder::new().name("waltcut-export".into()).spawn(run).map_err(|e| EngineError::Other(format!("cannot start export worker: {e}")))?;
        s.export_worker = Some(ExportWorker { progress: job.progress.clone(), handle: Some(handle) });
    }
    // Keep bounded history, never discard another subsystem's jobs.
    let finished_exports: Vec<u64> =
        s.jobs.iter().filter(|j| s.export_offline.contains_key(&j.id) && j.result.lock().unwrap_or_else(|e| e.into_inner()).is_some()).map(|j| j.id).collect();
    let remove: HashSet<u64> = finished_exports.iter().take(finished_exports.len().saturating_sub(31)).copied().collect();
    s.jobs.retain(|j| !remove.contains(&j.id));
    s.export_offline.retain(|id, _| !remove.contains(id));
    s.export_offline.insert(id, offline);
    s.jobs.push(job);
    let mut out = json!({"job": id, "path": path, "poll": "jobs.list", "cancel": "jobs.cancel"});
    if wait && let Some(job) = s.jobs.iter().find(|j| j.id == id) {
        let state = job_json(s, job);
        if let Some(error) = state["result"]["error"].as_str() {
            return Err(EngineError::Other(format!("export failed: {error}")));
        }
        out["result"] = state["result"].clone();
        out["offlineMedia"] = state["offlineMedia"].clone();
    }
    Ok(out)
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "export.quick",
        label: "Quick Export",
        menu: &[],
        shortcut: None,
        params: EXPORT_PARAMS,
        enabled: has_seq,
        run: quick,
        journal: false,
    }]
}

/// A `jobs.list` entry: [`crate::Job::to_json`], plus `offlineMedia` for an export job, so callers
/// that poll an export (the bridge, MCP long exports) can report it like `file.exportMedia` does.
pub(crate) fn job_json(s: &Session, j: &crate::Job) -> Value {
    let mut v = j.to_json();
    if s.export_offline.contains_key(&j.id) {
        v["offlineMedia"] = offline_media_json(s, j.id);
    }
    v
}

/// The offline items export job `job` drew the slate for, as JSON (`[]` while it runs or when none).
fn offline_media_json(s: &Session, job: u64) -> Value {
    let list = s.export_offline.get(&job).map(|o| o.lock().unwrap_or_else(|e| e.into_inner()).clone()).unwrap_or_default();
    serde_json::to_value(list).unwrap_or_else(|_| json!([]))
}

/// Last-resort guard for a background job body (export, previews, …): a panic marks the
/// job failed with an error instead of killing its thread and leaving the job "running" forever.
/// The panic hook has already logged where it happened.
pub(crate) fn guard_job(
    progress: Arc<waltcut_export::Progress>,
    result: Arc<std::sync::Mutex<Option<std::result::Result<waltcut_export::Report, String>>>>,
    run: impl FnOnce() + Send + 'static,
) -> impl FnOnce() + Send + 'static {
    move || {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).is_err() {
            let msg = "internal error (see the crash log); the job was stopped".to_string();
            *progress.error.lock().unwrap_or_else(|x| x.into_inner()) = Some(msg.clone());
            *progress.status.lock().unwrap_or_else(|x| x.into_inner()) = format!("Export failed: {msg}");
            *result.lock().unwrap_or_else(|x| x.into_inner()) = Some(Err(msg));
            progress.finished.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    /// A panicking job body must leave the job finished with an error, not "running" forever.
    #[test]
    fn panicking_job_is_reported_as_failed() {
        let progress: Arc<waltcut_export::Progress> = Arc::default();
        let result: Arc<std::sync::Mutex<Option<std::result::Result<waltcut_export::Report, String>>>> = Arc::default();
        let run = guard_job(progress.clone(), result.clone(), || panic!("job bug"));
        std::thread::spawn(run).join().expect("the guard catches the panic");
        assert!(progress.finished.load(Ordering::Relaxed));
        assert!(progress.error.lock().unwrap().is_some());
        assert!(matches!(&*result.lock().unwrap(), Some(Err(_))));
    }
}
