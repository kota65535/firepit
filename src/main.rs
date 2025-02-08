use ::log::error;

mod cli;
mod config;
mod cui;
mod error;
mod event;
mod graph;
mod log;
mod panic;
mod probe;
mod process;
mod runner;
mod signal;
mod tui;

mod project;

#[tokio::main]
async fn main() {
    std::panic::set_hook(Box::new(panic::panic_handler));

    match cli::run().await {
        Ok(_) => {}
        Err(e) => {
            error!("Error: {:?}", e);
        }
    }
}
