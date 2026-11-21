use super::Track;
use serde::{Deserialize, Serialize};
use std::{io, ops::Range};

const TRACKS_PER_SEGMENT: usize = 100;

#[derive(Debug, Deserialize, Serialize)]
pub struct QueueSegment {
    pub total_segments: usize,
    pub segment_index: usize,
    pub tracks: Vec<Track>,
}

pub fn encode_segments(
    tracks: &[Track],
    mut encode: impl FnMut(QueueSegment, bool) -> io::Result<Vec<u8>>,
) -> io::Result<Vec<Vec<u8>>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    loop {
        let mut end = (start + TRACKS_PER_SEGMENT).min(tracks.len());
        loop {
            let probe = segment(tracks, start..end, usize::MAX, usize::MAX);
            match encode(probe, true) {
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::InvalidData && end - start > 1 => {
                    end = start + (end - start) / 2;
                }
                Err(error) => return Err(error),
            }
        }
        ranges.push(start..end);
        if end == tracks.len() {
            break;
        }
        start = end;
    }
    let total = ranges.len();
    ranges
        .into_iter()
        .enumerate()
        .map(|(index, range)| encode(segment(tracks, range, total, index + 1), false))
        .collect()
}

fn segment(tracks: &[Track], range: Range<usize>, total: usize, index: usize) -> QueueSegment {
    QueueSegment {
        total_segments: total,
        segment_index: index,
        tracks: tracks[range].to_vec(),
    }
}

#[derive(Default)]
pub struct QueueTransfer {
    pending: Option<PendingQueue>,
}

struct PendingQueue {
    id: u64,
    total: usize,
    next: usize,
    tracks: Vec<Track>,
}

impl QueueTransfer {
    pub fn push(
        &mut self,
        id: u64,
        segment: QueueSegment,
    ) -> Result<Option<Vec<Track>>, &'static str> {
        let QueueSegment {
            total_segments,
            segment_index,
            tracks,
        } = segment;
        let error = "Invalid queue segment sequence";
        let previous = self.pending.take();
        if total_segments == 0
            || segment_index == 0
            || segment_index > total_segments
            || tracks.len() > TRACKS_PER_SEGMENT
            || (tracks.is_empty() && total_segments != 1)
        {
            return Err(error);
        }
        let mut pending = if segment_index == 1 {
            PendingQueue {
                id,
                total: total_segments,
                next: 1,
                tracks: Vec::new(),
            }
        } else {
            previous.ok_or(error)?
        };
        if pending.id != id || pending.total != total_segments || pending.next != segment_index {
            return Err(error);
        }
        pending.tracks.extend(tracks);
        if segment_index == total_segments {
            Ok(Some(pending.tracks))
        } else {
            pending.next += 1;
            self.pending = Some(pending);
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Command, MAX_FRAME, Request, encode_upload};

    fn track(title: String) -> Track {
        Track {
            path: "/song.mp3".into(),
            title,
            artist: None,
            album: None,
            track_number: None,
            duration_ms: None,
            genre: None,
        }
    }

    #[test]
    fn upload_has_numbered_segments_and_first_selection() {
        let tracks = vec![track("Song".into()); 235];
        let frames = encode_upload(45, &tracks, 234).unwrap();
        let mut receiver = QueueTransfer::default();
        for (index, frame) in frames.into_iter().enumerate() {
            let request: Request = serde_json::from_slice(&frame).unwrap();
            assert_eq!(request.request_id, 45);
            let Command::UploadQueue { segment, selected } = request.command else {
                panic!("Expected a queue upload");
            };
            assert_eq!(segment.total_segments, 3);
            assert_eq!(segment.segment_index, index + 1);
            assert_eq!(segment.tracks.len(), [100, 100, 35][index]);
            assert_eq!(selected, (index == 0).then_some(234));
            let queue = receiver.push(45, segment).unwrap();
            assert_eq!(queue.as_deref(), (index == 2).then_some(tracks.as_slice()));
        }
    }

    #[test]
    fn shrinks_segments_and_rejects_tracks_that_cannot_fit() {
        let tracks = vec![track("x".repeat(MAX_FRAME / 3)); 5];
        let frames = encode_upload(1, &tracks, 0).unwrap();
        assert!(frames.len() > 1);
        assert!(frames.iter().all(|frame| frame.len() <= MAX_FRAME));
        let oversized = [track("x".repeat(MAX_FRAME))];
        assert_eq!(
            encode_upload(1, &oversized, 0).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
