use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Span;
use std::collections::VecDeque;
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    inner: VecDeque<T>,
}

impl<T> RingBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: VecDeque::with_capacity(capacity),
        }
    }

    pub fn push(&mut self, item: T) {
        if self.inner.len() == self.inner.capacity() {
            self.inner.pop_front();
            self.inner.push_back(item);
            debug_assert!(self.inner.len() == self.inner.capacity());
        } else {
            self.inner.push_back(item);
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        self.inner.pop_front()
    }
}

impl Display for RingBuffer<Instant> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.inner
                .iter()
                .rev()
                .map(|i| i.elapsed().as_millis().to_string())
                .collect::<Vec<_>>()
                .join(" ms")
        )
    }
}

impl<T> Deref for RingBuffer<T> {
    type Target = VecDeque<T>;

    fn deref(&self) -> &VecDeque<T> {
        &self.inner
    }
}

pub fn key_help_spans<'a>(strs: (&'a str, &'a str)) -> Vec<Span<'a>> {
    vec![
        Span::styled(strs.0, Style::default().bold().fg(Color::Yellow)),
        Span::raw(" "),
        Span::raw(strs.1),
    ]
}
