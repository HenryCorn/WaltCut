//! Retained mono/stereo soundtrack playback and waveform helpers.

use waltcut_frame::AudioBuffer;
use waltcut_project::{Project, Sequence, TrackItem};
use waltcut_time::{Tick, TimeRange};

use crate::{MAX_NEST_DEPTH, SourceProvider};

/// Mix the enabled, unmuted audio tracks to stereo. Track order does not affect the sum.
pub fn mix_sequence(project: &Project, seq: &Sequence, start: i64, frames: usize, sources: &dyn SourceProvider) -> AudioBuffer {
    mix_sequence_at(project, seq, start, frames, sources, 0)
}

fn mix_sequence_at(project: &Project, seq: &Sequence, start: i64, frames: usize, sources: &dyn SourceProvider, depth: u32) -> AudioBuffer {
    let sr = seq.settings.sample_rate.max(1);
    let Ok(frames_i64) = i64::try_from(frames) else { return AudioBuffer::silence(sr, 2, 0) };
    let Some(end) = start.checked_add(frames_i64) else { return AudioBuffer::silence(sr, 2, 0) };
    let mut out = AudioBuffer::silence(sr, 2, frames);
    let range = TimeRange::from_bounds(Tick::from_units(start, i64::from(sr)), Tick::from_units(end, i64::from(sr)));
    for track in &seq.audio_tracks {
        if track.muted || !track.enabled {
            continue;
        }
        for item in track.items.iter().filter(|item| item.enabled && item.range().overlaps(&range)) {
            mix_item(project, seq, item, start, frames, sources, &mut out, depth);
        }
    }
    out
}

fn mix_item(project: &Project, seq: &Sequence, item: &TrackItem, start: i64, frames: usize, sources: &dyn SourceProvider, out: &mut AudioBuffer, depth: u32) {
    let sr = seq.settings.sample_rate.max(1);
    let Ok(frames_i64) = i64::try_from(frames) else { return };
    let Some(end) = start.checked_add(frames_i64) else { return };
    let first = start.max(item.start.to_units_floor(i64::from(sr)));
    let last = end.min(item.end().to_units_floor(i64::from(sr)));
    if last <= first {
        return;
    }
    let n = (last - first) as usize;
    let offset = (first - start) as usize;
    if let Some(nested) = project.sequence(item.item) {
        if depth >= MAX_NEST_DEPTH {
            return;
        }
        let read = |at: i64, count: usize| nested_audio(project, nested, at, count, sr, sources, depth + 1);
        mix_source(item, first, n, sr, &read, out, offset);
    } else {
        let Some(source) = sources.source(item.item) else { return };
        if !source.info().has_audio() {
            return;
        }
        let stream = item.audio_stream;
        let read = |at: i64, count: usize| source.audio_stream(stream, at, count, sr).ok();
        mix_source(item, first, n, sr, &read, out, offset);
    }
}

fn mix_source(item: &TrackItem, first: i64, frames: usize, sr: u32, read: &impl Fn(i64, usize) -> Option<AudioBuffer>, out: &mut AudioBuffer, offset: usize) {
    let speed = item.speed.abs();
    if !speed.is_finite() || speed <= 0.0 || item.gain_db.is_nan() {
        return;
    }
    let source_at = item.source_time_at(Tick::from_units(first, i64::from(sr))).to_units_floor(i64::from(sr));
    let Some(span_f) = ((frames.saturating_sub(1) as f64) * speed).ceil().is_finite().then_some(((frames.saturating_sub(1) as f64) * speed).ceil()) else {
        return;
    };
    let Some(span) = usize::try_from(span_f as u64).ok().and_then(|n| n.checked_add(2)) else { return };
    if span > 1_048_576 {
        return;
    }
    let source_start = if item.reverse { source_at.saturating_sub(i64::try_from(span.saturating_sub(1)).unwrap_or(i64::MAX)) } else { source_at };
    let Some(input) = read(source_start, span) else { return };
    let available = input.channels.len();
    if available == 0 {
        return;
    }
    let (left, right) = source_pair(item, available);
    let left_samples = input.channels.get(left);
    let right_samples = input.channels.get(right);
    let (Some(left_samples), Some(right_samples)) = (left_samples, right_samples) else { return };
    let (left_output, right_output) = out.channels.split_at_mut(1);
    let (Some(left_output), Some(right_output)) = (left_output.first_mut(), right_output.first_mut()) else { return };
    for i in 0..frames {
        let pos = if item.reverse { span.saturating_sub(1) as f64 - i as f64 * speed } else { i as f64 * speed };
        if !pos.is_finite() || pos < 0.0 {
            continue;
        }
        let i0 = pos.floor() as usize;
        let frac = (pos - i0 as f64) as f32;
        let l0 = left_samples.get(i0).copied().unwrap_or(0.0);
        let l1 = left_samples.get(i0.saturating_add(1)).copied().unwrap_or(l0);
        let r0 = right_samples.get(i0).copied().unwrap_or(0.0);
        let r1 = right_samples.get(i0.saturating_add(1)).copied().unwrap_or(r0);
        let Some(l) = left_output.get_mut(offset + i) else { continue };
        let Some(r) = right_output.get_mut(offset + i) else { continue };
        let Some(at) = i64::try_from(i).ok().and_then(|i| first.checked_add(i)) else { continue };
        let gain = clip_gain_at(item, Tick::from_units(at, i64::from(sr)));
        *l += (l0 + (l1 - l0) * frac) * gain;
        *r += (r0 + (r1 - r0) * frac) * gain;
    }
}

