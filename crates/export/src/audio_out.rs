//! Mono/stereo sequence audio for export.

use std::sync::Arc;

use waltcut_project::{ItemId, Project, Sequence};
use waltcut_render::SourceProvider;
use waltcut_time::TimeRange;

use crate::{ExportError, ExportSettings, Result};

pub(crate) struct AudioOut {
    project: Arc<Project>,
    seq: Sequence,
    pub sr: u32,
    pub channels: usize,
    pub out: i64,
    end: i64,
}

impl AudioOut {
    pub fn new(project: Arc<Project>, seq: ItemId, settings: &ExportSettings, range: TimeRange) -> Result<Self> {
        let q = project.sequence(seq).ok_or(ExportError::NoSequence)?;
        let resolved = settings.resolve(q.settings.width, q.settings.height, q.settings.frame_rate, q.settings.sample_rate);
        let mut seq = q.clone();
        seq.settings.sample_rate = resolved.sample_rate;
        let sr = resolved.sample_rate;
        let out = range.start.to_units_floor(i64::from(sr));
        let end = range.end().to_units_floor(i64::from(sr));
        Ok(Self { project, seq, sr, channels: 2, out, end })
    }

    /// Mix `n` samples from the current cursor, padding with silence past the range end.
    pub fn pull(&mut self, until: i64, sources: &dyn SourceProvider) -> Option<Vec<Vec<f32>>> {
        let until = until.min(self.end);
        if until <= self.out {
            return None;
        }
        let n = usize::try_from(until.checked_sub(self.out)?).ok()?;
        let real = usize::try_from(self.end.saturating_sub(self.out).min(i64::try_from(n).unwrap_or(i64::MAX))).ok()?;
        let mut stereo =
            if real > 0 { waltcut_render::audio::mix_sequence(&self.project, &self.seq, self.out, real, sources).channels } else { vec![Vec::new(); 2] };
        stereo.resize(2, Vec::new());
        for channel in &mut stereo {
            channel.resize(n, 0.0);
        }
        self.out = until;
        Some(stereo)
    }

    pub fn remaining(&self) -> u64 {
        u64::try_from(self.end.saturating_sub(self.out)).unwrap_or(0)
    }

    pub fn rest(&mut self, sources: &dyn SourceProvider) -> Option<Vec<Vec<f32>>> {
        self.pull(self.end, sources)
    }
}

/// Interleave planar samples.
pub(crate) fn interleave(planar: &[Vec<f32>]) -> Vec<f32> {
    let n = planar.first().map_or(0, Vec::len);
    let Some(capacity) = n.checked_mul(planar.len()) else { return Vec::new() };
    let mut out = Vec::with_capacity(capacity);
    for i in 0..n {
        for channel in planar {
            out.push(channel.get(i).copied().unwrap_or(0.0));
        }
    }
    out
}

#[cfg(test)]
mod bounds_tests {
    #[test]
    fn ragged_audio_planes_keep_channel_positions_without_panicking() {
        assert_eq!(super::interleave(&[vec![1.0, 2.0], vec![3.0]]), vec![1.0, 3.0, 2.0, 0.0]);
        assert!(super::interleave(&[]).is_empty());
    }
}
