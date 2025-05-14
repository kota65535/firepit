use std::time::Duration;

pub mod command;
pub mod cui;
pub mod signal;
pub mod tui;

pub const FRAME_RATE: Duration = Duration::from_millis(3);
