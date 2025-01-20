mod config;
mod project;
mod graph;
mod cli;
mod signal;
mod ui;
mod tui;
mod process;
mod event;
mod cui;
mod error;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}
