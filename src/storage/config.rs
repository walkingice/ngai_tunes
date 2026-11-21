use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub info_seconds: u64,
    pub warning_seconds: u64,
    pub error_seconds: u64,
    pub scan_directories: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            info_seconds: 5,
            warning_seconds: 5,
            error_seconds: 10,
            scan_directories: Vec::new(),
        }
    }
}

pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn user() -> io::Result<Self> {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        Ok(Self::new(
            PathBuf::from(home).join(".config/ngai_tunes/config.json"),
        ))
    }

    pub fn database(&self) -> super::database::Database {
        super::database::Database::new(self.path.with_file_name("database.jsonl"))
    }

    pub fn load(&self) -> io::Result<Config> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(error) => Err(error),
        }
    }

    pub fn save_if_unchanged(&self, original: &Config, config: &Config) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let lock = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path.with_extension("lock"))?;
        // The lock is held until the file is dropped after the atomic replacement.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if self.load()? != *original {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Config changed in another client; restart to reload it",
            ));
        }
        self.save(config)
    }

    pub fn save(&self, config: &Config) -> io::Result<()> {
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
        use std::io::Write;
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
        config.scan_directories.push("/music/音樂".into());
        store.save(&config).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.info_seconds, 1);
        assert_eq!(loaded.scan_directories, ["/music/音樂"]);
    }
}
