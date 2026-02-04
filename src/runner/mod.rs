use crate::app::command::{AppCommandChannel, TaskResult};
use crate::config::Restart;
use crate::probe::Probe;
use crate::process::{Child, ChildExit, Command, ProcessManager};
use crate::project::{Task, Workspace};
use crate::runner::command::{RunnerCommand, RunnerCommandChannel};
use crate::runner::graph::{CallbackMessage, NodeResult, TaskGraph, VisitorCommand, VisitorHandle, VisitorMessage};
use crate::runner::watcher::{FileWatcher, FileWatcherHandle, WatcherCommand};
use crate::{tokio_spawn, TASK_STOP_TIMEOUT};
use anyhow::Context;
use chrono::{DateTime, Local};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use indexmap::IndexMap;
use petgraph::Direction;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

pub mod command;
pub mod graph;
pub mod watcher;

pub const WATCHER_DEBOUNCE_DURATION: Duration = Duration::from_millis(300);

pub struct TaskRunner {
    pub target_tasks: Vec<String>,
    pub tasks: Vec<Task>,
    pub task_graph: TaskGraph,
    pub manager: ProcessManager,
    pub watcher: Option<FileWatcher>,
    pub concurrency: usize,

    pub command_tx: RunnerCommandChannel,
    pub command_rx: broadcast::Receiver<RunnerCommand>,

    pub fail_fast: bool,

    pub start_times: Arc<Mutex<IndexMap<String, DateTime<Local>>>>,
    pub end_times: Arc<Mutex<IndexMap<String, DateTime<Local>>>>,
}

impl TaskRunner {
    pub fn new(ws: &Workspace) -> anyhow::Result<TaskRunner> {
        let all_tasks = ws.tasks();
        let target_tasks = ws.target_tasks.clone();

        let task_graph_all = TaskGraph::new(&all_tasks, Some(&target_tasks), ws.force)?;
        let task_graph = task_graph_all.transitive_closure(&target_tasks, Direction::Outgoing)?;
        let tasks = task_graph.sort()?;
        debug!("Task graph:\n{:?}", task_graph);

        let file_watcher = if ws.watch {
            Some(FileWatcher::new(&all_tasks, &ws.dir, WATCHER_DEBOUNCE_DURATION))
        } else {
            None
        };

        let manager = ProcessManager::new(ws.use_pty);

        let (command_tx, command_rx) = RunnerCommandChannel::new(1024);

        Ok(TaskRunner {
            tasks,
            target_tasks,
            task_graph,
            watcher: file_watcher,
            manager,
            concurrency: ws.concurrency,
            command_tx,
            command_rx,
            fail_fast: ws.fail_fast,
            start_times: Arc::new(Mutex::new(IndexMap::new())),
            end_times: Arc::new(Mutex::new(IndexMap::new())),
        })
    }

    pub fn command_tx(&self) -> RunnerCommandChannel {
        self.command_tx.clone()
    }

    pub async fn start(&mut self, app_tx: &AppCommandChannel, quit_on_done: bool) -> anyhow::Result<()> {
        // Set pty size if possible
        if let Some(pane_size) = app_tx.pane_size().await {
            self.manager.set_pty_size(pane_size.rows, pane_size.cols).await;
        }

        let ret = self.run(&app_tx, quit_on_done).await;

        if let Err(err) = ret {
            error!("Error: {:?}", err);
            return Err(err);
        }
        Ok(())
    }

