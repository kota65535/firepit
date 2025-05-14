mod app;
mod cli;
mod config;
mod event;
mod graph;
mod log;
mod panic;
mod probe;
mod process;
mod project;
mod runner;
mod signal;
mod template;
mod util;
mod watcher;

#[tokio::main]
async fn main() {
    std::panic::set_hook(Box::new(panic::panic_handler));

    match cli::run().await {
        Ok(ret) => {
            std::process::exit(ret);
        }
        Err(e) => {
            eprintln!("Error: {:?}", e);
            std::process::exit(1);
        }
    }
}
