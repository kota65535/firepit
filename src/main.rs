mod cli;
mod config;
mod cui;
mod error;
mod event;
mod graph;
mod process;
mod project;
mod signal;
mod tui;
mod ui;
mod log;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
