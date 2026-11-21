use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum Status {
    Playing,
    Pausing,
    Stopping,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum Repeat {
    Off,
    All,
    One,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum Order {
    Sequential,
    Random,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub revision: u64,
    pub queue: Vec<Track>,
    pub current: Option<usize>,
    pub status: Status,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub repeat: Repeat,
    pub order: Order,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Subscribe,
    GetState,
    ReplaceAndPlay { queue: Vec<Track>, selected: usize },
    TogglePause,
    Pause,
    Resume,
    Stop,
    Restart,
    Seek { position_ms: u64 },
    Next,
    Previous,
    SetRepeat(Repeat),
    SetOrder(Order),
    Quit,
    ExitOnStopping,
}
