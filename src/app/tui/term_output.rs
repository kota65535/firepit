use crate::app::command::Direction;
use std::{io::Write, mem};

// Ensure that the scrollback length is sufficient to hold the entire log.
// If the number of rows exceeds this, search highlights may not work properly.
const SCROLLBACK_LEN: usize = 1024 * 1024;

pub struct TerminalOutput {
    output: Vec<u8>,
    parser: vt100::Parser,
    stdin: Option<Box<dyn Write + Send>>,
}

impl TerminalOutput {
    pub fn new(rows: u16, cols: u16, stdin: Option<Box<dyn Write + Send>>) -> Self {
        Self {
            output: Vec::new(),
            parser: vt100::Parser::new(rows, cols, SCROLLBACK_LEN),
            stdin,
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

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn screen_mut(&mut self) -> &mut vt100::Screen {
        self.parser.screen_mut()
    }

    pub fn entire_screen(&self) -> vt100::EntireScreen {
        self.parser.entire_screen()
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

    pub fn scroll(&mut self, direction: Direction, stride: usize) -> anyhow::Result<(usize, usize)> {
        let screen = self.parser.screen_mut();
        let scrollback = screen.scrollback();
        let new_scrollback = match direction {
            Direction::Up => {
                if stride == 0 {
                    SCROLLBACK_LEN
                } else {
                    scrollback + stride
                }
            }
            Direction::Down => {
                if stride == 0 {
                    0
                } else {
                    scrollback.saturating_sub(stride)
                }
            }
        };
        screen.set_scrollback(new_scrollback);
        let scrollback_len = screen.current_scrollback_len();
        Ok((new_scrollback, scrollback_len))
    }

    pub fn scroll_to(&mut self, row: u16) {
        let screen = self.parser.screen_mut();
        let scrollback_len = screen.current_scrollback_len();
        let row = row as usize;
        screen.set_scrollback(scrollback_len.saturating_sub(row));
    }

    pub fn has_selection(&self) -> bool {
        self.parser.screen().selected_text().map_or(false, |s| !s.is_empty())
    }

    pub fn reset_selection(&mut self) {
        self.clear_selection();
    }

    pub fn update_selection(&mut self, row: u16, col: u16) {
        self.parser.screen_mut().update_selection(row, col);
    }

    pub fn line_selection(&mut self, row: u16) {
        let (start_row, end_row, cols) = {
            let screen = self.parser.screen();
            let size = screen.size();
            let wrapped_flags: Vec<bool> = screen.grid().visible_rows().map(|r| r.wrapped()).collect();
            if wrapped_flags.is_empty() {
                return;
            }
            let max_row = wrapped_flags.len().saturating_sub(1) as u16;
            let row = row.min(max_row) as usize;

            // Find the start row of the line if wrapped
            let mut start = row;
            while start > 0 && wrapped_flags[start - 1] {
                start -= 1;
            }
            // Find the end row of the line of wrapped
            let mut end = row;
            while end + 1 < wrapped_flags.len() && wrapped_flags[end] {
                end += 1;
            }

            (start as u16, end as u16, size.1)
        };

        self.parser.screen_mut().set_selection(start_row, 0, end_row, cols)
    }

    pub fn copy_selection(&self) -> Option<String> {
        self.parser.screen().selected_text()
    }

    pub fn clear_selection(&mut self) {
        self.parser.screen_mut().clear_selection();
    }
}
