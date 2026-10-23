//! Render-bar segments: where a sequence's video changes structure, what each stretch costs to
//! play, and a content hash that names its render preview.
//!
//! # Segments
//! The timeline is cut at every edge of every clip on the enabled video tracks
//! (snapped up to the sequence frame grid). Between two cuts the *structure* of the picture is
//! constant: the same clips are visible on the same tracks. Each such stretch with
//! at least one visible layer is a [`Segment`]; gaps produce no segment (no bar, nothing to render).
//!
//! # Content hash
//! [`Segment::hash`] is a 128-bit FNV-1a hash of everything that affects the segment's pixels,
//! expressed *relative to the segment start*: sequence settings, the frame count, and for each
//! visible layer (bottom to top, with its track index) the track item (source in, speed, reverse,
//! frame hold, effects with every keyframe…, its timeline offset from the segment start) and its
//! source (the media reference and stream info, interpretation, nested sequences recursively,
//! drawing settings). Ids, names, labels, markers, link/group membership and source
//! In/Out marks are excluded because they don't change pixels. Because positions are relative, a
//! ripple edit that moves a segment along the timeline keeps its hash (and its preview); only the
//! segments whose content changed get a new hash. The hash is computed from the serde JSON
//! form of those values, so it is stable across save/load and across processes.
//!
//! # Cost estimate (yellow vs red)
//! [`Segment::cost_ms`] estimates the milliseconds one frame takes on the playback path, at full
//! sequence resolution, as the sum over visible layers of:
//! * **decode**: per source codec at 1080p — H.264 3 ms, HEVC 6 ms, ProRes 2.5 ms, MJPEG 4 ms,
//!   other files 4 ms, stills/generators 1 ms — scaled by source pixels / 1080p pixels;
//! * **layer path**: drawing, graphic and nested-sequence layers pay a CPU rasterisation
//!   estimate of 10 ms at 1080p, scaled by sequence pixels; media composition pays 0.5 ms.
//!
//! A segment is **red** when the estimate exceeds [`REALTIME_BUDGET`] × the frame duration,
//! **yellow** when it plays in real time but is not "native", and has **no bar** when it is a single
//! clip with untouched Motion/Opacity, normal speed, and a source whose frame
//! size and rate match the sequence. Green (a valid preview exists) is decided by the preview
//! store, which knows which hashes have files.

use serde::Serialize;
use waltcut_project::{ClipId, ItemId, ItemKind, Project, Sequence, TrackItem, TrackKind};
use waltcut_time::{FrameRate, Tick};

/// Bump when the hash layout or the preview format changes (invalidates every preview).
pub const HASH_VERSION: &str = "waltcut-preview-v1";

/// Multiple of the frame duration a frame may take before a segment is marked red. Playback
/// renders ahead on several worker threads (2–6), so a frame can take somewhat longer than its
/// display duration and still keep up.
///
/// Calibration (M4 Pro, shared with other jobs; 1080p 23.976 sequence, Full playback resolution,
/// 8 s of playback ≈ 192 frames, counted by the Program monitor's dropped-frame stats):
/// * 1080p H.264 High clip, no effects (no bar, est. 3.5 ms): 11 dropped;
/// * same clip + Sharpen, Levels, Tint (red, est. 183 ms): 189–191 dropped, ≤ 17 on time;
/// * after Render Effects In to Out (green; ProRes preview rendered at ~3 fps): 25–46 dropped on
///   the first play right after the render (frame workers still busy with live renders queued
///   before it), then 0–1 dropped on every later play.
pub const REALTIME_BUDGET: f64 = 1.5;

const HD_PIXELS: f64 = 1920.0 * 1080.0;

/// How a segment plays without a preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Need {
    /// Plays natively (no bar).
    None,
    /// Should play in real time (yellow).
    Realtime,
    /// Probably drops frames (red).
    Render,
}

