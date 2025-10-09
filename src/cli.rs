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
use std::fs::File;
use std::io::Write;
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

    /// Force only the specified tasks to run, ignoring their dependencies
    #[arg(short, long, default_value = "false")]
    pub force: bool,

    // The following 3 fields are a dirty workaround for clap not supporting automatic negation flags.
    // cf. https://github.com/clap-rs/clap/issues/815
    #[arg(long = "ff", overrides_with = "_no_fail_fast", hide = true)]
    _fail_fast: bool,

    #[arg(long = "no-ff", hide = true)]
    _no_fail_fast: bool,

    #[arg(
        long = "ff, --no-ff",
        help = "Enable or disable fail-fast. [default: false in TUI mode, and true in CUI mode]"
    )]
    _dummy_fail_fast_help: bool,

    /// Log file
    #[arg(long)]
    pub log_file: Option<String>,

    /// Log level
    #[arg(long)]
    pub log_level: Option<String>,

    /// Gantt chart file
    #[arg(long)]
    pub gantt_file: Option<String>,

    /// Force TUI mode
    #[arg(long, conflicts_with = "cui")]
    pub tui: bool,

    /// Force CUI mode
    #[arg(long, conflicts_with = "tui")]
    pub cui: bool,

    /// Enable instrumentation for tokio-console
    #[arg(long, hide = true, default_value = "false")]
    pub tokio_console: bool,
}

impl Args {
    /// Determine the fail-fast setting based on the command line arguments
    fn fail_fast(&self) -> Option<bool> {
        match (self._fail_fast, self._no_fail_fast) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            (false, false) => None,
            (true, true) => Some(true),
        }
    }
}

pub async fn run() -> anyhow::Result<i32> {
    // Arguments
    let args = Args::parse();
    let dir = path::absolute(&args.dir)?;
    let var = parse_var_and_env(&args.var)?;
    let env = parse_var_and_env(&args.env)?;
    let fail_fast = args.fail_fast();

    // Load config files
    let (mut root, children) = ProjectConfig::new_multi(&dir)?;

    // Override config files with CLI options
    root.log.file = args.log_file.or(root.log.file);
    root.log.level = args.log_level.unwrap_or(root.log.level);
    root.gantt_file = args.gantt_file.or(root.gantt_file);
    root.ui = if args.tui {
        UI::Tui
    } else if args.cui {
        UI::Cui
    } else {
        root.ui
    };

    init_logger(&root.log, args.tokio_console)?;

    info!("Tasks: {:?}", args.tasks);
    info!("Root project config: {:?}", root);
    info!("Child project config: {:?}", children);

    // Print workspace information if no task specified
    if args.tasks.is_empty() {
        print_summary(&root, &children);
        return Ok(0);
    }

    // Aggregate information in config files into a more workable form
    let ws = Workspace::new(
        &root,
        &children,
        &args.tasks,
        dir.as_path(),
        &var,
        &env,
        args.force,
        fail_fast,
    )?;

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
            let mut app = CuiApp::new(&runner.target_tasks, &ws.labels(), !args.watch, ws.fail_fast)?;
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
    let runner_fut = tokio_spawn!("runner", async move {
        let result = runner.start(&app_tx, quit_on_done).await;
        if let Ok(_) = result {
            if let Some(gantt_path) = root.gantt_file {
                if let Ok(gantt) = runner.gantt() {
                    save_gantt_chart(&gantt, &gantt_path);
                }
            }
        }
        result
    });
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

fn save_gantt_chart(gantt: &str, path: &str) {
    let mut file = match File::create(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to create gantt chart file: {:?}", e);
            return;
        }
    };
    if let Err(e) = file.write_all(gantt.as_bytes()) {
        eprintln!("failed to write gantt chart file");
    }
}
