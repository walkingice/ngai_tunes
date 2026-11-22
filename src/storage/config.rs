use crate::{
    daemon::runtime,
    protocol::{Repeat, Track},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub info_seconds: u64,
    pub warning_seconds: u64,
    pub error_seconds: u64,
    pub scan_directories: Vec<String>,
    pub repeat: Repeat,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            info_seconds: 5,
            warning_seconds: 5,
            error_seconds: 10,
            scan_directories: Vec::new(),
            repeat: Repeat::Off,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct PlayingCursor {
    pub fingerprint: u64,
    pub index: usize,
}

impl PlayingCursor {
    pub fn fingerprint(tracks: &[Track]) -> io::Result<u64> {
        // FNV-1a over the JSON Lines representation gives a stable on-disk key.
        let mut hash = 0xcbf29ce484222325_u64;
        let mut line = Vec::new();
        for track in tracks {
            line.clear();
            serde_json::to_writer(&mut line, track).map_err(io::Error::other)?;
            for byte in line.iter().copied().chain([b'\n']) {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
        Ok(hash)
    }
}

pub struct ConfigStore {
    path: PathBuf,
    lock_root: PathBuf,
}

impl ConfigStore {
    #[cfg(test)]
    pub fn new(path: PathBuf) -> Self {
        let lock_root = path.with_file_name("runtime");
        Self { path, lock_root }
    }

    pub fn user() -> io::Result<Self> {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        Ok(Self {
            path: PathBuf::from(home).join(".config/ngai_tunes/config.json"),
            lock_root: PathBuf::from(runtime::ROOT),
        })
    }

    pub fn database(&self) -> super::database::Database {
        super::database::Database::new(self.path.with_file_name("database.jsonl"))
    }

    pub fn load_playing(&self) -> io::Result<Vec<Track>> {
        let path = self.path.with_file_name("server_playing.jsonl");
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        BufReader::new(file)
            .lines()
            .map(|line| {
                let track: Track = serde_json::from_str(&line?).map_err(io::Error::other)?;
                if !track.path.is_absolute() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Playing queue needs absolute paths",
                    ));
                }
                Ok(track)
            })
            .collect()
    }

    pub fn save_playing(&self, tracks: &[Track]) -> io::Result<()> {
        let path = self.path.with_file_name("server_playing.jsonl");
        let parent = path.parent().filter(|path| !path.as_os_str().is_empty());
        let parent = parent.unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        {
            let mut writer = BufWriter::new(file.as_file_mut());
            for track in tracks {
                serde_json::to_writer(&mut writer, track).map_err(io::Error::other)?;
                writer.write_all(b"\n")?;
            }
            writer.flush()?;
        }
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    pub fn load_playing_cursor(&self) -> Option<PlayingCursor> {
        let path = self.path.with_file_name("server_playing_cursor.json");
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }

    pub fn save_playing_cursor(&self, cursor: &PlayingCursor) -> io::Result<()> {
        let path = self.path.with_file_name("server_playing_cursor.json");
        let parent = path.parent().filter(|path| !path.as_os_str().is_empty());
        let mut file = tempfile::NamedTempFile::new_in(parent.unwrap_or_else(|| Path::new(".")))?;
        serde_json::to_writer(file.as_file_mut(), cursor).map_err(io::Error::other)?;
        // Atomic replacement handles daemon termination without forcing a disk flush.
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    pub fn load(&self) -> io::Result<Config> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(error) => Err(error),
        }
    }

    pub fn save_if_unchanged(&self, original: &Config, config: &Config) -> io::Result<()> {
        let _lock = self.lock()?;
        let mut current = self.load()?;
        // The daemon owns repeat mode and can update it while a client is open.
        let repeat = current.repeat;
        current.repeat = original.repeat;
        if current != *original {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Config changed in another client; restart to reload it",
            ));
        }
        let mut config = config.clone();
        config.repeat = repeat;
        self.save(&config)
    }

    pub fn save_repeat(&self, repeat: Repeat) -> io::Result<()> {
        let _lock = self.lock()?;
        let mut config = self.load()?;
        config.repeat = repeat;
        self.save(&config)
    }

    fn lock(&self) -> io::Result<fs::File> {
        // Keep the lock until the atomic replacement finishes.
        runtime::lock_file(&self.lock_root, "config.lock", libc::LOCK_EX)
    }

    fn save(&self, config: &Config) -> io::Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let data = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
        // Write beside the destination so a failed write does not truncate the config.
        let mut temporary = tempfile_path(&self.path);
        let mut suffix = 0u32;
        let mut file = loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => break file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    suffix += 1;
                    temporary = self.path.with_extension(format!(
                        "json.{}.{}.tmp",
                        std::process::id(),
                        suffix
                    ));
                }
                Err(error) => return Err(error),
            }
        };
        let result = file
            .write_all(&data)
            .and_then(|()| fs::rename(&temporary, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

fn tempfile_path(path: &Path) -> PathBuf {
    path.with_extension(format!("json.{}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playing_queue_round_trips_as_json_lines() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("nested/config.json"));
        assert!(store.load_playing().unwrap().is_empty());
        let first = Track {
            path: "/music/one.mp3".into(),
            title: "One\nSong".into(),
            artist: None,
            album: None,
            track_number: None,
            duration_ms: None,
            genre: None,
        };
        let second = Track {
            path: "/music/two.flac".into(),
            ..first.clone()
        };
        let tracks = [first, second];
        store.save_playing(&tracks).unwrap();
        let path = dir.path().join("nested/server_playing.jsonl");
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
        assert_eq!(store.load_playing().unwrap(), tracks);
        store.save_playing(&[]).unwrap();
        assert!(store.load_playing().unwrap().is_empty());
        fs::write(path, "{\n").unwrap();
        assert!(store.load_playing().is_err());
    }

    #[test]
    fn rejects_stale_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.json"));
        let original = store.load().unwrap();
        let mut first = original.clone();
        first.scan_directories.push("/first".into());
        store.save_if_unchanged(&original, &first).unwrap();
        let mut stale = original.clone();
        stale.scan_directories.push("/second".into());
        assert_eq!(
            store
                .save_if_unchanged(&original, &stale)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(store.load().unwrap(), first);
    }

    #[test]
    fn reads_old_config_and_round_trips_scan_directories() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("nested/config.json"));
        assert!(store.load().unwrap().scan_directories.is_empty());
        fs::create_dir_all(dir.path().join("nested")).unwrap();
        fs::write(&store.path, r#"{"info_seconds":1}"#).unwrap();
        let mut config = store.load().unwrap();
        assert_eq!(config.repeat, Repeat::Off);
        config.scan_directories.push("/music/音樂".into());
        store.save(&config).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.info_seconds, 1);
        assert_eq!(loaded.scan_directories, ["/music/音樂"]);
    }

    #[test]
    fn scan_settings_and_repeat_updates_preserve_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.json"));
        let original = store.load().unwrap();
        store.save_repeat(Repeat::All).unwrap();
        assert!(dir.path().join("runtime/config.lock").is_file());
        assert!(!dir.path().join("config.lock").exists());
        let mut updated = original.clone();
        updated.scan_directories.push("/music".into());
        store.save_if_unchanged(&original, &updated).unwrap();
        assert_eq!(store.load().unwrap().repeat, Repeat::All);
        store.save_repeat(Repeat::One).unwrap();
        updated.repeat = Repeat::One;
        assert_eq!(store.load().unwrap(), updated);
    }
}
