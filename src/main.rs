use log::error;
use std::env;
use std::fs::File;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env::set_var("RUST_LOG", "debug");
    env_logger::init();

    cli::run().await
}
