use super::scan::{directory_entries, read_scan_track};
use crate::storage::database::{Database, ScanWriter};
use std::{
    collections::HashSet,
    ffi::OsString,
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

fn expand_path(path: &str, lookup: impl Fn(&str) -> Option<OsString>) -> io::Result<PathBuf> {
    let name_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let variable = |name: &str| {
        if name.is_empty() || !name.bytes().all(name_byte) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid path variable name",
            ));
        }
        lookup(name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} is not set")))
    };
    let (mut output, mut rest) = match path.strip_prefix("~/") {
        Some(rest) => {
            let mut home = variable("HOME")?;
            home.push("/");
            (home, rest)
        }
        None if path == "~" => return variable("HOME").map(PathBuf::from),
        None => (OsString::new(), path),
    };
    while let Some((literal, tail)) = rest.split_once('$') {
        output.push(literal);
        if let Some(remaining) = tail.strip_prefix('$') {
            output.push("$");
            rest = remaining;
        } else if let Some(braced) = tail.strip_prefix('{') {
            let (name, remaining) = braced.split_once('}').ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Unclosed path variable")
            })?;
            output.push(variable(name)?);
            rest = remaining;
        } else {
            let length = tail.bytes().take_while(|byte| name_byte(*byte)).count();
            if length == 0 {
                output.push("$");
            } else {
                output.push(variable(&tail[..length])?);
            }
            rest = &tail[length..];
        }
    }
    output.push(rest);
    Ok(PathBuf::from(output))
}

fn scan_roots(roots: Vec<String>, state: &Progress) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut unique = HashSet::new();
    for root in roots {
        state.check()?;
        let path = expand_path(&root, |name| std::env::var_os(name))
            .and_then(|path| path.canonicalize())
            .map_err(|error| at(Path::new(&root), error))?;
        if unique.insert(path.clone()) {
            paths.push(path);
        }
    }
    paths.retain(|path| {
        !path
            .ancestors()
            .skip(1)
            .any(|parent| unique.contains(parent))
    });
    Ok(paths)
}

fn run(database: Database, roots: Vec<String>, state: &Progress) -> io::Result<ScanWriter> {
    let roots = scan_roots(roots, state)?;
    let mut writer = database.stage()?;
    let mut scanned_files = HashSet::new();
    for root in roots {
        let mut pending = vec![root];
        while let Some(directory) = pending.pop() {
            state.check()?;
            *state.directory.lock().unwrap() = Some(directory.clone());
            let (files, children) = directory_entries(&directory, || state.check())
                .map_err(|error| at(&directory, error))?;
            for path in files {
                state.check()?;
                if scanned_files.insert(path.clone()) {
                    writer.write(&read_scan_track(path))?;
                }
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
    fn expands_home_and_environment_variables_in_scan_paths() {
        let expand = |path| {
            expand_path(path, |name| match name {
                "HOME" => Some("/home/listener".into()),
                "LIBRARY" => Some("my music".into()),
                _ => None,
            })
        };
        assert_eq!(expand("~").unwrap(), Path::new("/home/listener"));
        assert_eq!(
            expand("~/Music").unwrap(),
            Path::new("/home/listener/Music")
        );
        assert_eq!(
            expand("$HOME/${LIBRARY}/live").unwrap(),
            Path::new("/home/listener/my music/live")
        );
        assert_eq!(expand("music/$").unwrap(), Path::new("music/$"));
        assert!(expand("~/${UNSET}").is_err());
        assert_eq!(expand("music/$$HOME").unwrap(), Path::new("music/$HOME"));
        assert!(expand("${HOME").is_err());
        assert!(expand("${A=B}").is_err());
    }

    #[test]
    fn keeps_only_outermost_unique_scan_roots() {
        let root = tempfile::tempdir().unwrap();
        let music = root.path().join("music");
        let child = music.join("child");
        let sibling = root.path().join("music-live");
        let alias = root.path().join("alias");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir(&sibling).unwrap();
        std::os::unix::fs::symlink(&music, &alias).unwrap();

        let roots = [&child, &alias, &music, &music, &sibling]
            .map(|path| path.to_str().unwrap().to_owned())
            .to_vec();
        assert_eq!(
            scan_roots(roots, &Progress::default()).unwrap(),
            [
                music.canonicalize().unwrap(),
                sibling.canonicalize().unwrap()
            ]
        );
    }

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
        let roots = [&child, &music, &child, &music]
            .map(|path| path.to_str().unwrap().to_owned())
            .to_vec();
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
