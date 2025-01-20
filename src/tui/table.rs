use crate::event::TaskResult;
use crate::tui::task::TaskStatus;
use indexmap::IndexMap;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Style, Stylize},
    text::Text,
    widgets::{Block, Borders, Cell, Row, StatefulWidget, Table, TableState},
};

/// A widget that renders a table of their tasks and their current status
///
/// The table contains finished tasks, running tasks, and planned tasks rendered
/// in that order.
pub struct TaskTable<'b> {
    tasks: &'b IndexMap<String, TaskStatus>,
}

const TASK_NAVIGATE_INSTRUCTIONS: &str = "↑ ↓ to navigate";
const HIDE_INSTRUCTIONS: &str = "h to hide";

impl<'b> TaskTable<'b> {
    pub fn new(tasks: &'b IndexMap<String, TaskStatus>) -> Self {
        Self {
            tasks,
        }
    }
}

impl TaskTable<'_> {
    /// Provides a suggested width for the task table
    pub fn width_hint<'a>(tasks: impl Iterator<Item=&'a str>) -> u16 {
        let task_name_width = tasks
            .map(|task| task.len())
            .max()
            .unwrap_or_default()
            // Task column width should be large enough to fit "↑ ↓ to navigate instructions
            // and truncate tasks with more than 40 chars.
            .clamp(TASK_NAVIGATE_INSTRUCTIONS.len(), 40) as u16;
        // Add space for column divider and status emoji
        task_name_width + 1
    }

    fn rows(&self) -> Vec<Row> {
        self.tasks.iter()
            .map(|(n, r)| {
                match r {
                    TaskStatus::Planned(detail)=> {
                        let n = if detail.is_target { n.clone() } else { format!(" {}", n.clone()) };
                        Row::new(vec![Cell::new(n), Cell::new(Text::raw("\u{1FAB5}"))])
                    }
                    TaskStatus::Running => {
                        Row::new(vec![Cell::new(n.clone()), Cell::new(Text::raw("\u{1F525}"))])
                    }
                    TaskStatus::Finished(r) => {
                        Row::new(vec![
                            Cell::new(n.clone()),
                            match r {
                                // ✅
                                TaskResult::Success => {
                                    Cell::new(Text::styled("\u{2705}\u{200D}", Style::default().green().bold()))
                                }
                                // 🚫
                                TaskResult::Skipped => {
                                    Cell::new(Text::styled("\u{1F6AB}\u{200D}", Style::default().green().bold()))
                                }
                                // ⛔
                                TaskResult::Stopped => {
                                    Cell::new(Text::styled("\u{26D4}\u{200D}", Style::default().green().bold()))
                                }
                                // ❌
                                TaskResult::Failure => {
                                    Cell::new(Text::styled("\u{274C}\u{200D}", Style::default().red().bold()))
                                }
                                // ❓
                                TaskResult::Unknown => {
                                    Cell::new(Text::styled("\u{2753}\u{200D}", Style::default().red().bold()))
                                }
                            }
                        ])
                    }
                }
            })
            .collect()
    }
}

impl<'a> StatefulWidget for &'a TaskTable<'a> {
    type State = TableState;

    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer, state: &mut Self::State) {
        let width = area.width;
        let bar = "─".repeat(usize::from(width));
        let table = Table::new(self.rows(), [
                Constraint::Min(12),
                Constraint::Length(3),
            ],
        )
            .highlight_style(Style::default().fg(Color::Yellow))
            .column_spacing(0)
            .block(Block::new().borders(Borders::RIGHT))
            .header(
                vec![format!("\u{1f3d5}  Tasks\n{bar}"), " \n───".to_owned()]
                    .into_iter()
                    .map(Cell::from)
                    .collect::<Row>()
                    .height(2),
            )
            .footer(
                vec![
                    format!("{bar}\n{TASK_NAVIGATE_INSTRUCTIONS}\n{HIDE_INSTRUCTIONS}"),
                    format!("───\n "),
                ]
                    .into_iter()
                    .map(Cell::from)
                    .collect::<Row>()
                    .height(3),
            );
        StatefulWidget::render(table, area, buf, state);
    }
}
