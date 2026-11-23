use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf, sync::Arc};

mod segments;
pub use segments::{QueueSegment, QueueTransfer, encode_segments};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Track {
    #[serde(with = "unix_path")]
    pub path: PathBuf,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    #[serde(default)]
    pub track_number: Option<u32>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub genre: Option<String>,
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
    #[serde(skip)]
    pub queue: Arc<Vec<Track>>,
    pub current: Option<usize>,
    pub status: Status,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub repeat: Repeat,
    pub order: Order,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PlaybackSummary {
    pub status: Status,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration_ms: Option<u64>,
    pub repeat: Repeat,
    pub order: Order,
    pub has_queue: bool,
    pub error: Option<String>,
}

impl From<&Snapshot> for PlaybackSummary {
    fn from(state: &Snapshot) -> Self {
        let track = state.current.and_then(|index| state.queue.get(index));
        Self {
            status: state.status,
            title: track.map(|track| track.title.clone()),
            artist: track.and_then(|track| track.artist.clone()),
            duration_ms: track.and_then(|track| state.duration_ms.or(track.duration_ms)),
            repeat: state.repeat,
            order: state.order,
            has_queue: !state.queue.is_empty(),
            error: state.error.clone(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Subscribe,
    SubscribePlayback,
    GetState,
    #[serde(skip)]
    ReplaceAndPlay {
        queue: Vec<Track>,
        selected: usize,
    },
    UploadQueue {
        segment: QueueSegment,
        selected: Option<usize>,
    },
    TogglePause,
    Pause,
    Resume,
    Stop,
    Restart,
    Seek {
        position_ms: u64,
    },
    Next,
    Previous,
    SetRepeat(Repeat),
    SetOrder(Order),
    Quit,
    ExitOnStopping,
}

pub const VERSION: u32 = 2;
pub const MAX_FRAME: usize = 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
pub struct Request {
    pub version: u32,
    pub request_id: u64,
    pub command: Command,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Message {
    pub request_id: Option<u64>,
    pub state: Option<Snapshot>,
    pub error: Option<Failure>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireMessage {
    Playback {
        request_id: Option<u64>,
        summary: PlaybackSummary,
    },
    Queue {
        revision: u64,
        segment: QueueSegment,
    },
    Message {
        queue_revision: Option<u64>,
        message: Message,
    },
}

impl WireMessage {
    pub fn reply(message: Message) -> Self {
        Self::Message {
            queue_revision: None,
            message,
        }
    }
}

#[derive(Default)]
pub struct IncomingState {
    pending: QueueTransfer,
    queue: Option<(u64, Arc<Vec<Track>>)>,
}

impl IncomingState {
    pub fn receive(&mut self, wire: WireMessage) -> Result<Option<Message>, &'static str> {
        match wire {
            WireMessage::Playback { .. } => Err("Unexpected playback summary"),
            WireMessage::Queue { revision, segment } => {
                if let Some(queue) = self.pending.push(revision, segment)? {
                    self.queue = Some((revision, Arc::new(queue)));
                }
                Ok(None)
            }
            WireMessage::Message {
                queue_revision,
                mut message,
            } => {
                if let Some(state) = &mut message.state {
                    let (revision, queue) = self.queue.as_ref().ok_or("Missing snapshot queue")?;
                    if queue_revision != Some(*revision) {
                        return Err("Snapshot queue revision mismatch");
                    }
                    state.queue = Arc::clone(queue);
                }
                Ok(Some(message))
            }
        }
    }
}

pub fn encode_upload(id: u64, queue: &[Track], selected: usize) -> io::Result<Vec<Vec<u8>>> {
    encode_segments(queue, |segment, sizing| {
        let selected = (sizing || segment.segment_index == 1).then_some(selected);
        encode(&Request {
            version: VERSION,
            request_id: id,
            command: Command::UploadQueue { segment, selected },
        })
    })
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

pub fn encode(value: &impl Serialize) -> std::io::Result<Vec<u8>> {
    let mut data = serde_json::to_vec(value)?;
    if data.len() >= MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Frame is too large",
        ));
    }
    data.push(b'\n');
    Ok(data)
}

pub fn read_frame(reader: &mut impl std::io::BufRead) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::{BufRead, Error, ErrorKind, Read};
    let mut data = Vec::new();
    reader.take(MAX_FRAME as u64).read_until(b'\n', &mut data)?;
    if data.is_empty() {
        return Ok(None);
    }
    if data.pop() != Some(b'\n') {
        let kind = if data.len() + 1 == MAX_FRAME {
            ErrorKind::InvalidData
        } else {
            ErrorKind::UnexpectedEof
        };
        return Err(Error::new(kind, "Oversized or incomplete frame"));
    }
    Ok(Some(data))
}

pub(crate) mod unix_path {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::{
        ffi::OsString,
        os::unix::ffi::{OsStrExt, OsStringExt},
        path::{Path, PathBuf},
    };

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum WirePath {
        Text(String),
        Bytes(Vec<u8>),
    }

    pub fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        match path.to_str() {
            Some(text) => serializer.serialize_str(text),
            None => serializer.serialize_bytes(path.as_os_str().as_bytes()),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
        Ok(match WirePath::deserialize(deserializer)? {
            WirePath::Text(text) => text.into(),
            WirePath::Bytes(bytes) => OsString::from_vec(bytes).into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn reads_track_without_new_metadata() {
        let track: Track = serde_json::from_str(
            r#"{"path":"/song.mp3","title":"Song","artist":null,"album":null}"#,
        )
        .unwrap();
        assert_eq!(track.track_number, None);
        assert_eq!(track.duration_ms, None);
        assert_eq!(track.genre, None);
    }

    #[test]
    fn reads_multiple_frames_with_small_buffers_and_bounds_input() {
        let mut reader = BufReader::with_capacity(2, Cursor::new(b"bad\n{}\n"));
        assert_eq!(read_frame(&mut reader).unwrap().unwrap(), b"bad");
        assert_eq!(read_frame(&mut reader).unwrap().unwrap(), b"{}");
        assert!(read_frame(&mut reader).unwrap().is_none());
        let mut oversized = Cursor::new(vec![b'x'; MAX_FRAME + 1]);
        assert_eq!(
            read_frame(&mut oversized).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }
}
