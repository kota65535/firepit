mod config;
mod project;
mod graph;
mod cli;
mod run;
mod signal;
mod ui;
mod tui;
mod process;


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    std::panic::set_hook(Box::new(panic_handler));

    let exit_code = cli::run().await.unwrap_or_else(|err| {
        let err = anyhow::anyhow!("Some error occurred");
        eprintln!("{:?}", err);
        1
    });
    std::process::exit(exit_code)
}

pub fn panic_handler(panic_info: &std::panic::PanicInfo) {
    let cause = panic_info.to_string();

    let explanation = match panic_info.location() {
        Some(location) => format!("file '{}' at line {}\n", location.file(), location.line()),
        None => "unknown.".to_string(),
    };

    println!("{}", explanation);
}
