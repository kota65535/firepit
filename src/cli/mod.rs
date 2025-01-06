mod run;

use std::path;
use std::path::Path;
use clap::Parser;
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

    println!("tasks: {:?}", args.tasks);
    println!("dir: {:?}", args.dir);

    let (root, children) = ProjectConfig::load_all(Path::new(&args.dir))?;

    let mut runner = ProjectRunner::new(&root, &children)?;

    runner.run_tasks(&args.tasks).await?;

    // run::run(args.tasks).await

    // let config = ProjectConfig::load_all(Path::new(&args.dir)).unwrap();
    // 
    // let runner = ProjectRunner::load(config);
    // 
    // let tasks = vec!["#foo".to_string()];
    // runner.unwrap().run_tasks(&tasks).await;

    // println!("{:?}", runner)
    Ok(0)
}