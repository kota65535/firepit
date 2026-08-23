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

/// Grace period between `SIGINT` and `SIGKILL` for a readiness probe process.
/// Probes are short-lived checks with nothing to clean up, and a probe that is
/// being stopped has already been given its own `timeout`, so the grace period
/// is deliberately much shorter than a task's.
pub const PROBE_STOP_TIMEOUT: Duration = Duration::from_millis(1000);
pub const DYNAMIC_VAR_STOP_TIMEOUT: Duration = Duration::from_millis(1000);
