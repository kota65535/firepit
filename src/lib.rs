use std::time::Duration;

pub mod app;
pub mod cli;
pub mod config;
pub mod log;
pub mod panic;
pub mod probe;
pub mod process;
pub mod project;
pub mod runner;
pub mod template;
pub mod util;

pub const TASK_STOP_TIMEOUT: Duration = Duration::from_secs(5000);
pub const PROBE_STOP_TIMEOUT: Duration = Duration::from_secs(1000);