    pub async fn run(&mut self, app_tx: &AppCommandChannel, quit_on_done: bool) -> anyhow::Result<()> {
        info!("Runner started");

        for t in self.target_tasks.iter() {
            app_tx.plan_task(t)
        }

        // Run visitors
        let VisitorHandle {
            mut node_rx,
            visitor_tx,
            future: mut visitor_fut,
        } = self
            .task_graph
            .visit(self.concurrency, quit_on_done)
            .context("error while visiting task graph")?;

        // Run file watcher
        let watcher_handle = if let Some(watcher) = &mut self.watcher {
            Some(watcher.run(&self.command_tx)?)
        } else {
            None
        };

        // Task futures
        let mut task_fut = FuturesUnordered::new();
        let targets_remaining: HashSet<String> = self.target_tasks.iter().map(|s| s.clone()).collect();
        let targets_remaining = Arc::new(Mutex::new(targets_remaining));

        while !node_rx.is_closed() {
            tokio::select! {
                // Runner command branch
                Ok(event) = self.command_rx.recv() => {
                    match event {
                        RunnerCommand::StopTasks  => {
                           info!("Stopping all tasks");
                           self.manager.stop().await;
                        }
                        RunnerCommand::StopTask { task } => {
                            info!("Stopping task: {}", task);
                            let end_time =  Local::now();
                            self.end_times.lock().expect("not poisoned").insert( task.clone(), end_time);
                            app_tx.clone().with_name(&task).finish_task(TaskResult::Stopped, Some(end_time));
                            self.manager.stop_by_label(&task).await;
                        }
                        RunnerCommand::RestartTask { task, force } => {
                            let mut tasks = vec![task.clone()];
                            if !force {
                                let task_graph = self.task_graph.transitive_closure(&tasks, Direction::Incoming)?;
                                tasks = task_graph.sort()?.iter().map(|t| t.name.clone()).collect();
                            }
                            info!("Restarting task: {:?}", tasks);

                            info!("Stopping tasks");
                            for task in tasks.iter() {
                                let end_time =  Local::now();
                                self.end_times.lock().expect("not poisoned").insert( task.clone(), end_time);
                                app_tx.clone().with_name(&task).finish_task(TaskResult::Reloading, Some(end_time));
                                self.manager.stop_by_label(&task).await;
                            }
                            info!("Stopped tasks");
                            info!("Restarting visitors");
                            for task in tasks.iter() {
                                if let Err(err) = visitor_tx.send(VisitorCommand::Restart { task: task.clone(), force }) {
                                    warn!("Failed to restart visitor for task {:?}: {:?}", task, err);
                                }
                            }
                        }
                        RunnerCommand::Quit => {
                            info!("Stopping runner");
                            info!("Stopping tasks");
                            tokio::select! {
                                Ok(RunnerCommand::Quit) = self.command_rx.recv() => {
                                    info!("Killing tasks");
                                    self.manager.close_by_kill().await;
                                    info!("Killed tasks");
                                }
                                _ =  self.manager.close() => {
                                    info!("Stopped tasks");
                                }
                            }
                            info!("Stopping visitors");
                            if let Err(err) = visitor_tx.send(VisitorCommand::Stop) {
                                warn!("Failed to stop visitors: {:?}", err);
                            }
                            node_rx.close();
                        }
                    }
                }

                // Visitor message branch
                Some(message) = node_rx.recv() => {
                    let VisitorMessage {
                        node: task,
                        deps_ok,
                        num_runs,
                        num_restart,
                        callback,
                    } = message;

                    let mut app_tx = app_tx.clone().with_name(&task.name);
                    let fail_fast = self.fail_fast;
                    let command_tx = self.command_tx.clone();
                    let manager = self.manager.clone();
                    let task_name = task.name.clone();
                    let visitor_tx_cloned = visitor_tx.clone();
                    let targets_remaining_cloned = targets_remaining.clone();
                    let start_times_cloned = self.start_times.clone();
                    let end_times_cloned = self.end_times.clone();
                    task_fut.push(tokio_spawn!("task", { name = task_name }, async move {
                        // Skip the task if any dependency task didn't finish successfully
                        if !deps_ok {
                            info!("Task does not run as its dependency task failed");
                            app_tx.finish_task(TaskResult::BadDeps, None);
                            if let Err(e) = callback.send(CallbackMessage(NodeResult::Failure)).await {
                                warn!("Failed to send callback event: {:?}", e)
                            }
                            return Ok::<(), anyhow::Error>(());
                        }

                        // Skip the task if output files are newer than input files if both defined
                        if task.is_up_to_date() {
                            info!("Task output files are newer than input files");
                            app_tx.finish_task(TaskResult::UpToDate, None);
                            if let Err(e) = callback.send(CallbackMessage(NodeResult::Success)).await {
                                warn!("Failed to send callback event: {:?}", e)
                            }
                            return Ok::<(), anyhow::Error>(());
                        }

                        // Load environment variables
                        let env = task.env.load()?;

                        info!(
                            "Task is starting.\nrun: {:?}\nrestart: {:?}\nshell: {:?} {:?}\ncommand: {:?}\nenv: {:?}\nworking_dir: {:?}",
                            num_runs, num_restart, task.shell, &task.shell_args, task.command, env, task.working_dir
                        );

                        app_tx = app_tx.clone();

                        let process = match Self::spawn_process(task.clone(), env, manager.clone()).await {
                            Ok(Some(process)) => process,
                            Err(e) => {
                                app_tx.finish_task(TaskResult::Error, None);
                                anyhow::bail!("failed to spawn task {:?}: {:?}", task.name, e)
                            }
                            _ => {
                                app_tx.finish_task(TaskResult::Error, None);
                                anyhow::bail!("failed to spawn task {:?}", task.name)
                            }
                        };
                        let pid = process.pid().unwrap_or(0);
                        let start_time = Local::now();
                        start_times_cloned.lock().expect("not poisoned").insert(task.name.clone(), start_time);

                        // Notify the app the task started
                        app_tx.start_task(task.name.clone(), pid, num_restart, task.restart.max_restart(), num_runs, start_time);

                        let mut node_result = NodeResult::None;
                        if task.is_service {
                            // Service task branch
                            let (probe_cancel_tx, probe_cancel_rx) = watch::channel(());
                            let log_rx = app_tx.subscribe_output();
                            let mut task_fut = tokio_spawn!(
                                "process",
                                { name = task.name },
                                Self::run_process(task.clone(), process, app_tx.clone())
                            );
                            let mut probe_fut = tokio_spawn!(
                                "probe",
                                { name = task.name },
                                Self::run_probe(task.clone(), log_rx, probe_cancel_rx)
                            );

                            let mut task_result = None;
                            let mut probe_result = None;
                            tokio::select! {
                                // Process branch, waiting its completion
                                result = &mut task_fut, if task_result.is_none() => {
                                    // Service task process should not finish before the probe.
                                    // So the node result is considered as `false`
                                    let result = result.with_context(|| format!("task {:?} failed to run", task.name))??;

                                    let end_time =  Local::now();
                                    end_times_cloned.lock().expect("not poisoned").insert(task.name.clone(),  Local::now());
                                    app_tx.finish_task(result.unwrap_or(TaskResult::Unknown), Some(end_time));

                                    let should_restart = match result {
                                        Some(result) => {
                                            match task.restart {
                                                Restart::Never => false,
                                                Restart::OnFailure(max) => match result {
                                                    TaskResult::Success => false,
                                                    _ => match max {
                                                        Some(max) => num_restart < max,
                                                        None => true
                                                    },
                                                },
                                                Restart::Always(max) => match max {
                                                    Some(max) => num_restart < max,
                                                    None => true
                                                },
                                            }
                                        }
                                        None => false
                                    };
                                    if should_restart {
                                        info!("Task should restart");
                                        // Send a message to restart
                                        if let Err(e) = callback.send(CallbackMessage(NodeResult::None)).await {
                                            warn!("Failed to send callback event: {:?}", e)
                                        }
                                        // Finish this closure
                                        return Ok(());
                                    }
                                    task_result = Some(false)
                                }
                                // Probe branch
                                result = &mut probe_fut, if probe_result.is_none() => {
                                    let result = result.with_context(|| format!("task {:?} failed to run", task.name))?;
                                    // The probe result is the node result
                                    probe_result = Some(result.unwrap_or(false));
                                }
                            }

                            if probe_result.is_some() && task_result.is_some() {
                                info!("Task finished without restart after being ready state");
                                node_result = NodeResult::Success;
                            }
                            // If the probe finished first
                            if let Some(probe_ok) = probe_result {
                                if probe_ok {
                                    // ...and is successful, wait for the process
                                    info!("Task is ready");
                                    app_tx.ready_task();
                                    // Notify the visitor the task is ready
                                    if let Err(e) = callback.send(CallbackMessage(NodeResult::Success)).await {
                                        warn!("Failed to send callback event: {:?}", e)
                                    }
                                    node_result = NodeResult::Success;
                                } else {
                                    // ...and is failure, kill the process
                                    info!("Task is not ready");
                                    let end_time =  Local::now();
                                    end_times_cloned.lock().expect("not poisoned").insert(task.name.clone(),  Local::now());
                                    app_tx.finish_task(TaskResult::NotReady, Some(end_time));
                                    manager.stop_by_pid(pid).await;
                                    node_result = NodeResult::Failure;
                                }
                            }
                            // If the process finished before the probe, consider it as failed regardless of the result
                            if let Some(_) = task_result {
                                info!("Task finished before it becomes ready");
                                if let Err(e) = probe_cancel_tx.send(()) {
                                    warn!("Failed to send cancel probe: {:?}", e)
                                }
                                let end_time =  Local::now();
                                end_times_cloned.lock().expect("not poisoned").insert(task.name.clone(), end_time);
                                app_tx.finish_task(TaskResult::NotReady, Some(end_time));
                                node_result = NodeResult::Failure;
                            }
                        } else {
                            // Normal task branch
                            let result = Self::run_process(task.clone(), process, app_tx.clone()).await?;
                            let end_time =  Local::now();
                            end_times_cloned.lock().expect("not poisoned").insert(task.name.clone(), end_time);
                            app_tx.finish_task(result.unwrap_or(TaskResult::Unknown), Some(end_time));
                            node_result = match result {
                                Some(TaskResult::Success) => NodeResult::Success,
                                _ => NodeResult::Failure,
                            };
                        };

                        if fail_fast && matches!(node_result, NodeResult::Failure) {
                            info!("Fail-fast enabled, stopping all tasks");
                            command_tx.stop_tasks();
                        }

                        // Notify the visitor the task finished
                        if let Err(e) = callback.send(CallbackMessage(node_result)).await {
                            warn!("Failed to send callback event: {:?}", e)
                        }

                        info!("Task finished");
                        let targets_done = {
                            let mut t = targets_remaining_cloned.lock().expect("not poisoned");
                            t.remove(&task.name);
                            t.is_empty()
                        };
                        if quit_on_done && targets_done {
                            info!("All target tasks done, stopping visitors");
                            visitor_tx_cloned.send(VisitorCommand::Stop).ok();
                        }

                        Ok(())
                    }));
                }
            }
        }

        if let Err(err) = visitor_tx.send(VisitorCommand::Stop) {
            warn!("Failed to send cancel visitor: {:?}", err);
        }
        debug!("Waiting visitors to finish...");
        Self::join(&mut visitor_fut).await?;
        debug!("Visitors finished");

        debug!("Waiting tasks to finish...");
        Self::join(&mut task_fut).await?;
        debug!("Tasks finished");

        if let Some(FileWatcherHandle {
            watcher_tx,
            future: watcher_fut,
        }) = watcher_handle
        {
            if let Err(err) = watcher_tx.send(WatcherCommand::Stop) {
                warn!("Failed to send cancel watcher: {:?}", err);
            }
            debug!("Waiting watcher to finish...");
            watcher_fut.await?;
            debug!("Watcher finished");
        }

        // Notify app the runner finished
        app_tx.done().await;

        info!("Runner finished");
        Ok(())
    }

