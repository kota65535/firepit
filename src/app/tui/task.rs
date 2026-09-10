use crate::app::command::{TaskResult, TaskStatus};
use crate::app::cui::lib::BOLD;
use crate::app::tui::term_output::TerminalOutput;
use chrono::{DateTime, Local};
use console::Style;
use std::io::Write;

pub struct Task {
    pub name: String,
    pub label: String,
    pub is_target: bool,
    /// Whether the task is a finalizer, which runs after its target task finishes
    pub is_finalizer: bool,
    pub pid: Option<u32>,
    pub restart: u64,
    pub max_restart: Option<u64>,
    pub rerun: u64,
    status: TaskStatus,
    pub output: TerminalOutput,
    pub start_time: Option<DateTime<Local>>,
    pub end_time: Option<DateTime<Local>>,
    pub result: Option<TaskResult>,
}

impl Task {
    pub fn new(name: &str, is_target: bool, output: TerminalOutput, label: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            label: label.unwrap_or(name).to_string(),
            is_target,
            is_finalizer: false,
            pid: None,
            restart: 0,
            max_restart: None,
            rerun: 0,
            status: TaskStatus::Planned,
            output,
            start_time: None,
            end_time: None,
            result: None,
        }
    }

    pub fn status(&self) -> &TaskStatus {
        &self.status
    }
    pub fn set_status(&mut self, status: TaskStatus) {
        match &status {
            TaskStatus::Running(run) => {
                self.pid = Some(run.pid);
                self.restart = run.restart;
                self.max_restart = run.max_restart;
                self.rerun = run.rerun;
                self.start_time = Some(run.start_time);
            }
            TaskStatus::Finished(result, end_time) => {
                self.result = Some(result.clone());
                self.end_time = *end_time;
            }
            _ => {}
        }
        self.status = status;
    }

    /// Writes a firepit message into the pane on a line of its own, e.g. the
    /// result of the process. The styling is forced since `console` would drop
    /// it when stdout is not a TTY, but the pane is a terminal emulator regardless.
    pub fn note(&mut self, style: &Style, text: &str) {
        // Unfinished process output must not be overwritten.
        let (_, col) = self.output.screen().cursor_position();
        let prefix = if col == 0 { "" } else { "\r\n" };
        let line = format!("{prefix}{}\r\n", style.clone().force_styling(true).apply_to(text));
        self.output.process(line.as_bytes());
    }

    pub fn persist_screen(&self) -> anyhow::Result<()> {
        let mut stdout = std::io::stdout().lock();
        let screen = self.output.entire_screen();
        let title = self.title_line();
        let (_, cols) = screen.size();
        stdout.write_all(BOLD.apply_to(title).to_string().as_bytes())?;
        stdout.write_all(b"\r\n")?;
        for row in screen.rows_formatted(0, cols) {
            stdout.write_all(&row)?;
            stdout.write_all(b"\r\n")?;
        }
        stdout.write_all("\r\n".as_bytes())?;
        Ok(())
    }
}

impl Task {
    pub fn title_line(&self) -> String {
        let pid = match self.pid {
            Some(pid) => format!("{}", pid),
            None => "N/A".to_string(),
        };
        // A task that cannot restart has nothing to count, which is every task
        // that is not a service, as well as a service with `restart: never`.
        let restart = match self.max_restart {
            Some(0) => String::new(),
            Some(max) => format!("Restart: {}/{}, ", self.restart, max),
            None => format!("Restart: {}/\u{221e}, ", self.restart),
        };
        let rerun = format!("Re-run: {}", self.rerun);
        let elapsed = match (self.start_time, &self.status) {
            (Some(st), TaskStatus::Finished(_, end_time)) => {
                end_time.map_or("N/A".to_string(), |et| format!("{}s", (et - st).num_seconds()))
            }
            (Some(st), _) => format!("{}s", (Local::now() - st).num_seconds()),
            (None, _) => "N/A".to_string(),
        };

        let status = match &self.status {
            TaskStatus::Planned => "Waiting".to_string(),
            TaskStatus::Running(_) => format!("Running, PID: {pid}, {restart}{rerun}, Elapsed: {elapsed}"),
            TaskStatus::Ready => format!("Ready, PID: {pid}, {restart}{rerun}, Elapsed: {elapsed}"),
            TaskStatus::Finished(r, _) => {
                format!(
                    "Finished - {}, {restart}{rerun}, Elapsed: {elapsed}",
                    r.short_message(false)
                )
            }
        };

        format!("% {} ({})", self.label, status)
    }
}
