use super::scan::{directory_entries, read_scan_track};
use crate::storage::database::{Database, ScanWriter};
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};

pub struct Scan {
    state: Arc<Progress>,
    result: Receiver<io::Result<ScanWriter>>,
}

#[derive(Default)]
struct Progress {
    cancelled: AtomicBool,
    directory: Mutex<Option<PathBuf>>,
}

impl Scan {
    pub fn start(database: Database, roots: Vec<String>) -> io::Result<Self> {
        let state = Arc::new(Progress::default());
        let worker_state = Arc::clone(&state);
        let (sender, result) = mpsc::channel();
        std::thread::Builder::new()
            .name("music-scan".into())
            .spawn(move || {
                let _ = sender.send(run(database, roots, &worker_state));
            })?;
        Ok(Self { state, result })
    }

    pub fn directory(&self) -> Option<PathBuf> {
        self.state.directory.lock().unwrap().clone()
    }

    pub fn result(&self) -> Option<io::Result<ScanWriter>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err(io::Error::other("Scan worker exited unexpectedly")))
            }
        }
    }
}

impl Drop for Scan {
    fn drop(&mut self) {
        self.state.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Progress {
    fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Scan cancelled"));
        }
        Ok(())
    }
}

fn run(database: Database, roots: Vec<String>, state: &Progress) -> io::Result<ScanWriter> {
    let mut writer = database.stage()?;
    let mut visited = HashSet::new();
    for root in roots {
        state.check()?;
        let root = PathBuf::from(root);
        let mut pending = vec![root.canonicalize().map_err(|error| at(&root, error))?];
        while let Some(directory) = pending.pop() {
            state.check()?;
            if !visited.insert(directory.clone()) {
                continue;
            }
            *state.directory.lock().unwrap() = Some(directory.clone());
            let (files, children) = directory_entries(&directory, || state.check())
                .map_err(|error| at(&directory, error))?;
            for path in files {
                state.check()?;
                writer.write(&read_scan_track(path))?;
            }
            pending.extend(children.into_iter().rev());
        }
    }
    Ok(writer)
}

fn at(directory: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", directory.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::{
        config::WriteOptions,
        id3::v2::Id3v2Tag,
        prelude::{Accessor, TagExt},
    };
    use std::fs;

    #[test]
    fn scans_nested_tracks_and_keeps_multiple_values() {
        let root = tempfile::tempdir().unwrap();
        let music = root.path().join("music");
        let child = music.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(child.join("ignored.txt"), []).unwrap();
        let song = child.join("song.MP3");
        fs::write(&song, include_bytes!("../../tests/fixtures/tone.mp3")).unwrap();
        let mut tag = Id3v2Tag::new();
        tag.set_artist("A & B".into());
        tag.insert_user_text("Artists".into(), "A\0B".into());
        tag.set_genre("Rock\0Jazz".into());
        tag.save_to_path(&song, WriteOptions::default()).unwrap();

        let path = root.path().join("database.jsonl");
        let roots = vec![music.display().to_string(), child.display().to_string()];
        let progress = Progress::default();
        run(Database::new(path.clone()), roots, &progress)
            .unwrap()
            .publish()
            .unwrap();
        let child = child.canonicalize().unwrap();
        assert_eq!(*progress.directory.lock().unwrap(), Some(child.clone()));
        let tracks = Database::new(path).load().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].path, child.join("song.MP3"));
        assert_eq!(tracks[0].artists, ["A", "B"]);
        assert_eq!(tracks[0].genres, ["Rock", "Jazz"]);
    }
}
