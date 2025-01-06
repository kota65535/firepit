mod run;

use std::path;
use std::path::Path;
use clap::Parser;
use log::info;
use crate::config::ProjectConfig;
use crate::graph::TaskGraph;
use crate::project::ProjectRunner;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {

    #[arg(value_parser)]
    pub tasks: Vec<String>,

    // Working directory
    #[arg(short, long, default_value="./")]
    pub dir: String,
}


pub async fn run() -> anyhow::Result<i32> {
    let args = Args::parse();

    info!("tasks: {:?}", args.tasks);
    info!("dir: {:?}", args.dir);

    let (root, children) = ProjectConfig::new_multi(Path::new(&args.dir))?;

    let mut runner = ProjectRunner::new(&root, &children, &args.tasks)?;

    runner.run(&args.tasks).await?;

    Ok(0)
}