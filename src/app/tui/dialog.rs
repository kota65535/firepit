use once_cell::sync::Lazy;
use ratatui::prelude::{Line, Rect, Span, Style, Text};
use ratatui::style::Color;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

pub const QUIT_TXT: &str = "\u{1FAA3} Quitting... (press q to force quit)"; // 🪣
pub const FORCE_QUIT_TXT: &str = "\u{1F4A7} Force quitting..."; // 💧
pub const COPIED_TXT: &str = "\u{1F4CB} Copied to clipboard"; // 📋
const HELP_TXT: &str = include_str!("help.txt");

pub static HELP_LINES: Lazy<Vec<&str>> = Lazy::new(|| HELP_TXT.lines().collect());

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
    let area = f.area();

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
                Line::styled(line, Style::default().fg(Color::Blue))
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
        .title_top(Line::from(" Help ").left_aligned())
        .title_top(Line::from(instruction).right_aligned())
        .padding(Padding::horizontal(2));

    // Create message paragraph
    let message = Paragraph::new(Text::from(visible_lines))
        .block(block)
        .left_aligned()
        .wrap(Wrap { trim: true });

    f.render_widget(message, dialog_area);
}
