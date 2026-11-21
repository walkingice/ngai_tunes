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

use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};
use std::{
    fs::File,
    sync::mpsc::{self, Receiver},
};

#[derive(Default)]
pub struct Audio {
    sink: Option<Sink>,
    stream: Option<OutputStream>,
    errors: Option<Receiver<String>>,
    generation: u64,
}

impl Audio {
    fn open_output(&mut self) -> Result<(), StartError> {
        let (send, receive) = mpsc::sync_channel(1);
        let opened = OutputStreamBuilder::from_default_device().and_then(|builder| {
            builder
                .with_error_callback(move |error| {
                    let _ = send.try_send(error.to_string());
                })
                .open_stream()
        });
        self.stream = Some(opened.map_err(|error| StartError::Device(error.to_string()))?);
        self.errors = Some(receive);
        Ok(())
    }
}

impl Engine for Audio {
    fn start(&mut self, path: &Path, generation: u64) -> Result<Option<Duration>, StartError> {
        let source = File::open(path)
            .map_err(|error| StartError::Track(format!("{}: {error}", path.display())))?;
        let mut decoder = Decoder::try_from(source)
            .map_err(|error| StartError::Track(format!("{}: {error}", path.display())))?;
        let Some(first) = decoder.next() else {
            return Err(StartError::Track(format!(
                "{}: No audio samples",
                path.display()
            )));
        };
        let duration = decoder.total_duration();
        if self.stream.is_none() {
            self.open_output()?;
        }
        let sink = Sink::connect_new(self.stream.as_ref().unwrap().mixer());
        // Seek back to the beginning when the decoder supports it.
        // A non-seekable decoder is reopened so the first sample is retained.
        if decoder.try_seek(Duration::ZERO).is_err() {
            decoder = Decoder::try_from(
                File::open(path).map_err(|error| StartError::Track(error.to_string()))?,
            )
            .map_err(|error| StartError::Track(error.to_string()))?;
        }
        let _ = first;
        sink.append(decoder);
        self.sink = Some(sink);
        self.generation = generation;
        Ok(duration)
    }

    fn pause(&mut self, paused: bool) {
        if let Some(sink) = &self.sink {
            if paused {
                sink.pause();
            } else {
                sink.play();
            }
        }
    }

    fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
    }

    fn seek(&mut self, position: Duration) -> Result<(), String> {
        self.sink
            .as_ref()
            .ok_or("No active track")?
            .try_seek(position)
            .map_err(|error| error.to_string())
    }

    fn position(&self) -> Duration {
        self.sink.as_ref().map_or(Duration::ZERO, Sink::get_pos)
    }

    fn event(&mut self) -> Option<Event> {
        if let Some(error) = self
            .errors
            .as_ref()
            .and_then(|errors| errors.try_recv().ok())
        {
            self.stream.take();
            self.errors.take();
            return Some(Event::Device(error));
        }
        if self
            .sink
            .as_ref()
            .is_some_and(|sink| !sink.is_paused() && sink.empty())
        {
            return Some(Event::End(self.generation));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_both_supported_formats_without_an_output_device() {
        for name in ["tone.mp3", "tone.flac"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name);
            let mut decoder = Decoder::try_from(File::open(path).unwrap()).unwrap();
            assert!(decoder.next().is_some());
            decoder.try_seek(Duration::ZERO).unwrap();
            assert!(decoder.count() > 1000);
        }
    }
}
