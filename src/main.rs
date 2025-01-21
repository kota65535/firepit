use crate::panic::panic_handler;

mod cli;
mod config;
mod cui;
mod error;
pub mod event;
mod graph;
mod log;
mod process;
mod runner;
mod signal;
mod tui;
mod panic;
mod probe;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    std::panic::set_hook(Box::new(panic_handler));
    cli::run().await
}
