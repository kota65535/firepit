use crate::config::{HealthCheckConfig, ProjectConfig, Restart, ServiceConfig};
use crate::event::{EventSender, TaskResult};
use crate::graph::{CallbackMessage, TaskGraph, Visitor, VisitorMessage};
use crate::probe::{ExecProber, LogLineProber, Prober};
use crate::process::{Child, ChildExit, Command, ProcessManager};
use crate::signal::{get_signal, SignalHandler};
use anyhow::Context;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use log::{debug, info, warn};
use nix::NixPath;
use regex::Regex;
use std::collections::HashMap;
use std::path;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::watch;

#[derive(Debug)]
pub struct TaskRunner {
    pub target_tasks: Vec<Task>,
    pub tasks: Vec<Task>,
    pub task_graph: TaskGraph,
    pub manager: ProcessManager,
    pub signal_handler: SignalHandler,
    pub concurrency: usize,
}

impl TaskRunner {
    pub fn new(
        root: &ProjectConfig,
        children: &HashMap<String, ProjectConfig>,
        target_tasks: &Vec<String>,
        dir: &Path,
    ) -> anyhow::Result<TaskRunner> {
        let root_project = Project::new("", root)?;
        let mut child_projects = HashMap::new();
        for (k, v) in children.iter() {
            child_projects.insert(k.clone(), Project::new(k, v)?);
        }

        // All tasks
        let mut tasks = root_project.tasks.values().cloned().collect::<Vec<_>>();
        for p in child_projects.values() {
            tasks.extend(p.tasks.values().cloned().collect::<Vec<_>>());
        }

        let target_tasks = if root.dir == dir {
            target_tasks
                .iter()
                .flat_map(|t| {
                    if root_project.task(t).is_some() {
                        vec![root_project.task(t).unwrap()]
                    } else {
                        child_projects.values().filter_map(|p| p.task(t)).collect::<Vec<_>>()
                    }
                })
                .collect::<Vec<Task>>()
        } else if dir_contains(&root.dir, &dir).unwrap_or(false) {
            target_tasks
                .iter()
                .flat_map(|t| {
                    child_projects
                        .values()
                        .filter(|p| dir_contains(&dir, &p.dir).unwrap_or(false))
                        .filter_map(|p| p.task(t))
                })
                .collect::<Vec<Task>>()
        } else {
            Vec::new()
        };

        let mut task_graph = TaskGraph::new(&tasks)?;
        task_graph = task_graph.transitive_closure(&target_tasks.iter().map(|t| t.name.clone()).collect())?;
        tasks = task_graph.sort()?;

        debug!("Task graph:\n{:?}", task_graph);

        let manager = ProcessManager::infer();
        let signal = get_signal()?;
        let signal_handler = SignalHandler::new(signal);
        if let Some(subscriber) = signal_handler.subscribe() {
            let manager = manager.clone();
            tokio::spawn(async move {
                let _guard = subscriber.listen().await;
                debug!("Stopping ProcessManager");
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
        let Visitor {
            mut node_rx,
            cancel: cancel_tx,
            future: mut visitor_fut,
        } = self
            .task_graph
            .visit(self.concurrency)
            .with_context(|| "Error while visiting task graph")?;
        info!("Visitor started");

        // Cancel visitor if we received any signal
        if let Some(subscriber) = self.signal_handler.subscribe() {
            tokio::spawn(async move {
                let _guard = subscriber.listen().await;
                cancel_tx.send(true)
            });
        }

        let mut task_fut = FuturesUnordered::new();

        // Receive the next task when its dependencies finished
        while let Some(VisitorMessage {
            node: task,
            deps_ok,
            count: num_restart,
            callback,
        }) = node_rx.recv().await
        {
            let mut app_tx = app_tx.with_name(&task.name);
            let manager = self.manager.clone();

            task_fut.push(tokio::spawn(async move {
                // Skip the task if any dependency didn't finish successfully
                if !deps_ok {
                    info!("Task {:?} does not run because of its failed dependency", task.name);
                    app_tx.finish_task(TaskResult::BadDeps);
                    if let Err(e) = callback.send(CallbackMessage(Some(false))).await {
                        warn!("Failed to send callback event: {:?}", e)
                    }
                    return Ok::<(), anyhow::Error>(());
                }

                info!(
                    "Task {:?} is starting ({}). \nshell: {:?} {:?}\ncommand: {:?}\nenv: {:?}\nworking_dir: {:?}",
                    task.name, num_restart, task.shell, &task.shell_args, task.command, task.env, task.working_dir
                );

                app_tx = app_tx.clone();

                let process = match spawn_process(task.clone(), manager.clone()) {
                    Ok(Some(process)) => process,
                    Err(e) => anyhow::bail!("failed to spawn task {:?}: {:?}", task.name, e),
                    _ => anyhow::bail!("failed to spawn task {:?}", task.name),
                };
                let pid = process.pid().unwrap_or(0);

                // Notify the app the task started
                app_tx.start_task(task.name.clone(), pid, num_restart);

                let mut node_result = false;
                if task.is_service {
                    // Service task branch
                    let (cancel_prober_tx, cancel_prober_rx) = watch::channel(());
                    let log_rx = app_tx.subscribe_output();
                    let mut task_fut = tokio::spawn(run_process(task.clone(), process, app_tx.clone()));
                    let mut prober_fut = tokio::spawn(run_prober(task.clone(), log_rx, cancel_prober_rx));

                    let mut task_finished = None;
                    let mut prober_finished = None;
                    while task_finished.is_none() || prober_finished.is_none() {
                        tokio::select! {
                            // Process branch
                            result = &mut task_fut, if task_finished.is_none() => {
                                // Service task process should not finish before prober
                                // So the node result is considered as `false`
                                let result = result.with_context(|| format!("Task {:?} failed to run", task.name))?;
                                let should_restart = match result {
                                    Ok(result) => {
                                        match result {
                                            Some(result) => {
                                                match task.restart {
                                                    Restart::Never => false,
                                                    Restart::OnFailure(max) => match result {
                                                        TaskResult::Success => false,
                                                        _ => max == 0 || num_restart < max,
                                                    },
                                                    Restart::Always(max) => max == 0 || num_restart < max,
                                                }
                                            }
                                            None => true
                                        }
                                    }
                                    Err(e) => {
                                        info!("Task {:?} failed to run: {:?}", task.name, e);
                                        false
                                    }
                                };
                                if should_restart {
                                    info!("Task {:?} should restart", task.name);
                                    // Send restart message
                                    if let Err(e) = callback.send(CallbackMessage(None)).await {
                                        warn!("Failed to send callback event: {:?}", e)
                                    }
                                    // Finish this closure
                                    return Ok(());
                                }
                                task_finished = Some(false)
                            }
                            // Prober branch
                            result = &mut prober_fut, if prober_finished.is_none() => {
                                let result = result.with_context(|| format!("Task {:?} failed to run", task.name))?;
                                // The prober result is the node result
                                prober_finished = Some(result.unwrap_or(false));
                            }
                        }
                        // If prober finished first
                        if let Some(prober_ok) = prober_finished {
                            if prober_ok {
                                // ...and is successful, wait for the process
                                info!("Task {:?} is ready", task.name);
                                app_tx.ready_task();
                                // Notify the visitor the task is ready
                                if let Err(e) = callback.send(CallbackMessage(Some(true))).await {
                                    warn!("Failed to send callback event: {:?}", e)
                                }
                                continue;
                            } else {
                                // ...and is failure, the process will be stopped eventually
                                info!("Task {:?} is not ready", task.name);
                                app_tx.finish_task(TaskResult::NotReady);
                                node_result = false;
                                break;
                            }
                        }
                        // If task finished before prober, consider it as failed regardless of the result
                        if let Some(_) = task_finished {
                            info!("Task {:?} finished before it become ready", task.name);
                            if let Err(e) = cancel_prober_tx.send(()) {
                                warn!("Failed to send cancel prober: {:?}", e)
                            }
                            app_tx.finish_task(TaskResult::NotReady);
                            node_result = false;
                            break;
                        }
                    }
                    node_result
                } else {
                    // Normal task branch
                    let task_result = run_process(task.clone(), process, app_tx.clone()).await?;
                    node_result = match task_result {
                        Some(TaskResult::Success) => true,
                        _ => false,
                    };
                    node_result
                };

                // Notify the visitor the task finished
                if let Err(e) = callback.send(CallbackMessage(Some(node_result))).await {
                    warn!("Failed to send callback event: {:?}", e)
                }

                Ok(())
            }));
        }

        debug!("Waiting for visitor");
        while let Some(r) = visitor_fut.next().await {
            match r {
                Ok(r) => match r {
                    Ok(r) => r,
                    Err(e) => anyhow::bail!("error while waiting visitor thread: {:?}", e),
                },
                Err(e) => anyhow::bail!("error while waiting visitor thread: {:?}", e),
            }
        }
        debug!("Visitor finished");

        debug!("Waiting for tasks");
        while let Some(r) = task_fut.next().await {
            match r {
                Ok(r) => match r {
                    Ok(r) => r,
                    Err(e) => anyhow::bail!("error while waiting task thread: {:?}", e),
                },
                Err(e) => anyhow::bail!("error while waiting task thread: {:?}", e),
            }
        }
        debug!("Tasks finished");

        // Notify app the runner finished
        app_tx.stop().await;

        Ok(())
    }
}

async fn run_prober(
    task: Task,
    log_rx: UnboundedReceiver<Vec<u8>>,
    cancel: watch::Receiver<()>,
) -> anyhow::Result<bool> {
    match task.prober.clone() {
        Prober::LogLine(prober) => prober.probe(log_rx, cancel).await,
        Prober::Exec(prober) => prober.probe(cancel).await,
        Prober::None => Ok(true),
    }
}

fn spawn_process(task: Task, manager: ProcessManager) -> anyhow::Result<Option<Child>> {
    let mut args = Vec::new();
    args.extend(task.shell_args.clone());
    args.push(task.command.clone());

    let cmd = Command::new(task.shell.clone())
        .args(args)
        .envs(task.env.clone())
        .current_dir(task.working_dir.clone())
        .to_owned();

    let process = match manager.spawn(cmd, Duration::from_millis(500)) {
        Some(Ok(child)) => child,
        Some(Err(e)) => anyhow::bail!("failed to spawn task {:?}: {:?}", task.name, e),
        _ => return Ok(None),
    };
    let pid = process.pid().unwrap_or(0);

    info!("Task {:?} has started. PID={}", task.name, pid);

    Ok(Some(process))
}

async fn run_process(task: Task, mut process: Child, app_tx: EventSender) -> anyhow::Result<Option<TaskResult>> {
    let pid = process.pid().unwrap_or(0);

    // Transfer stdin of the process to the app
    if let Some(stdin) = process.stdin() {
        app_tx.set_stdin(task.name.clone(), stdin);
    }

    // Wait until complete
    info!("Task {:?} waiting for output. PID={}", task.name, pid);
    let result = match process.wait_with_piped_outputs(app_tx.clone()).await {
        Ok(Some(exit_status)) => match exit_status {
            ChildExit::Finished(Some(code)) if code == 0 => TaskResult::Success,
            ChildExit::Finished(Some(code)) => TaskResult::Failure(code),
            ChildExit::Killed | ChildExit::KilledExternal => TaskResult::Stopped,
            ChildExit::Failed => TaskResult::Unknown,
            _ => TaskResult::Unknown,
        },
        Err(e) => anyhow::bail!("error while waiting task {:?}: {:?}", task.name, e),

        Ok(None) => anyhow::bail!("unable to determine why child exited"),
    };

    info!("Task {:?} has finished. PID={}", task.name, pid);

    // Notify the app the task ended
    app_tx.finish_task(result.clone());

    Ok(Some(result))
}

fn dir_contains(a: &Path, b: &Path) -> anyhow::Result<bool> {
    let a = path::absolute(a)?;
    let mut b = path::absolute(b)?;
    while let Some(p) = b.parent() {
        if p.is_empty() {
            break;
        }
        if a == p {
            return Ok(true);
        }
        b = p.to_path_buf();
    }
    Ok(false)
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
            dir: config.dir.clone(),
            concurrency: config.concurrency,
        })
    }

