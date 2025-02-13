use crate::config::{ProjectConfig, UI};
use crate::cui::app::CuiApp;
use crate::log::init_logger;
use crate::project::Workspace;
use crate::runner::TaskRunner;
use crate::tokio_spawn;
use crate::tui::app::TuiApp;
use crate::watcher::FileWatcher;
use clap::Parser;
use std::path;
use std::time::Duration;
use tracing::{debug, info};

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

    init_logger(&root.log)?;

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
            let fut = tokio_spawn!("app", async move { app.run().await });
            (sender, fut)
        }
        UI::Tui => {
            let mut app = TuiApp::new(&runner.target_tasks, &dep_tasks)?;
            let sender = app.sender();
            let fut = tokio_spawn!("app", async move { app.run().await });
            (sender, fut)
        }
    };

    let mut file_watcher = FileWatcher::new()?;
    let fwh = file_watcher.run(&dir, Duration::from_millis(500))?;

    let mut task_watcher = runner.clone();

    // Start task runner
    let app_tx_cloned = app_tx.clone();
    let runner_fut = tokio_spawn!("runner", { n = 0 }, async move { runner.start(app_tx_cloned).await });

    // Start task watcher
    let app_tx_cloned = app_tx.clone();
    let watcher_fut = tokio_spawn!(
        "watcher",
        async move { task_watcher.watch(fwh.rx, app_tx_cloned).await }
    );

    // Wait all
    runner_fut.await??;
    watcher_fut.await??;
    app_fut.await??;
    fwh.future
        .join()
        .map_err(|e| anyhow::anyhow!("failed to join; {:?}", e))?;

    Ok(())
}