/// A stretch of the timeline with a constant set of visible layers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Segment {
    /// First frame (sequence frame index) and number of frames.
    pub first_frame: i64,
    pub frames: i64,
    /// Timeline range (frame-aligned).
    pub start: Tick,
    pub end: Tick,
    /// Content hash (32 hex digits) naming the preview file.
    pub hash: String,
    pub need: Need,
    /// Estimated per-frame playback cost at full resolution, in milliseconds.
    pub cost_ms: f64,
    /// Visible clips (including both sides of transitions), for Render Selection.
    pub clips: Vec<ClipId>,
}

impl Segment {
    pub fn contains_frame(&self, f: i64) -> bool {
        f >= self.first_frame && f < self.first_frame + self.frames
    }
}

/// First frame whose start is at or after `t`.
fn frame_ceil(rate: FrameRate, t: Tick) -> i64 {
    let f = rate.frame_at(t);
    if rate.tick_of(f) < t { f + 1 } else { f }
}

type Layer<'a> = (usize, &'a TrackItem);

fn layers_at<'a>(seq: &'a Sequence, t: Tick) -> Vec<Layer<'a>> {
    let mut out = Vec::new();
    for (ti, tr) in seq.video_tracks.iter().enumerate() {
        if !tr.enabled {
            continue;
        }
        if let Some(it) = tr.item_at(t).filter(|i| i.enabled) {
            out.push((ti, it));
        }
    }
    out
}

/// Compute the render-bar segments of sequence `seq_id`.
pub fn video_segments(project: &Project, seq_id: ItemId) -> Vec<Segment> {
    let Some(seq) = project.sequence(seq_id) else { return Vec::new() };
    let rate = seq.settings.frame_rate;
    let mut cuts: Vec<i64> = Vec::new();
    for tr in seq.video_tracks.iter().filter(|t| t.enabled) {
        for it in &tr.items {
            cuts.push(frame_ceil(rate, it.start));
            cuts.push(frame_ceil(rate, it.end()));
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let (fa, fb) = (w[0], w[1]);
        if fb <= fa {
            continue;
        }
        let t = rate.tick_of(fa);
        let layers = layers_at(seq, t);
        if layers.is_empty() {
            continue;
        }
        let mut h = Fnv128::new();
        h.json(&HASH_VERSION);
        h.json(&seq.settings);
        h.json(&(fb - fa));
        let mut clips = Vec::new();
        for (ti, it) in &layers {
            h.json(&("item", ti));
            hash_item(&mut h, project, it, t, TrackKind::Video, 0);
            clips.push(it.id);
        }
        let cost_ms = estimate_cost(project, seq, &layers);
        let budget = rate.frame_duration().seconds() * 1000.0 * REALTIME_BUDGET;
        let need = if cost_ms > budget {
            Need::Render
        } else if layers.len() == 1 && native(project, seq, layers[0].1) {
            Need::None
        } else {
            Need::Realtime
        };
        out.push(Segment { first_frame: fa, frames: fb - fa, start: t, end: rate.tick_of(fb), hash: h.hex(), need, cost_ms, clips });
    }
    out
}

/// The segment containing frame `f` (segments are sorted and disjoint).
pub fn segment_at(segments: &[Segment], f: i64) -> Option<&Segment> {
    let i = segments.partition_point(|s| s.first_frame <= f);
    i.checked_sub(1).map(|i| &segments[i]).filter(|s| s.contains_frame(f))
}

/// A stretch of sequence audio with constant structure (Render Audio), in sequence samples.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AudioSegment {
    pub first_sample: i64,
    pub samples: i64,
    pub hash: String,
}