    pub fn task(&self, name: &String) -> Option<Task> {
        self.tasks.get(&Task::qualified_name(&self.name, name)).cloned()
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    pub command: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    pub env: HashMap<String, String>,
    pub depends_on: Vec<String>,
    pub working_dir: PathBuf,
    pub is_service: bool,
    pub prober: Prober,
    pub restart: Restart,
}

pub fn load_env_files(files: &Vec<String>) -> anyhow::Result<HashMap<String, String>> {
    let mut ret = HashMap::new();
    for e in files.iter() {
        for item in dotenvy::from_path_iter(Path::new(e))? {
            let (key, value) = item?;
            ret.insert(key, value);
        }
    }
    Ok(ret)
}

impl Task {
    pub fn from_project_config(project_name: &str, config: &ProjectConfig) -> anyhow::Result<HashMap<String, Task>> {
        let mut ret = HashMap::new();
        for (task_name, task_config) in config.tasks.iter() {
            let task_config = task_config.clone();
            let task_name = Task::qualified_name(project_name, task_name);
            let shell = task_config.shell.clone().unwrap_or(config.shell.clone());
            let working_dir = PathBuf::from(task_config.working_dir.unwrap_or(config.working_dir.clone()));
            let working_dir = if working_dir.is_absolute() {
                working_dir
            } else {
                config.dir.join(working_dir)
            };
            let project_env = load_env_files(&config.env_files)?
                .into_iter()
                .chain(config.env.clone())
                .collect::<HashMap<_, _>>();
            let task_env = load_env_files(&task_config.env_files)?
                .into_iter()
                .chain(task_config.env.clone())
                .collect::<HashMap<_, _>>();
            let env = project_env.into_iter().chain(task_env).collect::<HashMap<_, _>>();

            let (is_service, prober, restart) = match task_config.service {
                Some(service) => match service {
                    ServiceConfig::Bool(bool) => (bool, Prober::None, Restart::Never),
                    ServiceConfig::Struct(st) => {
                        let prober = match st.healthcheck {
                            Some(healthcheck) => match healthcheck {
                                HealthCheckConfig::Log(c) => Prober::LogLine(LogLineProber::new(
                                    &task_name,
                                    Regex::new(&c.log).with_context(|| format!("invalid regex pattern {:?}", c.log))?,
                                    c.timeout,
                                    c.start_period,
                                )),
                                HealthCheckConfig::Exec(c) => {
                                    let healthcheck_shell = c.shell.unwrap_or(shell.clone());
                                    let healthcheck_working_dir = c
                                        .working_dir
                                        .map(|d| path::absolute(d))
                                        .unwrap_or(Ok(working_dir.clone()))?;
                                    Prober::Exec(ExecProber::new(
                                        &task_name,
                                        &c.command,
                                        &healthcheck_shell.command,
                                        healthcheck_shell.args,
                                        healthcheck_working_dir,
                                        env.clone(),
                                        c.interval,
                                        c.timeout,
                                        c.retries,
                                        c.start_period,
                                    ))
                                }
                            },
                            None => Prober::None,
                        };
                        (true, prober, st.restart)
                    }
                },
                _ => (false, Prober::None, Restart::Never),
            };

            ret.insert(
                task_name.clone(),
                Task {
                    name: task_name.clone(),
                    command: task_config.command,
                    shell: shell.command,
                    shell_args: shell.args,
                    working_dir,
                    env,
                    depends_on: task_config
                        .depends_on
                        .iter()
                        .map(|s| Task::qualified_name(project_name, s))
                        .collect(),
                    is_service,
                    prober,
                    restart,
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
