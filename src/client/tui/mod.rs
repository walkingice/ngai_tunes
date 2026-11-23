mod render;

use super::{
    ipc::Connection,
    library::{self, Facet, Library},
    recursive_scan::Scan,
    scan,
};
use crate::protocol::{Command, Failure, Message, Order, Repeat, Snapshot, Status, Track};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::style::Color;
use std::{
    io,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

pub(crate) fn run(directory: Option<PathBuf>) -> io::Result<()> {
    let tracks = if let Some(path) = &directory {
        scan::scan(path)?
    } else {
        Vec::new()
    };
    App::new(directory, tracks, Connection::connect().ok()).run()
}

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

use crate::storage::config::{ConfigStore, TuiConfig};

impl TuiConfig {
    fn duration(&self, level: Level) -> Duration {
        Duration::from_secs(match level {
            Level::Info => self.info_seconds,
            Level::Warning => self.warning_seconds,
            Level::Error => self.error_seconds,
        })
    }
}

enum ConfigLoad {
    Loaded(ConfigStore),
    Failed {
        store: ConfigStore,
        error: io::Error,
    },
    Unavailable(io::Error),
}

impl ConfigLoad {
    fn load(store: io::Result<ConfigStore>) -> (Self, TuiConfig) {
        let store = match store {
            Ok(store) => store,
            Err(error) => return (Self::Unavailable(error), TuiConfig::default()),
        };
        match store.load() {
            Ok(config) => (Self::Loaded(store), config.tui),
            Err(error) => (Self::Failed { store, error }, TuiConfig::default()),
        }
    }

    fn store(&self) -> Option<&ConfigStore> {
        match self {
            Self::Loaded(store) | Self::Failed { store, .. } => Some(store),
            Self::Unavailable(_) => None,
        }
    }

    fn error(&self) -> Option<&io::Error> {
        match self {
            Self::Loaded(_) => None,
            Self::Failed { error, .. } | Self::Unavailable(error) => Some(error),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Playing,
    Scanned,
    Scan,
    Browsing,
}

impl Page {
    const TABS: [(Self, &str); 4] = [
        (Self::Playing, "Playing"),
        (Self::Scanned, "Scanned"),
        (Self::Browsing, "Browsing"),
        (Self::Scan, "Settings"),
    ];

    fn step(self, tabs: &[(Self, &str)], offset: usize) -> Self {
        let index = tabs.iter().position(|(page, _)| *page == self).unwrap();
        tabs[(index + offset) % tabs.len()].0
    }

    fn main_focus(self) -> Focus {
        if self == Self::Scanned {
            Focus::Filter(Facet::Genre)
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
    fn visible(&self, config: &TuiConfig) -> bool {
        self.created.elapsed() < config.duration(self.level)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Sidebar,
    Filter(Facet),
    Main,
    Playback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsRow {
    Edit(usize),
    Delete(usize),
    Add,
    Scan,
    Spacer,
}

fn settings_rows(count: usize) -> impl Iterator<Item = SettingsRow> {
    (0..count)
        .flat_map(|index| {
            [
                SettingsRow::Edit(index),
                SettingsRow::Delete(index),
                SettingsRow::Spacer,
            ]
        })
        .chain((count == 0).then_some(SettingsRow::Spacer))
        .chain([SettingsRow::Add, SettingsRow::Spacer, SettingsRow::Scan])
}

struct App {
    source: Option<PathBuf>,
    directory_tracks: Vec<Track>,
    directory_cursor: usize,
    tracks: Vec<Track>,
    cursor: usize,
    focus: Focus,
    connection: Option<Connection>,
    snapshot: Option<Snapshot>,
    updated: Instant,
    info: Option<Notice>,
    config: TuiConfig,
    page: Page,
    scroll_offsets: render::ScrollOffsets,
    library: Library,
    library_loaded: bool,
    library_load: Option<Receiver<io::Result<Library>>>,
    scan_cursor: usize,
    scan_dialog: Option<ScanDialog>,
    config_load: ConfigLoad,
    initialized: bool,
    pending: Option<u64>,
    pending_play: Option<u64>,
}

impl App {
    fn new(source: Option<PathBuf>, tracks: Vec<Track>, connection: Option<Connection>) -> Self {
        let (config_load, config) = ConfigLoad::load(ConfigStore::user());
        let mut app = Self {
            page: if source.is_some() {
                Page::Browsing
            } else {
                Page::Playing
            },
            source,
            directory_tracks: tracks,
            directory_cursor: 0,
            tracks: Vec::new(),
            cursor: 0,
            focus: Focus::Main,
            connection,
            snapshot: None,
            updated: Instant::now(),
            info: None,
            config,
            config_load,
            library: Library::default(),
            library_loaded: false,
            scroll_offsets: render::ScrollOffsets::default(),
            library_load: None,
            scan_cursor: 0,
            scan_dialog: None,
            initialized: false,
            pending: None,
            pending_play: None,
        };
        if let Some(error) = app.config_load.error() {
            app.show_notice(Level::Error, format!("Could not load config: {error}"));
        } else if app.source.is_some() {
            app.show_notice(
                Level::Info,
                format!("{} tracks found", app.directory_tracks.len()),
            );
        }
        app
    }

    fn run(&mut self) -> io::Result<()> {
        self.reload_library();
        self.initialize();
        let mut terminal = ratatui::try_init()?;
        let result = self.event_loop(&mut terminal);
        let restored = ratatui::try_restore();
        self.save_library_position();
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
        if self.source.is_none()
            && self
                .config_load
                .store()
                .is_some_and(|store| store.database().exists())
        {
            self.page = Page::Scanned;
            self.focus = self.page.main_focus();
        }
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
                && !(key.kind == KeyEventKind::Repeat
                    && (key.code == KeyCode::Char('q')
                        || (scan_was_running && key.code == KeyCode::Enter)))
                && self.key_event(key.code, key.modifiers)
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
        self.handle_reply(message.request_id, message.error);
        if let Some(state) = message.state {
            self.apply_snapshot(state);
        }
    }

    fn handle_reply(&mut self, request_id: Option<u64>, error: Option<Failure>) {
        if request_id == self.pending && self.pending.is_some() {
            self.pending = None;
        }
        let play_response = self.pending_play.is_some() && request_id == self.pending_play;
        if play_response {
            self.pending_play = None;
        }
        if let Some(error) = error {
            if play_response {
                self.sync_queue();
            }
            self.show_notice(Level::Error, error.message);
        }
    }

    fn apply_snapshot(&mut self, state: Snapshot) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.revision > state.revision)
        {
            return;
        }
        let queue_changed = self
            .snapshot
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(&old.queue, &state.queue));
        if !self.initialized && self.pending_play.is_none() {
            self.cursor = state.current.unwrap_or(0);
        }
        if let Some(error) = &state.error
            && self.snapshot.as_ref().and_then(|old| old.error.as_ref()) != Some(error)
        {
            self.show_notice(Level::Error, error.clone());
        }
        self.initialized = true;
        self.updated = Instant::now();
        self.snapshot = Some(state);
        if self.pending_play.is_none() && queue_changed {
            self.sync_queue();
        }
    }

    fn sync_queue(&mut self) {
        if let Some(state) = &self.snapshot {
            self.tracks.clone_from(&state.queue);
        } else {
            self.tracks.clear();
        }
        self.cursor = self.cursor.min(self.tracks.len().saturating_sub(1));
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
        if self.pending_play.take().is_some() {
            self.sync_queue();
        }
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
        let mut connection = match Connection::start() {
            Ok(connection) => connection,
            Err(error) => {
                self.show_notice(Level::Error, error.to_string());
                return;
            }
        };
        let old_queue = std::mem::replace(&mut self.tracks, queue.clone());
        let old_cursor = std::mem::replace(&mut self.cursor, cursor);
        match connection.send(Command::ReplaceAndPlay {
            queue,
            selected: cursor,
        }) {
            Ok(id) => {
                self.connection = Some(connection);
                self.pending = Some(id);
                self.pending_play = Some(id);
                self.snapshot = None;
                self.initialized = true;
            }
            Err(error) => {
                self.tracks = old_queue;
                self.cursor = old_cursor;
                self.show_notice(Level::Error, error.to_string());
            }
        }
    }

    fn finish(&mut self, command: Command) {
        if let Ok(mut connection) = Connection::connect() {
            connection.finish(command);
        }
    }

    fn quit_key_stops_playback(&self) -> bool {
        self.snapshot
            .as_ref()
            .map_or(self.connection.is_some(), |state| {
                matches!(state.status, Status::Playing | Status::Pausing)
            })
    }

    fn key_event(&mut self, key: KeyCode, modifiers: KeyModifiers) -> bool {
        let key = if self.scan_dialog.is_none() && modifiers == KeyModifiers::CONTROL {
            match key {
                KeyCode::Char('u') => KeyCode::PageUp,
                KeyCode::Char('d') => KeyCode::PageDown,
                key => key,
            }
        } else {
            key
        };
        self.key(key)
    }

    fn key(&mut self, key: KeyCode) -> bool {
        if self.scan_dialog.is_some() {
            self.scan_dialog_key(key);
            return false;
        }
        let key = match key {
            KeyCode::Char('h') => KeyCode::Left,
            KeyCode::Char('j') => KeyCode::Down,
            KeyCode::Char('k') => KeyCode::Up,
            KeyCode::Char('l') => KeyCode::Right,
            key => key,
        };
        match key {
            KeyCode::Esc => {
                self.finish(Command::ExitOnStopping);
                return true;
            }
            KeyCode::Char('q') if self.quit_key_stops_playback() => self.send(Command::Stop),
            KeyCode::Char('q') => {
                self.finish(Command::Quit);
                return true;
            }
            KeyCode::Char('T') => self.focus = Focus::Sidebar,
            KeyCode::Char('P') => self.focus = Focus::Playback,
            KeyCode::Char('S') if self.page != Page::Scan => self.focus = Focus::Main,
            KeyCode::Char('R') if self.page == Page::Scanned => self.library.reset_filters(),
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Filter(facet) => facet.next().map_or(Focus::Main, Focus::Filter),
                    Focus::Main => Focus::Playback,
                    Focus::Sidebar | Focus::Playback => self.page.main_focus(),
                }
            }
            KeyCode::Char(' ') if self.focus != Focus::Sidebar => self.send(Command::TogglePause),
            _ => match self.focus {
                Focus::Sidebar => self.sidebar_key(key),
                Focus::Filter(facet) => self.filter_key(facet, key),
                Focus::Main => self.main_key(key),
                Focus::Playback => self.playback_key(key),
            },
        }
        false
    }

    fn sidebar_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up => self.page = self.page.step(self.tabs(), self.tabs().len() - 1),
            KeyCode::Down => self.page = self.page.step(self.tabs(), 1),
            KeyCode::Enter => self.focus = self.page.main_focus(),
            _ => {}
        }
    }

    fn filter_key(&mut self, facet: Facet, key: KeyCode) {
        match key {
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End => {
                let page_size = self.scroll_offsets.facet_height.get();
                let filter = self.library.filter_mut(facet);
                filter.cursor = move_cursor(filter.cursor, filter.row_count(), key, page_size);
            }
            KeyCode::Enter => self.library.apply(facet),
            _ => {}
        }
    }

    fn main_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End
                if self.page == Page::Scan =>
            {
                let actions = settings_rows(self.config.scan_directories.len())
                    .filter(|row| *row != SettingsRow::Spacer)
                    .count();
                self.scan_cursor = move_cursor(
                    self.scan_cursor,
                    actions,
                    key,
                    self.scroll_offsets.main_height.get(),
                );
            }
            KeyCode::Enter if self.page == Page::Scan => self.scan_activate(),
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End => {
                let (songs, cursor) = self.songs();
                let cursor = move_cursor(
                    cursor,
                    songs.len(),
                    key,
                    self.scroll_offsets.main_height.get(),
                );
                match self.page {
                    Page::Scanned => self.library.cursor = cursor,
                    Page::Browsing => self.directory_cursor = cursor,
                    _ => self.cursor = cursor,
                }
            }
            KeyCode::Enter => self.play_selection(),
            _ => self.playback_key(key),
        }
    }

    fn playback_key(&mut self, key: KeyCode) {
        if let Some(command) = self.playback_command(key) {
            self.send(command);
        }
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

    fn scan_activate(&mut self) {
        let directories = &self.config.scan_directories;
        self.scan_dialog = settings_rows(directories.len())
            .filter(|row| *row != SettingsRow::Spacer)
            .nth(self.scan_cursor)
            .map(|row| match row {
                SettingsRow::Edit(index) => ScanDialog::Edit {
                    index: Some(index),
                    text: directories[index].clone(),
                },
                SettingsRow::Delete(index) => ScanDialog::Delete(index),
                SettingsRow::Add => ScanDialog::Edit {
                    index: None,
                    text: String::new(),
                },
                SettingsRow::Scan => ScanDialog::ConfirmScan,
                SettingsRow::Spacer => unreachable!("spacers are skipped"),
            });
    }

    fn scan_dialog_key(&mut self, key: KeyCode) {
        let Some(mut dialog) = self.scan_dialog.take() else {
            return;
        };
        match (&mut dialog, key) {
            (_, KeyCode::Esc) => return,
            (ScanDialog::Delete(index), KeyCode::Enter | KeyCode::Char('y')) => {
                let mut config = self.config.clone();
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
                let mut config = self.config.clone();
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
        let ConfigLoad::Loaded(store) = &self.config_load else {
            self.show_notice(
                Level::Error,
                "Config could not be loaded; scan was not started",
            );
            return;
        };
        match Scan::start(store.database(), self.config.scan_directories.clone()) {
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

    fn save_config(&mut self, config: TuiConfig) -> bool {
        let ConfigLoad::Loaded(store) = &self.config_load else {
            self.show_notice(
                Level::Error,
                "Config could not be loaded; changes were not saved",
            );
            return false;
        };
        let result = store.save_tui_if_unchanged(&self.config, &config);
        match result {
            Ok(()) => {
                self.config = config;
                true
            }
            Err(error) => {
                self.show_notice(Level::Error, format!("Could not save config: {error}"));
                false
            }
        }
    }

    fn save_library_position(&self) {
        if self.library_loaded
            && self.library_load.is_none()
            && let ConfigLoad::Loaded(store) = &self.config_load
        {
            let position = self.library.position();
            if position != self.config.scanned {
                // Cursor persistence is best effort and must not prevent exit.
                let _ = store.save_scanned(position);
            }
        }
    }

    fn reload_library(&mut self) {
        let result = self
            .config_load
            .store()
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
            Ok(Ok(mut library)) => {
                if !self.library_loaded {
                    library.restore_position(self.config.scanned);
                }
                self.library = library;
                self.library_loaded = true;
            }
            Ok(Err(error)) => self.library_load_failed(error),
            Err(TryRecvError::Disconnected) => self.library_load_failed("loader disconnected"),
            Err(TryRecvError::Empty) => return,
        }
        self.library_load = None;
    }

    fn library_load_failed(&mut self, error: impl std::fmt::Display) {
        self.library = Library::default();
        self.library_loaded = false;
        self.show_notice(
            Level::Error,
            format!("Could not load database.jsonl: {error}"),
        );
    }

    fn tabs(&self) -> &[(Page, &str)] {
        if self.source.is_some() {
            &Page::TABS
        } else {
            &[Page::TABS[0], Page::TABS[1], Page::TABS[3]]
        }
    }

    fn songs(&self) -> (&[Track], usize) {
        match self.page {
            Page::Scanned => (&self.library.songs, self.library.cursor),
            Page::Browsing => (&self.directory_tracks, self.directory_cursor),
            _ => (&self.tracks, self.cursor),
        }
    }
}

fn move_cursor(cursor: usize, count: usize, key: KeyCode, page_size: usize) -> usize {
    let distance = match key {
        KeyCode::PageUp | KeyCode::PageDown => page_size.max(1),
        _ => 1,
    };
    match key {
        KeyCode::Home => 0,
        KeyCode::End => count.saturating_sub(1),
        KeyCode::Up | KeyCode::PageUp => cursor.saturating_sub(distance),
        _ => cursor.saturating_add(distance).min(count.saturating_sub(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_keys_move_and_clamp_the_cursor() {
        let mut app = App::new(None, Vec::new(), None);
        app.page = Page::Scan;
        app.focus = Focus::Main;
        app.config.scan_directories = (0..10).map(|n| n.to_string()).collect();
        app.scroll_offsets.main_height.set(4);
        app.key_event(KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(app.scan_cursor, 4);
        app.key_event(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(app.scan_cursor, 8);
        app.key_event(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(app.scan_cursor, 4);
        app.key_event(KeyCode::PageUp, KeyModifiers::NONE);
        app.key_event(KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(app.scan_cursor, 0);
        assert_eq!(move_cursor(0, 0, KeyCode::PageDown, 4), 0);
        assert_eq!(move_cursor(9, 10, KeyCode::PageDown, 4), 9);
    }

    #[test]
    fn home_and_end_select_list_boundaries() {
        let mut app = App::new(None, Vec::new(), None);
        app.page = Page::Scan;
        app.focus = Focus::Main;
        app.config.scan_directories = vec!["one".into(), "two".into()];
        app.key(KeyCode::End);
        assert_eq!(app.scan_cursor, 5);
        app.key(KeyCode::Home);
        assert_eq!(app.scan_cursor, 0);
        assert_eq!(move_cursor(0, 0, KeyCode::End, 1), 0);
        assert_eq!(move_cursor(3, 10, KeyCode::End, 1), 9);
        assert_eq!(move_cursor(3, 10, KeyCode::Home, 1), 0);
    }

    #[test]
    fn settings_rows_keep_spacers_out_of_action_order() {
        use SettingsRow::*;
        assert_eq!(
            settings_rows(0).collect::<Vec<_>>(),
            [Spacer, Add, Spacer, Scan]
        );
        let actions: Vec<_> = settings_rows(2).filter(|row| *row != Spacer).collect();
        assert_eq!(actions, [Edit(0), Delete(0), Edit(1), Delete(1), Add, Scan]);
    }

    fn wait_for_library(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.library_load.is_some() && Instant::now() < deadline {
            app.poll_library();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(app.library_load.is_none());
    }

    #[test]
    fn scanned_position_round_trips_on_exit_and_defaults_invalid_indices() {
        use crate::storage::config::{FilterPosition, ScannedPosition};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let store = ConfigStore::new(path.clone());
        let mut writer = store.database().stage().unwrap();
        for title in ["First", "Second"] {
            writer
                .write(&crate::storage::database::ScanTrack {
                    path: dir.path().join(title),
                    title: title.into(),
                    duration_ms: None,
                    artists: vec!["Artist".into()],
                    album: Some("Album".into()),
                    genres: vec!["Genre".into()],
                    track_number: None,
                })
                .unwrap();
        }
        writer.publish().unwrap();
        let mut app = App::new(None, Vec::new(), None);
        (app.config_load, app.config) = ConfigLoad::load(Ok(store));
        app.reload_library();
        wait_for_library(&mut app);
        for facet in Facet::ALL {
            app.filter_key(facet, KeyCode::Down);
            app.filter_key(facet, KeyCode::Enter);
        }
        app.filter_key(Facet::Artist, KeyCode::Up);
        app.page = Page::Scanned;
        app.main_key(KeyCode::Down);
        let position = app.library.position();
        assert_eq!(
            position.artist,
            FilterPosition {
                cursor: 0,
                selected: 1
            }
        );
        assert_eq!(position.song, 1);
        assert!(!path.exists());
        app.save_library_position();

        let store = ConfigStore::new(path);
        assert_eq!(store.load().unwrap().tui.scanned, position);
        app.config_load = ConfigLoad::Loaded(store);
        app.library_loaded = false;
        app.config.scanned = ScannedPosition {
            album: FilterPosition {
                cursor: 99,
                selected: 99,
            },
            song: 99,
            ..position
        };
        app.reload_library();
        wait_for_library(&mut app);
        assert_eq!(
            app.library.position(),
            ScannedPosition {
                album: FilterPosition::default(),
                song: 0,
                ..position
            }
        );
    }

    #[test]
    fn scanned_tab_filters_after_enter_and_clears_on_load_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(None, Vec::new(), None);
        let store = ConfigStore::new(dir.path().join("config.json"));
        let track = |title: &str, artist: &str| crate::storage::database::ScanTrack {
            path: dir.path().join(title),
            title: title.into(),
            duration_ms: None,
            artists: vec![artist.into()],
            album: None,
            genres: vec![],
            track_number: None,
        };
        let mut writer = store.database().stage().unwrap();
        writer.write(&track("First", "A")).unwrap();
        writer.write(&track("Second", "B")).unwrap();
        writer.publish().unwrap();
        app.config_load = ConfigLoad::Loaded(store);
        app.reload_library();
        assert!(app.library.songs.is_empty());
        wait_for_library(&mut app);

        app.key(KeyCode::Char('T'));
        app.key(KeyCode::Down);
        app.key(KeyCode::Enter);
        assert!(app.page == Page::Scanned);
        assert!(app.focus == Focus::Filter(Facet::Genre));
        app.key(KeyCode::Tab);
        assert!(app.focus == Focus::Filter(Facet::Artist));
        app.library.cursor = 1;
        app.key(KeyCode::Down);
        assert_eq!(app.library.songs.len(), 2);
        assert_eq!(app.library.cursor, 1);
        app.key(KeyCode::Enter);
        assert_eq!(app.library.songs[0].title, "First");
        assert_eq!(app.library.songs.len(), 1);
        assert_eq!(app.library.cursor, 0);
        app.key(KeyCode::Up);
        assert_eq!(app.library.songs.len(), 1);
        app.key(KeyCode::Enter);
        assert_eq!(app.library.songs.len(), 2);
        app.key(KeyCode::Down);
        app.key(KeyCode::Enter);
        app.key(KeyCode::Tab);
        assert!(app.focus == Focus::Filter(Facet::Album));
        app.key(KeyCode::Tab);
        assert!(app.focus == Focus::Main);
        assert_eq!(app.songs().0.len(), 1);
        app.key(KeyCode::Char('R'));
        assert_eq!(app.songs().0.len(), 2);
        assert_eq!(app.library.filter(Facet::Artist).selected, 0);
        assert_eq!(app.library.filter(Facet::Artist).cursor, 0);

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
        app.config_load = ConfigLoad::Loaded(ConfigStore::new(dir.path().join("config.json")));
        app.config.scan_directories = vec![target.clone()];
        app.scan_cursor = 3;
        for cancel in [KeyCode::Esc, KeyCode::Char('n')] {
            app.key(KeyCode::Enter);
            assert!(matches!(app.scan_dialog, Some(ScanDialog::ConfirmScan)));
            app.key(cancel);
            assert!(app.scan_dialog.is_none());
            assert_eq!(app.config.scan_directories, std::slice::from_ref(&target));
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
            assert_eq!(app.config.scan_directories, std::slice::from_ref(&target));
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
        app.config_load = ConfigLoad::Loaded(ConfigStore::new(dir.path().join("config.json")));
        app.config = TuiConfig::default();
        app.focus = Focus::Sidebar;
        app.key(KeyCode::Down);
        app.key(KeyCode::Down);
        app.key(KeyCode::Enter);
        app.key(KeyCode::Enter);
        for c in "/music".chars() {
            app.key(KeyCode::Char(c));
        }
        app.key(KeyCode::Enter);
        assert_eq!(app.config.scan_directories, ["/music"]);
        app.key(KeyCode::Enter);
        app.key(KeyCode::Backspace);
        app.key(KeyCode::Char('x'));
        app.key(KeyCode::Enter);
        assert_eq!(app.config.scan_directories, ["/musix"]);
        app.key(KeyCode::Down);
        app.key(KeyCode::Enter);
        app.key(KeyCode::Esc);
        assert_eq!(app.config.scan_directories.len(), 1);
        app.key(KeyCode::Enter);
        app.key(KeyCode::Enter);
        assert!(app.config.scan_directories.is_empty());
        assert!(
            app.config_load
                .store()
                .unwrap()
                .load()
                .unwrap()
                .tui
                .scan_directories
                .is_empty()
        );
    }

    #[test]
    fn malformed_config_allows_reading_the_existing_library() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, b"{").unwrap();
        let store = ConfigStore::new(path.clone());
        store.database().stage().unwrap().publish().unwrap();
        let mut app = App::new(None, Vec::new(), None);
        (app.config_load, app.config) = ConfigLoad::load(Ok(store));
        assert!(app.config_load.error().is_some());
        assert_eq!(app.config, TuiConfig::default());

        app.initialize();
        assert!(app.page == Page::Scanned);
        app.reload_library();
        assert!(app.library_load.is_some());
        wait_for_library(&mut app);
        assert!(app.library.songs.is_empty());

        app.scan_dialog = Some(ScanDialog::Edit {
            index: None,
            text: "/music".into(),
        });
        app.scan_dialog_key(KeyCode::Enter);
        assert!(
            matches!(&app.scan_dialog, Some(ScanDialog::Edit { text, .. }) if text == "/music")
        );
        assert_eq!(app.config, TuiConfig::default());
        assert_eq!(std::fs::read(path).unwrap(), b"{");

        app.scan_dialog = Some(ScanDialog::ConfirmScan);
        app.scan_dialog_key(KeyCode::Enter);
        assert!(app.scan_dialog.is_none());
        assert_eq!(app.info.as_ref().unwrap().level, Level::Error);
    }

    #[test]
    fn initial_page_prefers_a_directory_then_the_scanned_database() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.json");
        let mut app = App::new(None, Vec::new(), None);
        app.config_load = ConfigLoad::Loaded(ConfigStore::new(config.clone()));
        app.initialize();
        assert!(app.page == Page::Playing);

        app.config_load
            .store()
            .unwrap()
            .database()
            .stage()
            .unwrap()
            .publish()
            .unwrap();
        app.initialize();
        assert!(app.page == Page::Scanned);
        assert!(app.focus == Focus::Filter(Facet::Genre));

        let mut app = App::new(Some(dir.path().into()), Vec::new(), None);
        app.config_load = ConfigLoad::Loaded(ConfigStore::new(config));
        app.initialize();
        assert!(app.page == Page::Browsing);
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
        let mut app = App::new(None, Vec::new(), None);
        app.tracks = tracks;
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        for (cursor, focus, fg, bg) in [
            (0, Focus::Main, Color::Black, Color::Cyan),
            (1, Focus::Main, Color::Black, Color::Cyan),
            (0, Focus::Main, Color::Black, Color::Cyan),
            (15, Focus::Main, Color::Black, Color::Cyan),
            (14, Focus::Main, Color::Black, Color::Cyan),
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
            if cursor == 14 || cursor == 15 {
                assert_eq!(selected_row, 2 + (cursor - 14) as u16);
            }
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
        let config: TuiConfig =
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
        let defaults: TuiConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(defaults.duration(Level::Info), Duration::from_secs(5));
        assert_eq!(defaults.duration(Level::Warning), Duration::from_secs(5));
        assert_eq!(defaults.duration(Level::Error), Duration::from_secs(10));
    }

    #[test]
    fn repeated_snapshot_error_does_not_refresh_notice() {
        let mut app = App::new(None, Vec::new(), None);
        let startup_notice = app.info.as_ref().map(|notice| notice.created);
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
        assert_eq!(
            app.info.as_ref().map(|notice| notice.created),
            startup_notice
        );
    }

    #[test]
    fn play_failure_restores_latest_queue_even_with_stale_snapshot() {
        let track = |title: &str| Track {
            path: format!("/{title}.flac").into(),
            title: title.into(),
            artist: None,
            album: None,
            track_number: None,
            duration_ms: None,
            genre: None,
        };
        let mut app = App::new(None, Vec::new(), None);
        app.tracks = vec![track("Replacement")];
        app.pending = Some(2);
        app.pending_play = Some(1);
        let state = Snapshot {
            revision: 2,
            queue: Arc::new(vec![track("Daemon queue")]),
            current: Some(0),
            status: Status::Playing,
            position_ms: 0,
            duration_ms: None,
            repeat: Repeat::Off,
            order: Order::Sequential,
            error: None,
        };
        app.receive(Message {
            request_id: None,
            error: None,
            state: Some(state.clone()),
        });
        assert_eq!(app.tracks[0].title, "Replacement");
        app.receive(Message {
            request_id: Some(1),
            error: Some(Failure {
                code: "invalid_command".into(),
                message: "Upload failed".into(),
            }),
            state: Some(Snapshot {
                revision: 1,
                ..state
            }),
        });
        assert_eq!(app.pending_play, None);
        assert_eq!(app.pending, Some(2));
        assert_eq!(app.snapshot.as_ref().unwrap().revision, 2);
        assert_eq!(app.tracks[0].title, "Daemon queue");
        app.receive(Message {
            request_id: Some(2),
            error: None,
            state: None,
        });
        assert_eq!(app.pending, None);
    }

    #[test]
    fn attach_without_daemon_opens_an_empty_queue_with_a_message() {
        let mut app = App::new(None, Vec::new(), None);
        app.initialize();
        assert_eq!(
            app.tabs()
                .iter()
                .map(|&(_, label)| label)
                .collect::<Vec<_>>(),
            ["Playing", "Scanned", "Settings"]
        );
        assert!(!app.key(KeyCode::Enter));
        assert!(app.tracks.is_empty());
        assert!(app.connection.is_none());
        assert_eq!(
            app.info.as_ref().unwrap().text,
            "No daemon available to attach"
        );
    }

    #[test]
    fn directory_scan_and_playing_queue_are_independent() {
        let track = |title: &str| Track {
            path: format!("/{title}.flac").into(),
            title: title.into(),
            artist: None,
            album: None,
            track_number: None,
            duration_ms: None,
            genre: None,
        };
        let mut app = App::new(Some(".".into()), vec![track("Local")], None);
        assert!(app.page == Page::Browsing);
        assert_eq!(
            app.tabs()
                .iter()
                .map(|&(_, label)| label)
                .collect::<Vec<_>>(),
            ["Playing", "Scanned", "Browsing", "Settings"]
        );
        assert_eq!(app.songs().0[0].title, "Local");
        app.page = Page::Playing;
        assert!(app.songs().0.is_empty());
        app.receive(Message {
            request_id: None,
            error: None,
            state: Some(Snapshot {
                revision: 1,
                queue: Arc::new(vec![track("First"), track("Current")]),
                current: Some(1),
                status: Status::Playing,
                position_ms: 0,
                duration_ms: None,
                repeat: Repeat::Off,
                order: Order::Sequential,
                error: None,
            }),
        });
        assert_eq!(app.songs().0[1].title, "Current");
        assert_eq!(app.songs().1, 1);
        app.page = Page::Browsing;
        assert_eq!(app.songs().0[0].title, "Local");
        assert_eq!(app.songs().1, 0);
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
    fn tab_switches_between_main_and_playback_and_leaves_tabs() {
        let mut app = App::new(None, Vec::new(), None);
        for focus in [Focus::Main, Focus::Playback] {
            app.focus = focus;
            app.key(KeyCode::Char('t'));
            assert!(app.focus == focus);
            app.key(KeyCode::Char('T'));
            assert!(app.focus == Focus::Sidebar);
            app.key(KeyCode::Down);
            let page = app.page;
            app.key(KeyCode::Tab);
            assert!(app.focus == page.main_focus());
            assert!(app.page == page);
            app.page = Page::Playing;
            app.key(KeyCode::Char('S'));
            app.key(KeyCode::Tab);
            assert!(app.focus == Focus::Playback);
            app.key(KeyCode::Tab);
            assert!(app.focus == Focus::Main);
        }
        app.page = Page::Scanned;
        app.key(KeyCode::Char('T'));
        for focus in [
            Focus::Filter(Facet::Genre),
            Focus::Filter(Facet::Artist),
            Focus::Filter(Facet::Album),
            Focus::Main,
            Focus::Playback,
            Focus::Filter(Facet::Genre),
        ] {
            app.key(KeyCode::Tab);
            assert!(app.focus == focus);
        }
    }

    #[test]
    fn uppercase_shortcuts_focus_playback_and_the_visible_song_list() {
        let mut app = App::new(Some(".".into()), Vec::new(), None);
        for page in [Page::Playing, Page::Scanned, Page::Browsing] {
            app.page = page;
            app.key(KeyCode::Char('T'));
            app.key(KeyCode::Char('P'));
            assert!(app.focus == Focus::Playback);
            app.key(KeyCode::Char('s'));
            assert!(app.focus == Focus::Playback);
            app.key(KeyCode::Char('S'));
            assert!(app.focus == Focus::Main);
            assert!(app.page == page);
        }

        app.page = Page::Scanned;
        app.key(KeyCode::Tab);
        assert!(app.focus == Focus::Playback);
        app.key(KeyCode::Tab);
        assert!(app.focus == Focus::Filter(Facet::Genre));
        app.key(KeyCode::Char('S'));
        assert!(app.focus == Focus::Main);

        app.page = Page::Scan;
        app.key(KeyCode::Char('P'));
        app.key(KeyCode::Char('S'));
        assert!(app.focus == Focus::Playback);
        assert!(app.page == Page::Scan);
    }

    #[test]
    fn quit_key_stops_active_playback_without_closing_the_client() {
        let mut app = App::new(None, Vec::new(), None);
        assert!(!app.quit_key_stops_playback());

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
            assert_eq!(app.quit_key_stops_playback(), status != Status::Stopping);
            if status != Status::Stopping {
                app.info = None;
                assert!(!app.key(KeyCode::Char('q')));
                assert_eq!(app.info.as_ref().unwrap().text, "No daemon is reachable");
            }
        }
    }

    #[test]
    fn vim_keys_navigate_tabs_and_remain_text_in_directory_editor() {
        let mut app = App::new(None, Vec::new(), None);
        app.page = Page::Playing;
        app.key(KeyCode::Char('T'));
        app.key(KeyCode::Char('j'));
        assert!(app.page == Page::Scanned);
        app.key(KeyCode::Char('k'));
        assert!(app.page == Page::Playing);

        app.scan_dialog = Some(ScanDialog::Edit {
            index: None,
            text: String::new(),
        });
        for key in "hjklq".chars() {
            assert!(!app.key(KeyCode::Char(key)));
        }
        assert!(!app.key(KeyCode::Tab));
        assert!(app.focus == Focus::Sidebar);
        assert!(matches!(
            app.scan_dialog,
            Some(ScanDialog::Edit { ref text, .. }) if text == "hjklq"
        ));
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
