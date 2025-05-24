use crate::app::cui::lib::BOLD;
use crate::app::cui::CuiApp;
use crate::app::tui::TuiApp;
use crate::config::{ProjectConfig, UI};
use crate::log::init_logger;
use crate::project::Workspace;
use crate::runner::TaskRunner;
use crate::tokio_spawn;
use clap::Parser;
use itertools::Itertools;
use std::collections::HashMap;
use std::{env, path};
use tracing::info;

/// Firepit: Simple task & service runner with a comfortable TUI
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

    /// Force only the specified tasks
    #[arg(short, long, default_value = "false")]
    pub force: bool,

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

pub async fn run() -> anyhow::Result<i32> {
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
        return Ok(0);
    }

    // Aggregate information in config files into a more workable form
    let ws = Workspace::new(&root, &children, &args.tasks, dir.as_path(), &var, &env, args.force)?;

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
            let mut app = CuiApp::new(&runner.target_tasks, &ws.labels(), !args.watch)?;
            let runner_tx = runner.command_tx();
            let command_tx = app.command_tx();
            let fut = tokio_spawn!("app", async move { app.run(&runner_tx).await });
            (command_tx, fut)
        }
        UI::Tui => {
            let mut app = TuiApp::new(&runner.target_tasks, &dep_tasks, &ws.labels())?;
            let runner_tx = runner.command_tx();
            let command_tx = app.command_tx();
            let fut = tokio_spawn!("app", async move { app.run(&runner_tx).await });
            (command_tx, fut)
        }
    };

    let quit_on_done = !args.watch && root.ui != UI::Tui;
    let runner_fut = tokio_spawn!(
        "runner",
        { n = 0 },
        async move { runner.start(&app_tx, quit_on_done).await }
    );
    runner_fut.await??;
    app_fut.await?
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
