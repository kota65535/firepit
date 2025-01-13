use std::path;
use clap::Parser;
use log::info;
use crate::config::ProjectConfig;
use crate::project::ProjectRunner;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(value_parser)]
    pub tasks: Vec<String>,

    // Working directory
    #[arg(short, long, default_value = ".")]
    pub dir: String,
}


pub async fn run() -> anyhow::Result<i32> {
    let args = Args::parse();

    info!("Tasks: {:?}", args.tasks);

    let dir = path::absolute(args.dir)?;
    info!("Working dir: {:?}", dir);

    let (root, children) = ProjectConfig::new_multi(&dir)?;
    info!("Root project dir: {:?}", root.dir);
    if !children.is_empty() {
        info!("Child projects: \n{}", children.iter()
        .map(|(k, v)| format!("{}: {:?}", k, v.dir))
        .collect::<Vec<String>>()
        .join("\n"));
    }

    let mut runner = ProjectRunner::new(&root, &children, &args.tasks, dir)?;

    info!("Target tasks: {:?}", runner.target_tasks.iter()
        .map(|t| t.name.clone())
        .collect::<Vec<String>>());
    
    info!("Running tasks: {:?}", runner.tasks.iter()
        .map(|t| t.name.clone())
        .collect::<Vec<String>>());
    
    runner.run().await?;
    

    Ok(0)
}