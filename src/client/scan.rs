use crate::{protocol::Track, storage::database::ScanTrack};
use lofty::{
    file::{AudioFile, TaggedFileExt},
    prelude::{Accessor, ItemKey},
    tag::Tag,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub fn scan(directory: &Path) -> io::Result<Vec<Track>> {
    let (paths, _) = directory_entries(&directory.canonicalize()?, || Ok(()))?;
    Ok(paths
        .into_iter()
        .map(|path| {
            read_metadata(path, |tag| {
                (
                    tag.and_then(Accessor::artist)
                        .map(|value| value.into_owned())
                        .into_iter()
                        .collect(),
                    tag.and_then(Accessor::genre)
                        .map(|value| value.into_owned())
                        .into_iter()
                        .collect(),
                )
            })
        })
        .map(|track| Track {
            title: track.title,
            artist: track.artists.into_iter().next(),
            album: track.album,
            track_number: track.track_number,
            duration_ms: track.duration_ms,
            genre: track.genres.into_iter().next(),
            path: track.path,
        })
        .collect())
}

pub(super) fn directory_entries(
    directory: &Path,
    check: impl Fn() -> io::Result<()>,
) -> io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut directories = Vec::new();
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        check()?;
        let entry = entry?;
        let path = entry.path();
        let supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("mp3") || extension.eq_ignore_ascii_case("flac")
            });
        let kind = entry.file_type()?;
        if kind.is_dir() {
            directories.push(path);
        } else if supported && kind.is_file() {
            paths.push(path);
        }
    }
    paths.sort_unstable();
    directories.sort_unstable();
    Ok((paths, directories))
}

pub(super) fn read_scan_track(path: PathBuf) -> ScanTrack {
    read_metadata(path, |tag| tag.map(read_lists).unwrap_or_default())
}

fn read_metadata(
    path: PathBuf,
    lists: impl FnOnce(Option<&Tag>) -> (Vec<String>, Vec<String>),
) -> ScanTrack {
    let file = lofty::read_from_path(&path).ok();
    let tag = file
        .as_ref()
        .and_then(|file| file.primary_tag().or_else(|| file.first_tag()));
    let title = tag
        .and_then(Accessor::title)
        .filter(|title| !title.trim().is_empty())
        .map(|title| title.into_owned())
        .unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let (artists, genres) = lists(tag);
    ScanTrack {
        title,
        artists,
        album: tag
            .and_then(Accessor::album)
            .map(|value| value.into_owned()),
        track_number: tag.and_then(Accessor::track),
        duration_ms: file
            .as_ref()
            .map(|file| crate::playback::millis(file.properties().duration())),
        genres,
        path,
    }
}

fn read_lists(tag: &Tag) -> (Vec<String>, Vec<String>) {
    let values = |key| {
        tag.get_strings(&key)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let mut artists = values(ItemKey::TrackArtists);
    if artists.is_empty() {
        artists = values(ItemKey::TrackArtist);
    }
    (artists, values(ItemKey::Genre))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_immediate_supported_files_in_order() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        for name in ["z.MP3", "a.flac", "ignore.txt", "nested/song.mp3"] {
            fs::write(root.path().join(name), []).unwrap();
        }
        fs::write(
            root.path().join("tagged.mp3"),
            include_bytes!("../../tests/fixtures/tone.mp3"),
        )
        .unwrap();
        let tracks = scan(root.path()).unwrap();
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>(),
            ["a.flac", "Fixture tone", "z.MP3"]
        );
        assert_eq!(tracks[1].artist.as_deref(), Some("ngai_tunes"));
        assert!(tracks[1].duration_ms.is_some_and(|duration| duration > 0));
        assert_eq!(tracks[0].track_number, None);
        assert_eq!(tracks[0].genre, None);
        assert!(tracks.iter().all(|track| track.path.is_absolute()));
        assert!(scan(&root.path().join("missing")).is_err());
    }
}
