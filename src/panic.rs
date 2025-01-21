use crossterm::ExecutableCommand;
use human_panic::report::{Method, Report};

pub fn panic_handler(panic_info: &std::panic::PanicInfo) {
    let cause = panic_info.to_string();

    crossterm::terminal::disable_raw_mode().ok();
    std::io::stdout()
        .execute(crossterm::terminal::LeaveAlternateScreen)
        .ok();

    let explanation = match panic_info.location() {
        Some(location) => format!("file '{}' at line {}\n", location.file(), location.line()),
        None => "unknown.".to_string(),
    };

    let report = Report::new("firepit", "0.1.0", Method::Panic, explanation, cause);

    let report_message = if let Some(backtrace) = report.serialize() {
        format!("Backtrace: \n{backtrace}\n")
    } else {
        format!("Unable to serialize backtrace.")
    };
    eprintln!("Oops! Firepit has crashed. {}", report_message);
}
