use std::{path::Path, time::Duration};

pub enum StartError {
    Track(String),
    Device(String),
}

pub enum Event {
    End(u64),
    Device(String),
}

pub trait Engine {
    fn start(&mut self, path: &Path, generation: u64) -> Result<Option<Duration>, StartError>;
    fn pause(&mut self, paused: bool);
    fn stop(&mut self);
    fn seek(&mut self, position: Duration) -> Result<(), String>;
    fn position(&self) -> Duration;
    fn event(&mut self) -> Option<Event>;
}

pub fn millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
