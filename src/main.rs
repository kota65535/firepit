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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
