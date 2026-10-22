//! Frontend state of the Events and Progress panels
//! (`UiState::panels`). Plain serde, so agents read it with `ui.inspect` (`ui.panels`) and change
//! it with `ui.set {"panels": {"events": {...}, ...}}` (fields merged).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PanelsState {
    pub events: EventsState,
    pub progress: ProgressState,
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
