mod native;
mod session;

use crate::protocol::{Command, Order, PlaybackSummary, Repeat, Status};
use std::error::Error;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Play,
    Pause,
    Next,
    Previous,
    Repeat(Repeat),
    Order(Order),
    Exit,
}

#[derive(Clone, Default)]
struct State {
    summary: Option<PlaybackSummary>,
    busy: Option<&'static str>,
    error: Option<String>,
    connection_error: Option<String>,
}

impl State {
    fn track_details(&self) -> Option<String> {
        let summary = self.summary.as_ref()?;
        let duration = summary.duration_ms.map(super::clock);
        let artist = summary
            .artist
            .as_deref()
            .filter(|artist| !artist.is_empty());
        match (duration, artist) {
            (Some(duration), Some(artist)) => Some(format!("{duration}  {artist}")),
            (Some(duration), None) => Some(duration),
            (None, artist) => artist.map(str::to_owned),
        }
    }

    fn status_text(&self) -> &str {
        if let Some(busy) = self.busy {
            return busy;
        }
        match &self.summary {
            Some(summary) if !summary.has_queue => "Queue empty — add songs in the TUI",
            Some(summary) => match summary.status {
                Status::Playing => "Playing",
                Status::Pausing => "Paused",
                Status::Stopping => "Stopped",
            },
            None => "Server offline",
        }
    }

    fn playback_action(&self) -> Action {
        if self
            .summary
            .as_ref()
            .is_some_and(|s| s.status == Status::Playing)
        {
            Action::Pause
        } else {
            Action::Play
        }
    }

    fn command(&self, action: Action) -> Option<Command> {
        if self.busy.is_some() {
            return None;
        }
        if action == Action::Exit {
            return Some(Command::Quit);
        }
        let summary = self.summary.as_ref()?;
        match action {
            Action::Play if summary.has_queue => match summary.status {
                Status::Stopping if summary.title.is_some() => Some(Command::Restart),
                Status::Pausing => Some(Command::Resume),
                _ => None,
            },
            Action::Pause if summary.has_queue && summary.status == Status::Playing => {
                Some(Command::Pause)
            }
            Action::Next if summary.has_queue => Some(Command::Next),
            Action::Previous if summary.has_queue => Some(Command::Previous),
            Action::Repeat(repeat) => Some(Command::SetRepeat(repeat)),
            Action::Order(order) => Some(Command::SetOrder(order)),
            _ => None,
        }
    }
}

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    native::run()
}

#[cfg(test)]
mod tests;
