use super::{App, Facet, Focus, Page, ScanDialog, SettingsRow, settings_rows};
use crate::protocol::{Order, Repeat, Status};
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
use std::cell::Cell as OffsetCell;

#[derive(Default)]
pub(super) struct ScrollOffsets {
    pages: [OffsetCell<usize>; Page::TABS.len()],
    facets: [OffsetCell<usize>; Facet::ALL.len()],
}

fn remember_offset(offset: &OffsetCell<usize>, render: impl FnOnce(usize) -> usize) {
    offset.set(render(offset.get()));
}

fn render_scrolling_list(
    frame: &mut Frame,
    list: List<'_>,
    area: Rect,
    selected: Option<usize>,
    offset: &OffsetCell<usize>,
) {
    remember_offset(offset, |offset| {
        let mut state = ListState::default()
            .with_offset(offset)
            .with_selected(selected);
        frame.render_stateful_widget(list, area, &mut state);
        state.offset()
    });
}

impl App {
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

    pub(super) fn render(&self, frame: &mut Frame) {
        let [top, playback, info, keys] = Layout::vertical([
            Constraint::Min(self.tabs().len() as u16 + 2),
            Constraint::Length(5),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [sidebar, main] =
            Layout::horizontal([Constraint::Length(13), Constraint::Min(0)]).areas(top);
        frame.render_widget(
            Paragraph::new(
                self.tabs()
                    .iter()
                    .map(|&(page, label)| {
                        let active = self.page == page;
                        let prefix = if active { "> " } else { "  " };
                        let style = if active && self.focus == Focus::Sidebar {
                            Style::default().add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        Line::from(vec![Span::raw(prefix), Span::styled(label, style)])
                    })
                    .collect::<Vec<_>>(),
            )
            .block(self.panel("Tabs".into(), Focus::Sidebar)),
            sidebar,
        );
        match self.page {
            Page::Playing | Page::Browsing => self.render_list(frame, main),
            Page::Scanned => self.render_library(frame, main),
            Page::Scan => self.render_scan(frame, main),
        }
        self.render_playback(frame, playback);
        if let Some(notice) = self
            .info
            .as_ref()
            .filter(|notice| notice.visible(&self.config))
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
            Focus::Filter(_) => "[Enter] Filter",
            Focus::Main if self.page == Page::Scan => "[Enter] Activate",
            Focus::Main => "[Enter] ▶   [Space] ▶/⏸",
            Focus::Playback
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|state| state.status == Status::Stopping) =>
            {
                "[Enter] ↺   [Space] ▶/⏸   [n] Next   [p/N] Previous   [r] Repeat   [o] Order"
            }
            Focus::Playback => {
                "[Enter] ⏹   [Space] ▶/⏸   [n] Next   [p/N] Previous   [r] Repeat   [o] Order"
            }
        };
        let songs_key = if self.page == Page::Scan {
            ""
        } else {
            "   [S] Songs"
        };
        let quit_action = if self.quit_key_stops_playback() {
            "Stop"
        } else {
            "Quit"
        };
        let reset_key = if self.page == Page::Scanned {
            "[R] Reset filters   "
        } else {
            ""
        };
        frame.render_widget(
            Paragraph::new(format!(
                "{reset_key}[Tab] Focus   [T] Tabs   [P] Playback{songs_key}   {keys_text}   [Esc] Detach   [q] {quit_action}"
            ))
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true }),
            keys,
        );
        if let Some(dialog) = &self.scan_dialog {
            self.render_scan_dialog(frame, dialog);
        }
    }

    fn render_scan(&self, frame: &mut Frame, area: Rect) {
        use ratatui::widgets::ListItem;
        let directories = &self.config.scan_directories;
        let rows = settings_rows(directories.len());
        let selected = rows
            .enumerate()
            .filter(|(_, row)| *row != SettingsRow::Spacer)
            .nth(self.scan_cursor)
            .map(|(index, _)| index);
        let items = settings_rows(directories.len()).map(|row| match row {
            SettingsRow::Edit(index) => ListItem::new(format!("  {}", directories[index])),
            SettingsRow::Delete(_) => ListItem::new("  [ Delete ]"),
            SettingsRow::Add => ListItem::new("  [ Add directory ]"),
            SettingsRow::Scan => ListItem::new("  [ Scan ]"),
            SettingsRow::Spacer => ListItem::new(""),
        });
        let highlight = if self.focus == Focus::Main {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(Color::White).bg(Color::DarkGray)
        };
        let list = List::new(items)
            .block(self.panel("Recursive Scan Directories".into(), Focus::Main))
            .highlight_style(highlight);
        render_scrolling_list(
            frame,
            list,
            area,
            selected,
            &self.scroll_offsets.pages[Page::Scan as usize],
        );
    }

    fn render_scan_dialog(&self, frame: &mut Frame, dialog: &ScanDialog) {
        let (title, text, buttons) = match dialog {
            ScanDialog::ConfirmScan => (
                "Scan directories",
                "Scan directories and replace the music database?".into(),
                "[Enter/y] Scan    [Esc/n] Cancel",
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
                        &self.config.scan_directories[*index],
                        frame.area().width.min(60).saturating_sub(4) as usize
                    )
                ),
                "[Enter/y] Delete    [Esc/n] Cancel",
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

    fn render_library(&self, frame: &mut Frame, area: Rect) {
        let [facets, songs] =
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);
        let columns = Layout::horizontal([Constraint::Fill(1); Facet::ALL.len()]).split(facets);
        for (index, (facet, &column)) in Facet::ALL.into_iter().zip(columns.iter()).enumerate() {
            let focus = Focus::Filter(facet);
            let filter = self.library.filter(facet);
            let items = filter.labels().enumerate().map(|(row, label)| {
                let marker = if row == filter.selected { "*" } else { " " };
                format!("{marker} {label}")
            });
            let list = List::new(items)
                .block(self.panel(facet.label().into(), focus))
                .highlight_style(self.selection_style(focus));
            render_scrolling_list(
                frame,
                list,
                column,
                Some(filter.cursor),
                &self.scroll_offsets.facets[index],
            );
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
            .block(self.panel("Songs".into(), Focus::Main))
            .row_highlight_style(self.selection_style(Focus::Main));
        remember_offset(&self.scroll_offsets.pages[self.page as usize], |offset| {
            let mut state = TableState::default().with_offset(offset);
            state.select((!tracks.is_empty()).then_some(cursor));
            frame.render_stateful_widget(table, area, &mut state);
            state.offset()
        });
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
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn song_lists_share_the_songs_panel_title() {
        let mut app = App::new(Some("music".into()), Vec::new(), None);
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        for page in [Page::Playing, Page::Browsing, Page::Scanned] {
            app.page = page;
            terminal
                .draw(|frame| app.render_list(frame, frame.area()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let title: String = (1..=5).map(|x| buffer[(x, 0)].symbol()).collect();
            assert_eq!(title, "Songs");
        }
    }

    #[test]
    fn settings_highlight_moves_before_scrolling_back() {
        let mut app = App::new(None, Vec::new(), None);
        app.config.scan_directories = (0..10).map(|index| format!("/music/{index}")).collect();
        let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();

        for (cursor, row) in [(9, 4), (8, 3), (6, 1), (5, 1)] {
            app.scan_cursor = cursor;
            terminal
                .draw(|frame| app.render_scan(frame, frame.area()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let highlighted_row = (1..5).find(|&y| buffer[(1, y)].bg == Color::Cyan);
            assert_eq!(highlighted_row, Some(row));
        }
    }

    #[test]
    fn narrow_directory_dialog_truncates_path_inside_padding() {
        let mut app = App::new(None, Vec::new(), None);
        app.scan_dialog = Some(ScanDialog::Edit {
            index: None,
            text: "/music/long/directory/abcdefghijklmnopqrs".into(),
        });
        let mut terminal = Terminal::new(TestBackend::new(24, 12)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..24).map(|x| buffer[(x, 4)].symbol()).collect();
        assert_eq!(row, "│ …abcdefghijklmnopqrs │");
    }
}
