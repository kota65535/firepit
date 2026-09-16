use crate::app::command::Direction;
use std::{io::Write, mem};

// Ensure that the scrollback length is sufficient to hold the entire log.
// If the number of rows exceeds this, search highlights may not work properly.
const SCROLLBACK_LEN: usize = 1024 * 1024;

/// The number of rows the line starting at `start` occupies.
fn rows_in_line(screen: &vt100::Screen, start: usize) -> usize {
    screen
        .grid()
        .all_rows()
        .skip(start)
        .position(|row| !row.wrapped())
        .unwrap_or(0)
}

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

    pub fn stdin(&self) -> Option<&(dyn Write + Send)> {
        self.stdin.as_deref()
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

    pub fn entire_screen(&self) -> vt100::EntireScreen<'_> {
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
        if rows == 0 || cols == 0 || self.parser.screen().size() == (rows, cols) {
            return;
        }
        // Where the view sits has to be expressed in something a re-wrap does not move. A row index
        // is not that, so take the line the top row belongs to and how far into it that row is.
        let anchor = (self.parser.screen().scrollback() > 0).then(|| self.view_anchor());
        let mut new_parser = vt100::Parser::new(rows, cols, SCROLLBACK_LEN);
        new_parser.process(&self.output);
        // Completely swap out the old vterm with a new correctly sized one
        mem::swap(&mut self.parser, &mut new_parser);
        // Without an anchor the view was at the bottom, which a fresh parser already shows
        if let Some(anchor) = anchor {
            self.restore_view(anchor);
        }
    }

    /// The line the top row of the view belongs to, and how many rows into that line it is.
    fn view_anchor(&self) -> (usize, usize) {
        let screen = self.parser.screen();
        let top = screen.current_scrollback_len() - screen.scrollback();
        let mut line = 0;
        let mut row_in_line = 0;
        for row in screen.grid().all_rows().take(top) {
            if row.wrapped() {
                row_in_line += 1;
            } else {
                line += 1;
                row_in_line = 0;
            }
        }
        (line, row_in_line)
    }

    /// Scrolls so the given line is back at the top of the view.
    fn restore_view(&mut self, (line, row_in_line): (usize, usize)) {
        let screen = self.parser.screen_mut();
        let mut seen = 0;
        let mut top = screen.current_scrollback_len() + screen.size().0 as usize;
        for (idx, row) in screen.grid().all_rows().enumerate() {
            if seen == line {
                // A narrower terminal can leave the line with fewer rows than it had
                top = idx + row_in_line.min(rows_in_line(screen, idx));
                break;
            }
            if !row.wrapped() {
                seen += 1;
            }
        }
        let scrollback = screen.current_scrollback_len().saturating_sub(top);
        screen.set_scrollback(scrollback);
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

    pub fn scroll_to_bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    pub fn scroll_to(&mut self, row: usize) {
        let screen = self.parser.screen_mut();
        let scrollback_len = screen.current_scrollback_len();
        screen.set_scrollback(scrollback_len.saturating_sub(row));
    }

    pub fn has_selection(&self) -> bool {
        self.parser.screen().selected_text().is_some_and(|s| !s.is_empty())
    }

    pub fn reset_selection(&mut self) {
        self.clear_selection();
    }

    pub fn update_selection(&mut self, row: u16, col: u16) {
        self.parser.screen_mut().update_selection(row, col);
    }

    pub fn word_selection(&mut self, row: u16, col: u16) {
        let selection = {
            let screen = self.parser.screen();
            let size = screen.size();
            let cols = size.1;
            let wrapped_flags: Vec<bool> = screen.grid().visible_rows().map(|r| r.wrapped()).collect();
            if wrapped_flags.is_empty() {
                return;
            }
            let max_row = wrapped_flags.len().saturating_sub(1) as u16;
            let row = row.min(max_row);
            let col = col.min(cols.saturating_sub(1));

            // Classify each cell as whitespace or word (non-whitespace).
            // Using only two classes allows double-click to select URLs, file paths, etc.
            let is_word = |r: u16, c: u16| -> bool {
                let Some(visible_row) = screen.grid().visible_row(r) else {
                    return false;
                };
                match visible_row.get(c) {
                    Some(cell) if cell.is_wide_continuation() => {
                        c > 0
                            && visible_row.get(c - 1).is_some_and(|prev| {
                                let contents = prev.contents();
                                !contents.is_empty() && !contents.chars().all(|ch| ch.is_whitespace())
                            })
                    }
                    Some(cell) => {
                        let contents = cell.contents();
                        !contents.is_empty() && !contents.chars().all(|ch| ch.is_whitespace())
                    }
                    None => false,
                }
            };

            let target_is_word = is_word(row, col);

            // Expand left, crossing wrapped line boundaries
            let (mut start_row, mut start_col) = (row, col);
            loop {
                if start_col > 0 {
                    if is_word(start_row, start_col - 1) == target_is_word {
                        start_col -= 1;
                    } else {
                        break;
                    }
                } else if start_row > 0 && wrapped_flags[start_row as usize - 1] {
                    if is_word(start_row - 1, cols - 1) == target_is_word {
                        start_row -= 1;
                        start_col = cols - 1;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            // Expand right, crossing wrapped line boundaries
            let (mut end_row, mut end_col) = (row, col);
            loop {
                if end_col + 1 < cols {
                    if is_word(end_row, end_col + 1) == target_is_word {
                        end_col += 1;
                    } else {
                        break;
                    }
                } else if wrapped_flags.get(end_row as usize) == Some(&true) {
                    if is_word(end_row + 1, 0) == target_is_word {
                        end_row += 1;
                        end_col = 0;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            (start_row, start_col, end_row, end_col)
        };

        let (start_row, start_col, end_row, end_col) = selection;
        self.parser
            .screen_mut()
            .set_selection(start_row, start_col, end_row, end_col);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Produce a terminal output with more lines than the visible rows so that the view can be
    /// scrolled back.
    fn output_with_scrollback() -> TerminalOutput {
        let mut output = TerminalOutput::new(4, 20, None);
        for i in 0..20 {
            output.process(format!("line {}\r\n", i).as_bytes());
        }
        output
    }

    /// The text of the row at the top of the view.
    fn top_row(output: &TerminalOutput) -> String {
        let cols = output.size().1;
        let mut s = String::new();
        output
            .screen()
            .grid()
            .visible_rows()
            .next()
            .unwrap()
            .write_contents(&mut s, 0, cols, true);
        s
    }

    /// Lines long enough to wrap at 20 columns but not at 40, so a width change between the two
    /// alters the total row count.
    fn output_with_wrapped_lines() -> TerminalOutput {
        let mut output = TerminalOutput::new(4, 20, None);
        for i in 0..10 {
            output.process(format!("{i}{}\r\n", "x".repeat(25)).as_bytes());
        }
        output
    }

    #[test]
    fn resize_keeps_the_line_at_the_top_of_the_view() {
        let mut output = output_with_wrapped_lines();
        output.scroll(Direction::Up, 9).unwrap();
        let before = top_row(&output);
        assert!(before.starts_with('4'), "{before}");

        output.resize(4, 40);
        assert!(top_row(&output).starts_with('4'), "{}", top_row(&output));
    }

    #[test]
    fn resize_keeps_the_line_at_the_top_when_narrowing() {
        let mut output = TerminalOutput::new(4, 40, None);
        for i in 0..10 {
            output.process(format!("{i}{}\r\n", "x".repeat(25)).as_bytes());
        }
        output.scroll(Direction::Up, 4).unwrap();
        let before = top_row(&output);

        // Narrower: every line now wraps, so the row count grows
        output.resize(4, 20);
        let after = top_row(&output);
        assert!(
            before.starts_with(after.chars().next().unwrap()),
            "before={before} after={after}"
        );
    }

    #[test]
    fn resize_falls_back_to_the_start_of_a_line_it_cannot_land_inside() {
        let mut output = output_with_wrapped_lines();
        // The top row is the second row of a wrapped line, which a wider terminal does not have
        output.scroll(Direction::Up, 8).unwrap();
        assert!(!top_row(&output).starts_with('4'));

        output.resize(4, 40);
        assert!(top_row(&output).starts_with('4'), "{}", top_row(&output));
    }

    #[test]
    fn resize_stays_at_the_bottom_when_not_scrolled_back() {
        let mut output = TerminalOutput::new(4, 20, None);
        for i in 0..10 {
            output.process(format!("{i}{}\r\n", "x".repeat(25)).as_bytes());
        }
        assert_eq!(output.screen().scrollback(), 0);

        output.resize(4, 40);
        assert_eq!(output.screen().scrollback(), 0);
    }

    #[test]
    fn scroll_to_bottom_resets_scrollback() {
        let mut output = output_with_scrollback();
        output.scroll(Direction::Up, 5).unwrap();
        assert_eq!(output.screen().scrollback(), 5);

        output.scroll_to_bottom();
        assert_eq!(output.screen().scrollback(), 0);
    }

    #[test]
    fn scroll_to_bottom_is_noop_at_bottom() {
        let mut output = output_with_scrollback();
        assert_eq!(output.screen().scrollback(), 0);

        output.scroll_to_bottom();
        assert_eq!(output.screen().scrollback(), 0);
    }
}
