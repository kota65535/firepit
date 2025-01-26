use crate::event::{Direction, TaskResult, TaskStatus};
use std::{io::Write, mem};

const SCROLLBACK_LEN: usize = 1024;

pub struct TerminalOutput {
    pub name: String,
    output: Vec<u8>,
    pub parser: vt100::Parser,
    stdin: Option<Box<dyn Write + Send>>,
    status: TaskStatus,
}

impl TerminalOutput {
    pub fn new(name: &str, rows: u16, cols: u16, stdin: Option<Box<dyn Write + Send>>) -> Self {
        Self {
            name: name.to_string(),
            output: Vec::new(),
            parser: vt100::Parser::new(rows, cols, SCROLLBACK_LEN),
            stdin,
            status: TaskStatus::Planned,
        }
    }

    pub fn stdin(&self) -> Option<&Box<dyn Write + Send>> {
        self.stdin.as_ref()
    }

    pub fn stdin_mut(&mut self) -> Option<&mut Box<dyn Write + Send>> {
        self.stdin.as_mut()
    }

    pub fn set_stdin(&mut self, stdin: Option<Box<dyn Write + Send>>) {
        self.stdin = stdin
    }

    pub fn title(&self, task_name: &str) -> String {
        format!("\u{26F0}  {task_name} ({}) ", self.status)
    }

    pub fn size(&self) -> (u16, u16) {
        self.parser.screen().size()
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.output.extend_from_slice(bytes);
    }

    pub fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
        match status {
            TaskStatus::Running(info) => {
                if info.restart_count > 0 {
                    let msg = format!(
                        "Process restarted (PID: {}, Restart: {})\r\n",
                        info.pid, info.restart_count
                    );
                    self.process(console::style(msg).bold().to_string().as_bytes());
                }
            }
            TaskStatus::Finished(result) => {
                let msg = match result {
                    TaskResult::Success => {
                        format!("Process finished with exit code 0")
                    }
                    TaskResult::Failure(code) => {
                        format!("Process finished with exit code {code}")
                    }
                    TaskResult::Stopped => {
                        format!("Process killed by someone else")
                    }
                    TaskResult::BadDeps => {
                        format!("Some dependency task failed")
                    }
                    TaskResult::NotReady => {
                        format!("Task is not ready before timeout")
                    }
                    TaskResult::Unknown => {
                        format!("Task finished by unknown reason")
                    }
                };
                self.process(console::style(format!("\r\n{}\r\n", msg)).bold().to_string().as_bytes());
            }
            _ => {}
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.parser.screen().size() != (rows, cols) {
            let scrollback = self.parser.screen().scrollback();
            let mut new_parser = vt100::Parser::new(rows, cols, SCROLLBACK_LEN);
            new_parser.process(&self.output);
            new_parser.screen_mut().set_scrollback(scrollback);
            // Completely swap out the old vterm with a new correctly sized one
            mem::swap(&mut self.parser, &mut new_parser);
        }
    }

    pub fn scroll(&mut self, direction: Direction, stride: usize) -> anyhow::Result<()> {
        let scrollback = self.parser.screen().scrollback();
        let new_scrollback = match direction {
            Direction::Up => {
                if stride == 0 {
                    0
                } else {
                    scrollback + stride
                }
            }
            Direction::Down => {
                if stride == 0 {
                    SCROLLBACK_LEN
                } else {
                    scrollback.saturating_sub(stride)
                }
            }
        };
        self.parser.screen_mut().set_scrollback(new_scrollback);
        Ok(())
    }

    pub fn persist_screen(&self) -> anyhow::Result<()> {
        let mut stdout = std::io::stdout().lock();
        let title = self.title(&self.name);
        let screen = self.parser.entire_screen();
        let (_, cols) = screen.size();
        stdout.write_all(title.as_bytes())?;
        stdout.write_all(b"\r\n")?;
        for row in screen.rows_formatted(0, cols) {
            stdout.write_all(&row)?;
            stdout.write_all(b"\r\n")?;
        }
        stdout.write_all("\r\n".as_bytes())?;
        Ok(())
    }
}
