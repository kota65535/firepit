use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub struct MultiError {
    errors: Vec<anyhow::Error>
}

impl MultiError {
    pub fn new(errors: Vec<anyhow::Error>) -> Self {
        MultiError { errors }
    }

    pub fn push(&mut self, err: anyhow::Error) {
        self.errors.push(err);
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }
}

impl fmt::Display for MultiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.errors.len() == 1 {
            self.errors[0].fmt(f)?;
        } else {
            writeln!(f, "")?;
            for (i, err) in self.errors.iter().enumerate() {
                writeln!(f, "  {}. {}", i + 1, err)?;
            }
        }
        Ok(())
    }
}

impl Error for MultiError {}
