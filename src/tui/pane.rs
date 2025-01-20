use ratatui::{
    style::Style,
    text::Line,
    widgets::{Block, Widget},
};
use tui_term::widget::PseudoTerminal;

use super::{app::LayoutSections, TerminalOutput};

const FOOTER_TEXT_ACTIVE: &str = "Press`Ctrl-Z` to stop interacting.";
const FOOTER_TEXT_INACTIVE: &str = "Press `Enter` to interact.";
const TASK_LIST_HIDDEN: &str = "Press `h` to show task list.";

pub struct TerminalPane<'a> {
    terminal_output: &'a TerminalOutput,
    task_name: &'a str,
    section: &'a LayoutSections,
    has_sidebar: bool,
}

impl<'a> TerminalPane<'a> {
    pub fn new(
        terminal_output: &'a TerminalOutput,
        task_name: &'a str,
        section: &'a LayoutSections,
        has_sidebar: bool,
    ) -> Self {
        Self {
            terminal_output,
            section,
            task_name,
            has_sidebar,
        }
    }

    fn highlight(&self) -> bool {
        matches!(self.section, LayoutSections::Pane)
    }

    fn footer(&self) -> Line {
        let task_list_message = if !self.has_sidebar {
            TASK_LIST_HIDDEN
        } else {
            ""
        };

        match self.section {
            LayoutSections::Pane => Line::from(FOOTER_TEXT_ACTIVE.to_owned()).centered(),
            LayoutSections::TaskList => {
                Line::from(format!("{FOOTER_TEXT_INACTIVE} {task_list_message}")).centered()
            }
        }
    }
}

impl<'a> Widget for &TerminalPane<'a> {
    
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let screen = self.terminal_output.parser.screen();
        let block = Block::default()
            .title(self.terminal_output.title(self.task_name))
            .title_bottom(self.footer())
            .style(if self.highlight() {
                Style::new().fg(ratatui::style::Color::Yellow)
            } else {
                Style::new()
            });

        let term = PseudoTerminal::new(screen).block(block);
        term.render(area, buf)
    }
}
