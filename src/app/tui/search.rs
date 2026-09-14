use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub struct SearchResults {
    pub task: String,
    pub query: String,
    pub matches: Vec<Match>,
    pub index: usize,
    /// Search direction, as chosen by `/` or `?`.
    /// It decides which way `n` walks the matches; `N` always walks the other way.
    pub backward: bool,
}

/// The row and column a match starts at, counted over the whole grid including the scrollback.
#[derive(Debug, Clone)]
pub struct Match(pub usize, pub usize);

/// Finds every occurrence of `query` over the whole grid, scrollback included.
///
/// Rows are joined into logical lines first, so a match broken up by a wrap is still found, and the
/// offset within the line is then mapped back to the row the match starts on.
pub fn find_matches(screen: &vt100::Screen, query: &str) -> Vec<Match> {
    let cols = screen.size().1;
    let mut matches = Vec::new();
    let mut line_buf = String::new();
    let mut previous_row_widths = Vec::new();
    for (row_idx, row) in screen.grid().all_rows().enumerate() {
        let mut s = String::new();
        row.write_contents(&mut s, 0, cols, true);
        let current_row_width = s.width();
        line_buf.push_str(&s);
        if row.wrapped() {
            previous_row_widths.push(current_row_width);
            continue;
        }
        for (offset, _) in line_buf.match_indices(query) {
            // Convert byte offset to display width to handle wide chars properly
            let mut col_idx = line_buf[..offset].width();
            if previous_row_widths.is_empty() {
                matches.push(Match(row_idx, col_idx));
            } else {
                // The line is wrapped Reset the current row index to the first line
                let first_row_idx = row_idx - previous_row_widths.len();
                for (row_idx, width) in
                    (first_row_idx..).zip(previous_row_widths.iter().chain(std::iter::once(&current_row_width)))
                {
                    if col_idx < *width {
                        // The match exists in this line
                        matches.push(Match(row_idx, col_idx));
                        break;
                    }
                    // The match may be in the next line
                    col_idx -= *width;
                }
            }
        }
        previous_row_widths.clear();
        line_buf.clear();
    }
    matches
}

/// Picks the match to show first: the one past the view in the direction being searched, or the
/// match at the far end of the log when there is none left that way.
pub fn initial_index(matches: &[Match], screen: &vt100::Screen, backward: bool) -> usize {
    let offset = first_visible_row(screen);
    if backward {
        matches.iter().rposition(|m| m.0 < offset)
    } else {
        matches.iter().position(|m| offset <= m.0)
    }
    .unwrap_or(matches.len().saturating_sub(1))
}

/// The grid row the top of the view currently shows.
pub fn first_visible_row(screen: &vt100::Screen) -> usize {
    screen.current_scrollback_len() - screen.scrollback()
}

impl SearchResults {
    pub fn new(task: &str, query: String, matches: Vec<Match>, index: usize, backward: bool) -> anyhow::Result<Self> {
        Ok(Self {
            task: task.to_string(),
            query,
            matches,
            index,
            backward,
        })
    }

    pub fn current(&self) -> Option<Match> {
        if self.matches.is_empty() {
            return None;
        }
        Some(self.matches[self.index].clone())
    }

    /// Moves to the match `n` would show next: down the log for a forward search, up for a backward
    /// one.
    pub fn next(&mut self) -> Option<Match> {
        if self.backward {
            self.step_up()
        } else {
            self.step_down()
        }
    }

    /// Moves to the match `N` would show: the opposite way from [`Self::next`].
    pub fn previous(&mut self) -> Option<Match> {
        if self.backward {
            self.step_down()
        } else {
            self.step_up()
        }
    }

    fn step_down(&mut self) -> Option<Match> {
        if self.matches.is_empty() {
            return None;
        }
        self.index = if self.index == self.matches.len() - 1 {
            0
        } else {
            self.index + 1
        };
        Some(self.matches[self.index].clone())
    }

    fn step_up(&mut self) -> Option<Match> {
        if self.matches.is_empty() {
            return None;
        }
        self.index = if self.index == 0 {
            self.matches.len() - 1
        } else {
            self.index - 1
        };
        Some(self.matches[self.index].clone())
    }

    pub fn reset(&mut self) {
        self.query = "".to_string();
        self.matches.clear();
    }
}
