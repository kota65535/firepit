use crate::config::{ProjectConfig, UI};
use crate::cui::app::CuiApp;
use crate::log::init_logger;
use crate::runner::TaskRunner;
use crate::tui::app::TuiApp;
use clap::Parser;
use log::{debug, info};
use schemars::schema_for;
use std::path;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(required = true)]
    pub tasks: Vec<String>,

    // Working directory
    #[arg(short, long, default_value = ".")]
    pub dir: String,
}

pub async fn run() -> anyhow::Result<()> {
    let args = Args::parse();

    let dir = path::absolute(args.dir)?;
    let (root, children) = ProjectConfig::new_multi(&dir)?;

    init_logger(&root.log, &root.ui)?;

    let schema = schema_for!(ProjectConfig);
    debug!("Json schema: \n{}", serde_json::to_string(&schema).unwrap());

    info!("Root project dir: {:?}", root.dir);
    if !children.is_empty() {
        info!(
            "Child projects: \n{}",
            children
                .iter()
                .map(|(k, v)| format!("{}: {:?}", k, v.dir))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    info!("Working dir: {:?}", dir);
    info!("Tasks: {:?}", args.tasks);

    let mut runner = TaskRunner::new(&root, &children, &args.tasks, dir)?;

    let target_tasks = runner.target_tasks.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    info!("Target tasks: {:?}", target_tasks);

    let dep_tasks = runner
        .tasks
        .iter()
        .map(|t| t.name.clone())
        .filter(|t| !target_tasks.contains(t))
        .collect::<Vec<_>>();
    info!("Dep tasks: {:?}", dep_tasks);

    let (app_tx, app_fut) = match root.ui {
        UI::Cui => {
            let mut app = CuiApp::new();
            let sender = app.sender();
            let fut = tokio::spawn(async move { app.run().await });
            (sender, fut)
        }
        UI::Tui => {
            let mut app = TuiApp::new(target_tasks, dep_tasks)?;
            let sender = app.sender();
            let fut = tokio::spawn(async move { app.run().await });
            (sender, fut)
        }
    };

    runner.run(app_tx.clone()).await?;
    app_fut.await?;

    Ok(())
}
