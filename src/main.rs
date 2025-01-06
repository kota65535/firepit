use log::error;
use std::env;
use std::panic::PanicInfo;

mod config;
mod project;
mod graph;
mod cli;
mod run;
mod signal;
mod ui;
mod tui;
mod process;


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env::set_var("RUST_LOG", "info");
    env_logger::init();

    let exit_code = cli::run().await.unwrap_or_else(|err| {
        let err = err.context("Some error occurred");
        error!("{:?}", err);
        1
    });
    std::process::exit(exit_code)
}

// TODO: panic handler?