/// Audio segments of sequence `seq_id`, cut at clip boundaries. Hash the source, clip Volume
/// automation and gain, plus track mute/enabled state, relative to the segment start.
pub fn audio_segments(project: &Project, seq_id: ItemId) -> Vec<AudioSegment> {
    let Some(seq) = project.sequence(seq_id) else { return Vec::new() };
    let sr = seq.settings.sample_rate.max(1) as i64;
    let ceil = |t: Tick| {
        let s = t.to_units_floor(sr);
        if Tick::from_units(s, sr) < t { s + 1 } else { s }
    };
    let mut cuts: Vec<i64> = Vec::new();
    for tr in &seq.audio_tracks {
        for it in &tr.items {
            cuts.push(ceil(it.start));
            cuts.push(ceil(it.end()));
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut global = Fnv128::new();
    global.json(&(HASH_VERSION, "audio", sr));
    for (ti, tr) in seq.audio_tracks.iter().enumerate() {
        global.json(&(ti, tr.muted, tr.enabled, tr.channels));
    }
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b <= a {
            continue;
        }
        let t = Tick::from_units(a, sr);
        let mut h = Fnv128(global.0);
        h.json(&(b - a));
        let mut heard = false;
        for (ti, tr) in seq.audio_tracks.iter().enumerate() {
            if tr.muted || !tr.enabled {
                continue;
            }
            if let Some(it) = tr.item_at(t).filter(|i| i.enabled) {
                h.json(&("item", ti));
                hash_item(&mut h, project, it, t, TrackKind::Audio, 0);
                heard = true;
            }
        }
        if heard {
            out.push(AudioSegment { first_sample: a, samples: b - a, hash: h.hex() });
        }
    }
    out
}

/// Hash a clip and its source as heard (`kind` Audio) or seen (`kind` Video): a nested sequence
/// contributes only the tracks of that kind, so editing its picture keeps its sound's previews.
fn hash_item(h: &mut Fnv128, project: &Project, it: &TrackItem, origin: Tick, kind: TrackKind, depth: u32) {
    let mut c = it.clone();
    c.id = Default::default();
    c.start -= origin;
    c.label = waltcut_project::Label::ALL[0];
    c.markers.clear();
    c.link = None;
    c.group = None;
    c.name.clear();
    h.json(&c);
    hash_source(h, project, it.item, kind, depth);
}

fn hash_source(h: &mut Fnv128, project: &Project, id: ItemId, kind: TrackKind, depth: u32) {
    let Some(pi) = project.item(id) else {
        h.json(&"missing");
        return;
    };
    if depth > 8 {
        h.json(&"deep");
        return;
    }
    match &pi.kind {
        ItemKind::Media(m) => {
            let mut c = m.clone();
            c.mark_in = None;
            c.mark_out = None;
            c.markers.clear();
            h.json(&("media", &c));
            if matches!(c.media, waltcut_project::MediaRef::Generator(_)) {
                // generators may draw their name (demo footage)
                h.json(&pi.name);
            }
        }
        ItemKind::Subclip { parent, range, .. } => {
            h.json(&("subclip", range));
            hash_source(h, project, *parent, kind, depth + 1);
        }
        ItemKind::Graphic { .. } => h.json(&("graphic", &pi.kind)),
        ItemKind::Drawing(_) => h.json(&("drawing", &pi.kind)),
        ItemKind::Sequence(s) => {
            h.json(&("sequence", &s.settings));
            if kind == TrackKind::Audio {
                // the nest's sound is the nested sequence's whole mix (see `audio_segments`)
                for (ti, tr) in s.audio_tracks.iter().enumerate() {
                    h.json(&("track", ti, tr.muted, tr.enabled, tr.channels, tr.items.len()));
                    for it in &tr.items {
                        hash_item(h, project, it, Tick::ZERO, kind, depth + 1);
                    }
                }
                return;
            }
            for (ti, tr) in s.video_tracks.iter().enumerate() {
                h.json(&("track", ti, tr.enabled, tr.items.len()));
                for it in &tr.items {
                    hash_item(h, project, it, Tick::ZERO, kind, depth + 1);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- cost

fn decode_cost_ms(project: &Project, id: ItemId, depth: u32) -> f64 {
    let Some(pi) = project.item(id) else { return 0.0 };
    match &pi.kind {
        ItemKind::Media(m) => {
            let Some(v) = &m.info.video else { return 0.0 };
            let px = (v.width as f64 * v.height as f64) / HD_PIXELS;
            let c = v.codec.to_ascii_lowercase();
            let base = if matches!(m.info.kind, waltcut_media::MediaKind::Still | waltcut_media::MediaKind::Synthetic)
                || matches!(m.media, waltcut_project::MediaRef::Generator(_))
            {
                1.0
            } else if c.contains("264") || c.contains("avc") {
                3.0
            } else if c.contains("hevc") || c.contains("265") {
                6.0
            } else if c.contains("prores") {
                2.5
            } else {
                // MJPEG and anything else
                4.0
            };
            base * px
        }
        ItemKind::Subclip { parent, .. } => decode_cost_ms(project, *parent, depth + 1),
        // vector layers are drawn on the CPU (cached while static)
        ItemKind::Graphic { .. } => 1.5,
        ItemKind::Drawing(_) => 1.0,
        ItemKind::Sequence(s) => {
            if depth > 8 {
                return 0.0;
            }
            // a nested sequence renders its worst segment on the CPU
            let id_cost = s
                .video_tracks
                .iter()
                .filter(|t| t.enabled)
                .flat_map(|t| t.items.iter())
                .map(|it| decode_cost_ms(project, it.item, depth + 1))
                .fold(0.0, f64::max);
            10.0 + id_cost
        }
    }
}

fn cpu_layer(project: &Project, it: &TrackItem) -> bool {
    let nested = project.item(it.item).is_some_and(|p| matches!(p.kind, ItemKind::Sequence(_)));
    let drawn = project.item(it.item).is_some_and(|p| matches!(p.kind, ItemKind::Graphic { .. } | ItemKind::Drawing(_)));
    drawn || nested
}

fn layer_cost(project: &Project, it: &TrackItem, px: f64) -> f64 {
    let decode = decode_cost_ms(project, it.item, 0);
    let path = if cpu_layer(project, it) { 10.0 * px } else { 0.5 };
    decode + path
}

fn estimate_cost(project: &Project, seq: &Sequence, layers: &[Layer]) -> f64 {
    let px = (seq.settings.width as f64 * seq.settings.height as f64) / HD_PIXELS;
    layers.iter().map(|(_, it)| layer_cost(project, it, px)).sum()
}

/// A single clip that plays natively: untouched intrinsic effects, normal speed,
/// source frame size and rate equal to the sequence's.
fn native(project: &Project, seq: &Sequence, it: &TrackItem) -> bool {
    if it.has_modified_intrinsics() || (it.speed - 1.0).abs() > 1e-9 || it.reverse || it.frame_hold.is_some() {
        return false;
    }
    match project.item(it.item).map(|p| &p.kind) {
        Some(ItemKind::Media(m)) => {
            m.info.video.as_ref().is_some_and(|v| v.width == seq.settings.width && v.height == seq.settings.height && m.frame_rate() == seq.settings.frame_rate)
        }
        _ => false,
    }
}

// ---------------------------------------------------------------- hashing

/// 128-bit FNV-1a over the serde JSON form of values.
struct Fnv128(u128);

impl Fnv128 {
    const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013B;
    fn new() -> Self {
        Self(Self::OFFSET)
    }
    fn json<T: Serialize + ?Sized>(&mut self, v: &T) {
        let _ = serde_json::to_writer(&mut *self, v);
        self.0 ^= 0xff; // value separator
        self.0 = self.0.wrapping_mul(Self::PRIME);
    }
    fn hex(&self) -> String {
        format!("{:032x}", self.0)
    }
}

impl std::io::Write for Fnv128 {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for &b in buf {
            self.0 ^= b as u128;
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
