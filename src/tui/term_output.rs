use crate::event::Direction;
use std::{io::Write, mem};

const SCROLLBACK_LEN: usize = 4096;

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

    pub fn handle_mouse(&mut self, event: crossterm::event::MouseEvent, clicks: usize) -> anyhow::Result<()> {
        match event.kind {
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                // We need to update the vterm so we don't continue to render the selection
                if clicks == 1 {
                    self.clear_selection();
                } else {
                    let size = self.size();
                    self.parser.screen_mut().set_selection(event.row, 0, event.row, size.1)
                }
            }
            crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                // Update selection of underlying parser
                self.parser.screen_mut().update_selection(event.row, event.column);
            }
            // Scrolling is handled elsewhere
            crossterm::event::MouseEventKind::ScrollDown => (),
            crossterm::event::MouseEventKind::ScrollUp => (),
            // I think we can ignore this?
            crossterm::event::MouseEventKind::Moved => (),
            // Don't care about other mouse buttons
            crossterm::event::MouseEventKind::Down(_) => {
                self.parser.screen_mut().clear_selection();
            }
            crossterm::event::MouseEventKind::Drag(_) => (),
            // We don't support horizontal scroll
            crossterm::event::MouseEventKind::ScrollLeft | crossterm::event::MouseEventKind::ScrollRight => (),
            // Cool, person stopped holding down mouse
            crossterm::event::MouseEventKind::Up(_) => (),
        }
        Ok(())
    }

    pub fn copy_selection(&self) -> Option<String> {
        self.parser.screen().selected_text()
    }

    pub fn clear_selection(&mut self) {
        self.parser.screen_mut().clear_selection();
    }
}
