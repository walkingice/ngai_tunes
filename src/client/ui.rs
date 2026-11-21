use super::ipc::Connection;
use crate::protocol::{Command, Message, Order, Repeat, Snapshot, Status, Track};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
};
use std::{
    io,
    path::PathBuf,
    sync::mpsc::TryRecvError,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    fn color(self) -> Color {
        match self {
            Self::Info => Color::Reset,
            Self::Warning => Color::Yellow,
            Self::Error => Color::Red,
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct NoticeConfig {
    info_seconds: u64,
    warning_seconds: u64,
    error_seconds: u64,
}

impl Default for NoticeConfig {
    fn default() -> Self {
        Self {
            info_seconds: 5,
            warning_seconds: 5,
            error_seconds: 10,
        }
    }
}

impl NoticeConfig {
    fn load() -> Self {
        let Some(home) = std::env::var_os("HOME") else {
            return Self::default();
        };
        let path = PathBuf::from(home).join(".config/ngai_tunes/config.json");
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("Invalid config: {error}");
                    Self::default()
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                eprintln!("Could not read config: {error}");
                Self::default()
            }
        }
    }

    fn duration(&self, level: Level) -> Duration {
        Duration::from_secs(match level {
            Level::Info => self.info_seconds,
            Level::Warning => self.warning_seconds,
            Level::Error => self.error_seconds,
        })
    }
}

struct Notice {
    text: String,
    level: Level,
    created: Instant,
}

impl Notice {
    fn visible(&self, config: &NoticeConfig) -> bool {
        self.created.elapsed() < config.duration(self.level)
    }
}

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
    info: Option<Notice>,
    notice_config: NoticeConfig,
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
        let mut app = Self {
            source,
            tracks,
            cursor: 0,
            focus: Focus::Main,
            connection,
            snapshot: None,
            updated: Instant::now(),
            info: None,
            notice_config: NoticeConfig::load(),
            confirm: false,
            initialized: false,
            pending: None,
        };
        if app.source.is_some() {
            app.show_notice(Level::Info, format!("{} tracks found", app.tracks.len()));
        }
        app
    }

    pub fn run(&mut self) -> io::Result<()> {
        self.initialize();
        let mut terminal = ratatui::try_init()?;
        let result = self.event_loop(&mut terminal);
        let restored = ratatui::try_restore();
        result.and(restored)
    }

    fn show_notice(&mut self, level: Level, text: impl Into<String>) {
        self.info = Some(Notice {
            text: text.into(),
            level,
            created: Instant::now(),
        });
    }

    fn initialize(&mut self) {
        if self.source.is_some() {
            return;
        }
        if let Some(message) = self.connection.as_ref().and_then(|peer| peer.first().ok()) {
            self.receive(message);
        } else {
            self.connection = None;
            self.show_notice(Level::Warning, "No daemon available to attach");
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
            self.show_notice(Level::Error, error.message);
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
        if let Some(error) = &state.error
            && self.snapshot.as_ref().and_then(|old| old.error.as_ref()) != Some(error)
        {
            self.show_notice(Level::Error, error.clone());
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
            self.show_notice(Level::Warning, "Daemon disconnected");
        }
        self.snapshot = None;
    }

    fn send(&mut self, command: Command) {
        let Some(connection) = &mut self.connection else {
            self.show_notice(Level::Warning, "No daemon is reachable");
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
                self.show_notice(Level::Error, error.to_string());
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
            Err(error) => self.show_notice(Level::Error, error.to_string()),
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
            _ => {
                if let Some(command) = self.playback_command(key) {
                    self.send(command);
                }
            }
        }
        false
    }

    fn playback_command(&self, key: KeyCode) -> Option<Command> {
        let state = self.snapshot.as_ref()?;
        let playing_tab_seek = self.focus == Focus::Main
            && state.current.is_some()
            && matches!(state.status, Status::Playing | Status::Pausing)
            && matches!(key, KeyCode::Left | KeyCode::Right);
        if self.focus != Focus::Playback && !playing_tab_seek {
            return None;
        }
        Some(match key {
            KeyCode::Enter if state.status == Status::Stopping => Command::Restart,
            KeyCode::Enter => Command::Stop,
            KeyCode::Char('n') => Command::Next,
            KeyCode::Char('p' | 'N') => Command::Previous,
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
            _ => return None,
        })
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
            Constraint::Length(1),
            Constraint::Length(1),
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
        if let Some(notice) = self
            .info
            .as_ref()
            .filter(|notice| notice.visible(&self.notice_config))
        {
            frame.render_widget(
                Paragraph::new(notice.text.as_str())
                    .right_aligned()
                    .style(notice.level.color()),
                info,
            );
        }
        let keys_text = match self.focus {
            Focus::Sidebar => "[Enter] Open",
            Focus::Main => "[↑/↓] Move   [Enter] ▶   [←/→] Seek   [Space] ▶/⏸",
            Focus::Playback
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|state| state.status == Status::Stopping) =>
            {
                "[Enter] ↺   [Space] ▶/⏸   [n] Next   [N/p] Previous   [←/→] Seek   [R] Repeat   [O] Order"
            }
            Focus::Playback => {
                "[Enter] ⏹   [Space] ▶/⏸   [n] Next   [N/p] Previous   [←/→] Seek   [R] Repeat   [O] Order"
            }
        };
        frame.render_widget(
            Paragraph::new(format!(
                "[Tab] Focus   {keys_text}   [Esc] Detach   [Q] Quit"
            ))
            .style(Style::default().fg(Color::DarkGray))
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
        let rows = self.tracks.iter().map(|track| {
            let playing = if current.is_some_and(|live| live.path == track.path) {
                "♪"
            } else {
                ""
            };
            Row::new(vec![
                Cell::from(playing),
                Cell::from(
                    Line::from(
                        track
                            .track_number
                            .map_or_else(String::new, |number| number.to_string()),
                    )
                    .right_aligned(),
                ),
                Cell::from(track.title.clone()),
                Cell::from(track.duration_ms.map_or_else(String::new, clock)),
                Cell::from(track.artist.clone().unwrap_or_default()),
                Cell::from(track.album.clone().unwrap_or_default()),
                Cell::from(track.genre.clone().unwrap_or_default()),
            ])
        });
        let name = self
            .source
            .as_ref()
            .map_or_else(|| "Queue".into(), |source| source.display().to_string());
        let mut state =
            TableState::default().with_selected((!self.tracks.is_empty()).then_some(self.cursor));
        let time_width = self
            .tracks
            .iter()
            .filter_map(|track| track.duration_ms)
            .map(|duration| clock(duration).len().min(u16::MAX as usize) as u16)
            .max()
            .unwrap_or(0)
            .max(5);
        let track_width = self
            .tracks
            .iter()
            .filter_map(|track| track.track_number)
            .map(|number| number.to_string().len() as u16)
            .max()
            .unwrap_or(0)
            .max(5);
        let widths = [
            Constraint::Length(1),
            Constraint::Length(track_width),
            Constraint::Fill(3),
            Constraint::Length(time_width),
            Constraint::Fill(2),
            Constraint::Fill(2),
            Constraint::Min(5),
        ];
        let highlight = if self.focus == Focus::Main {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        };
        let table = Table::new(rows, widths)
            .header(
                Row::new(["", "Track", "Title", "Time", "Artist", "Album", "Genre"])
                    .style(Style::default().fg(Color::DarkGray)),
            )
            .block(self.panel(name, Focus::Main))
            .row_highlight_style(highlight);
        frame.render_stateful_widget(table, area, &mut state);
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
        let position = self.position();
        let duration = state.duration_ms.map_or_else(|| "—".into(), clock);
        let order = match state.order {
            Order::Sequential => "→",
            Order::Random => "⤨",
        };
        let play = match state.status {
            Status::Playing => "⏸",
            Status::Pausing | Status::Stopping => "▶",
        };
        let repeat = match state.repeat {
            Repeat::Off => "•",
            Repeat::All => "↻",
            Repeat::One => "↻₁",
        };
        let [title, progress, controls] = Layout::vertical([Constraint::Length(1); 3]).areas(inner);
        frame.render_widget(Line::from(track).centered(), title);
        frame.render_widget(
            playback_progress(position, state.duration_ms, progress.width.min(100)),
            progress,
        );

        let time = Line::from(format!("{} / {duration}", clock(position)));
        let symbols = Line::from(format!("{order}   ≪   {play}   ≫   {repeat}"));
        let time_width = (time.width() as u16).min(controls.width);
        let centered_offset = controls.width.saturating_sub(symbols.width() as u16) / 2;
        let available = controls.width.saturating_sub(time_width.saturating_add(1));
        let offset = centered_offset.min(available.saturating_sub(symbols.width() as u16));
        frame.render_widget(
            symbols,
            Rect::new(
                controls.x + offset,
                controls.y,
                available.saturating_sub(offset),
                controls.height,
            ),
        );
        frame.render_widget(time.right_aligned(), controls);
    }
}

