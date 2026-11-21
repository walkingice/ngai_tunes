use crate::protocol::Track;
use lofty::{file::TaggedFileExt, prelude::Accessor};
use std::{fs, io, path::Path};

pub fn scan(directory: &Path) -> io::Result<Vec<Track>> {
    let directory = directory.canonicalize()?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("mp3") || extension.eq_ignore_ascii_case("flac")
            });
        if supported && entry.file_type()?.is_file() {
            paths.push(path);
        }
    }
    paths.sort_unstable();
    Ok(paths.into_iter().map(read_track).collect())
}

fn read_track(path: std::path::PathBuf) -> Track {
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
    Track {
        title,
        artist: tag
            .and_then(Accessor::artist)
            .map(|value| value.into_owned()),
        album: tag
            .and_then(Accessor::album)
            .map(|value| value.into_owned()),
        path,
    }
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
        assert!(tracks.iter().all(|track| track.path.is_absolute()));
        assert!(scan(&root.path().join("missing")).is_err());
    }
}
