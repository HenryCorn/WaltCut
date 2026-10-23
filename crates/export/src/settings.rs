//! Internal compositor selection and fixed retained-output properties.
use crate::{ExportSettings, Format};
use waltcut_time::FrameRate;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuRendering {
    Auto,
    #[default]
    Off,
}

/// Output properties derived only from the sequence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolved {
    pub width: u32,
    pub height: u32,
    pub rate: FrameRate,
    pub sample_rate: u32,
    pub channels: u32,
}

impl ExportSettings {
    pub fn keeps_alpha(&self) -> bool {
        self.format != Format::H264
    }
    pub fn is_image_sequence(&self) -> bool {
        self.format.is_image_sequence()
    }
    pub fn has_audio(&self) -> bool {
        self.include_audio && !self.is_image_sequence()
    }
    pub fn resolve(&self, width: u32, height: u32, rate: FrameRate, sample_rate: u32) -> crate::Result<Resolved> {
        self.validate()?;
        waltcut_project::SequenceSettings { width, height, frame_rate: rate, sample_rate, ..Default::default() }
            .validate()
            .map_err(crate::ExportError::Unsupported)?;
        let rate = FrameRate::new(rate.num, rate.den);
        let scaled = |size: u32| -> crate::Result<u32> {
            let value = (f64::from(size) * f64::from(self.scale)).round().max(1.0);
            if !value.is_finite() || value > 8192.0 {
                return Err(crate::ExportError::Unsupported("scaled export dimension exceeds 8192 pixels".into()));
            }
            Ok(value as u32)
        };
        let (mut width, mut height) = (scaled(width)?, scaled(height)?);
        if self.format == Format::H264 {
            width = width.max(2) & !1;
            height = height.max(2) & !1;
        }
        crate::validate_output_size(width, height)?;
        Ok(Resolved { width, height, rate, sample_rate, channels: 2 })
    }
}

/// A time left, rounded up to the second: `45 s`, `2:05`, `1:02:05`.
pub fn format_eta(d: std::time::Duration) -> String {
    let s = d.as_secs().saturating_add(u64::from(d.subsec_nanos() > 0));
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{}:{:02}", s / 60, s % 60),
        _ => format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60),
    }
}
