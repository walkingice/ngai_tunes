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
    fn info_text(&self) -> String {
        let notice = self
            .busy
            .or(self.error.as_deref())
            .or(self.connection_error.as_deref())
            .or_else(|| self.summary.as_ref()?.error.as_deref());
        if let Some(notice) = notice {
            return notice.to_owned();
        }

        let Some(summary) = &self.summary else {
            return "Server offline".into();
        };
        if !summary.has_queue {
            return "Queue empty — add songs in the TUI".into();
        }
        let Some(title) = summary.title.as_deref().filter(|title| !title.is_empty()) else {
            return "No current track".into();
        };

        let artist = summary
            .artist
            .as_deref()
            .filter(|artist| !artist.is_empty());
        let duration = summary
            .duration_ms
            .map(|ms| format!("({})", super::clock(ms)));
        [artist, duration.as_deref(), Some(title)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
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
