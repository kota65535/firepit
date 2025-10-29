use crate::app::cui::lib::{BOLD, BOLD_YELLOW};
use crate::app::cui::CuiApp;
use crate::app::tui::TuiApp;
use crate::config::{ProjectConfig, UI};
use crate::log::init_logger;
use crate::project::Workspace;
use crate::runner::TaskRunner;
use crate::tokio_spawn;
use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use nix::unistd::getcwd;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path;
use tracing::info;

/// Firepit: Simple task & service runner with a comfortable TUI
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Task names or variables.
    /// Variable are in "Name=Value" format (e.g. ENV=prod, DEBUG=true)
    #[arg(required = false)]
    pub tasks_or_vars: Vec<String>,

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
    let (tasks, vars) = parse_tasks_or_vars(&args.tasks_or_vars)?;
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

    info!("Tasks: {:?}", tasks);
    info!("Vars: {:?}", vars);
    info!("Raw root project config:\n{:#?}", root);
    info!("Raw child project config:\n{:#?}", children);

    // Print workspace information if no task specified
    if tasks.is_empty() {
        print_summary(&root, &children);
        return Ok(0);
    }

    // Aggregate information in config files into a more workable form
    let ws = Workspace::new(
        &root,
        &children,
        &tasks,
        dir.as_path(),
        &vars,
        args.force,
        args.watch,
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

fn parse_tasks_or_vars(items: &Vec<String>) -> anyhow::Result<(Vec<String>, IndexMap<String, Value>)> {
    let mut tasks = Vec::new();
    let mut vars = IndexMap::new();
    for item in items.iter() {
        match item.split_once('=') {
            Some((k, v)) => {
                let value = match serde_json::from_str::<Value>(v) {
                    Ok(parsed) => parsed,
                    Err(_) => {
                        let unquoted = if let Some(stripped) = strip_matching_quotes(v) {
                            stripped
                        } else {
                            v
                        };
                        Value::String(unquoted.to_string())
                    }
                };
                vars.insert(k.to_string(), value);
            }
            None => tasks.push(item.to_string()),
        }
    }
    Ok((tasks, vars))
}

fn strip_matching_quotes(input: &str) -> Option<&str> {
    if input.len() >= 2 {
        let first = input.as_bytes().first()?;
        let last = input.as_bytes().last()?;
        if (first == &b'"' && last == &b'"') || (first == &b'\'' && last == &b'\'') {
            return Some(&input[1..input.len() - 1]);
        }
    }
    None
}
fn print_summary(root: &ProjectConfig, children: &IndexMap<String, ProjectConfig>) -> anyhow::Result<()> {
    let (term_width, _) = crossterm::terminal::size()?;
    let mut lines = Vec::new();
    if children.is_empty() {
        // Show single project tasks
        lines.extend(project_task_lines(root));
    } else {
        // Show multi project tasks
        let cwd = getcwd().unwrap_or(path::PathBuf::new());
        if cwd == root.dir {
            // Show all projects' tasks
            lines.push(format!("{} {}", BOLD.apply_to("Project:").to_string(), "root"));
            lines.extend(project_task_lines(root));
            lines.push("".to_string());
            for c in children.values() {
                let dir = root.projects.get(&c.name).cloned().unwrap_or_default();
                lines.push("─".to_string());
                lines.push(format!("{} {}", BOLD.apply_to("Project:  ").to_string(), c.name));
                lines.push(format!("{} {}", BOLD.apply_to("Directory:").to_string(), dir));
                lines.extend(project_task_lines(c));
                lines.push("".to_string());
            }
        } else {
            // Show the current project's tasks only
            if let Some(c) = children.values().find(|v| cwd == v.dir) {
                let dir = root.projects.get(&c.name).cloned().unwrap_or_default();
                lines.push(format!("{} {}", BOLD.apply_to("Project:  ").to_string(), c.name));
                lines.push(format!("{} {}", BOLD.apply_to("Directory:").to_string(), dir));
                lines.extend(project_task_lines(c));
                lines.push("".to_string());
            }
        }
    }
    let max_width = lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let lines = lines
        .iter()
        .map(|line| {
            if line.starts_with("─") {
                "─".repeat(max_width.min(usize::from(term_width)))
            } else {
                line.clone()
            }
        })
        .collect_vec();
    for line in lines.iter() {
        eprintln!("{}", line);
    }

    Ok(())
}

fn project_task_lines(project: &ProjectConfig) -> Vec<String> {
    let mut ret = Vec::new();
    ret.push(BOLD.apply_to("Tasks:").to_string());
    // Show tasks with description first
    for v in project.tasks.values().filter(|v| v.description.is_some()) {
        ret.push(format!("  • {}", BOLD_YELLOW.apply_to(v.name.clone())));
        if let Some(description) = v.description.clone() {
            for line in description.trim_end().split("\n") {
                ret.push(format!("      {}", line));
            }
        }
    }
    // Then show tasks without description
    for v in project.tasks.values().filter(|v| v.description.is_none()) {
        ret.push(format!("  • {}", BOLD_YELLOW.apply_to(v.name.clone())));
    }
    ret
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

#[cfg(test)]
mod tests {
    use super::parse_tasks_or_vars;
    use serde_json::json;

    #[test]
    fn parse_tasks_and_vars_handles_strings_and_json() {
        let inputs = vec![
            "build".to_string(),
            "count=10".to_string(),
            r#"quoted="foo=bar""#.to_string(),
            "raw=foo=bar".to_string(),
            r#"single='baz=qux'"#.to_string(),
            "flag=true".to_string(),
        ];

        let (tasks, vars) = parse_tasks_or_vars(&inputs).unwrap();

        assert_eq!(tasks, vec!["build".to_string()]);
        assert_eq!(vars.get("count"), Some(&json!(10)));
        assert_eq!(vars.get("quoted"), Some(&json!("foo=bar")));
        assert_eq!(vars.get("raw"), Some(&json!("foo=bar")));
        assert_eq!(vars.get("single"), Some(&json!("baz=qux")));
        assert_eq!(vars.get("flag"), Some(&json!(true)));
    }
}
