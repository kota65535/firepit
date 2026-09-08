use crate::app::command::TaskResult;
use crate::app::cui::lib::{BOLD_RED, RED};
use std::time::Duration;

pub mod command;
pub mod cui;
pub mod signal;
pub mod tui;

/// Prints the failed tasks to stderr in red, as a summary at the end of a run.
/// `failed` holds `(label, result)` pairs. Both UIs use this so the output is the same.
///
/// With `fail_fast` only the first failure is shown: it is the one that made the
/// runner stop the other tasks, which are listed as failed only because of that.
pub fn print_failure_summary(failed: &[(String, TaskResult)], fail_fast: bool) {
    match failed {
        [] => {}
        [(label, result), ..] if fail_fast => {
            eprintln!();
            eprintln!("{}", RED.apply_to(format!("FAILURE: {}", result.long_message(label))));
        }
        _ => {
            eprintln!();
            eprintln!(
                "{}",
                BOLD_RED.apply_to(format!("FAILURE: {} tasks failed", failed.len()))
            );
            let max_label_len = failed.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
            for (label, result) in failed {
                eprintln!(
                    "{}",
                    RED.apply_to(format!("* {:max_label_len$} : {}", label, result.short_message(true)))
                );
            }
        }
    }
}

pub const FRAME_RATE: Duration = Duration::from_millis(3);

pub const FORCE_RENDER_RATE: Duration = Duration::from_millis(100);

pub const DOUBLE_CLICK_DURATION: Duration = Duration::from_millis(300);