    async fn join<T>(futures: &mut FuturesUnordered<JoinHandle<T>>) -> anyhow::Result<()> {
        while let Some(r) = futures.next().await {
            match r {
                Ok(_) => match r {
                    Err(e) => anyhow::bail!("error while waiting futures: {:?}", e),
                    _ => {}
                },
                Err(e) => anyhow::bail!("error while waiting futures: {:?}", e),
            }
        }
        Ok(())
    }

    async fn run_probe(
        task: Task,
        log_rx: UnboundedReceiver<Vec<u8>>,
        cancel: watch::Receiver<()>,
    ) -> anyhow::Result<bool> {
        match task.probe.clone() {
            Probe::LogLine(probe) => probe.run(log_rx, cancel).await,
            Probe::Exec(probe) => probe.run(cancel).await,
            Probe::None => Ok(true),
        }
    }

    async fn spawn_process(
        task: Task,
        env: HashMap<String, String>,
        manager: ProcessManager,
    ) -> anyhow::Result<Option<Child>> {
        let mut args = Vec::new();
        args.extend(task.shell_args.clone());
        args.push(task.command.clone());

        let cmd = Command::new(task.shell.clone())
            .with_args(args)
            .with_envs(env)
            .with_current_dir(task.working_dir.clone())
            .with_label(&task.name)
            .to_owned();

        let process = match manager.spawn(cmd, TASK_STOP_TIMEOUT).await {
            Some(Ok(child)) => child,
            Some(Err(e)) => anyhow::bail!("failed to spawn task process {:?}: {:?}", task.name, e),
            _ => anyhow::bail!("failed to spawn task process {:?}", task.name),
        };

        info!("Task started. PID={}", process.pid().unwrap_or(0));

        Ok(Some(process))
    }

