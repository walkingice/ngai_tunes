use crate::{
    protocol::Track,
    storage::database::{Database, ScanTrack},
};
use std::{collections::BTreeSet, io, sync::mpsc, thread};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Facet {
    Genre,
    Artist,
    Album,
}

impl Facet {
    pub(super) const ALL: [Self; 3] = [Self::Genre, Self::Artist, Self::Album];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Genre => "Genres",
            Self::Artist => "Artists",
            Self::Album => "Albums",
        }
    }

    pub(super) fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&facet| facet == self)
            .expect("all facets are ordered")
    }

    fn tags(self, track: &ScanTrack) -> &[String] {
        match self {
            Self::Genre => &track.genres,
            Self::Artist => &track.artists,
            Self::Album => track.album.as_slice(),
        }
    }
}

#[derive(Default)]
pub struct Library {
    entries: Vec<ScanTrack>,
    filters: [Filter; Facet::ALL.len()],
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
        let filters = Facet::ALL.map(|facet| Filter {
            values: entries
                .iter()
                .flat_map(|track| facet.tags(track))
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

    pub(super) fn filter(&self, facet: Facet) -> &Filter {
        &self.filters[facet.index()]
    }

    pub(super) fn filter_mut(&mut self, facet: Facet) -> &mut Filter {
        &mut self.filters[facet.index()]
    }

    pub(super) fn apply(&mut self, facet: Facet) {
        let filter = self.filter_mut(facet);
        if filter.selected != filter.cursor {
            filter.selected = filter.cursor;
            self.update_songs();
        }
    }

    pub fn reset_filters(&mut self) {
        for filter in &mut self.filters {
            filter.cursor = 0;
            filter.selected = 0;
        }
        self.update_songs();
    }

    fn update_songs(&mut self) {
        self.songs =
            self.entries
                .iter()
                .filter(|track| {
                    Facet::ALL.into_iter().all(|facet| {
                        let filter = self.filter(facet);
                        filter.selected.checked_sub(1).is_none_or(|selected| {
                            facet.tags(track).contains(&filter.values[selected])
                        })
                    })
                })
                .map(to_song)
                .collect();
        self.cursor = 0;
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
        assert_eq!(library.filter(Facet::Genre).values, ["Jazz", "Pop"]);
        for (facet, cursor) in Facet::ALL.into_iter().zip([1, 2, 1]) {
            library.filter_mut(facet).cursor = cursor;
            library.apply(facet);
        }
        assert_eq!(library.songs.len(), 1);
        assert_eq!(library.songs[0].title, "First");
        library.filter_mut(Facet::Genre).cursor = 0;
        library.apply(Facet::Genre);
        assert_eq!(library.songs.len(), 2);
        library.reset_filters();
        assert_eq!(library.songs.len(), 4);
        assert!(library.filters.iter().all(|filter| {
            filter.selected == 0 && filter.cursor == 0 && !filter.values.is_empty()
        }));
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
