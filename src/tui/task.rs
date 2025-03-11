use crate::cui::lib::BOLD;
use crate::event::{TaskResult, TaskStatus};
use crate::tui::term_output::TerminalOutput;
use std::io::Write;

pub struct Task {
    pub name: String,
    pub label: String,
    pub is_target: bool,
    pub pid: Option<u32>,
    pub restart: u64,
    pub max_restart: Option<u64>,
    pub reload: u64,
    status: TaskStatus,
    pub output: TerminalOutput,
}

impl Task {
    pub fn new(name: &str, is_target: bool, output: TerminalOutput, label: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            label: label.unwrap_or(name).to_string(),
            is_target,
            pid: None,
            restart: 0,
            max_restart: None,
            reload: 0,
            status: TaskStatus::Planned,
            output,
        }
    }

    pub fn status(&self) -> TaskStatus {
        self.status
    }
    pub fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
        match status {
            TaskStatus::Running(run) => {
                self.pid = Some(run.pid);
                self.restart = run.restart;
                self.max_restart = run.max_restart;
                self.reload = run.reload;
            }
            _ => {}
        }
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
        let max_restart = match self.max_restart {
            Some(max_restart) => format!("{}", max_restart),
            None => "∞".to_string(),
        };
        let pid = match self.pid {
            Some(pid) => format!("{}", pid),
            None => "N/A".to_string(),
        };

        let status = match self.status {
            TaskStatus::Planned => format!("Waiting"),
            TaskStatus::Running(_) => format!(
                "Running, PID: {}, Restart: {}/{}, Reload: {}",
                pid, self.restart, max_restart, self.reload
            ),
            TaskStatus::Ready => format!(
                "Ready, PID: {}, Restart: {}/{}, Reload: {}",
                pid, self.restart, max_restart, self.reload
            ),
            TaskStatus::Finished(result) => {
                let result = match result {
                    TaskResult::Success => format!("Success"),
                    TaskResult::Failure(code) => format!("Failure with exit code {code}"),
                    TaskResult::UpToDate => format!("Up-to-date"),
                    TaskResult::BadDeps => format!("Dependency task failed"),
                    TaskResult::Stopped => format!("Stopped"),
                    TaskResult::NotReady => format!("Service not ready"),
                    TaskResult::Reloading => format!("Service is reloading..."),
                    TaskResult::Unknown => format!("Unknown"),
                };
                format!("Finished: {}", result)
            }
        };

        format!("% {} ({})", self.label, status)
    }

    pub fn finish_line(&self) -> Option<String> {
        match self.status {
            TaskStatus::Running(info) => {
                if info.restart > 0 || info.reload > 0 {
                    Some("Process restarted".to_string())
                } else {
                    None
                }
            }
            TaskStatus::Finished(result) => match result {
                TaskResult::Success => Some("Process finished with exit code 0".to_string()),
                TaskResult::Failure(code) => Some(format!("Process finished with exit code {code}")),
                TaskResult::Stopped => Some("Process is terminated".to_string()),
                _ => None,
            },
            _ => None,
        }
    }
}
