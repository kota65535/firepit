use crossterm::ExecutableCommand;
use human_panic::report::{Method, Report};

pub fn panic_handler(panic_info: &std::panic::PanicHookInfo) {
    let cause = panic_info.to_string();

    crossterm::terminal::disable_raw_mode().ok();
    std::io::stdout()
        .execute(crossterm::terminal::LeaveAlternateScreen)
        .ok();

    let explanation = match panic_info.location() {
        Some(location) => format!("file '{}' at line {}\n", location.file(), location.line()),
        None => "unknown.".to_string(),
    };
    let name = env!("CARGO_PKG_NAME");
    let version = env!("CARGO_PKG_VERSION");
    let report = Report::new(name, version, Method::Panic, explanation, cause);

    let report_message = if let Some(backtrace) = report.serialize() {
        format!("Backtrace: \n{backtrace}\n")
    } else {
        format!("Unable to serialize backtrace.")
    };
    eprintln!("Oops! {} has crashed. {}", name, report_message);

    std::process::exit(1);
}
