mod cli;
mod config;
mod cui;
mod error;
mod graph;
mod process;
mod project;
mod signal;
mod tui;
mod ui;
mod log;
pub mod event;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
