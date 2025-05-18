use crate::app::command::{TaskResult, TaskStatus};
use crate::app::tui::lib::key_help_spans;
use crate::app::tui::task::Task;
use indexmap::IndexMap;
use ratatui::text::Line;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Style, Stylize},
    text::Text,
    widgets::{Block, Cell, Row, StatefulWidget, Table, TableState},
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
static STOP_TASK: &'static (&str, &str) = &("[s] ", "Stop");
static RESTART_TASK: &'static (&str, &str) = &("[r] ", "Restart");

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
                            TaskResult::UpToDate => Cell::new(Text::raw("\u{1F966}")),       // 🥬
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
}

impl<'a> StatefulWidget for &'a TaskTable<'a> {
    type State = TableState;

    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer, state: &mut Self::State) {
        let width = area.width;
        // Task name column & status column
        let widths = [
            Constraint::Min(width.saturating_sub(STATUS_COLUMN_WIDTH + 1)),
            Constraint::Length(1),
            Constraint::Length(STATUS_COLUMN_WIDTH),
        ];
        let name_col_bar = "─".repeat(usize::from(width.saturating_sub(STATUS_COLUMN_WIDTH + 1)));
        let status_col_bar = "─".repeat(usize::from(STATUS_COLUMN_WIDTH));
        let table = Table::new(self.rows(), widths)
            .highlight_style(Style::default().fg(Color::Yellow))
            .column_spacing(0)
            .block(Block::new())
            .header(
                vec![
                    format!("\u{1f3d5}  Tasks\n{name_col_bar}"),
                    "\n─".to_owned(),
                    format!("\n{status_col_bar}"),
                ]
                .into_iter()
                .map(Cell::from)
                .collect::<Row>()
                .height(2),
            )
            .footer(
                Row::new(vec![
                    Cell::new(Text::from(vec![
                        Line::from(format!("{name_col_bar}\n")),
                        Line::from(key_help_spans(*NAVIGATE_TASKS)),
                        Line::from(key_help_spans(*HIDE_TASKS)),
                        Line::from(key_help_spans(*STOP_TASK)),
                        Line::from(key_help_spans(*RESTART_TASK)),
                    ])),
                    Cell::new("─"),
                    Cell::new(status_col_bar),
                ])
                .height(5),
            );
        StatefulWidget::render(table, area, buf, state);
    }
}
