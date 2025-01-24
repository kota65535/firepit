use schemars::schema_for;
use crate::config::ProjectConfig;
use crate::panic::panic_handler;

mod cli;
mod config;
mod cui;
mod error;
pub mod event;
mod graph;
mod log;
mod panic;
mod probe;
mod process;
mod runner;
mod signal;
mod tui;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    std::panic::set_hook(Box::new(panic_handler));

    cli::run().await
}
