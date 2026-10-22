//! Long exports over MCP (docs/agents.md § Long exports): `file.exportMedia` with `wait: true`
//! runs as a background engine job; the server reports its progress as MCP
//! `notifications/progress` (when the request carried a `progressToken`) and stops it on
//! `notifications/cancelled`, awaiting the engine's ownership-aware staging cleanup. A late
//! cancellation reports a committed output rather than claiming it was removed. The session lock
//! is only held for short polls, so other requests are answered while the export runs.

use std::time::Duration;

use rmcp::RoleServer;
use rmcp::model::{CallToolResult, ContentBlock as Content, ProgressNotificationParam, ProgressToken};
use rmcp::service::{Peer, RequestContext};
use serde_json::{Value, json};

use crate::server::FilmcraftMcp;

/// Engine commands that block until a render is written when called with `wait: true`.
pub const LONG_COMMANDS: &[&str] = &["file.exportMedia", "export.quick"];

/// How often the job is polled (and at most how often progress is reported).
const POLL: Duration = Duration::from_millis(100);

/// Whether `command_run {id, params}` is a long call this module runs.
pub fn is_long(id: &str, params: &Value) -> bool {
    LONG_COMMANDS.contains(&id) && params.get("wait").and_then(Value::as_bool) == Some(true)
}

/// Sends `notifications/progress` for one request; progress only ever increases.
struct Reporter {
    peer: Peer<RoleServer>,
    token: Option<ProgressToken>,
    last: f64,
}

impl Reporter {
    async fn report(&mut self, done: f64, total: f64, message: &str) {
        let Some(token) = self.token.clone() else { return };
        if total <= 0.0 || done <= self.last {
            return;
        }
        self.last = done;
        let mut p = ProgressNotificationParam::new(token, done).with_total(total);
        if !message.is_empty() {
            p = p.with_message(message);
        }
        let _ = self.peer.notify_progress(p).await;
    }
}

impl FilmcraftMcp {
    /// Run `id` (a [`LONG_COMMANDS`] entry, `wait: true`) as a job, reporting progress and
    /// honouring cancellation.
    pub(crate) async fn run_long(&self, id: &str, mut params: Value, context: &RequestContext<RoleServer>) -> CallToolResult {
        if let Some(p) = params.as_object_mut() {
            p.insert("wait".into(), json!(false));
        }
        let start = match self.run(id, params).await {
            Ok(v) => v,
            Err(e) => return CallToolResult::error(vec![Content::text(e.to_string())]),
        };
        let Some(job) = start.get("job").and_then(Value::as_u64) else {
            return CallToolResult::error(vec![Content::text(format!("{id} started no job: {start}"))]);
        };
        let mut rep = Reporter { peer: context.peer.clone(), token: context.meta.get_progress_token(), last: 0.0 };
        loop {
            let state = match self.job_state(job).await {
                Ok(Some(state)) => state,
                Ok(None) => return CallToolResult::error(vec![Content::text(format!("export job {job} is missing; no successful result was received"))]),
                Err(error) => {
                    let cleanup = self.cancel_and_wait(job).await;
                    return CallToolResult::error(vec![Content::text(format!("cannot poll export job {job}: {error}; cancellation: {cleanup:?}"))]);
                }
            };
            let (done, total) = (state["done"].as_f64().unwrap_or(0.0), state["total"].as_f64().unwrap_or(0.0));
            rep.report(done, total, state["status"].as_str().unwrap_or_default()).await;
            if state["finished"].as_bool() == Some(true) {
                let offline = state.get("offlineMedia").cloned();
                let result = state["result"].clone();
                if !result.is_object() {
                    return CallToolResult::error(vec![Content::text(format!("export job {job} finished without a valid result"))]);
                }
                if let Some(e) = result.get("error").and_then(Value::as_str) {
                    return CallToolResult::error(vec![Content::text(format!("export failed: {e}"))]);
                }
                if !successful_report(&result) {
                    return CallToolResult::error(vec![Content::text(format!("export job {job} finished without a valid output report"))]);
                }
                let mut out = start;
                if let Some(o) = out.as_object_mut() {
                    o.insert("result".into(), result);
                    if let Some(off) = offline {
                        o.insert("offlineMedia".into(), off);
                    }
                }
                return match serde_json::to_string_pretty(&out) {
                    Ok(text) => CallToolResult::success(vec![Content::text(text)]),
                    Err(error) => CallToolResult::error(vec![Content::text(format!("cannot serialize export job {job}: {error}"))]),
                };
            }
            tokio::select! {
                _ = context.ct.cancelled() => break,
                _ = tokio::time::sleep(POLL) => {}
            }
        }
        // Cancelled: stop the encode at the next batch, wait for the worker, let the engine discard only this job's staging files.
        let message = match self.cancel_and_wait(job).await {
            Ok(message) | Err(message) => message,
        };
        CallToolResult::error(vec![Content::text(message)])
    }

