use super::{
    ipc::Connection,
    library::{self, FACETS, Library},
    recursive_scan::Scan,
};
use crate::protocol::{Command, Message, Order, Repeat, Snapshot, Status, Track};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListState, Padding, Paragraph, Row, Table, TableState,
        Wrap,
    },
};
use std::{
    io,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError},
    },
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

use crate::storage::config::{Config, ConfigStore};

impl Config {
    fn duration(&self, level: Level) -> Duration {
        Duration::from_secs(match level {
            Level::Info => self.info_seconds,
            Level::Warning => self.warning_seconds,
            Level::Error => self.error_seconds,
        })
    }
}

type NoticeConfig = Config;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Playing,
    Scanned,
    Scan,
}

impl Page {
    const TABS: [(Self, &str); 3] = [
        (Self::Playing, "Playing"),
        (Self::Scanned, "Scanned"),
        (Self::Scan, "Settings"),
    ];

    fn step(self, offset: usize) -> Self {
        let index = Self::TABS
            .iter()
            .position(|(page, _)| *page == self)
            .unwrap();
        Self::TABS[(index + offset) % Self::TABS.len()].0
    }

    fn main_focus(self) -> Focus {
        if self == Self::Scanned {
            Focus::Filter(0)
        } else {
            Focus::Main
        }
    }
}

enum ScanDialog {
    Edit { index: Option<usize>, text: String },
    Delete(usize),
    ConfirmScan,
    Running(Scan),
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
    Filter(usize),
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
    page: Page,
    library: Library,
    library_load: Option<Receiver<io::Result<Library>>>,
    scan_cursor: usize,
    scan_dialog: Option<ScanDialog>,
    store: Option<ConfigStore>,
    config_valid: bool,
    initialized: bool,
    pending: Option<u64>,
}

impl App {
    pub fn new(
        source: Option<PathBuf>,
        tracks: Vec<Track>,
        connection: Option<Connection>,
    ) -> Self {
        let store_result = ConfigStore::user();
        let config_error = store_result.as_ref().err().map(ToString::to_string);
        let store = store_result.ok();
        let loaded = store.as_ref().map(ConfigStore::load);
        let config_valid = matches!(loaded, Some(Ok(_)));
        let config_error = config_error.or_else(|| {
            loaded
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .map(ToString::to_string)
        });
        let mut app = Self {
            source,
            tracks,
            cursor: 0,
            focus: Focus::Main,
            connection,
            snapshot: None,
            updated: Instant::now(),
            info: None,
            notice_config: loaded.and_then(Result::ok).unwrap_or_default(),
            store,
            config_valid,
            page: Page::Playing,
            library: Library::default(),
            library_load: None,
            scan_cursor: 0,
            scan_dialog: None,
            confirm: false,
            initialized: false,
            pending: None,
        };
        if let Some(error) = config_error {
            app.show_notice(Level::Error, format!("Could not load config: {error}"));
        } else if app.source.is_some() {
            app.show_notice(Level::Info, format!("{} tracks found", app.tracks.len()));
        }
        app
    }

    pub fn run(&mut self) -> io::Result<()> {
        self.reload_library();
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
        if self.source.is_none() && self.connection.is_none() {
            self.show_notice(Level::Warning, "No daemon available to attach");
        }
    }

