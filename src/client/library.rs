use crate::{
    protocol::Track,
    storage::database::{Database, ScanTrack},
};
use std::{collections::BTreeSet, io, sync::mpsc, thread};

pub const FACETS: [&str; 3] = ["Genres", "Artists", "Albums"];

#[derive(Default)]
pub struct Library {
    entries: Vec<ScanTrack>,
    pub filters: [Filter; 3],
    pub songs: Vec<Track>,
    pub cursor: usize,
}

#[derive(Default)]
pub struct Filter {
    pub values: Vec<String>,
    pub cursor: usize,
    pub selected: usize,
}

pub fn load(database: Database) -> io::Result<mpsc::Receiver<io::Result<Library>>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("library-load".into())
        .spawn(move || {
            let _ = sender.send(database.load().map(Library::new));
        })?;
    Ok(receiver)
}

impl Library {
    fn new(entries: Vec<ScanTrack>) -> Self {
        let filters = std::array::from_fn(|index| Filter {
            values: entries
                .iter()
                .flat_map(|track| tags(track, index))
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            ..Filter::default()
        });
        let mut library = Self {
            entries,
            filters,
            ..Self::default()
        };
        library.update_songs();
        library
    }

    pub fn apply(&mut self, index: usize) {
        let filter = &mut self.filters[index];
        if filter.selected != filter.cursor {
            filter.selected = filter.cursor;
            self.update_songs();
        }
    }

    fn update_songs(&mut self) {
        self.songs = self
            .entries
            .iter()
            .filter(|track| {
                self.filters.iter().enumerate().all(|(index, filter)| {
                    filter.selected.checked_sub(1).is_none_or(|selected| {
                        tags(track, index).contains(&filter.values[selected])
                    })
                })
            })
            .map(to_song)
            .collect();
        self.cursor = 0;
    }
}

fn tags(track: &ScanTrack, index: usize) -> &[String] {
    match index {
        0 => &track.genres,
        1 => &track.artists,
        _ => track.album.as_slice(),
    }
}

fn to_song(track: &ScanTrack) -> Track {
    Track {
        path: track.path.clone(),
        title: track.title.clone(),
        artist: joined(&track.artists),
        album: track.album.clone(),
        track_number: track.track_number,
        duration_ms: track.duration_ms,
        genre: joined(&track.genres),
    }
}

fn joined(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, genres: &[&str], artists: &[&str], album: &str) -> ScanTrack {
        ScanTrack {
            path: format!("{title}.flac").into(),
            title: title.into(),
            duration_ms: None,
            artists: artists.iter().map(|value| (*value).into()).collect(),
            album: Some(album.into()),
            genres: genres.iter().map(|value| (*value).into()).collect(),
            track_number: None,
        }
    }

    #[test]
    fn combines_filters_and_matches_secondary_tags() {
        let mut library = Library::new(vec![
            track("First", &["Pop", "Jazz"], &["A", "B"], "X"),
            track("Second", &["Jazz"], &["B"], "Y"),
            track("Third", &["Pop"], &["B"], "X"),
            track("Fourth", &["Jazz"], &["A"], "X"),
        ]);
        assert_eq!(library.songs.len(), 4);
        assert_eq!(library.filters[0].values, ["Jazz", "Pop"]);
        for (index, cursor) in [1, 2, 1].into_iter().enumerate() {
            library.filters[index].cursor = cursor;
            library.apply(index);
        }
        assert_eq!(library.songs.len(), 1);
        assert_eq!(library.songs[0].title, "First");
        library.filters[0].cursor = 0;
        library.apply(0);
        assert_eq!(library.songs.len(), 2);
    }

    #[test]
    fn background_load_reports_missing_and_malformed_database() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("database.jsonl");
        let result = load(Database::new(path.clone())).unwrap().recv().unwrap();
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::NotFound));

        let valid = track("First", &[], &[], "X");
        std::fs::write(
            &path,
            format!("{}\n{{", serde_json::to_string(&valid).unwrap()),
        )
        .unwrap();
        let result = load(Database::new(path)).unwrap().recv().unwrap();
        assert!(result.is_err());
    }
}
