use super::ipc::Connection;
use crate::protocol::{Command, Message, Order, Repeat, Snapshot, Status, Track};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    io,
    path::PathBuf,
    sync::mpsc::TryRecvError,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Sidebar,
    Main,
    Playback,
}

pub struct App {
    source: Option<PathBuf>,
    tracks: Vec<Track>,
    cursor: usize,
    focus: Focus,
    connection: Option<Connection>,
    snapshot: Option<Snapshot>,
    updated: Instant,
    info: String,
    confirm: bool,
    initialized: bool,
    pending: Option<u64>,
}

impl App {
    pub fn new(
        source: Option<PathBuf>,
        tracks: Vec<Track>,
        connection: Option<Connection>,
    ) -> Self {
        let info = if source.is_some() {
            format!("{} tracks found", tracks.len())
        } else {
            String::new()
        };
        Self {
            source,
            tracks,
            cursor: 0,
            focus: Focus::Main,
            connection,
            snapshot: None,
            updated: Instant::now(),
            info,
            confirm: false,
            initialized: false,
            pending: None,
        }
    }

    pub fn run(&mut self) -> io::Result<()> {
        self.initialize();
        let mut terminal = ratatui::try_init()?;
        let result = self.event_loop(&mut terminal);
        let restored = ratatui::try_restore();
        result.and(restored)
    }

    fn initialize(&mut self) {
        if self.source.is_some() {
            return;
        }
        if let Some(message) = self.connection.as_ref().and_then(|peer| peer.first().ok()) {
            self.receive(message);
        } else {
            self.connection = None;
            self.info = "No daemon available to attach".into();
        }
    }

