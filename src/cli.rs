use crate::config::{ProjectConfig, UI};
use crate::cui::app::CuiApp;
use crate::log::init_logger;
use crate::project::Workspace;
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

    // Watch mode
    #[arg(short, long, default_value = "false")]
    pub watch: bool,
}

pub async fn run() -> anyhow::Result<()> {
    // Arguments
    let args = Args::parse();
    let dir = path::absolute(args.dir)?;

    info!("Working dir: {:?}", dir);
    info!("Tasks: {:?}", args.tasks);

    // Load config files
    let (root, children) = ProjectConfig::new_multi(&dir)?;
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

    init_logger(&root.log, &root.ui)?;

    debug!("Json schema: \n{}", root.schema()?);

    // Aggregate information in config files into more workable form
    let ws = Workspace::new(&root, &children)?;

    // Create runner
    let mut runner = TaskRunner::new(&ws, &args.tasks, dir.as_path())?;
    info!("Target tasks: {:?}", runner.target_tasks);

    let dep_tasks = runner
        .tasks
        .iter()
        .map(|t| t.name.clone())
        .filter(|t| !runner.target_tasks.contains(t))
        .collect::<Vec<_>>();
    info!("Dep tasks: {:?}", dep_tasks);

    // Create & start UI
    let (app_tx, app_fut) = match root.ui {
        UI::Cui => {
            let mut app = CuiApp::new();
            let sender = app.sender();
            let fut = tokio::spawn(async move { app.run().await });
            (sender, fut)
        }
        UI::Tui => {
            let mut app = TuiApp::new(&runner.target_tasks, &dep_tasks)?;
            let sender = app.sender();
            let fut = tokio::spawn(async move { app.run().await });
            (sender, fut)
        }
    };

    // Start runner
    let runner_fut = tokio::spawn(async move { runner.run(app_tx.clone()).await });

    // Wait for both threads
    runner_fut.await??;
    app_fut.await??;

    Ok(())
}
