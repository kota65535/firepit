use crate::tui::table::TaskTable;

const PANE_SIZE_RATIO: f32 = 3.0 / 4.0;

#[derive(Debug, Clone, Copy)]
pub struct SizeInfo {
    task_width_hint: u16,
    rows: u16,
    cols: u16,
}

impl SizeInfo {
    pub fn new<'a>(rows: u16, cols: u16, tasks: impl Iterator<Item = &'a str>) -> Self {
        let task_width_hint = TaskTable::width_hint(tasks);
        Self {
            rows,
            cols,
            task_width_hint,
        }
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
    }

    pub fn pane_rows(&self) -> u16 {
        self.rows
            // Account for header and footer in layout
            .saturating_sub(2)
            // Always allocate at least one row as vt100 crashes if emulating a zero area terminal
            .max(1)
    }

    pub fn pane_cols(&self, has_sidebar: bool) -> u16 {
        // Want to maximize pane width
        let ratio_pane_width = (f32::from(self.cols) * PANE_SIZE_RATIO) as u16;
        let full_task_width = if has_sidebar {
            // We need to account for the left border of the pane
            self.cols.saturating_sub(self.task_width_hint + 1)
        } else {
            self.cols
        };
        full_task_width.max(ratio_pane_width)
    }

    pub fn output_cols(&self, has_sidebar: bool) -> u16 {
        // Account for the pane scrollbar and ...what?
        self.pane_cols(has_sidebar) - 2
    }

    /// Return the actual task table width.
    pub fn task_list_width(&self) -> u16 {
        self.cols - self.pane_cols(true) + 1
    }
}
