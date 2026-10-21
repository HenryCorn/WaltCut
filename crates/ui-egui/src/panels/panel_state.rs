//! Frontend state of the Timecode, Events and Progress panels
//! (`UiState::panels`). Plain serde, so agents read it with `ui.inspect` (`ui.panels`) and change
//! it with `ui.set {"panels": {"timecode": {...}, ...}}` (fields merged).

use serde::{Deserialize, Serialize};
use waltcut_time::TimeDisplay;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PanelsState {
    pub timecode: TimecodeState,
    pub events: EventsState,
    pub progress: ProgressState,
    /// Preferred selection for Metadata and Info; inspector focus does not replace it.
    pub inspector_source: InspectorSource,
}

/// The last selection panel used by the item inspectors. Only the source is retained, so
/// project changes, deletion and deselection always resolve against the current selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InspectorSource {
    #[default]
    Project,
    Timeline,
}

/// What a Timecode panel row measures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TcMode {
    /// The playhead (sequence time / source clip time).
    #[default]
    Current,
    /// The media timecode of the clip under the playhead (topmost video clip; the file's own
    /// timecode for the Source monitor).
    Media,
    /// The sequence's / clip's duration.
    Duration,
    /// In to Out duration (the whole duration when no marks).
    InOut,
    /// From the playhead to the end (or to the Out point).
    Remaining,
}

impl TcMode {
    pub const ALL: [TcMode; 5] = [TcMode::Current, TcMode::Media, TcMode::Duration, TcMode::InOut, TcMode::Remaining];
    pub fn label(self) -> &'static str {
        match self {
            TcMode::Current => "Current Time",
            TcMode::Media => "Media Time",
            TcMode::Duration => "Duration",
            TcMode::InOut => "In/Out Duration",
            TcMode::Remaining => "Remaining",
        }
    }
}

/// Which monitor a Timecode row follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TcSource {
    /// The Source monitor when it has focus, else the Program / Timeline.
    #[default]
    Active,
    Program,
    Source,
}

impl TcSource {
    pub const ALL: [TcSource; 3] = [TcSource::Active, TcSource::Program, TcSource::Source];
    pub fn label(self) -> &'static str {
        match self {
            TcSource::Active => "Active Monitor",
            TcSource::Program => "Program",
            TcSource::Source => "Source",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TimecodeRow {
    pub source: TcSource,
    pub mode: TcMode,
    pub display: TimeDisplay,
}

/// The Timecode panel: one large row plus optional smaller rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TimecodeState {
    pub rows: Vec<TimecodeRow>,
    /// Show the clip / sequence name under the timecode.
    pub show_name: bool,
}

impl Default for TimecodeState {
    fn default() -> Self {
        TimecodeState { rows: vec![TimecodeRow::default()], show_name: true }
    }
}

/// The Events panel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventsState {
    /// Lowest level shown ("info", "warning", "error").
    pub level: String,
    /// The selected entry (details shown below the list).
    pub selected: Option<u64>,
    /// Entries seen when the panel was last shown (newer ones are "new").
    pub seen: u64,
}

/// The Progress panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProgressState {
    /// Also list finished jobs.
    pub show_finished: bool,
}

impl Default for ProgressState {
    fn default() -> Self {
        ProgressState { show_finished: true }
    }
}
