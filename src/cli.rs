use crate::config::{ProjectConfig, UI};
use crate::cui::app::CuiApp;
use crate::error::MultiError;
use crate::log::init_logger;
use crate::project::TaskRunner;
use crate::tui::app::TuiApp;
use anyhow::anyhow;
use clap::Parser;
use log::info;
use std::path;
use tokio::task::JoinSet;

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

    init_logger(&root.log.clone())?;

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

    let target_tasks = runner
        .target_tasks
        .iter()
        .map(|t| t.name.clone())
        .collect::<Vec<_>>();
    info!("Target tasks: {:?}", target_tasks);

    let dep_tasks = runner
        .tasks
        .iter()
        .map(|t| t.name.clone())
        .filter(|t| !target_tasks.contains(t))
        .collect::<Vec<_>>();
    info!("Dep tasks: {:?}", dep_tasks);

    let mut set = JoinSet::new();

    let app_tx = match root.ui {
        UI::CUI => {
            let mut app = CuiApp::new();
            let sender = app.sender();
            set.spawn(async move { app.run().await });
            sender
        }
        UI::TUI => {
            let mut app = TuiApp::new(target_tasks, dep_tasks)?;
            let sender = app.sender();
            set.spawn(async move { app.run().await });
            sender
        }
    };

    set.spawn(async move { runner.run(app_tx.clone()).await });

    let results = set.join_all().await;

    let errors = results
        .into_iter()
        .filter_map(|r| r.err())
        .collect::<Vec<_>>();

    if !errors.is_empty() {
        return Err(anyhow!(MultiError::new(errors)));
    }

    Ok(())
}
