use crate::app::tui::lib::key_help_spans;
use crate::app::tui::task::Task;
use crate::app::tui::LayoutSections;
use itertools::Itertools;
use ratatui::prelude::{Color, Constraint, Layout, Rect, Span, Stylize, Text};
use ratatui::widgets::{Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};
use ratatui::{
    style::Style,
    text::Line,
    widgets::{Block, Widget},
};
use tui_term::widget::PseudoTerminal;

static STOP_TASK: &'static (&str, &str) = &("[s]", "Stop");
static RESTART_TASK: &'static (&str, &str) = &("[r]", "Restart");
static START_INTERACTION: &'static (&str, &str) = &("[Enter]", "Interact");
static EXIT_INTERACTION: &'static (&str, &str) = &("[Ctrl-z]", "Exit Interaction");
static START_SEARCH: &'static (&str, &str) = &("[/]", "Search");
static EXIT_SEARCH: &'static (&str, &str) = &("[Esc]", "Exit Search");
static COPY_SELECTION: &'static (&str, &str) = &("[c]", "Copy Selection");
static SHOW_TASKS: &'static (&str, &str) = &("[h]", "Show Tasks");
static NAVIGATE_SEARCH_RESULT: &'static (&str, &str) = &("[n\u{FF65}N]", "Next/Prev Match");
static CLEAR_SEARCH_RESULT: &'static (&str, &str) = &("[Esc]", "Clear");
static QUIT: &'static (&str, &str) = &("[q]", "Quit");
static HELP: &'static (&str, &str) = &("[?]", "Help");

pub struct TerminalPane<'a> {
    task: &'a Task,
    section: &'a LayoutSections,
    has_sidebar: bool,
}

impl<'a> TerminalPane<'a> {
    pub fn new(task: &'a Task, section: &'a LayoutSections, has_sidebar: bool) -> Self {
        Self {
            task,
            section,
            has_sidebar,
        }
    }

    fn highlight(&self) -> bool {
        matches!(self.section, LayoutSections::Pane)
    }

    fn footer(&self) -> Text {
        let mut help_spans = Vec::new();
        let mut search_spans = Vec::new();
        match self.section {
            LayoutSections::Pane => {
                if self.task.output.has_selection() {
                    help_spans.push(key_help_spans(*COPY_SELECTION));
                }
                help_spans.push(key_help_spans(*EXIT_INTERACTION));
            }
            LayoutSections::TaskList(s) => {
                if self.task.output.has_selection() {
                    help_spans.push(key_help_spans(*COPY_SELECTION));
                }
                if !self.has_sidebar {
                    help_spans.push(key_help_spans(*SHOW_TASKS));
                }
                if self.task.output.stdin().is_some() {
                    help_spans.push(key_help_spans(*START_INTERACTION));
                }
                help_spans.push(key_help_spans(*START_SEARCH));
                if let Some(search) = s {
                    if search.matches.len() > 0 {
                        help_spans.push(key_help_spans(*NAVIGATE_SEARCH_RESULT));
                        help_spans.push(key_help_spans(*CLEAR_SEARCH_RESULT));
                    }
                }
                help_spans.push(key_help_spans(*RESTART_TASK));
                help_spans.push(key_help_spans(*STOP_TASK));
                help_spans.push(key_help_spans(*QUIT));
                help_spans.push(key_help_spans(*HELP));
            }
            LayoutSections::Search { query } => {
                if self.task.output.has_selection() {
                    help_spans.push(key_help_spans(*COPY_SELECTION));
                }
                if !self.has_sidebar {
                    help_spans.push(key_help_spans(*SHOW_TASKS));
                }
                help_spans.push(key_help_spans(*EXIT_SEARCH));
                search_spans.push(Span::styled(format!("/ {}\n", query), Style::default().bold()));
            }
            LayoutSections::Help { .. } => {
                // No footer content for help dialog
            }
        }

        Text::from(vec![
            Line::from(search_spans).left_aligned(),
            Line::from(
                help_spans
                    .into_iter()
                    .intersperse_with(|| vec![Span::raw("  ")])
                    .flatten()
                    .collect::<Vec<_>>(),
            )
            .centered(),
        ])
    }
}

impl<'a> Widget for &TerminalPane<'a> {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        // Create vertical layout with terminal and fixed height block
        let layout = Layout::vertical([Constraint::Fill(1), Constraint::Length(2)]);
        let [terminal_area, block_area] = layout.areas(area);

        let screen = self.task.output.screen();
        let terminal_block = Block::default()
            .padding(Padding::top(1))
            .borders(if self.has_sidebar { Borders::LEFT } else { Borders::NONE })
            .border_style(if self.highlight() {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new()
            })
            .title(if self.highlight() {
                Line::styled(self.task.title_line(), Style::new().fg(Color::Yellow).bold())
            } else {
                Line::styled(self.task.title_line(), Style::new().bold())
            });

        // Terminal widget
        let term = PseudoTerminal::new(screen).block(terminal_block);
        term.render(terminal_area, buf);

        // Footer widget
        let footer_block = Block::default()
            .borders(if self.has_sidebar { Borders::LEFT } else { Borders::NONE })
            .border_style(if self.highlight() {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new()
            });
        let footer_widget = Paragraph::new(self.footer()).block(footer_block);
        footer_widget.render(block_area, buf);
    }
}

pub struct TerminalScroll<'a> {
    section: &'a LayoutSections,
}

impl<'a> TerminalScroll<'a> {
    pub fn new(section: &'a LayoutSections) -> Self {
        Self { section }
    }
}

impl<'a> StatefulWidget for TerminalScroll<'a> {
    type State = ScrollbarState;
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer, state: &mut Self::State) {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"))
            .style(if matches!(self.section, LayoutSections::Pane) {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new()
            });
        StatefulWidget::render(scrollbar, area, buf, state);
    }
}
