use crate::config::{ProjectConfig, ServiceConfig};
use crate::event::{EventReceiver, EventSender, TaskResult};
use crate::graph::TaskGraph;
use crate::probe::{ExecProber, LogLineProber, Prober};
use crate::process::{ChildExit, Command, ProcessManager};
use crate::signal::{get_signal, SignalHandler};
use anyhow::{anyhow, Context};
use futures::future::{join, join_all};
use log::{debug, info};
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug)]
pub struct TaskRunner {
    pub target_tasks: Vec<Task>,
    pub tasks: Vec<Task>,
    pub task_graph: TaskGraph,
    pub manager: ProcessManager,
    pub signal_handler: SignalHandler,
    pub concurrency: usize,
}

fn dir_contains(a: &PathBuf, b: &PathBuf) -> bool {
    while let Some(p) = b.parent() {
        if a == p {
            return true;
        }
    }
    false
}

impl TaskRunner {
    pub fn new(
        root: &ProjectConfig,
        children: &HashMap<String, ProjectConfig>,
        target_tasks: &Vec<String>,
        dir: PathBuf,
    ) -> anyhow::Result<TaskRunner> {
        let root_project = Project::new("", root)?;
        let mut child_projects = HashMap::new();
        for (k, v) in children.iter() {
            child_projects.insert(k.clone(), Project::new(k, v)?);
        }

        let mut tasks = root_project.tasks.values().cloned().collect::<Vec<_>>();
        for p in child_projects.values() {
            tasks.extend(p.tasks.values().cloned().collect::<Vec<_>>());
        }

        let target_tasks = if root.dir == dir {
            target_tasks
                .iter()
                .flat_map(|t| {
                    child_projects
                        .values()
                        .filter_map(|p| p.task(t))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<Task>>()
        } else if dir_contains(&root.dir, &dir) {
            target_tasks
                .iter()
                .flat_map(|t| {
                    child_projects
                        .values()
                        .filter(|p| dir_contains(&dir, &p.dir))
                        .filter_map(|p| p.task(t))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<Task>>()
        } else {
            Vec::new()
        };

        let mut task_graph = TaskGraph::new(&tasks)?;
        task_graph = task_graph
            .transitive_closure(&target_tasks.iter().map(|t| t.name.clone()).collect())?;
        tasks = task_graph.sort()?;

        debug!("Task graph:\n{:?}", task_graph);

        let manager = ProcessManager::infer();
        let signal = get_signal()?;
        let signal_handler = SignalHandler::new(signal);
        if let Some(subscriber) = signal_handler.subscribe() {
            let manager = manager.clone();
            tokio::spawn(async move {
                let _guard = subscriber.listen().await;
                manager.stop().await;
            });
        }

        Ok(TaskRunner {
            tasks,
            target_tasks,
            task_graph,
            signal_handler,
            manager,
            concurrency: root_project.concurrency,
        })
    }

    pub async fn run(&mut self, mut app_tx: EventSender) -> anyhow::Result<()> {
        // Set pty size if possible
        if let Some(pane_size) = app_tx.pane_size().await {
            self.manager.set_pty_size(pane_size.rows, pane_size.cols);
        }

        // Run visitor
        let (mut task_rx, cancel_tx, visitor_fut) = self.task_graph.visit(self.concurrency)?;

        // Cancel visitor if we received any signal
        if let Some(subscriber) = self.signal_handler.subscribe() {
            tokio::spawn(async move {
                let _guard = subscriber.listen().await;
                cancel_tx.send(true)
            });
        }

        let mut task_futs = Vec::new();

        // Receive next task when deps end (finished or killed)
        while let Some((task, deps_ok, callback)) = task_rx.recv().await {
            let mut app_tx = app_tx.with_name(&task.name);

            let manager = self.manager.clone();

            // Run log line prober
            match task.prober {
                Prober::LogLine(mut prober) => {
                    let (tx, rx) = mpsc::unbounded_channel();
                    let mut log_tx = EventSender::new(tx).with_name(&task.name);
                    let log_rx = EventReceiver::new(rx);
                    std::mem::swap(&mut app_tx, &mut log_tx);
                    task_futs.push(tokio::spawn(async move {
                        prober.probe(log_tx, log_rx).await;
                        Ok(())
                    }));
                }
                Prober::Exec(mut prover) => {
                    let probe_tx = app_tx.clone();
                    task_futs.push(tokio::spawn(async move {
                        prover.probe(probe_tx).await;
                        Ok(())
                    }));
                }
                _ => {}
            }

            task_futs.push(tokio::spawn(async move {
                // Skip the task if any deps didn't end successfully
                if !deps_ok {
                    info!("Skipping task {:?}", task.name);
                    app_tx.end_task(task.name, TaskResult::Skipped);
                    callback.send(TaskResult::Skipped).ok();
                    return Ok(());
                }

                // Build command
                let mut args = Vec::new();
                args.extend(task.shell_args.clone());
                args.push(task.command.clone());
                info!(
                    "Starting task {:?}:\nshell: {:?} {:?}\ncommand: {:?}\nenvs: {:?}",
                    task.name, task.shell, &task.shell_args, task.command, task.envs
                );
                let cmd = Command::new(task.shell)
                    .args(args)
                    .envs(task.envs)
                    .current_dir(task.working_dir)
                    .to_owned();
                // Spawn process from the command
                let mut process = match manager.spawn(cmd, Duration::from_millis(500)) {
                    Some(Ok(child)) => child,
                    Some(Err(e)) => {
                        return Err(anyhow!("unable to spawn task {:?}: {:?}", task.name, e));
                    }
                    _ => return Ok(()),
                };
                // Transfer stdin of the process to the app
                if let Some(stdin) = process.stdin() {
                    app_tx.set_stdin(task.name.clone(), stdin);
                }
                // Notify the app the task started
                app_tx.start_task(task.name.clone());

                // Wait until end
                let result = match process.wait_with_piped_outputs(app_tx.clone()).await {
                    Ok(Some(exit_status)) => match exit_status {
                        ChildExit::Finished(Some(code)) if code == 0 => TaskResult::Success,
                        ChildExit::Finished(_) => TaskResult::Failure,
                        ChildExit::Killed | ChildExit::KilledExternal => TaskResult::Stopped,
                        ChildExit::Failed => TaskResult::Failure,
                    },
                    Err(e) => return Err(anyhow!(e.to_string())),
                    Ok(None) => return Err(anyhow!("unable to determine why child exited")),
                };
                info!("Task {:?} finished. reason: {:?}", task.name, result);

                // Notify the app the task ended
                app_tx.end_task(task.name.clone(), result);
                // Notify the visitor the task ended
                callback.send(result).ok();
                Ok(())
            }));
        }
        // Wait visitor and task processes to finish
        let task_fut = join_all(task_futs);
        join(visitor_fut, task_fut).await;
        // Notify app the runner finish
        app_tx.stop().await;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub tasks: HashMap<String, Task>,
    pub dir: PathBuf,
    pub concurrency: usize,
}

impl Project {
    pub fn new(name: &str, config: &ProjectConfig) -> anyhow::Result<Project> {
        let config = config.clone();
        Ok(Project {
            name: name.to_owned(),
            tasks: Task::from_project_config(name, &config)?,
            dir: config.dir,
            concurrency: config.concurrency,
        })
    }

    pub fn task(&self, name: &String) -> Option<Task> {
        self.tasks
            .get(&Task::qualified_name(&self.name, name))
            .cloned()
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    pub command: String,
    pub envs: HashMap<String, String>,
    pub depends_on: Vec<String>,
    pub working_dir: PathBuf,
    pub is_service: bool,
    pub prober: Prober,
    pub shell: String,
    pub shell_args: Vec<String>,
}

impl Task {
    pub fn from_project_config(
        project_name: &str,
        config: &ProjectConfig,
    ) -> anyhow::Result<HashMap<String, Task>> {
        let mut ret = HashMap::new();
        for (task_name, task_config) in config.tasks.iter() {
            let task_config = task_config.clone();
            let task_name = Task::qualified_name(project_name, task_name);
            let (is_service, service_config) = match task_config.service {
                Some(service) => match service {
                    ServiceConfig::Bool(bool) => (bool, None),
                    ServiceConfig::Struct(st) => (true, Some(st)),
                },
                _ => (false, None),
            };
            let prober = if let Some(Some(readiness_probe)) =
                service_config.map(|s| s.readiness_probe)
            {
                if let Some(log_line) = readiness_probe.log_line {
                    Prober::LogLine(LogLineProber::new(
                        &task_name,
                        Regex::new(&log_line).with_context(|| format!("invalid regex pattern"))?,
                    ))
                } else if let Some(exec) = readiness_probe.exec {
                    Prober::Exec(ExecProber::new(
                        &task_name,
                        &exec.command,
                        exec.working_dir
                            .map(|t| {
                                if t.is_absolute() {
                                    t
                                } else {
                                    config.dir.join(t)
                                }
                            })
                            .unwrap_or(config.dir.clone()),
                        exec.shell
                            .clone()
                            .unwrap_or(config.shell.clone().expect("should be set"))
                            .command,
                        exec.shell
                            .clone()
                            .unwrap_or(config.shell.clone().expect("should be set"))
                            .args,
                        exec.envs
                            .clone()
                            .into_iter()
                            .chain(config.envs.clone())
                            .collect(),
                        readiness_probe.initial_delay_seconds,
                        readiness_probe.period_seconds,
                        readiness_probe.timeout_seconds,
                        readiness_probe.success_threshold,
                        readiness_probe.failure_threshold,
                    ))
                } else {
                    Prober::None
                }
            } else {
                Prober::None
            };

            ret.insert(
                task_name.clone(),
                Task {
                    name: task_name.clone(),
                    command: task_config.command,
                    envs: config
                        .envs
                        .clone()
                        .into_iter()
                        .chain(task_config.envs)
                        .collect(),
                    depends_on: task_config
                        .depends_on
                        .iter()
                        .map(|s| Task::qualified_name(project_name, s))
                        .collect(),
                    is_service,
                    prober,
                    working_dir: task_config
                        .working_dir
                        .map(|t| {
                            if t.is_absolute() {
                                t
                            } else {
                                config.dir.join(t)
                            }
                        })
                        .unwrap_or(config.dir.clone()),
                    shell: task_config
                        .shell
                        .clone()
                        .unwrap_or(config.shell.clone().expect("should be set"))
                        .command,
                    shell_args: task_config
                        .shell
                        .clone()
                        .unwrap_or(config.shell.clone().expect("should be set"))
                        .args,
                },
            );
        }
        Ok(ret)
    }
    pub fn qualified_name(project_name: &str, task_name: &str) -> String {
        if task_name.contains('#') {
            task_name.to_string()
        } else {
            format!("{}#{}", project_name, task_name)
        }
    }
}
