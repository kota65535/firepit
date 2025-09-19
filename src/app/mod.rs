use std::time::Duration;

pub mod command;
pub mod cui;
mod input;
pub mod signal;
pub mod tui;

pub const FRAME_RATE: Duration = Duration::from_millis(3);
pub const DOUBLE_CLICK_DURATION: Duration = Duration::from_millis(300);
