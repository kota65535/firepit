use crate::config::{HealthCheckConfig, ProjectConfig, Restart, ServiceConfig};
use crate::event::{EventReceiver, EventSender, TaskResult};
use crate::graph::{CallbackMessage, NodeStatus, TaskGraph, Visitor, VisitorMessage};
use crate::probe::{ExecProber, LogLineProber, NullProber, Prober};
use crate::process::{ChildExit, Command, ProcessManager};
use crate::signal::{get_signal, SignalHandler};
use anyhow::Context;
use futures::StreamExt;
use log::{debug, info, warn};
use regex::Regex;
use std::collections::HashMap;
use std::path;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{JoinHandle, JoinSet};

#[derive(Debug)]
pub struct TaskRunner {
    pub target_tasks: Vec<Task>,
    pub tasks: Vec<Task>,
    pub task_graph: TaskGraph,
    pub manager: Arc<Mutex<ProcessManager>>,
    pub signal_handler: SignalHandler,
    pub concurrency: usize,
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
                .flat_map(|t| child_projects.values().filter_map(|p| p.task(t)).collect::<Vec<_>>())
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
        task_graph = task_graph.transitive_closure(&target_tasks.iter().map(|t| t.name.clone()).collect())?;
        tasks = task_graph.sort()?;

        debug!("Task graph:\n{:?}", task_graph);