fn playback_progress(position: u64, duration: Option<u64>, width: u16) -> Line<'static> {
    let filled = duration
        .filter(|&duration| duration > 0)
        .map_or(0, |duration| {
            (u128::from(position.min(duration)) * u128::from(width) / u128::from(duration)) as usize
        });
    Line::from(vec![
        Span::styled("█".repeat(filled), Style::default().fg(Color::Cyan)),
        Span::styled(
            "░".repeat(usize::from(width) - filled),
            Style::default().fg(Color::DarkGray),
        ),
    ])
    .centered()
}

fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_color_follows_focus_while_scrolling() {
        use ratatui::{Terminal, backend::TestBackend};

        let tracks = (0..20)
            .map(|number| Track {
                path: format!("/{number}").into(),
                title: format!("Track {number} 音樂"),
                artist: None,
                album: None,
                track_number: Some(number),
                duration_ms: None,
                genre: None,
            })
            .collect();
        let mut app = App::new(None, tracks, None);
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        for (cursor, focus, fg, bg) in [
            (0, Focus::Main, Color::Black, Color::Cyan),
            (1, Focus::Main, Color::Black, Color::Cyan),
            (0, Focus::Main, Color::Black, Color::Cyan),
            (15, Focus::Main, Color::Black, Color::Cyan),
            (4, Focus::Main, Color::Black, Color::Cyan),
            (4, Focus::Playback, Color::White, Color::DarkGray),
            (4, Focus::Main, Color::Black, Color::Cyan),
        ] {
            app.cursor = cursor;
            app.focus = focus;
            terminal.draw(|frame| app.render(frame)).unwrap();
            let buffer = terminal.backend().buffer();
            let title = format!("Track {cursor}");
            let selected_row = (0..buffer.area.height)
                .find(|&y| {
                    let line: String = (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect();
                    line.contains(&title)
                })
                .expect("selected track should be visible");
            for x in 14..99 {
                if x > 14 && matches!(buffer[(x - 1, selected_row)].symbol(), "音" | "樂") {
                    continue;
                }
                assert_eq!(
                    (buffer[(x, selected_row)].fg, buffer[(x, selected_row)].bg),
                    (fg, bg)
                );
            }
            for y in 2..5 {
                if y != selected_row {
                    assert_ne!(buffer[(16, y)].bg, Color::Cyan);
                    assert_ne!(buffer[(16, y)].bg, Color::DarkGray);
                }
            }
        }
    }

    #[test]
    fn notices_use_configured_durations_and_colors() {
        let config: NoticeConfig =
            serde_json::from_str(r#"{"info_seconds":1,"warning_seconds":2,"error_seconds":3}"#)
                .unwrap();
        for (level, seconds, color) in [
            (Level::Info, 1, Color::Reset),
            (Level::Warning, 2, Color::Yellow),
            (Level::Error, 3, Color::Red),
        ] {
            let notice = Notice {
                text: "test".into(),
                level,
                created: Instant::now(),
            };
            assert!(notice.visible(&config));
            assert!(
                !Notice {
                    created: Instant::now() - Duration::from_secs(seconds),
                    ..notice
                }
                .visible(&config)
            );
            assert_eq!(level.color(), color);
        }
        let defaults: NoticeConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(defaults.duration(Level::Info), Duration::from_secs(5));
        assert_eq!(defaults.duration(Level::Warning), Duration::from_secs(5));
        assert_eq!(defaults.duration(Level::Error), Duration::from_secs(10));
    }

    #[test]
    fn repeated_snapshot_error_does_not_refresh_notice() {
        let mut app = App::new(None, Vec::new(), None);
        let state = Snapshot {
            revision: 1,
            queue: Vec::new(),
            current: None,
            status: Status::Stopping,
            position_ms: 0,
            duration_ms: None,
            repeat: Repeat::Off,
            order: Order::Sequential,
            error: Some("Broken".into()),
        };
        app.snapshot = Some(state.clone());
        app.receive(Message {
            request_id: None,
            error: None,
            state: Some(state),
        });
        assert!(app.info.is_none());
    }

    #[test]
    fn attach_without_daemon_opens_an_empty_queue_with_a_message() {
        let mut app = App::new(None, Vec::new(), None);
        app.initialize();
        assert!(!app.key(KeyCode::Enter));
        assert!(app.tracks.is_empty());
        assert!(app.connection.is_none());
        assert_eq!(
            app.info.as_ref().unwrap().text,
            "No daemon available to attach"
        );
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

    #[test]
    fn playing_tab_seeks_only_with_an_active_track() {
        let mut app = App::new(None, Vec::new(), None);
        assert!(app.playback_command(KeyCode::Right).is_none());
        app.snapshot = Some(Snapshot {
            revision: 0,
            queue: Vec::new(),
            current: Some(0),
            status: Status::Pausing,
            position_ms: 3000,
            duration_ms: None,
            repeat: Repeat::Off,
            order: Order::Sequential,
            error: None,
        });
        assert!(matches!(
            app.playback_command(KeyCode::Left),
            Some(Command::Seek { position_ms: 0 })
        ));
        assert!(matches!(
            app.playback_command(KeyCode::Right),
            Some(Command::Seek { position_ms: 8000 })
        ));
        app.snapshot.as_mut().unwrap().status = Status::Playing;
        assert!(matches!(
            app.playback_command(KeyCode::Right),
            Some(Command::Seek { .. })
        ));
        app.snapshot.as_mut().unwrap().status = Status::Stopping;
        assert!(app.playback_command(KeyCode::Right).is_none());
    }

    #[test]
    fn playback_uses_case_to_distinguish_next_and_previous() {
        let mut app = App::new(None, Vec::new(), None);
        app.snapshot = Some(Snapshot {
            revision: 0,
            queue: Vec::new(),
            current: None,
            status: Status::Stopping,
            position_ms: 0,
            duration_ms: None,
            repeat: Repeat::Off,
            order: Order::Sequential,
            error: None,
        });
        assert!(app.playback_command(KeyCode::Char('N')).is_none());
        app.focus = Focus::Playback;
        assert!(matches!(
            app.playback_command(KeyCode::Char('n')),
            Some(Command::Next)
        ));
        for key in ['N', 'p'] {
            assert!(matches!(
                app.playback_command(KeyCode::Char(key)),
                Some(Command::Previous)
            ));
        }
    }
}