    fn event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
        loop {
            self.poll_connection();
            terminal.draw(|frame| self.render(frame))?;
            if event::poll(Duration::from_millis(75))?
                && let Event::Key(key) = event::read()?
                && key.kind != KeyEventKind::Release
                && self.key(key.code)
            {
                return Ok(());
            }
        }
    }

    fn receive(&mut self, message: Message) {
        if message.request_id == self.pending && self.pending.is_some() {
            self.pending = None;
        }
        if let Some(error) = message.error {
            self.info = error.message;
        }
        let Some(state) = message.state else { return };
        if self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.revision > state.revision)
        {
            return;
        }
        if self.source.is_none() {
            if !self.initialized {
                self.cursor = state.current.unwrap_or(0);
            }
            self.tracks.clone_from(&state.queue);
            self.cursor = self.cursor.min(self.tracks.len().saturating_sub(1));
        }
        if let Some(error) = &state.error {
            self.info.clone_from(error);
        }
        self.initialized = true;
        self.updated = Instant::now();
        self.snapshot = Some(state);
    }

    fn poll_connection(&mut self) {
        for _ in 0..16 {
            match self.connection.as_mut().map(Connection::next) {
                Some(Ok(message)) => self.receive(message),
                Some(Err(TryRecvError::Disconnected)) => {
                    self.disconnect();
                    if self.pending.take().is_some() {
                        self.connection = Connection::connect().ok();
                    }
                    break;
                }
                _ => break,
            }
        }
    }

    fn disconnect(&mut self) {
        self.connection = None;
        if self
            .snapshot
            .as_ref()
            .is_none_or(|state| state.error.is_none())
        {
            self.info = "Daemon disconnected".into();
        }
        self.snapshot = None;
    }

    fn send(&mut self, command: Command) {
        let Some(connection) = &mut self.connection else {
            self.info = "No daemon is reachable".into();
            return;
        };
        match connection.send(command) {
            Ok(id) => self.pending = Some(id),
            Err(error) => {
                if error.kind() != io::ErrorKind::InvalidData {
                    self.disconnect();
                    self.pending = None;
                    self.connection = Connection::connect().ok();
                }
                self.info = error.to_string();
            }
        }
    }

    fn play_selection(&mut self) {
        if self.tracks.is_empty() {
            return;
        }
        match Connection::start() {
            Ok(connection) => {
                self.connection = Some(connection);
                self.pending = None;
                self.snapshot = None;
                self.send(Command::ReplaceAndPlay {
                    queue: self.tracks.clone(),
                    selected: self.cursor,
                });
            }
            Err(error) => self.info = error.to_string(),
        }
    }

    fn finish(&mut self, command: Command) {
        if let Ok(mut connection) = Connection::connect() {
            connection.finish(command);
        }
    }

    fn quit_requires_confirmation(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|state| state.status == Status::Playing)
    }

    fn key(&mut self, key: KeyCode) -> bool {
        if self.confirm {
            match key {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.finish(Command::Quit);
                    return true;
                }
                KeyCode::Esc | KeyCode::Char('n') => self.confirm = false,
                _ => {}
            }
            return false;
        }
        match key {
            KeyCode::Esc => {
                self.finish(Command::ExitOnStopping);
                return true;
            }
            KeyCode::Char('q') if self.quit_requires_confirmation() => self.confirm = true,
            KeyCode::Char('q') => {
                self.finish(Command::Quit);
                return true;
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Main,
                    Focus::Main => Focus::Playback,
                    Focus::Playback => Focus::Sidebar,
                }
            }
            KeyCode::Up if self.focus == Focus::Main => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down if self.focus == Focus::Main => {
                self.cursor = (self.cursor + 1).min(self.tracks.len().saturating_sub(1));
            }
            KeyCode::Enter if self.focus == Focus::Main => self.play_selection(),
            KeyCode::Enter if self.focus == Focus::Sidebar => self.focus = Focus::Main,
            KeyCode::Char(' ') if self.focus != Focus::Sidebar => self.send(Command::TogglePause),
            _ if self.focus == Focus::Playback => self.playback_key(key),
            _ => {}
        }
        false
    }

    fn playback_key(&mut self, key: KeyCode) {
        let Some(state) = &self.snapshot else { return };
        let command = match key {
            KeyCode::Enter if state.status == Status::Stopping => Command::Restart,
            KeyCode::Enter => Command::Stop,
            KeyCode::Char('n') => Command::Next,
            KeyCode::Char('p') => Command::Previous,
            KeyCode::Char('r') => Command::SetRepeat(match state.repeat {
                Repeat::Off => Repeat::All,
                Repeat::All => Repeat::One,
                Repeat::One => Repeat::Off,
            }),
            KeyCode::Char('o') => Command::SetOrder(match state.order {
                Order::Sequential => Order::Random,
                Order::Random => Order::Sequential,
            }),
            KeyCode::Left => Command::Seek {
                position_ms: self.position().saturating_sub(5000),
            },
            KeyCode::Right => Command::Seek {
                position_ms: self.position().saturating_add(5000),
            },
            _ => return,
        };
        self.send(command);
    }

    fn position(&self) -> u64 {
        self.snapshot.as_ref().map_or(0, |state| {
            let elapsed = if state.status == Status::Playing {
                crate::playback::millis(self.updated.elapsed())
            } else {
                0
            };
            state
                .position_ms
                .saturating_add(elapsed)
                .min(state.duration_ms.unwrap_or(u64::MAX))
        })
    }

    fn panel(&self, name: String, focus: Focus) -> Block<'static> {
        let color = if self.focus == focus {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        Block::default()
            .borders(Borders::ALL)
            .title(name)
            .border_style(color)
    }

    fn render(&self, frame: &mut Frame) {
        let [top, playback, info, keys] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .areas(frame.area());
        let [sidebar, main] =
            Layout::horizontal([Constraint::Length(13), Constraint::Min(0)]).areas(top);
        frame.render_widget(
            Paragraph::new("Playing").block(self.panel("Tabs".into(), Focus::Sidebar)),
            sidebar,
        );
        self.render_list(frame, main);
        self.render_playback(frame, playback);
        frame.render_widget(
            Paragraph::new(self.info.as_str()).block(Block::bordered().title("Info")),
            info,
        );
        let keys_text = match self.focus {
            Focus::Sidebar => "Enter Open",
            Focus::Main => "↑/↓ Select · Enter Play · Space Pause/Resume",
            Focus::Playback
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|state| state.status == Status::Stopping) =>
            {
                "Enter Restart · Space Pause/Resume · N/P Next/Previous · ←/→ Seek · R Repeat · O Order"
            }
            Focus::Playback => {
                "Enter Stop · Space Pause/Resume · N/P Next/Previous · ←/→ Seek · R Repeat · O Order"
            }
        };
        frame.render_widget(
            Paragraph::new(format!("{keys_text} · Tab Focus · Esc Detach · Q Quit"))
                .block(Block::bordered().title("Hotkey"))
                .wrap(Wrap { trim: true }),
            keys,
        );
        if self.confirm {
            let [_, middle, _] = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(4),
                Constraint::Fill(1),
            ])
            .areas(frame.area());
            let [_, dialog, _] = Layout::horizontal([
                Constraint::Fill(1),
                Constraint::Length(40),
                Constraint::Fill(1),
            ])
            .areas(middle);
            frame.render_widget(Clear, dialog);
            frame.render_widget(
                Paragraph::new("Stop playback and exit?\nEnter/Y Confirm · Esc/N Cancel")
                    .block(Block::bordered().title("Quit")),
                dialog,
            );
        }
    }

    fn render_list(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let current = self
            .snapshot
            .as_ref()
            .and_then(|state| state.current.and_then(|index| state.queue.get(index)));
        let items = self.tracks.iter().map(|track| {
            let playing = if current.is_some_and(|live| live.path == track.path) {
                "♪ "
            } else {
                "  "
            };
            let suffix = track
                .artist
                .as_ref()
                .map_or(String::new(), |artist| format!(" — {artist}"));
            ListItem::new(format!("{playing}{}{suffix}", track.title))
        });
        let name = self
            .source
            .as_ref()
            .map_or_else(|| "Queue".into(), |source| source.display().to_string());
        let mut state =
            ListState::default().with_selected((!self.tracks.is_empty()).then_some(self.cursor));
        let list = List::new(items)
            .block(self.panel(name, Focus::Main))
            .highlight_symbol("> ")
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_playback(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let panel = self.panel("Playback".into(), Focus::Playback);
        let inner = panel.inner(area);
        frame.render_widget(panel, area);
        let Some(state) = &self.snapshot else {
            frame.render_widget(Paragraph::new("No live session"), inner);
            return;
        };
        let track = state
            .current
            .and_then(|index| state.queue.get(index))
            .map_or("—", |track| &track.title);
        let [details, progress] =
            Layout::vertical([Constraint::Length(2), Constraint::Length(1)]).areas(inner);
        let position = self.position();
        frame.render_widget(
            Paragraph::new(format!(
                "{track} · {:?}\n{} · Repeat {:?} · {:?}",
                state.status,
                clock(position),
                state.repeat,
                state.order
            )),
            details,
        );
        if let Some(duration) = state.duration_ms.filter(|&duration| duration > 0) {
            frame.render_widget(
                Gauge::default()
                    .ratio(position as f64 / duration as f64)
                    .gauge_style(Style::default().fg(Color::Cyan))
                    .label(format!("{} / {}", clock(position), clock(duration))),
                progress,
            );
        }
    }
}

fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_without_daemon_opens_an_empty_queue_with_a_message() {
        let mut app = App::new(None, Vec::new(), None);
        app.initialize();
        assert!(!app.key(KeyCode::Enter));
        assert!(app.tracks.is_empty());
        assert!(app.connection.is_none());
        assert_eq!(app.info, "No daemon available to attach");
    }

    #[test]
    fn empty_scan_can_change_focus() {
        let mut app = App::new(Some(".".into()), Vec::new(), None);
        assert!(!app.key(KeyCode::Enter));
        assert!(app.connection.is_none());
        app.key(KeyCode::Tab);
        assert!(app.focus == Focus::Playback);
    }

    #[test]
    fn quit_requires_confirmation_only_while_playing() {
        let mut app = App::new(None, Vec::new(), None);
        assert!(!app.quit_requires_confirmation());

        for status in [Status::Stopping, Status::Pausing, Status::Playing] {
            app.snapshot = Some(Snapshot {
                revision: 0,
                queue: Vec::new(),
                current: None,
                status,
                position_ms: 0,
                duration_ms: None,
                repeat: Repeat::Off,
                order: Order::Sequential,
                error: None,
            });
            assert_eq!(app.quit_requires_confirmation(), status == Status::Playing);
        }
        assert!(!app.key(KeyCode::Char('q')));
        assert!(app.confirm);
        assert!(!app.key(KeyCode::Esc));
        assert!(!app.confirm);
    }
}
