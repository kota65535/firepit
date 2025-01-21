use crate::event::Direction;
use crate::tui::task::TaskStatus;
use std::{io::Write, mem};

const SCROLLBACK_LEN: usize = 1024;

pub struct TerminalOutput {
    pub name: String,
    output: Vec<u8>,
    pub parser: vt100::Parser,
    pub stdin: Option<Box<dyn Write + Send>>,
    pub status: TaskStatus,
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

    pub fn scroll(&mut self, direction: Direction) -> anyhow::Result<()> {
        let scrollback = self.parser.screen().scrollback();
        let new_scrollback = match direction {
            Direction::Up => scrollback + 1,
            Direction::Down => scrollback.saturating_sub(1),
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
