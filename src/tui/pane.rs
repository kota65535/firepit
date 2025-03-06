use crate::tui::app::LayoutSections;
use crate::tui::task::Task;
use ratatui::layout::Rect;
use ratatui::style::{Color, Stylize};
use ratatui::widgets::{Borders, Padding, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};
use ratatui::{
    style::Style,
    text::Line,
    widgets::{Block, Widget},
};
use tui_term::widget::PseudoTerminal;

const FOOTER_TEXT_ACTIVE: &str = "Press`Ctrl-Z` to stop interacting.";
const FOOTER_TEXT_INACTIVE: &str = "Press `Enter` to interact.";
const HAS_SELECTION: &str = "Press `c` to copy selection";
const TASK_LIST_HIDDEN: &str = "Press `h` to show task list.";

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

    fn footer(&self) -> Line {
        let task_list_message = if !self.has_sidebar { TASK_LIST_HIDDEN } else { "" };

        if let Some(time) = self.remaining_time {
            Line::from(format!("Shutting down... ({} sec)", time))
                .centered()
                .style(Style::default().bg(Color::LightRed).fg(Color::White))
        } else {
            match self.section {
                LayoutSections::Pane if self.task.output.has_selection() => {
                    Line::from(format!("{FOOTER_TEXT_ACTIVE} {task_list_message} {HAS_SELECTION}")).centered()
                }
                LayoutSections::Pane => Line::from(FOOTER_TEXT_ACTIVE.to_owned()).centered(),
                LayoutSections::TaskList(_) if self.task.output.has_selection() => {
                    Line::from(format!("{FOOTER_TEXT_INACTIVE} {task_list_message} {HAS_SELECTION}")).centered()
                }
                LayoutSections::TaskList(_) => {
                    Line::from(format!("{FOOTER_TEXT_INACTIVE} {task_list_message}")).centered()
                }
                LayoutSections::Search { query } => Line::from(format!("/ {}", query)).left_aligned(),
            }
        }
    }
}

impl<'a> Widget for &TerminalPane<'a> {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let screen = self.task.output.screen();
        let block = Block::default()
            .padding(Padding::top(1))
            .borders(if self.has_sidebar { Borders::LEFT } else { Borders::NONE })
            .border_style(if self.highlight() {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new()
            })
            .title(self.task.title_line())
            .title_bottom(self.footer())
            .title_style(if self.highlight() {
                Style::new().fg(Color::Yellow).bold()
            } else {
                Style::new().bold()
            });

        let term = PseudoTerminal::new(screen).block(block);
        term.render(area, buf)
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
