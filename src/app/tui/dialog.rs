use once_cell::sync::Lazy;
use ratatui::layout::Alignment;
use ratatui::prelude::{Line, Rect, Span, Style, Stylize, Text};
use ratatui::style::Color;
use ratatui::widgets::block::{Position, Title};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

const QUIT_TXT: &str = "Quitting...\n(Press q to force quit)";
const FORCE_QUIT_TXT: &str = "Force quitting...";
const HELP_TXT: &str = include_str!("help.txt");

pub static QUIT_LINES: Lazy<Vec<&str>> = Lazy::new(|| QUIT_TXT.lines().collect());
pub static FORCE_QUIT_LINES: Lazy<Vec<&str>> = Lazy::new(|| FORCE_QUIT_TXT.lines().collect());
pub static HELP_LINES: Lazy<Vec<&str>> = Lazy::new(|| HELP_TXT.lines().collect());

pub fn quit_lines(force: bool) -> &'static Lazy<Vec<&'static str>> {
    if force {
        &FORCE_QUIT_LINES
    } else {
        &QUIT_LINES
    }
}

pub fn quit_dialog_size(screen_width: u16, screen_height: u16, force: bool) -> (Rect, usize, usize) {
    let content_height = quit_lines(force).len();
    let content_width = quit_lines(force).iter().map(|line| line.len()).max().unwrap_or(0);
    let width = (content_width as u16 + 6).min(screen_width.saturating_sub(4));
    let height = (content_height as u16 + 2).min(screen_height.saturating_sub(4));

    // Ensure we don't create negative coordinates
    let x = if screen_width > width {
        (screen_width - width) / 2
    } else {
        0
    };
    let y = if screen_height > height {
        (screen_height - height) / 2
    } else {
        0
    };

    let dialog_rect = Rect { x, y, width, height };

    (dialog_rect, content_width, content_height)
}

pub fn render_quit_dialog(f: &mut Frame, force: bool) {
    let area = f.size();
    let (dialog_area, _content_width, _content_height) = quit_dialog_size(area.width, area.height, force);

    // Clear the background
    f.render_widget(Clear, dialog_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .bg(Color::DarkGray)
        .padding(Padding::horizontal(2));

    let lines = if force {
        vec![Line::from(FORCE_QUIT_TXT)]
    } else {
        QUIT_LINES
            .iter()
            .cloned()
            .map(|line| Line::from(line))
            .collect::<Vec<_>>()
    };

    // Create message paragraph
    let message = Paragraph::new(Text::from(lines))
        .block(block)
        .centered()
        .wrap(Wrap { trim: true });

    f.render_widget(message, dialog_area)
}

pub fn help_dialog_size(screen_width: u16, screen_height: u16) -> (Rect, usize, usize) {
    let content_height = HELP_LINES.len();
    let content_width = HELP_LINES.iter().map(|line| line.len()).max().unwrap_or(0);
    let width = (content_width as u16 + 6).min(screen_width.saturating_sub(4));
    let height = (content_height as u16 + 2).min(screen_height.saturating_sub(4));

    // Ensure we don't create negative coordinates
    let x = if screen_width > width {
        (screen_width - width) / 2
    } else {
        0
    };
    let y = if screen_height > height {
        (screen_height - height) / 2
    } else {
        0
    };

    (Rect { x, y, width, height }, content_width, content_height)
}

pub fn render_help_dialog(f: &mut Frame, scroll: usize) {
    let area = f.size();

    // Clear the entire background
    f.render_widget(Clear, area);

    let (dialog_area, _content_width, content_height) = help_dialog_size(area.width, area.height);

    // Calculate visible content based on scroll
    let visual_height = dialog_area.height.saturating_sub(2) as usize;
    let scroll_offset = scroll.min(content_height.saturating_sub(visual_height));
    let visible_lines = HELP_LINES
        .iter()
        .cloned()
        .skip(scroll_offset)
        .take(visual_height)
        .map(|line| {
            if line.starts_with("#") {
                Line::styled(line, Style::default().fg(Color::LightBlue))
            } else {
                Line::raw(line)
            }
        })
        .collect::<Vec<_>>();

    let mut instruction = Vec::new();
    if visual_height < content_height {
        instruction.push(Span::styled(" [↑↓] ", Style::default().fg(Color::Yellow)));
        instruction.push(Span::raw("Scroll"));
    }
    instruction.push(Span::styled(" [Esc] ", Style::default().fg(Color::Yellow)));
    instruction.push(Span::raw("Close "));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Title::from(" Help ").position(Position::Top).alignment(Alignment::Left))
        .title(
            Title::from(Line::from(instruction))
                .position(Position::Top)
                .alignment(Alignment::Right),
        )
        .padding(Padding::horizontal(2));

    // Create message paragraph
    let message = Paragraph::new(Text::from(visible_lines))
        .block(block)
        .left_aligned()
        .wrap(Wrap { trim: true });

    f.render_widget(message, dialog_area);
}