    async fn cancel_and_wait(&self, job: u64) -> Result<String, String> {
        self.run("jobs.cancel", json!({"job": job})).await.map_err(|e| format!("cannot cancel export job {job}: {e}"))?;
        for _ in 0..600 {
            let state = self.job_state(job).await?.ok_or_else(|| format!("export job {job} disappeared while awaiting cancellation"))?;
            if state["finished"].as_bool() == Some(true) {
                return cancellation_result(job, &state);
            }
            tokio::time::sleep(POLL).await;
        }
        Err(format!("cancellation requested for export job {job}, but it has not stopped after 60 seconds; inspect jobs.list"))
    }

    /// The job's `jobs.list` entry.
    async fn job_state(&self, job: u64) -> Result<Option<Value>, String> {
        let jobs = self.run("jobs.list", json!({})).await.map_err(|e| e.to_string())?;
        let jobs = jobs.as_array().ok_or_else(|| "jobs.list returned no job array".to_string())?;
        Ok(jobs.iter().find(|j| j["id"].as_u64() == Some(job)).cloned())
    }
}

fn successful_report(result: &Value) -> bool {
    result.get("error").is_none()
        && result["path"].as_str().is_some_and(|path| !path.is_empty())
        && result["frames"].as_u64().is_some_and(|frames| (1..=1_000_000).contains(&frames))
}

fn cancellation_result(job: u64, state: &Value) -> Result<String, String> {
    let result = &state["result"];
    match result["error"].as_str() {
        Some("cancelled") => Ok("cancelled; the export worker stopped and discarded its staging files".into()),
        Some(error) => Err(format!("export job {job} stopped with an error: {error}")),
        None if successful_report(result) => Ok(format!("export completed before cancellation; output retained at {}", result["path"])),
        None => Err(format!("export job {job} finished without a valid result; inspect jobs.list")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_retained_waited_exports_use_the_long_job_adapter() {
        for command in ["file.exportMedia", "export.quick"] {
            assert!(is_long(command, &json!({"wait":true})));
            for params in [json!({}), json!({"wait":false}), json!({"wait":"true"}), Value::Null] {
                assert!(!is_long(command, &params));
            }
        }
        for command in ["export.queue.start", "export.queue.add", "file.exportFrame", "export.resolve"] {
            assert!(!is_long(command, &json!({"wait":true})));
        }
    }

    #[test]
    fn cancellation_reports_cleanup_errors_and_late_completion_truthfully() {
        let cancelled = cancellation_result(7, &json!({"result":{"error":"cancelled"}})).unwrap();
        assert!(cancelled.contains("stopped and discarded"));
        let failed = cancellation_result(7, &json!({"result":{"error":"cancelled; could not remove export staging directory"}})).unwrap_err();
        assert!(failed.contains("could not remove"), "{failed}");
        let completed = cancellation_result(7, &json!({"result":{"path":"kept.mov","frames":6}})).unwrap();
        assert!(completed.contains("output retained at \"kept.mov\""), "{completed}");
        assert!(!completed.contains("discarded"));
        for result in
            [Value::Null, json!({}), json!({"path":"kept.mov"}), json!({"path":"kept.mov","frames":0}), json!({"path":"kept.mov","frames":6,"error":false})]
        {
            assert!(!successful_report(&result));
            assert!(cancellation_result(7, &json!({"result":result})).is_err());
        }
    }
}