        let manager = Arc::new(Mutex::new(ProcessManager::infer()));
        let signal = get_signal()?;
        let signal_handler = SignalHandler::new(signal);
        if let Some(subscriber) = signal_handler.subscribe() {
            let manager = manager.clone();
            tokio::spawn(async move {
                let _guard = subscriber.listen().await;
                debug!("Stopping ProcessManager");
                manager.lock().await.stop().await;
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
            self.manager.lock().await.set_pty_size(pane_size.rows, pane_size.cols);
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

        let mut task_fut = JoinSet::new();

        // Receive the next task when its dependencies finished
        while let Some(VisitorMessage {
            node: task,
            deps_ok,
            callback,
        }) = node_rx.recv().await
        {
            let mut app_tx = app_tx.with_name(&task.name);
            let manager = self.manager.clone();

            // Keep original EventSender
            let app_tx_orig = app_tx.clone();

            task_fut.spawn(tokio::spawn(async move {
                // Skip the task if any dependency didn't finish successfully
                if !deps_ok {
                    info!("Task {:?} does not run because of its failed dependency", task.name);
                    app_tx.finish_task(task.name, TaskResult::BadDeps);
                    callback.send(CallbackMessage(NodeStatus::Failure)).await?;
                    return Ok::<(), anyhow::Error>(());
                }

                let mut restart_count = 0;
                loop {
                    info!(
                        "Task {:?} is starting. \nshell: {:?} {:?}\ncommand: {:?}\nenv: {:?}\nworking_dir: {:?}",
                        task.name, task.shell, &task.shell_args, task.command, task.env, task.working_dir
                    );

                    app_tx = app_tx_orig.clone();

                    // Start prober
                    let prober_fut = spawn_prober(&task, &mut app_tx, callback.clone());
                    // Start task process
                    let task_result =
                        match run_task_process(task.clone(), manager.clone(), app_tx.clone(), restart_count).await {
                            Ok(r) => r,
                            Err(e) => {
                                info!("Task {:?} does not start. reason: {:?}", task.name, e);
                                callback.send(CallbackMessage(NodeStatus::Failure)).await.ok();
                                prober_fut.await;
                                break;
                            }
                        };
                    // Stop prober after task process exits
                    prober_fut.await;

                    // Is this task successful?
                    let node_result = match task_result {
                        Some(task_result) => {
                            info!("Task {:?} finished. reason: {:?}", task.name, task_result);

                            if manager.lock().await.is_closed() {
                                debug!("Exiting because ProcessManager is closed");
                                callback.send(CallbackMessage(NodeStatus::Failure)).await.ok();
                                break;
                            }

                            if task.is_service {
                                let should_restart = match task.restart {
                                    Restart::Never => false,
                                    Restart::OnFailure => match task_result {
                                        TaskResult::Success => false,
                                        _ => true,
                                    },
                                    Restart::Always => true,
                                };
                                if should_restart {
                                    restart_count += 1;
                                    continue;
                                }
                            }

                            match task_result {
                                TaskResult::Success => NodeStatus::Success,
                                _ => NodeStatus::Failure,
                            }
                        }
                        None => NodeStatus::Failure,
                    };

                    // Notify the visitor the task finished
                    callback.send(CallbackMessage(node_result)).await.ok();
                    break;
                }

                Ok(())
            }));
        }

        while let Some(r) = visitor_fut.next().await {
            match r {
                Ok(r) => match r {
                    Ok(r) => r,
                    Err(e) => anyhow::bail!("error while waiting visitor thread: {:?}", e),
                },
                Err(e) => anyhow::bail!("error while waiting visitor thread: {:?}", e),
            }
        }

        info!("Visitor is exiting");

        // Notify app the runner finish
        app_tx.stop().await;

        info!("Runner is exiting");

        Ok(())
    }
}

fn spawn_prober(
    task: &Task,
    app_tx: &mut EventSender,
    callback: mpsc::Sender<CallbackMessage>,
) -> JoinHandle<anyhow::Result<()>> {
    if task.is_service {
        match task.prober.clone() {
            Prober::LogLine(mut prober) => {
                let (tx, rx) = mpsc::unbounded_channel();
                let mut log_tx = EventSender::new(tx).with_name(&task.name);
                let log_rx = EventReceiver::new(rx);
                std::mem::swap(app_tx, &mut log_tx);
                let callback = callback.clone();
                tokio::spawn(async move { prober.probe(log_tx, log_rx, callback).await })
            }
            Prober::Exec(mut prober) => {
                let (tx, rx) = mpsc::unbounded_channel();
                let mut log_tx = EventSender::new(tx).with_name(&task.name);
                let log_rx = EventReceiver::new(rx);
                std::mem::swap(app_tx, &mut log_tx);
                let callback = callback.clone();
                tokio::spawn(async move { prober.probe(log_tx, log_rx, callback).await })
            }
            Prober::None(prober) => {
                let probe_tx = app_tx.clone();
                tokio::spawn(async move { prober.probe(probe_tx).await })
            }
        }
    } else {
        tokio::spawn(async move { Ok(()) })
    }
}

async fn run_task_process(
    task: Task,
    manager: Arc<Mutex<ProcessManager>>,
    app_tx: EventSender,
    restart_count: u64,
) -> anyhow::Result<Option<TaskResult>> {
    let mut args = Vec::new();
    args.extend(task.shell_args.clone());
    args.push(task.command.clone());

    let cmd = Command::new(task.shell.clone())
        .args(args)
        .envs(task.env.clone())
        .current_dir(task.working_dir.clone())
        .to_owned();

    if app_tx.tx.is_closed() {
        return Ok(None);
    }

    let mut process = match manager.lock().await.spawn(cmd, Duration::from_millis(500)) {
        Some(Ok(child)) => child,
        Some(Err(e)) => anyhow::bail!("failed to spawn task {:?}: {:?}", task.name, e),
        _ => return Ok(None),
    };

    info!("Task {:?} has started. PID={}", task.name, process.pid().unwrap_or(0));

    // Notify the app the task started
    app_tx.start_task(task.name.clone(), process.pid().unwrap(), restart_count);

    // Transfer stdin of the process to the app
    if let Some(stdin) = process.stdin() {
        app_tx.set_stdin(task.name.clone(), stdin);
    }

    // Wait until complete
    info!(
        "Task {:?} waiting for output. PID={}",
        task.name,
        process.pid().unwrap_or(0)
    );
    let result = match process.wait_with_piped_outputs(app_tx.clone()).await {
        Ok(Some(exit_status)) => match exit_status {
            ChildExit::Finished(Some(code)) if code == 0 => TaskResult::Success,
            ChildExit::Finished(Some(code)) => TaskResult::Failure(code),
            ChildExit::Killed | ChildExit::KilledExternal => TaskResult::Stopped,
            ChildExit::Failed => TaskResult::Unknown,
            _ => TaskResult::Unknown,
        },
        Err(e) => {
            return Err(anyhow::Error::from(e).context(format!("error while waiting task {:?}", task.name)));
        }
        Ok(None) => return Err(anyhow::anyhow!("unable to determine why child exited")),
    };

    info!("Task {:?} has finished. PID={}", task.name, process.pid().unwrap_or(0));

    // Notify the app the task ended
    app_tx.finish_task(task.name.clone(), result.clone());

    Ok(Some(result))
}

fn dir_contains(a: &PathBuf, b: &PathBuf) -> bool {
    while let Some(p) = b.parent() {
        if a == p {
            return true;
        }
    }
    false
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
                    ServiceConfig::Bool(bool) => (bool, Prober::None(NullProber::new(&task_name)), Restart::Never),
                    ServiceConfig::Struct(st) => {
                        let prober = match st.healthcheck {
                            Some(healthcheck) => match healthcheck {
                                HealthCheckConfig::Log(c) => Prober::LogLine(LogLineProber::new(
                                    &task_name,
                                    Regex::new(&c.log).with_context(|| format!("invalid regex pattern"))?,
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
                                        healthcheck_shell.command,
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
                            None => Prober::None(NullProber::new(&task_name)),
                        };
                        (true, prober, st.restart)
                    }
                },
                _ => (false, Prober::None(NullProber::new(&task_name)), Restart::Never),
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