/// Static Audio Gain plus intrinsic Volume automation, evaluated in source media time.
/// Disabling/bypassing Volume removes only its level; the independent clip gain still applies.
pub fn clip_gain_at(item: &TrackItem, time: Tick) -> f32 {
    let media_time = item.source_time_at(time);
    let volume = item.effect("volume").filter(|e| e.enabled && e.f64_at("bypass", media_time) == 0.0);
    let db = item.gain_db + volume.map_or(0.0, |e| e.f64_at("level", media_time));
    db_to_gain(db)
}

fn nested_audio(
    project: &Project,
    nested: &Sequence,
    start: i64,
    frames: usize,
    outer_rate: u32,
    sources: &dyn SourceProvider,
    depth: u32,
) -> Option<AudioBuffer> {
    let own_rate = nested.settings.sample_rate.max(1);
    let frames_i64 = i64::try_from(frames).ok()?;
    let first = i64::try_from(i128::from(start) * i128::from(own_rate) / i128::from(outer_rate.max(1))).ok()?;
    let after = i128::from(start.saturating_add(frames_i64)) * i128::from(own_rate);
    let count =
        usize::try_from((after + i128::from(outer_rate.max(1)) - 1).div_euclid(i128::from(outer_rate.max(1))) - i128::from(first)).ok()?.checked_add(2)?;
    if count > 1_048_576 {
        return None;
    }
    let input = mix_sequence_at(project, nested, first, count, sources, depth);
    let mut output = AudioBuffer::silence(outer_rate.max(1), 2, frames);
    for i in 0..frames {
        let pos = ((start as f64 + i as f64) * f64::from(own_rate) / f64::from(outer_rate.max(1))) - first as f64;
        if !pos.is_finite() || pos < 0.0 {
            continue;
        }
        let index = pos.floor() as usize;
        let frac = (pos - index as f64) as f32;
        for channel in 0..2 {
            let source = input.channels.get(channel)?;
            let a = source.get(index).copied().unwrap_or(0.0);
            let b = source.get(index.saturating_add(1)).copied().unwrap_or(a);
            if let Some(value) = output.channels.get_mut(channel).and_then(|ch| ch.get_mut(i)) {
                *value = a + (b - a) * frac;
            }
        }
    }
    Some(output)
}

/// Decibels to linear amplitude; values below −96 dB are silent and values above 96 dB are capped.
pub fn db_to_gain(db: f64) -> f32 {
    if !db.is_finite() || db <= -96.0 { 0.0 } else { 10f64.powf(db.clamp(-96.0, 96.0) / 20.0) as f32 }
}

/// The source-channel pair used by a mono/stereo clip. Missing channels safely fall back to the
/// first available channel.
pub fn source_pair(item: &TrackItem, available: usize) -> (usize, usize) {
    let valid = |channel: u16| if usize::from(channel) < available { usize::from(channel) } else { 0 };
    match item.source_channels.as_slice() {
        [] => (0, usize::from(available > 1)),
        [mono] => (valid(*mono), valid(*mono)),
        [left, right, ..] => (valid(*left), valid(*right)),
    }
}

/// Peak (min, max) pairs per bucket of `bucket` samples for channel `ch` — waveform display.
pub fn peaks(buf: &AudioBuffer, ch: usize, bucket: usize) -> Vec<(f32, f32)> {
    let Some(channel) = buf.channels.get(ch) else { return Vec::new() };
    channel.chunks(bucket.max(1)).map(|part| part.iter().fold((0.0f32, 0.0f32), |(lo, hi), sample| (lo.min(*sample), hi.max(*sample)))).collect()
}
