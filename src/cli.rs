use crate::config::{LogConfig, ProjectConfig, UI};
use crate::cui::CuiApp;
use crate::error::MultiError;
use crate::project::TaskRunner;
use crate::tui::app::TuiApp;
use anyhow::{anyhow, Context};
use clap::Parser;
use log::{info, LevelFilter};
use std::fs::File;
use std::path;
use std::str::FromStr;
use tokio::task::JoinSet;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(value_parser)]
    pub tasks: Vec<String>,

    // Working directory
    #[arg(short, long, default_value = ".")]
    pub dir: String,
}

pub fn init_logger(config: &LogConfig) -> anyhow::Result<()> {
    let mut builder = env_logger::Builder::new();
    builder.filter_level(
        LevelFilter::from_str(&config.level)
            .with_context(|| format!("invalid log level: {}", &config.level))?,
    );
    match &config.file {
        Some(file) => {
            let target = Box::new(
                File::create(file).with_context(|| format!("cannot create log file {}", file))?,
            );
            Ok(builder.target(env_logger::Target::Pipe(target)).init())
        }
        None => Ok(builder.init()),
    }
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
    let task_tx = runner.sender();

    let app_tx = match root.ui {
        UI::CUI => {
            let mut app = CuiApp::new();
            let sender = app.sender();
            set.spawn(async move { app.handle_events(task_tx).await });
            sender
        }
        UI::TUI => {
            let mut app = TuiApp::new(target_tasks, dep_tasks)?;
            let sender = app.sender();
            set.spawn(async move { app.run(task_tx).await });
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
