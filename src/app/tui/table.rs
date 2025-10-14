use crate::app::command::{TaskResult, TaskStatus};
use crate::app::tui::lib::key_help_spans;
use crate::app::tui::task::Task;
use crate::app::tui::LayoutSections;
use indexmap::IndexMap;
use ratatui::prelude::{Layout, Line};
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Style, Stylize},
    text::Text,
    widgets::{Block, Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget},
};

/// A widget that renders a table of their tasks and their current status
///
/// The table contains finished tasks, running tasks, and planned tasks rendered
/// in that order.
pub struct TaskTable<'b> {
    tasks: &'b IndexMap<String, Task>,
    section: &'b LayoutSections,
}

static NAVIGATE_TASKS: &'static (&str, &str) = &("[↑↓]", "Navigate");
static HIDE_TASKS: &'static (&str, &str) = &("[h] ", "Hide");
static MAX_WIDTH: usize = 40;
static STATUS_COLUMN_WIDTH: u16 = 3;

impl<'b> TaskTable<'b> {
    pub fn new(tasks: &'b IndexMap<String, Task>, section: &'b LayoutSections) -> Self {
        Self { tasks, section }
    }
}

impl TaskTable<'_> {
    /// Provides a suggested width for the task table
    pub fn width_hint<'a>(tasks: impl Iterator<Item = &'a str>) -> u16 {
        let min_width = NAVIGATE_TASKS.0.len() + NAVIGATE_TASKS.1.len();
        let task_name_width = tasks
            .map(|task| task.len())
            .max()
            .unwrap_or_default()
            .clamp(min_width, MAX_WIDTH) as u16;
        // Additional spaces before and after status emoji
        task_name_width + STATUS_COLUMN_WIDTH + 1
    }

    fn rows(&self) -> Vec<Row> {
        self.tasks
            .iter()
            .map(|(_, r)| {
                let style = if r.is_target {
                    Style::default().bold()
                } else {
                    Style::default()
                };
                let name_cell = Cell::new(Text::styled(r.label.clone(), style));
                let status_cell = match r.status() {
                    TaskStatus::Planned => Cell::new(Text::raw("\u{1FAB5}")), // 🪵
                    TaskStatus::Running(_) => Cell::new(Text::raw("\u{1F525}")), // 🔥
                    TaskStatus::Ready => Cell::new(Text::raw("\u{1F356}")),   // 🍖
                    TaskStatus::Finished(r, _) => {
                        // Append `\u{FE0F}` (Variation Selector-16) so that the terminal treat the emoji as full-width
                        match r {
                            TaskResult::Success => Cell::new(Text::raw("\u{2705}\u{FE0F}")), // ✅
                            TaskResult::Failure(_) => Cell::new(Text::raw("\u{274C}\u{FE0F}")), // ❌
                            TaskResult::UpToDate => Cell::new(Text::raw("\u{1F96C}")),       // 🥬
                            TaskResult::BadDeps | TaskResult::NotReady | TaskResult::Stopped => {
                                Cell::new(Text::raw("\u{1F6AB}")) // 🚫
                            }
                            TaskResult::Reloading => Cell::new(Text::raw("\u{267B}\u{FE0F}")), // ♻️
                            TaskResult::Unknown => Cell::new(Text::raw("\u{2753}\u{FE0F}")),   // ❓
                        }
                    }
                };
                Row::new(vec![name_cell, Cell::new(" "), status_cell])
            })
            .collect()
    }

    fn status_summary(&self) -> Line {
        // Failure: any task has failed
        if self
            .tasks
            .values()
            .any(|t| matches!(t.status(), TaskStatus::Finished(r, _) if r.is_failure()))
        {
            return Line::styled(" Failure ", Style::default().fg(Color::White).bg(Color::Red));
        }

        // Running: some tasks are still running
        if self.tasks.values().any(|t| {
            matches!(
                t.status(),
                TaskStatus::Planned | TaskStatus::Running(_) | TaskStatus::Finished(TaskResult::Reloading, _)
            )
        }) {
            return Line::styled(" Running ", Style::default().fg(Color::White).bg(Color::Blue));
        }

        // All tasks should have finished successfully or become ready!
        Line::styled(" Success ", Style::default().fg(Color::White).bg(Color::Green))
    }
}

impl<'a> StatefulWidget for &'a TaskTable<'a> {
    type State = TableState;

    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer, state: &mut Self::State) {
        let [header_area, header_line, table_area, footer_line, footer_area] = Layout::vertical([
            Constraint::Length(1), // Header
            Constraint::Length(1), // Header line
            Constraint::Fill(1),   // Table
            Constraint::Length(1), // Footer line
            Constraint::Length(2), // Footer
        ])
        .areas(area);

        // Render header
        let [title_area, status_area, _margin] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1), Constraint::Length(1)]).areas(header_area);
        let title = Paragraph::new(Line::from("\u{1f3d5}  Tasks")).left_aligned(); // 🏕
        let status = Paragraph::new(self.status_summary()).right_aligned();
        Widget::render(title, title_area, buf);
        Widget::render(status, status_area, buf);

        // Render table
        let widths = [
            Constraint::Min(area.width.saturating_sub(STATUS_COLUMN_WIDTH + 1)),
            Constraint::Length(1),
            Constraint::Length(STATUS_COLUMN_WIDTH),
        ];
        let table = Table::new(self.rows(), widths)
            .highlight_style(Style::default().fg(Color::Yellow))
            .column_spacing(0)
            .block(Block::new());
        StatefulWidget::render(table, table_area, buf, state);

        // Render footer
        if matches!(self.section, LayoutSections::TaskList { .. }) {
            let footer = Paragraph::new(Text::from(vec![
                Line::from(key_help_spans(*NAVIGATE_TASKS)),
                Line::from(key_help_spans(*HIDE_TASKS)),
            ]));
            Widget::render(footer, footer_area, buf);
        }

        // Render header and footer lines
        let line = Paragraph::new(Text::from("─".repeat(usize::from(area.width))));
        Widget::render(line.clone(), header_line, buf);
        Widget::render(line.clone(), footer_line, buf);
    }
}
