use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

pub const VERSION: u32 = 1;
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

mod unix_path {
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
