use crate::app::command::{TaskResult, TaskStatus};
use crate::app::tui::lib::key_help_spans;
use crate::app::tui::task::Task;
use indexmap::IndexMap;
use ratatui::prelude::{Line, Span};
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
}

static NAVIGATE_TASKS: &'static (&str, &str) = &("[↑↓]", "Navigate");
static HIDE_TASKS: &'static (&str, &str) = &("[h] ", "Hide");
static MAX_WIDTH: usize = 40;
static STATUS_COLUMN_WIDTH: u16 = 3;

impl<'b> TaskTable<'b> {
    pub fn new(tasks: &'b IndexMap<String, Task>) -> Self {
        Self { tasks }
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
                    TaskStatus::Finished(r) => {
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

    fn status_summary(&self) -> Span {
        let targets = self.tasks.values().filter(|t| t.is_target).collect::<Vec<_>>();

        // Running: some tasks are still running
        if targets.iter().any(|t| {
            matches!(
                t.status(),
                TaskStatus::Planned | TaskStatus::Running(_) | TaskStatus::Finished(TaskResult::Reloading)
            )
        }) {
            return Span::styled(" Running ", Style::default().fg(Color::White).bg(Color::LightBlue));
        }
        // Failure: some tasks have failed
        if targets
            .iter()
            .any(|t| matches!(t.status(), TaskStatus::Finished(r) if r.is_failure()))
        {
            return Span::styled(" Failure ", Style::default().fg(Color::White).bg(Color::LightRed));
        }

        // All tasks should have finished successfully or become ready

        // Ready: all service tasks are ready to use
        if targets.iter().any(|t| matches!(t.status(), TaskStatus::Ready)) {
            return Span::styled("  Ready  ", Style::default().fg(Color::White).bg(Color::LightGreen));
        }
        // Success: all tasks have finished successfully
        Span::styled(" Success ", Style::default().fg(Color::White).bg(Color::LightGreen))
    }
}

impl<'a> StatefulWidget for &'a TaskTable<'a> {
    type State = TableState;

    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer, state: &mut Self::State) {
        let layout = ratatui::prelude::Layout::vertical([
            Constraint::Length(2), // Header
            Constraint::Fill(1),   // Table
            Constraint::Length(3), // Footer
        ]);
        let [header_area, table_area, footer_area] = layout.areas(area);
        let width = table_area.width;

        // Render header
        let title_span = Span::raw("\u{1f3d5}  Tasks"); // 🏕
        let status_span = self.status_summary();
        let space_span =
            Span::raw(" ".repeat(usize::from(width).saturating_sub(title_span.width() + status_span.width())));
        let header = Paragraph::new(Text::from(vec![
            Line::from(vec![title_span, space_span, status_span]),
            Line::from("─".repeat(usize::from(width))),
        ]));

        // Render table
        let widths = [
            Constraint::Min(width.saturating_sub(STATUS_COLUMN_WIDTH + 1)),
            Constraint::Length(1),
            Constraint::Length(STATUS_COLUMN_WIDTH),
        ];
        let table = Table::new(self.rows(), widths)
            .highlight_style(Style::default().fg(Color::Yellow))
            .column_spacing(0)
            .block(Block::new());

        // Render footer
        let footer = Paragraph::new(Text::from(vec![
            Line::raw("─".repeat(usize::from(width))),
            Line::from(key_help_spans(*NAVIGATE_TASKS)),
            Line::from(key_help_spans(*HIDE_TASKS)),
        ]));

        Widget::render(header, header_area, buf);
        StatefulWidget::render(table, table_area, buf, state);
        Widget::render(footer, footer_area, buf);
    }
}
