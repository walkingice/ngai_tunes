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
    values: Vec<String>,
    pub cursor: usize,
    pub selected: usize,
}

impl Filter {
    pub fn labels(&self) -> impl Iterator<Item = &str> {
        std::iter::once("All").chain(self.values.iter().map(String::as_str))
    }

    pub fn row_count(&self) -> usize {
        self.values.len() + 1
    }

    fn replace_values(&mut self, values: Vec<String>) {
        let find_row = |row: usize| {
            let value = self.values.get(row.checked_sub(1)?)?;
            values.binary_search(value).ok().map(|index| index + 1)
        };
        self.selected = find_row(self.selected).unwrap_or(0);
        if self.cursor != 0 {
            self.cursor = find_row(self.cursor).unwrap_or(self.selected);
        }
        self.values = values;
    }

    fn selected_value(&self) -> Option<&String> {
        self.selected
            .checked_sub(1)
            .map(|index| &self.values[index])
    }
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
        let mut library = Self {
            entries,
            ..Self::default()
        };
        library.update_lists(0);
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
            self.update_lists(facet.index() + 1);
        }
    }

    pub fn reset_filters(&mut self) {
        for filter in &mut self.filters {
            filter.cursor = 0;
            filter.selected = 0;
        }
        self.update_lists(0);
    }

    fn update_lists(&mut self, rebuild_from: usize) {
        let mut tracks: Vec<_> = self.entries.iter().collect();
        for (index, facet) in Facet::ALL.into_iter().enumerate() {
            let filter = &mut self.filters[index];
            if index >= rebuild_from {
                let values = tracks
                    .iter()
                    .flat_map(|track| facet.tags(track))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .cloned()
                    .collect();
                filter.replace_values(values);
            }
            if let Some(value) = filter.selected_value() {
                tracks.retain(|track| facet.tags(track).contains(value));
            }
        }
        self.songs = tracks.into_iter().map(to_song).collect();
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

    #[test]
    fn all_row_is_distinct_from_a_tag_named_all() {
        let mut filter = Filter::default();
        assert_eq!(filter.row_count(), 1);
        assert_eq!(filter.labels().collect::<Vec<_>>(), ["All"]);
        assert_eq!(filter.selected_value(), None);

        filter.values.push("All".into());
        filter.selected = 1;
        assert_eq!(filter.labels().collect::<Vec<_>>(), ["All", "All"]);
        assert_eq!(filter.selected_value().map(String::as_str), Some("All"));
    }

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
    fn scopes_options_downstream_and_remaps_selections() {
        let mut library = Library::new(vec![
            track("First", &["Jazz"], &["B"], "Y"),
            track("Second", &["Jazz"], &["C"], "Z"),
            track("Third", &["Pop"], &["A"], "X"),
        ]);

        library.filter_mut(Facet::Artist).cursor = 2;
        library.apply(Facet::Artist);
        assert_eq!(library.filter(Facet::Genre).values, ["Jazz", "Pop"]);
        assert_eq!(library.filter(Facet::Artist).values, ["A", "B", "C"]);
        assert_eq!(library.filter(Facet::Album).values, ["Y"]);
        library.filter_mut(Facet::Album).cursor = 1;
        library.apply(Facet::Album);

        library.filter_mut(Facet::Genre).cursor = 1;
        library.apply(Facet::Genre);
        let artist = library.filter(Facet::Artist);
        assert_eq!(artist.values, ["B", "C"]);
        assert_eq!((artist.selected, artist.cursor), (1, 1));
        assert_eq!(library.filter(Facet::Album).selected, 1);
        assert_eq!(library.songs[0].title, "First");

        library.filter_mut(Facet::Artist).cursor = 0;
        library.apply(Facet::Artist);
        assert_eq!(library.filter(Facet::Album).values, ["Y", "Z"]);
        library.filter_mut(Facet::Album).cursor = 2;
        library.apply(Facet::Album);
        assert_eq!(library.filter(Facet::Album).values, ["Y", "Z"]);
        assert_eq!(library.filter(Facet::Artist).values, ["B", "C"]);
        assert_eq!(library.filter(Facet::Genre).selected, 1);
        assert_eq!(library.songs.len(), 1);
        assert_eq!(library.songs[0].title, "Second");

        library.filter_mut(Facet::Artist).cursor = 2;
        library.apply(Facet::Artist);
        library.filter_mut(Facet::Genre).cursor = 2;
        library.apply(Facet::Genre);
        for facet in [Facet::Artist, Facet::Album] {
            let filter = library.filter(facet);
            assert_eq!((filter.selected, filter.cursor), (0, 0));
        }
        assert_eq!(library.filter(Facet::Artist).values, ["A"]);
        assert_eq!(library.filter(Facet::Album).values, ["X"]);
        assert_eq!(library.songs[0].title, "Third");

        library.reset_filters();
        assert_eq!(library.filter(Facet::Artist).values, ["A", "B", "C"]);
        assert_eq!(library.filter(Facet::Album).values, ["X", "Y", "Z"]);
        assert_eq!(library.songs.len(), 3);
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
