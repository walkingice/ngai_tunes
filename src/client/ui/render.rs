use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Padding, Paragraph, Wrap},
};

pub(super) fn path_tail(path: &str, width: usize) -> String {
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

pub(super) fn render_dialog(frame: &mut Frame, title: &str, text: &str, buttons: &str) {
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

pub(super) fn playback_progress(position: u64, duration: Option<u64>, width: u16) -> Line<'static> {
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

pub(super) fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
