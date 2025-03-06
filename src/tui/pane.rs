use crate::tui::app::LayoutSections;
use crate::tui::lib::key_help_spans;
use crate::tui::task::Task;
use itertools::Itertools;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Stylize};
use ratatui::text::{Span, Text};
use ratatui::widgets::{Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};
use ratatui::{
    style::Style,
    text::Line,
    widgets::{Block, Widget},
};
use tui_term::widget::PseudoTerminal;

static START_INTERACTION: &'static (&str, &str) = &("[Enter]", "Interact");
static EXIT_INTERACTION: &'static (&str, &str) = &("[Ctrl-Z]", "Exit Interaction");
static START_SEARCH: &'static (&str, &str) = &("[/]", "Search");
static EXIT_SEARCH: &'static (&str, &str) = &("[ESC]", "Exit Search");
static COPY_SELECTION: &'static (&str, &str) = &("[C]", "Copy Selection");
static SHOW_TASKS: &'static (&str, &str) = &("[H]", "Show Tasks");

pub struct TerminalPane<'a> {
    task: &'a Task,
    section: &'a LayoutSections,
    has_sidebar: bool,
    remaining_time: Option<u64>,
}

impl<'a> TerminalPane<'a> {
    pub fn new(task: &'a Task, section: &'a LayoutSections, has_sidebar: bool, remaining_time: Option<u64>) -> Self {
        Self {
            task,
            section,
            has_sidebar,
            remaining_time,
        }
    }

    fn highlight(&self) -> bool {
        matches!(self.section, LayoutSections::Pane)
    }

    fn footer(&self) -> Text {
        if let Some(time) = self.remaining_time {
            Text::from(
                Line::from(format!("Shutting down... ({} sec)", time))
                    .centered()
                    .style(Style::default().bg(Color::LightRed).fg(Color::White)),
            )
        } else {
            let mut help_spans = Vec::new();
            let mut search_spans = Vec::new();
            match self.section {
                LayoutSections::Pane => {
                    if self.task.output.has_selection() {
                        help_spans.push(key_help_spans(*COPY_SELECTION));
                    }
                    help_spans.push(key_help_spans(*EXIT_INTERACTION));
                }
                LayoutSections::TaskList(_) => {
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
}

impl<'a> Widget for &TerminalPane<'a> {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
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
