#[derive(Debug, Clone)]
pub struct SearchResults {
    pub task: String,
    pub query: String,
    pub matches: Vec<Match>,
    pub index: usize,
    /// Search direction, as chosen by `/` or `?`. It decides which way `n` walks the matches; `N`
    /// always walks the other way.
    pub backward: bool,
}

#[derive(Debug, Clone)]
pub struct Match(pub u16, pub u16);

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
