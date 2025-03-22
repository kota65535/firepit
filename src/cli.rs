use crate::config::{ProjectConfig, UI};
use crate::cui::app::CuiApp;
use crate::cui::lib::BOLD;
use crate::log::init_logger;
use crate::project::Workspace;
use crate::runner::TaskRunner;
use crate::tokio_spawn;
use crate::tui::app::TuiApp;
use crate::watcher::FileWatcher;
use clap::Parser;
use itertools::Itertools;
use std::collections::HashMap;
use std::time::Duration;
use std::{env, path};
use tracing::info;

/// Firepit: Simple task & service runner with comfortable TUI
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Task names
    #[arg(required = false)]
    pub tasks: Vec<String>,

    /// Variables in format: {Name}={Value}
    #[arg(short, long)]
    pub var: Vec<String>,

    /// Environment variables in format: {Name}={Value}
    #[arg(short, long)]
    pub env: Vec<String>,

    /// Working directory
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// Watch mode
    #[arg(short, long, default_value = "false")]
    pub watch: bool,

    /// Log file
    #[arg(long)]
    pub log_file: Option<String>,

    /// Log level
    #[arg(long)]
    pub log_level: Option<String>,

    /// Force TUI
    #[arg(long, conflicts_with = "cui")]
    pub tui: bool,

    /// Force CUI
    #[arg(long, conflicts_with = "tui")]
    pub cui: bool,

    /// Enable instrumentation for tokio-console
    #[arg(long, hide = true, default_value = "false")]
    pub tokio_console: bool,
}

pub async fn run() -> anyhow::Result<()> {
    // Arguments
    let args = Args::parse();
    let dir = path::absolute(args.dir)?;
    let var = parse_var_and_env(&args.var)?;
    let env = parse_var_and_env(&args.env)?;

    info!("Working dir: {:?}", dir);
    info!("Tasks: {:?}", args.tasks);

    // Load config files
    let (mut root, children) = ProjectConfig::new_multi(&dir)?;

    // Override config files with CLI options
    root.log.file = args.log_file.or(root.log.file);
    root.log.level = args.log_level.unwrap_or(root.log.level);
    root.ui = if args.tui {
        UI::Tui
    } else if args.cui {
        UI::Cui
    } else {
        root.ui
    };

    init_logger(&root.log, args.tokio_console)?;

    // Print workspace information if no task specified
    if args.tasks.is_empty() {
        print_summary(&root, &children);
        return Ok(());
    }

    // Aggregate information in config files into more workable form
    let ws = Workspace::new(&root, &children, &args.tasks, dir.as_path(), &var, &env)?;

    // Create runner
    let mut runner = TaskRunner::new(&ws)?;
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
            let mut app = CuiApp::new(&runner.target_tasks, &ws.labels(), args.watch)?;
            let sender = app.sender();
            let fut = tokio_spawn!("app", async move { app.run().await });
            (sender, fut)
        }
        UI::Tui => {
            let mut app = TuiApp::new(&runner.target_tasks, &dep_tasks, &ws.labels())?;
            let sender = app.sender();
            let fut = tokio_spawn!("app", async move { app.run().await });
            (sender, fut)
        }
    };

    if args.watch {
        // Run file watcher
        let mut file_watcher = FileWatcher::new()?;
        let watcher_handle = file_watcher.run(&dir, Duration::from_millis(500))?;

        // Start runner for file watcher
        let mut watch_runner = runner.clone();
        let watch_app_tx = app_tx.clone();
        let watch_runner_fut = tokio_spawn!("watch-runner", async move {
            watch_runner.watch(watcher_handle.rx, watch_app_tx).await
        });

        // Start normal task runner
        let runner_fut = tokio_spawn!("runner", { n = 0 }, async move { runner.start(&app_tx, true).await });

        // Wait all
        runner_fut.await??;
        app_fut.await??;
        watch_runner_fut.await??;
        watcher_handle
            .future
            .join()
            .map_err(|e| anyhow::anyhow!("failed to join; {:?}", e))?;
    } else {
        // Start task runner
        let no_quit = root.ui == UI::Tui;
        let app_tx = app_tx.clone();
        let runner_fut = tokio_spawn!("runner", { n = 0 }, async move { runner.start(&app_tx, no_quit).await });

        // Wait all
        runner_fut.await??;
        app_fut.await??;
    }

    Ok(())
}

fn parse_var_and_env(env: &Vec<String>) -> anyhow::Result<HashMap<String, String>> {
    env.iter()
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => Ok((k.to_string(), v.to_string())),
            None => Ok((pair.to_string(), env::var(pair)?)),
        })
        .collect()
}

fn print_summary(root: &ProjectConfig, children: &HashMap<String, ProjectConfig>) {
    let mut lines = Vec::new();
    lines.push(format!(
        "{}:\n  dir: {:?}\n  tasks: {:?}",
        BOLD.apply_to("root").to_string(),
        root.dir,
        root.tasks.keys().sorted().collect::<Vec<_>>()
    ));
    for (k, v) in children.iter() {
        lines.push(format!(
            "{}:\n  dir: {:?}\n  tasks: {:?}",
            BOLD.apply_to(k).to_string(),
            v.dir,
            v.tasks.keys().sorted().collect::<Vec<_>>()
        ))
    }
    eprintln!("{}", lines.join("\n"));
}
