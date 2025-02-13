#[derive(Debug, Clone)]
pub struct SearchResults {
    pub task: String,
    pub query: String,
    pub matches: Vec<Match>,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct Match(pub u16, pub u16);

impl SearchResults {
    pub fn new(task: &str, query: String, matches: Vec<Match>) -> anyhow::Result<Self> {
        Ok(Self {
            task: task.to_string(),
            query,
            matches,
            index: 0,
        })
    }

    pub fn current(&self) -> Option<Match> {
        if self.matches.is_empty() {
            return None;
        }
        Some(self.matches[self.index].clone())
    }

    pub fn next(&mut self) -> Option<Match> {
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

    pub fn previous(&mut self) -> Option<Match> {
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