    fn event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
        loop {
            self.poll_connection();
            terminal.draw(|frame| self.render(frame))?;
            let scan_was_running = matches!(self.scan_dialog, Some(ScanDialog::Running(_)));
            if event::poll(Duration::from_millis(75))?
                && let Event::Key(key) = event::read()?
                && key.kind != KeyEventKind::Release
                && !(scan_was_running
                    && key.kind == KeyEventKind::Repeat
                    && key.code == KeyCode::Enter)
                && self.key(key.code)
            {
                return Ok(());
            }
            self.poll_library();
            if scan_was_running {
                self.poll_scan();
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
            if self
                .snapshot
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(&old.queue, &state.queue))
            {
                self.tracks.clone_from(&state.queue);
                self.cursor = self.cursor.min(self.tracks.len().saturating_sub(1));
            }
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
        let (tracks, cursor) = self.songs();
        if tracks.is_empty() {
            return;
        }
        let queue = tracks.to_vec();
        match Connection::start() {
            Ok(connection) => {
                self.connection = Some(connection);
                self.pending = None;
                self.snapshot = None;
                self.send(Command::ReplaceAndPlay {
                    queue,
                    selected: cursor,
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
        if self.scan_dialog.is_some() {
            self.scan_dialog_key(key);
            return false;
        }
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
        match (key, self.focus) {
            (KeyCode::Up | KeyCode::Down, Focus::Filter(index)) => {
                let filter = &mut self.library.filters[index];
                filter.cursor = move_cursor(filter.cursor, filter.values.len() + 1, key);
                return false;
            }
            (KeyCode::Enter, Focus::Filter(index)) => {
                self.library.apply(index);
                return false;
            }
            _ => {}
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
                    Focus::Sidebar => self.page.main_focus(),
                    Focus::Filter(index) if index + 1 < FACETS.len() => Focus::Filter(index + 1),
                    Focus::Filter(_) => Focus::Main,
                    Focus::Main => Focus::Playback,
                    Focus::Playback => Focus::Sidebar,
                }
            }
            KeyCode::Up if self.focus == Focus::Sidebar => {
                self.page = self.page.step(Page::TABS.len() - 1)
            }
            KeyCode::Down if self.focus == Focus::Sidebar => self.page = self.page.step(1),
            KeyCode::Up if self.focus == Focus::Main && self.page == Page::Scan => {
                self.scan_cursor = self.scan_cursor.saturating_sub(1);
            }
            KeyCode::Down if self.focus == Focus::Main && self.page == Page::Scan => {
                self.scan_cursor =
                    (self.scan_cursor + 1).min(self.notice_config.scan_directories.len() * 2 + 1);
            }
            KeyCode::Enter if self.focus == Focus::Main && self.page == Page::Scan => {
                self.scan_activate()
            }
            KeyCode::Up | KeyCode::Down if self.focus == Focus::Main => {
                let (songs, cursor) = self.songs();
                let cursor = move_cursor(cursor, songs.len(), key);
                if self.page == Page::Scanned {
                    self.library.cursor = cursor;
                } else {
                    self.cursor = cursor;
                }
            }
            KeyCode::Enter if self.focus == Focus::Main => self.play_selection(),
            KeyCode::Enter if self.focus == Focus::Sidebar => self.focus = self.page.main_focus(),
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
        let song_list_seek = self.focus == Focus::Main
            && self.page != Page::Scan
            && state.current.is_some()
            && matches!(state.status, Status::Playing | Status::Pausing)
            && matches!(key, KeyCode::Left | KeyCode::Right);
        if self.focus != Focus::Playback && !song_list_seek {
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
            Constraint::Min(5),
            Constraint::Length(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [sidebar, main] =
            Layout::horizontal([Constraint::Length(13), Constraint::Min(0)]).areas(top);
        frame.render_widget(
            Paragraph::new(
                Page::TABS
                    .map(|(page, label)| {
                        let prefix = if self.page == page { "> " } else { "  " };
                        Line::from(format!("{prefix}{label}"))
                    })
                    .to_vec(),
            )
            .block(self.panel("Tabs".into(), Focus::Sidebar)),
            sidebar,
        );
        match self.page {
            Page::Playing => self.render_list(frame, main),
            Page::Scanned => self.render_library(frame, main),
            Page::Scan => self.render_scan(frame, main),
        }
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
            Focus::Filter(_) => "[↑/↓] Move   [Enter] Filter",
            Focus::Main if self.page == Page::Scan => "[↑/↓] Select   [Enter] Activate",
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
        if let Some(dialog) = &self.scan_dialog {
            self.render_scan_dialog(frame, dialog);
        }
        if self.confirm {
            render_dialog(
                frame,
                "Quit",
                "Stop playback and exit?",
                "[Enter/Y] Confirm    [Esc/N] Cancel",
            );
        }
    }

    fn scan_activate(&mut self) {
        let count = self.notice_config.scan_directories.len();
        self.scan_dialog = if self.scan_cursor < count * 2 {
            let index = self.scan_cursor / 2;
            if self.scan_cursor.is_multiple_of(2) {
                Some(ScanDialog::Edit {
                    index: Some(index),
                    text: self.notice_config.scan_directories[index].clone(),
                })
            } else {
                Some(ScanDialog::Delete(index))
            }
        } else if self.scan_cursor == count * 2 {
            Some(ScanDialog::Edit {
                index: None,
                text: String::new(),
            })
        } else {
            Some(ScanDialog::ConfirmScan)
        };
    }

    fn scan_dialog_key(&mut self, key: KeyCode) {
        let Some(mut dialog) = self.scan_dialog.take() else {
            return;
        };
        match (&mut dialog, key) {
            (_, KeyCode::Esc) => return,
            (ScanDialog::Delete(index), KeyCode::Enter | KeyCode::Char('y')) => {
                let mut config = self.notice_config.clone();
                config.scan_directories.remove(*index);
                if self.save_config(config) {
                    self.scan_cursor = self.scan_cursor.saturating_sub(1);
                    return;
                }
            }
            (ScanDialog::Delete(_), KeyCode::Char('n'))
            | (ScanDialog::ConfirmScan, KeyCode::Char('n')) => return,
            (ScanDialog::Running(_), KeyCode::Enter | KeyCode::Char('n')) => return,
            (ScanDialog::ConfirmScan, KeyCode::Enter | KeyCode::Char('y')) => {
                self.start_scan();
                return;
            }
            (ScanDialog::Edit { index, text }, KeyCode::Enter) if !text.trim().is_empty() => {
                let mut config = self.notice_config.clone();
                if let Some(index) = index {
                    config.scan_directories[*index] = text.clone();
                } else {
                    config.scan_directories.push(text.clone());
                }
                if self.save_config(config) {
                    return;
                }
            }
            (ScanDialog::Edit { text, .. }, KeyCode::Backspace) => {
                text.pop();
            }
            (ScanDialog::Edit { text, .. }, KeyCode::Char(c)) => text.push(c),
            _ => {}
        }
        self.scan_dialog = Some(dialog);
    }

    fn start_scan(&mut self) {
        let Some(store) = self.store.as_ref().filter(|_| self.config_valid) else {
            self.show_notice(
                Level::Error,
                "Config could not be loaded; scan was not started",
            );
            return;
        };
        match Scan::start(
            store.database(),
            self.notice_config.scan_directories.clone(),
        ) {
            Ok(scan) => self.scan_dialog = Some(ScanDialog::Running(scan)),
            Err(error) => self.show_notice(Level::Error, format!("Could not start scan: {error}")),
        }
    }

    fn poll_scan(&mut self) {
        let Some(ScanDialog::Running(scan)) = &self.scan_dialog else {
            return;
        };
        let Some(result) = scan.result() else {
            return;
        };
        self.scan_dialog = None;
        match result.and_then(|writer| writer.publish()) {
            Ok(()) => {
                self.show_notice(Level::Info, "Scan complete");
                self.reload_library();
            }

            Err(error) => self.show_notice(Level::Error, format!("Scan failed: {error}")),
        }
    }

    fn save_config(&mut self, config: Config) -> bool {
        if !self.config_valid {
            self.show_notice(
                Level::Error,
                "Config could not be loaded; changes were not saved",
            );
            return false;
        }
        let result = self
            .store
            .as_ref()
            .expect("valid config has a store")
            .save_if_unchanged(&self.notice_config, &config);
        match result {
            Ok(()) => {
                self.notice_config = config;
                true
            }
            Err(error) => {
                self.show_notice(Level::Error, format!("Could not save config: {error}"));
                false
            }
        }
    }

    fn render_scan(&self, frame: &mut Frame, area: Rect) {
        use ratatui::widgets::ListItem;
        let mut items = Vec::new();
        for path in &self.notice_config.scan_directories {
            items.push(ListItem::new(format!("  {path}")));
            items.push(ListItem::new("  [ Delete ]"));
            items.push(ListItem::new(""));
        }
        if self.notice_config.scan_directories.is_empty() {
            items.push(ListItem::new(""));
        }
        items.push(ListItem::new("  [ Add directory ]"));
        items.push(ListItem::new(""));
        items.push(ListItem::new("  [ Scan ]"));
        let count = self.notice_config.scan_directories.len();
        let selected = if self.scan_cursor < count * 2 {
            self.scan_cursor + self.scan_cursor / 2
        } else {
            count * 3 + usize::from(count == 0) + (self.scan_cursor - count * 2) * 2
        };
        let highlight = if self.focus == Focus::Main {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        };
        let list = List::new(items)
            .block(self.panel("Recursive Scan Directories".into(), Focus::Main))
            .highlight_style(highlight);
        frame.render_stateful_widget(
            list,
            area,
            &mut ListState::default().with_selected(Some(selected)),
        );
    }

    fn render_scan_dialog(&self, frame: &mut Frame, dialog: &ScanDialog) {
        let (title, text, buttons) = match dialog {
            ScanDialog::ConfirmScan => (
                "Scan directories",
                "Scan directories and replace the music database?".into(),
                "[Enter/Y] Scan    [Esc/N] Cancel",
            ),
            ScanDialog::Running(scan) => (
                "Scanning directories",
                scan.directory().map_or_else(
                    || "Starting scan…".into(),
                    |path| {
                        path_tail(
                            &path.to_string_lossy(),
                            frame.area().width.min(60).saturating_sub(4) as usize,
                        )
                    },
                ),
                "[Enter/Esc/N] Cancel",
            ),
            ScanDialog::Delete(index) => (
                "Delete directory",
                format!(
                    "Delete this scan directory?\n{}",
                    path_tail(
                        &self.notice_config.scan_directories[*index],
                        frame.area().width.min(60).saturating_sub(4) as usize
                    )
                ),
                "[Enter/Y] Delete    [Esc/N] Cancel",
            ),
            ScanDialog::Edit { index, text } => (
                if index.is_some() {
                    "Edit directory"
                } else {
                    "Add directory"
                },
                format!(
                    "Directory path:\n{}",
                    path_tail(text, frame.area().width.min(60).saturating_sub(4) as usize)
                ),
                "[Enter] Save    [Esc] Cancel",
            ),
        };
        render_dialog(frame, title, &text, buttons);
    }

    fn reload_library(&mut self) {
        let result = self
            .store
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Config directory unavailable"))
            .and_then(|store| library::load(store.database()));
        self.library_load = match result {
            Ok(receiver) => Some(receiver),
            Err(error) => {
                self.library_load_failed(error);
                None
            }
        };
    }

    fn poll_library(&mut self) {
        let Some(receiver) = &self.library_load else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(library)) => self.library = library,
            Ok(Err(error)) => self.library_load_failed(error),
            Err(TryRecvError::Disconnected) => self.library_load_failed("loader disconnected"),
            Err(TryRecvError::Empty) => return,
        }
        self.library_load = None;
    }

    fn library_load_failed(&mut self, error: impl std::fmt::Display) {
        self.library = Library::default();
        self.show_notice(
            Level::Error,
            format!("Could not load database.jsonl: {error}"),
        );
    }

    fn songs(&self) -> (&[Track], usize) {
        if self.page == Page::Scanned {
            (&self.library.songs, self.library.cursor)
        } else {
            (&self.tracks, self.cursor)
        }
    }

    fn render_library(&self, frame: &mut Frame, area: Rect) {
        let [facets, songs] =
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);
        let columns = Layout::horizontal([Constraint::Fill(1); FACETS.len()]).split(facets);
        for (index, name) in FACETS.into_iter().enumerate() {
            let focus = Focus::Filter(index);
            let filter = &self.library.filters[index];
            let items = std::iter::once("All")
                .chain(filter.values.iter().map(String::as_str))
                .enumerate()
                .map(|(row, label)| {
                    let marker = if row == filter.selected { "*" } else { " " };
                    format!("{marker} {label}")
                });
            let list = List::new(items)
                .block(self.panel(name.into(), focus))
                .highlight_style(self.selection_style(focus));
            let mut state = ListState::default().with_selected(Some(filter.cursor));
            frame.render_stateful_widget(list, columns[index], &mut state);
        }
        self.render_list(frame, songs);
    }

    fn selection_style(&self, focus: Focus) -> Style {
        if self.focus == focus {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        }
    }

    fn render_list(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let (tracks, cursor) = self.songs();
        let current = self
            .snapshot
            .as_ref()
            .and_then(|state| state.current.and_then(|index| state.queue.get(index)));
        let rows = tracks.iter().map(|track| {
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
        let name = match self.page {
            Page::Scanned => "Songs".into(),
            _ => self
                .source
                .as_ref()
                .map_or_else(|| "Queue".into(), |source| source.display().to_string()),
        };
        let mut state = TableState::default().with_selected((!tracks.is_empty()).then_some(cursor));
        let time_width = tracks
            .iter()
            .filter_map(|track| track.duration_ms)
            .map(|duration| clock(duration).len().min(u16::MAX as usize) as u16)
            .max()
            .unwrap_or(0)
            .max(5);
        let track_width = tracks
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
        let table = Table::new(rows, widths)
            .header(
                Row::new(["", "Track", "Title", "Time", "Artist", "Album", "Genre"])
                    .style(Style::default().fg(Color::DarkGray)),
            )
            .block(self.panel(name, Focus::Main))
            .row_highlight_style(self.selection_style(Focus::Main));
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

fn move_cursor(cursor: usize, count: usize, key: KeyCode) -> usize {
    if key == KeyCode::Up {
        cursor.saturating_sub(1)
    } else {
        (cursor + 1).min(count.saturating_sub(1))
    }
}

fn path_tail(path: &str, width: usize) -> String {
    if Line::from(path).width() <= width {
        return path.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut tail = String::new();
    let mut remaining = width - 1;
    for character in path.chars().rev() {
        let character_width = Line::from(character.to_string()).width();
        if character_width > remaining {
            break;
        }
        remaining -= character_width;
        tail.push(character);
    }
    format!("…{}", tail.chars().rev().collect::<String>())
}

fn render_dialog(frame: &mut Frame, title: &str, text: &str, buttons: &str) {
    let height = 9.min(frame.area().height);
    let width = 60.min(frame.area().width);
    let area = Rect::new(
        frame.area().x + (frame.area().width - width) / 2,
        frame.area().y + (frame.area().height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    let block = Block::bordered().title(title).padding(Padding::uniform(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [content, actions] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), content);
    frame.render_widget(
        Paragraph::new(buttons)
            .centered()
            .style(Style::default().add_modifier(Modifier::BOLD)),
        actions,
    );
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

    fn wait_for_library(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.library_load.is_some() && Instant::now() < deadline {
            app.poll_library();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(app.library_load.is_none());
    }

    #[test]
    fn scanned_tab_filters_after_enter_and_clears_on_load_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(None, Vec::new(), None);
        let store = ConfigStore::new(dir.path().join("config.json"));
        let track = |title: &str, genre: &str| crate::storage::database::ScanTrack {
            path: dir.path().join(title),
            title: title.into(),
            duration_ms: None,
            artists: vec![],
            album: None,
            genres: vec![genre.into()],
            track_number: None,
        };
        let mut writer = store.database().stage().unwrap();
        writer.write(&track("First", "Jazz")).unwrap();
        writer.write(&track("Second", "Rock")).unwrap();
        writer.publish().unwrap();
        app.store = Some(store);
        app.reload_library();
        assert!(app.library.songs.is_empty());
        wait_for_library(&mut app);

        app.key(KeyCode::Tab);
        app.key(KeyCode::Tab);
        app.key(KeyCode::Down);
        app.key(KeyCode::Enter);
        assert!(app.page == Page::Scanned);
        assert!(app.focus == Focus::Filter(0));
        app.key(KeyCode::Down);
        assert_eq!(app.library.songs.len(), 2);
        app.key(KeyCode::Enter);
        assert_eq!(app.library.songs[0].title, "First");
        assert_eq!(app.library.songs.len(), 1);
        for _ in 0..3 {
            app.key(KeyCode::Tab);
        }
        assert!(app.focus == Focus::Main);
        assert_eq!(app.songs().0.len(), 1);

        std::fs::write(dir.path().join("database.jsonl"), b"{").unwrap();
        app.reload_library();
        wait_for_library(&mut app);
        assert!(app.library.songs.is_empty());
        let notice = app.info.as_ref().unwrap();
        assert_eq!(notice.level, Level::Error);
        assert!(notice.text.contains("database.jsonl"));
    }

    #[test]
    fn scan_requires_confirmation_and_does_not_change_targets() {
        let mut app = App::new(None, Vec::new(), None);
        app.page = Page::Scan;
        app.focus = Focus::Main;
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir(&music).unwrap();
        std::fs::write(music.join("song.flac"), []).unwrap();
        let target = music.display().to_string();
        app.store = Some(ConfigStore::new(dir.path().join("config.json")));
        app.config_valid = true;
        app.notice_config.scan_directories = vec![target.clone()];
        app.scan_cursor = 3;
        for cancel in [KeyCode::Esc, KeyCode::Char('n')] {
            app.key(KeyCode::Enter);
            assert!(matches!(app.scan_dialog, Some(ScanDialog::ConfirmScan)));
            app.key(cancel);
            assert!(app.scan_dialog.is_none());
            assert_eq!(
                app.notice_config.scan_directories,
                std::slice::from_ref(&target)
            );
        }
        let database = dir.path().join("database.jsonl");
        std::fs::write(&database, b"previous database").unwrap();
        for confirm in [KeyCode::Enter, KeyCode::Char('y')] {
            app.key(KeyCode::Enter);
            app.key(confirm);
            assert!(matches!(app.scan_dialog, Some(ScanDialog::Running(_))));
            app.key(KeyCode::Esc);
            app.poll_scan();
            assert!(app.scan_dialog.is_none());
            assert_eq!(
                app.notice_config.scan_directories,
                std::slice::from_ref(&target)
            );
            assert_eq!(std::fs::read(&database).unwrap(), b"previous database");
        }
        app.key(KeyCode::Enter);
        app.key(KeyCode::Enter);
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.scan_dialog.is_some() && Instant::now() < deadline {
            app.poll_scan();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(app.scan_dialog.is_none());
        assert_eq!(app.info.as_ref().unwrap().text, "Scan complete");
        wait_for_library(&mut app);
        assert_eq!(app.library.songs[0].title, "song.flac");
    }

    #[test]
    fn scan_targets_can_be_added_edited_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(None, Vec::new(), None);
        app.store = Some(ConfigStore::new(dir.path().join("config.json")));
        app.config_valid = true;
        app.notice_config = Config::default();
        app.focus = Focus::Sidebar;
        app.key(KeyCode::Down);
        app.key(KeyCode::Down);
        app.key(KeyCode::Enter);
        app.key(KeyCode::Enter);
        for c in "/music".chars() {
            app.key(KeyCode::Char(c));
        }
        app.key(KeyCode::Enter);
        assert_eq!(app.notice_config.scan_directories, ["/music"]);
        app.key(KeyCode::Enter);
        app.key(KeyCode::Backspace);
        app.key(KeyCode::Char('x'));
        app.key(KeyCode::Enter);
        assert_eq!(app.notice_config.scan_directories, ["/musix"]);
        app.key(KeyCode::Down);
        app.key(KeyCode::Enter);
        app.key(KeyCode::Esc);
        assert_eq!(app.notice_config.scan_directories.len(), 1);
        app.key(KeyCode::Enter);
        app.key(KeyCode::Enter);
        assert!(app.notice_config.scan_directories.is_empty());
        assert!(
            app.store
                .as_ref()
                .unwrap()
                .load()
                .unwrap()
                .scan_directories
                .is_empty()
        );
    }

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
            queue: Default::default(),
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
                queue: Default::default(),
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
            queue: Default::default(),
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
            queue: Default::default(),
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
