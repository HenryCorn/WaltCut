//! `perf.stats`: performance counters an agent can read (headless engine part). The desktop UI
//! extends the same query with playback, frame-worker and UI timings (`ui-egui` `perf.rs`).

use serde_json::{Value, json};

use crate::Session;

/// Decoder / GOP-cache counters since the process started (they only grow: diff two readings to
/// measure an interval).
pub fn decode_json() -> Value {
    let g = waltcut_codecs::gop_stats();
    let hw = waltcut_codecs::hw::hw_stats();
    json!({
        "requests": g.hits + g.misses,
        "cacheHits": g.hits,
        "cacheMisses": g.misses,
        "cacheHitRate": g.hit_rate(),
        "seeks": g.seeks,
        "samplesDecoded": g.decoded,
        "samplesSkipped": g.skipped,
        "draftFrames": g.draft,
        "h264Threads": waltcut_codecs::video::h264_threads(),
        "evicted": g.evicted,
        // sources that hold a decoder right now (idle ones give theirs up beyond a cap)
        "liveDecoders": waltcut_codecs::live_decoders(),
        // decoded frames all sources hold, and the budget idle sources are trimmed to
        "cacheMB": waltcut_codecs::cached_bytes() as f64 / 1e6,
        "cacheBudgetMB": waltcut_codecs::FRAME_BUDGET as f64 / 1e6,
        // plane buffers of evicted frames waiting for the next decoded pictures, and how many
        // planes were decoded into a recycled buffer
        "planePoolMB": waltcut_frame::pool::stats().idle_bytes as f64 / 1e6,
        "planesReused": waltcut_frame::pool::stats().reused,
        "decodeMs": g.decode_ns as f64 / 1e6,
        "decodeMsPerSample": g.decode_ms_per_sample(),
        "framesDecoded": g.frames,
        // Settings ▸ Playback ▸ Hardware decoding: pictures from OS hardware decoders vs ours,
        // hardware decoders created, streams handed to software up front, mid-stream fallbacks.
        "hardware": {
            "enabled": waltcut_codecs::hw::hardware_decoding(),
            // the OS decoder backend registered at startup (null where there is none)
            "backend": waltcut_codecs::hw::hw_backend(),
            "frames": hw.frames,
            "softwareFrames": g.frames.saturating_sub(hw.frames),
            "sessions": hw.sessions,
            "declined": hw.declined,
            "fallbacks": hw.fallbacks,
        },
    })
}

/// Export counters: the wall
/// time of each export stage in milliseconds (`stages`: setup, render, encode with its
/// `convert` part, audio, mux, finish, wait), summed over the exports of this process (diff two readings).
/// Render and encode overlap (the next batch renders while this one is encoded), so their sum can exceed
/// the export's wall time; `waitMs` is the encoding side's idle wait for the render.
pub fn export_json() -> Value {
    let stages: serde_json::Map<String, Value> =
        waltcut_export::stage_times().into_iter().map(|(s, ns)| (format!("{}Ms", s.name()), json!(ns as f64 / 1e6))).collect();
    json!({"stages": stages})
}

/// The cel raster cache (#71): lookups answered from it (`hits`), cels extended from a cached
/// earlier version (`extended`, a stroke added), cels rasterised whole (`misses`) or not looked up
/// (`bypassed`: too small to be worth it, or too large to keep), entries evicted, and what it
/// holds against its budget. Cumulative counters: diff two readings.
pub fn drawing_cache_json() -> Value {
    let c = waltcut_render::drawing_cache::global().stats();
    json!({
        "hits": c.hits,
        "extended": c.extended,
        "misses": c.misses,
        "bypassed": c.bypassed,
        "evicted": c.evicted,
        "entries": c.entries,
        "MB": c.bytes as f64 / 1e6,
        "budgetMB": c.budget as f64 / 1e6,
    })
}

/// The engine's `perf.stats`.
pub fn stats(s: &Session) -> Value {
    let running = s.jobs.iter().filter(|j| j.result.lock().map(|r| r.is_none()).unwrap_or(false)).count();
    json!({
        "decode": decode_json(),
        "export": export_json(),
        "drawingCache": drawing_cache_json(),
        "media": {"openSources": s.media.open_sources()},
        "jobs": {"total": s.jobs.len(), "running": running},
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn perf_stats_query_reports_decode_counters() {
        let mut s = Session::default();
        assert!(s.execute("perf.stats", json!({})).is_ok(), "available without a project");
        s.execute("file.openDemoProject", json!({})).unwrap();
        let undo = s.history.undo.len();
        let v = s.execute("perf.stats", json!({})).unwrap();
        for k in [
            "requests",
            "cacheHitRate",
            "seeks",
            "samplesDecoded",
            "samplesSkipped",
            "draftFrames",
            "h264Threads",
            "decodeMs",
            "decodeMsPerSample",
            "framesDecoded",
            "liveDecoders",
            "cacheMB",
            "cacheBudgetMB",
            "planePoolMB",
            "planesReused",
        ] {
            assert!(v["decode"][k].is_number(), "decode.{k} in {v}");
        }
        for k in ["frames", "softwareFrames", "sessions", "declined", "fallbacks"] {
            assert!(v["decode"]["hardware"][k].is_number(), "decode.hardware.{k} in {v}");
        }
        assert!(v["decode"]["hardware"]["enabled"].is_boolean());
        assert!(v["decode"]["hardware"]["backend"].is_null() || v["decode"]["hardware"]["backend"].is_string());
        for k in ["setupMs", "renderMs", "encodeMs", "convertMs", "audioMs", "muxMs", "finishMs", "waitMs"] {
            assert!(v["export"]["stages"][k].is_number(), "export.stages.{k} in {v}");
        }
        for k in ["hits", "extended", "misses", "bypassed", "evicted", "entries", "MB", "budgetMB"] {
            assert!(v["drawingCache"][k].is_number(), "drawingCache.{k} in {v}");
        }
        assert!(v["media"]["openSources"].is_number());
        assert_eq!(v["jobs"]["running"], json!(0));
        assert_eq!(s.history.undo.len(), undo, "a query adds no undo step");
    }
}
