use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
};
use tempfile::NamedTempFile;

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub struct ScanTrack {
    #[serde(with = "crate::protocol::unix_path")]
    pub path: PathBuf,
    pub title: String,
    pub duration_ms: Option<u64>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub genres: Vec<String>,
    pub track_number: Option<u32>,
}

pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    pub fn load(&self) -> io::Result<Vec<ScanTrack>> {
        let file = File::open(&self.path)?;
        BufReader::new(file)
            .lines()
            .map(|line| serde_json::from_str(&line?).map_err(io::Error::other))
            .collect()
    }

    pub fn stage(&self) -> io::Result<ScanWriter> {
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty());
        let parent = parent.unwrap_or_else(|| std::path::Path::new("."));
        fs::create_dir_all(parent)?;
        Ok(ScanWriter {
            file: NamedTempFile::new_in(parent)?,
            path: self.path.clone(),
        })
    }
}

pub struct ScanWriter {
    file: NamedTempFile,
    path: PathBuf,
}

impl ScanWriter {
    pub fn write(&mut self, track: &ScanTrack) -> io::Result<()> {
        serde_json::to_writer(&mut self.file, track).map_err(io::Error::other)?;
        self.file.write_all(b"\n")
    }

    pub fn publish(self) -> io::Result<()> {
        self.file.persist(self.path).map_err(|error| error.error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_json_lines_only_on_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("database.jsonl");
        let database = Database::new(path.clone());
        let track = ScanTrack {
            path: "/music/song.mp3".into(),
            title: "Song\nTitle".into(),
            artists: vec!["First".into(), "Second".into()],
            genres: vec!["Rock".into(), "Jazz".into()],
            duration_ms: None,
            album: None,
            track_number: None,
        };
        assert_eq!(database.load().unwrap_err().kind(), io::ErrorKind::NotFound);
        let mut writer = database.stage().unwrap();
        writer.write(&track).unwrap();
        writer.write(&track).unwrap();
        writer.publish().unwrap();
        let original = fs::read_to_string(&path).unwrap();
        assert_eq!(original.lines().count(), 2);
        assert_eq!(
            database.load().unwrap().iter().collect::<Vec<_>>(),
            [&track, &track]
        );
        let mut cancelled = database.stage().unwrap();
        cancelled.write(&track).unwrap();
        drop(cancelled);
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }
}
