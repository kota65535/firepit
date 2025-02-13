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
    /// Task names to run
    #[arg(required = false)]
    pub tasks: Vec<String>,

    /// Working directory
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// Watch mode
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

    debug!("Json schema: \n{}", root.schema()?);

    // Aggregate information in config files into more workable form
    let ws = Workspace::new(&root, &children)?;
    init_logger(&root.log)?;

    // Print workspace information if no task specified
    ws.print_info();
    if args.tasks.is_empty() {
        return Ok(());
    }

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

    let mut watcher_fut = None;
    let mut watch_runner_fut = None;
    if args.watch {
        let mut file_watcher = FileWatcher::new()?;
        let watcher_handle = file_watcher.run(&dir, Duration::from_millis(500))?;
        let mut watch_runner = runner.clone();
        let app_tx = app_tx.clone();
        watcher_fut = Some(watcher_handle.future);
        watch_runner_fut = Some(tokio_spawn!("watch-runner", async move {
            watch_runner.watch(watcher_handle.rx, app_tx).await
        }));
    }

    // Start task runner
    let app_tx = app_tx.clone();
    let runner_fut = tokio_spawn!("runner", { n = 0 }, async move { runner.start(app_tx).await });

    // Wait all
    runner_fut.await??;
    app_fut.await??;

    if let (Some(watcher_fut), Some(watch_runner_fut)) = (watcher_fut, watch_runner_fut) {
        watcher_fut
            .join()
            .map_err(|e| anyhow::anyhow!("failed to join; {:?}", e))?;
        watch_runner_fut.await??;
    }

    Ok(())
}
