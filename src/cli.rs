use crate::config::ProjectConfig;
use crate::cui::CuiApp;
use crate::event::task_event_channel;
use crate::project::ProjectRunner;
use clap::Parser;
use futures::future::join;
use log::info;
use std::path;

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

    let mut app = CuiApp::new();
    let (event_tx, event_rx) = task_event_channel();
    let app_future = app.handle_events(event_rx);

    let mut runner = ProjectRunner::new(&root, &children, &args.tasks, dir)?;
    info!("Target tasks: {:?}", runner.target_tasks.iter()
        .map(|t| t.name.clone())
        .collect::<Vec<String>>());
    
    info!("Running tasks: {:?}", runner.tasks.iter()
        .map(|t| t.name.clone())
        .collect::<Vec<String>>());
    
    let runner_future = runner.run(event_tx);

    let result = join(app_future, runner_future).await;

    Ok(0)
}