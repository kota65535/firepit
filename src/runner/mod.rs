use crate::app::command::{AppCommandChannel, TaskResult};
use crate::config::Restart;
use crate::probe::Probe;
use crate::process::{Child, ChildExit, Command, ProcessManager};
use crate::project::{Task, Workspace};
use crate::runner::command::{RunnerCommand, RunnerCommandChannel};
use crate::runner::graph::{CallbackMessage, NodeResult, TaskGraph, VisitorCommand, VisitorHandle, VisitorMessage};
use crate::runner::watcher::{FileWatcher, FileWatcherHandle, WatcherCommand};
use crate::tokio_spawn;
use anyhow::Context;
use chrono::{DateTime, Local};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use indexmap::IndexMap;
use petgraph::Direction;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Finalizers pulled into the run by `finalized_by`. Awaited like the targets before
    /// quitting, but not targets themselves
    pub finalizer_tasks: Vec<String>,
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
        let finalizer_tasks = ws.finalizer_tasks.clone();

        // The awaited tasks: the run pulls in the finalizers and waits for them just like the targets
        let awaited_tasks = target_tasks.iter().chain(&finalizer_tasks).cloned().collect::<Vec<_>>();
        let task_graph_all = TaskGraph::new(&all_tasks, Some(&awaited_tasks), ws.force)?;
        let task_graph = task_graph_all.transitive_closure(&awaited_tasks, Direction::Outgoing)?;
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
            finalizer_tasks,
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

        let ret = self.run(app_tx, quit_on_done).await;

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
            .visit(self.concurrency, quit_on_done, self.fail_fast)
            .context("error while visiting task graph")?;

        // Run file watcher
        let watcher_handle = if let Some(watcher) = &mut self.watcher {
            Some(watcher.run(&self.command_tx)?)
        } else {
            None
        };

        // Task futures
        let mut task_fut = FuturesUnordered::new();
        // The awaited tasks, whose completion ends the run: the targets and their finalizers
        let awaited_remaining: HashSet<String> =
            self.target_tasks.iter().chain(&self.finalizer_tasks).cloned().collect();
        let awaited_remaining = Arc::new(Mutex::new(awaited_remaining));
        // The finalizers, spared by a stop
        let finalizer_tasks: HashSet<String> = self.finalizer_tasks.iter().cloned().collect();
        let finalizers_remaining = Arc::new(Mutex::new(finalizer_tasks.clone()));
        // Set once the runner is told to quit. From then on only the finalizers run, and the
        // visitors are stopped when the last of them is done
        let quitting = Arc::new(AtomicBool::new(false));

        while !node_rx.is_closed() {
            tokio::select! {
                // Runner command branch
                Ok(event) = self.command_rx.recv() => {
                    match event {
                        RunnerCommand::StopTasks  => {
                           // Finalizers are left running: they are meant to run to completion
                           // after the tasks they finalize, failed or not
                           info!("Stopping all tasks but finalizers");
                           self.manager.stop_except(&finalizer_tasks).await;
                        }
                        RunnerCommand::StopTask { task } => {
                            info!("Stopping task: {}", task);
                            let end_time =  Local::now();
                            self.end_times.lock().expect("not poisoned").insert( task.clone(), end_time);
                            app_tx.clone().with_name(&task).finish_task(TaskResult::Stopped, Some(end_time));
                            self.manager.stop_by_label(&task).await;
                        }
                        RunnerCommand::RestartTask { task, force } => {
                            if quitting.load(Ordering::SeqCst) {
                                info!("Ignoring restart of task {:?} while quitting", task);
                                continue;
                            }
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
                                app_tx.clone().with_name(task).finish_task(TaskResult::Reloading, Some(end_time));
                                self.manager.stop_by_label(task).await;
                            }
                            info!("Stopped tasks");
                            info!("Restarting visitors");
                            for task in tasks.iter() {
                                if let Err(err) = visitor_tx.send(VisitorCommand::Restart { task: task.clone(), force }) {
                                    warn!("Failed to restart visitor for task {:?}: {:?}", task, err);
                                }
                            }
                        }
                        RunnerCommand::Quit if quitting.load(Ordering::SeqCst) => {
                            // A second quit means the user gave up on the graceful shutdown
                            info!("Killing tasks");
                            self.manager.close_by_kill().await;
                            info!("Killed tasks");
                            info!("Stopping visitors");
                            if let Err(err) = visitor_tx.send(VisitorCommand::Stop) {
                                warn!("Failed to stop visitors: {:?}", err);
                            }
                            node_rx.close();
                        }
                        RunnerCommand::Quit => {
                            info!("Stopping runner");
                            // The finalizers are left running, and those of the stopped tasks are
                            // released by the stop, so the visitors stay up until they are done
                            info!("Stopping tasks but finalizers");
                            let stop_fut = self.manager.stop_except(&finalizer_tasks);
                            tokio::pin!(stop_fut);
                            let killed = loop {
                                tokio::select! {
                                    event = self.command_rx.recv() => {
                                        // A second quit means the user gave up on the graceful
                                        // shutdown. Anything else is irrelevant while shutting
                                        // down, so keep waiting instead of disabling this branch.
                                        if matches!(event, Ok(RunnerCommand::Quit)) {
                                            info!("Killing tasks");
                                            self.manager.close_by_kill().await;
                                            info!("Killed tasks");
                                            break true;
                                        }
                                    }
                                    _ = &mut stop_fut => {
                                        info!("Stopped tasks");
                                        break false;
                                    }
                                }
                            };
                            quitting.store(true, Ordering::SeqCst);
                            let finalizers_done = finalizers_remaining.lock().expect("not poisoned").is_empty();
                            if killed || finalizers_done {
                                info!("Stopping visitors");
                                if let Err(err) = visitor_tx.send(VisitorCommand::Stop) {
                                    warn!("Failed to stop visitors: {:?}", err);
                                }
                                node_rx.close();
                            }
                        }
                    }
                }

                // Visitor message branch
                message = node_rx.recv() => {
                    let Some(message) = message else {
                        debug!("All visitors finished");
                        break;
                    };
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
                    let awaited_remaining_cloned = awaited_remaining.clone();
                    let finalizers_remaining_cloned = finalizers_remaining.clone();
                    let quitting_cloned = quitting.clone();
                    let is_finalizer = finalizer_tasks.contains(&task.name);
                    let start_times_cloned = self.start_times.clone();
                    let end_times_cloned = self.end_times.clone();
                    task_fut.push(tokio_spawn!("task", { name = task_name }, async move {
                        let node_done = || {
                            Self::node_done(
                                &task.name,
                                &awaited_remaining_cloned,
                                &finalizers_remaining_cloned,
                                quit_on_done,
                                &quitting_cloned,
                                &visitor_tx_cloned,
                            )
                        };

                        // Skip the task if any dependency task didn't finish successfully
                        if !deps_ok {
                            info!("Task does not run as its dependency task failed");
                            app_tx.finish_task(TaskResult::BadDeps, None);
                            if let Err(e) = callback.send(CallbackMessage(NodeResult::Failure)).await {
                                warn!("Failed to send callback event: {:?}", e)
                            }
                            node_done();
                            return Ok::<(), anyhow::Error>(());
                        }

                        // Skip the task if the runner is quitting, unless it is a finalizer
                        if quitting_cloned.load(Ordering::SeqCst) && !is_finalizer {
                            info!("Task does not run as the runner is quitting");
                            app_tx.finish_task(TaskResult::Stopped, None);
                            if let Err(e) = callback.send(CallbackMessage(NodeResult::Failure)).await {
                                warn!("Failed to send callback event: {:?}", e)
                            }
                            node_done();
                            return Ok::<(), anyhow::Error>(());
                        }

                        // Skip the task if output files are newer than input files if both defined
                        if task.is_up_to_date() {
                            info!("Task output files are newer than input files");
                            app_tx.finish_task(TaskResult::UpToDate, None);
                            if let Err(e) = callback.send(CallbackMessage(NodeResult::Success)).await {
                                warn!("Failed to send callback event: {:?}", e)
                            }
                            node_done();
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

                        let node_result = if task.is_service {
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

                            let mut task_result: Option<Option<TaskResult>> = None;
                            let mut probe_result = None;
                            loop {
                                tokio::select! {
                                    // Process branch, waiting its completion
                                    result = &mut task_fut, if task_result.is_none() => {
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
                                        task_result = Some(result);
                                    }
                                    // Probe branch
                                    result = &mut probe_fut, if probe_result.is_none() => {
                                        let result = result.with_context(|| format!("task {:?} failed to run", task.name))?;
                                        probe_result = Some(result.unwrap_or(false));
                                        if probe_result == Some(true) {
                                            // Release the dependents, and keep waiting for the process to finish
                                            info!("Task is ready");
                                            app_tx.ready_task();
                                            if let Err(e) = callback.send(CallbackMessage(NodeResult::Ready)).await {
                                                warn!("Failed to send callback event: {:?}", e)
                                            }
                                        }
                                    }
                                }
                                if task_result.is_some() || probe_result == Some(false) {
                                    break;
                                }
                            }

                            match (probe_result, task_result) {
                                // The process finished after being ready
                                (Some(true), Some(result)) => {
                                    info!("Task finished after being ready");
                                    match result {
                                        Some(TaskResult::Success) => NodeResult::Success,
                                        _ => NodeResult::Failure,
                                    }
                                }
                                // The probe failed: kill the process
                                (Some(false), _) => {
                                    info!("Task is not ready");
                                    let end_time =  Local::now();
                                    end_times_cloned.lock().expect("not poisoned").insert(task.name.clone(),  Local::now());
                                    app_tx.finish_task(TaskResult::NotReady, Some(end_time));
                                    manager.stop_by_pid(pid).await;
                                    NodeResult::Failure
                                }
                                // The process finished before the probe, which is a failure regardless of the result
                                _ => {
                                    info!("Task finished before it becomes ready");
                                    if let Err(e) = probe_cancel_tx.send(()) {
                                        warn!("Failed to send cancel probe: {:?}", e)
                                    }
                                    let end_time =  Local::now();
                                    end_times_cloned.lock().expect("not poisoned").insert(task.name.clone(), end_time);
                                    app_tx.finish_task(TaskResult::NotReady, Some(end_time));
                                    NodeResult::Failure
                                }
                            }
                        } else {
                            // Normal task branch
                            let result = Self::run_process(task.clone(), process, app_tx.clone()).await?;
                            let end_time =  Local::now();
                            end_times_cloned.lock().expect("not poisoned").insert(task.name.clone(), end_time);
                            app_tx.finish_task(result.unwrap_or(TaskResult::Unknown), Some(end_time));
                            match result {
                                Some(TaskResult::Success) => NodeResult::Success,
                                _ => NodeResult::Failure,
                            }
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
                        node_done();

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

        // Stop the processes still running, ex: the services the targets depended on
        self.manager.close().await;

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

    /// Records that a node is done, and stops the visitors once the run is over: when all the
    /// awaited tasks are done under `quit_on_done`, or all the finalizers while quitting.
    fn node_done(
        name: &str,
        awaited_remaining: &Mutex<HashSet<String>>,
        finalizers_remaining: &Mutex<HashSet<String>>,
        quit_on_done: bool,
        quitting: &AtomicBool,
        visitor_tx: &broadcast::Sender<VisitorCommand>,
    ) {
        let awaited_done = {
            let mut t = awaited_remaining.lock().expect("not poisoned");
            t.remove(name);
            t.is_empty()
        };
        let finalizers_done = {
            let mut f = finalizers_remaining.lock().expect("not poisoned");
            f.remove(name);
            f.is_empty()
        };
        if (quit_on_done && awaited_done) || (quitting.load(Ordering::SeqCst) && finalizers_done) {
            info!("All awaited tasks done, stopping visitors");
            visitor_tx.send(VisitorCommand::Stop).ok();
        }
    }

    async fn join<T>(futures: &mut FuturesUnordered<JoinHandle<T>>) -> anyhow::Result<()> {
        while let Some(r) = futures.next().await {
            if let Err(e) = r {
                anyhow::bail!("error while waiting futures: {:?}", e);
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

        let process = match manager.spawn(cmd, task.stop_timeout).await {
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
                ChildExit::Finished(Some(0)) => TaskResult::Success,
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
            .map(|(k, v)| (k.clone(), *v))
            .collect::<IndexMap<_, _>>();
        let finished_times = self
            .end_times
            .lock()
            .expect("not poisoned")
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect::<IndexMap<_, _>>();

        let title = self.target_tasks.join(", ");

        let mut gantt = format!("gantt\n\ttitle {}\n\tdateFormat x\n\taxisFormat %H:%M:%S\n", title);

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