    async fn run_process(
        task: Task,
        mut process: Child,
        app_tx: AppCommandChannel,
    ) -> anyhow::Result<Option<TaskResult>> {
        let pid = process.pid().unwrap_or(0);

        // Transfer stdin of the process to the app
        if let Some(stdin) = process.stdin() {
            app_tx.set_stdin(task.name.clone(), stdin);
        }

        // Wait until complete
        info!("Process is waiting for output. PID={}", pid);
        let result = match process.wait_with_piped_outputs(app_tx.clone(), app_tx.clone()).await {
            Ok(Some(exit_status)) => match exit_status {
                ChildExit::Finished(Some(code)) if code == 0 => TaskResult::Success,
                ChildExit::Finished(Some(code)) => TaskResult::Failure(code),
                ChildExit::Killed | ChildExit::KilledExternal => TaskResult::Stopped,
                ChildExit::Failed => TaskResult::Unknown,
                _ => TaskResult::Unknown,
            },
            Err(e) => anyhow::bail!("error while waiting task {:?}: {:?}", task.name, e),
            Ok(None) => anyhow::bail!("unable to determine why task {:?} exited", task.name),
        };
        info!("Process finished. PID={}, result={:?}", pid, result);
        Ok(Some(result))
    }

    pub fn gantt(&self) -> anyhow::Result<String> {
        let started_times = self
            .start_times
            .lock()
            .expect("not poisoned")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<IndexMap<_, _>>();
        let finished_times = self
            .end_times
            .lock()
            .expect("not poisoned")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<IndexMap<_, _>>();

        let title = self.target_tasks.join(", ");

        let mut gantt = String::from(format!(
            "gantt\n\ttitle {}\n\tdateFormat x\n\taxisFormat %H:%M:%S\n",
            title
        ));

        for (task, start_time) in started_times.iter() {
            let end_time = finished_times.get(task);
            if end_time.is_none() {
                continue;
            }
            let end_time = end_time.unwrap();

            gantt.push_str(&format!(
                "\t{} : {}, {}\n",
                task,
                start_time.timestamp_millis(),
                end_time.timestamp_millis()
            ));
        }

        Ok(gantt)
    }
}
