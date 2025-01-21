use crate::panic::panic_handler;

mod cli;
mod config;
mod cui;
mod error;
pub mod event;
mod graph;
mod log;
mod process;
mod project;
mod signal;
mod tui;
mod panic;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    std::panic::set_hook(Box::new(panic_handler));
    cli::run().await
}
