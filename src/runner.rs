use crate::config::Restart;
use crate::event::{EventSender, TaskResult};
use crate::graph::{CallbackMessage, TaskGraph, Visitor, VisitorMessage};
use crate::probe::Probe;
use crate::process::{Child, ChildExit, Command, ProcessManager};
use crate::project::{Task, Workspace};
use crate::signal::{get_signal, SignalHandler};
use crate::tokio_spawn;
use anyhow::Context;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use log::{debug, info, warn};
use petgraph::Direction;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::{broadcast, watch};

#[derive(Debug)]
pub struct TaskRunner {
    pub target_tasks: Vec<String>,
    pub tasks: Vec<Task>,
    pub task_graph: TaskGraph,
    pub manager: ProcessManager,
    pub signal_handler: SignalHandler,
    pub concurrency: usize,

    // Senders/Receivers to cancel each tasks
    pub task_cancel_txs: HashMap<String, broadcast::Sender<()>>,
    pub task_cancel_rxs: HashMap<String, broadcast::Receiver<()>>,

    // Sender/Receiver to cancel this runner
    pub cancel_tx: watch::Sender<()>,
    pub cancel_rx: watch::Receiver<()>,
}

impl Clone for TaskRunner {
    fn clone(&self) -> Self {
        let mut cancel_rxs = HashMap::new();
        // Create new receivers from the corresponding senders
        // so that the sender can cancel all the task executions
        for (k, v) in self.task_cancel_txs.iter() {
            cancel_rxs.insert(k.clone(), v.subscribe());
        }

        Self {
            target_tasks: self.target_tasks.clone(),
            tasks: self.tasks.clone(),
            task_graph: self.task_graph.clone(),
            manager: self.manager.clone(),
            signal_handler: self.signal_handler.clone(),
            concurrency: self.concurrency,
            task_cancel_txs: self.task_cancel_txs.clone(),
            task_cancel_rxs: cancel_rxs,
            cancel_tx: self.cancel_tx.clone(),
            cancel_rx: self.cancel_rx.clone(),
        }
    }
}

impl TaskRunner {
    pub fn new(ws: &Workspace, target_tasks: &Vec<String>, dir: &Path) -> anyhow::Result<TaskRunner> {
        let all_tasks = ws.tasks();
        let target_tasks = ws.target_tasks(target_tasks, dir)?;

        let task_graph = TaskGraph::new(&all_tasks)?.transitive_closure(&target_tasks, Direction::Outgoing)?;
        let tasks = task_graph.sort()?;
        debug!("Task graph:\n{:?}", task_graph);

        let signal_handler = SignalHandler::new(get_signal()?);
        let manager = ProcessManager::infer();

        let mut task_cancel_txs = HashMap::new();
        let mut task_cancel_rxs = HashMap::new();
        for t in tasks.iter() {
            let (cancel_tx, cancel_rx) = broadcast::channel(1);
            task_cancel_txs.insert(t.name.clone(), cancel_tx);
            task_cancel_rxs.insert(t.name.clone(), cancel_rx);
        }
        let (cancel_tx, cancel_rx) = watch::channel(());

        Ok(TaskRunner {
            tasks,
            target_tasks,
            task_graph,
            signal_handler,
            manager,
            concurrency: ws.concurrency,
            task_cancel_txs,
            task_cancel_rxs,
            cancel_tx,
            cancel_rx,
        })
    }

    pub async fn start(&mut self, app_tx: EventSender) -> anyhow::Result<()> {
        // Set pty size if possible
        if let Some(pane_size) = app_tx.pane_size().await {
            self.manager.set_pty_size(pane_size.rows, pane_size.cols).await;
        }

        let task_graph = self.task_graph.clone();
        self.run(task_graph, app_tx, 0).await
    }

    pub async fn watch(
        &mut self,
        mut tokio_rx: UnboundedReceiver<HashSet<PathBuf>>,
        app_tx: EventSender,
    ) -> anyhow::Result<()> {
        let manager = self.manager.clone();
        let tasks = self.tasks.clone();
        let cancel_txs = self.task_cancel_txs.clone();
        let mut count = 1;

        // Cancel runner when got signal
        let cancel_tx = self.cancel_tx.clone();
        if let Some(subscriber) = self.signal_handler.subscribe() {
            tokio_spawn!("watcher-canceller", async move {
                let _guard = subscriber.listen().await;
                cancel_tx.send(()).ok();
            });
        }

        let mut cancel_rx = self.cancel_rx.clone();

        loop {
            tokio::select! {
                // Cancelling branch, quits immediately
                _ = cancel_rx.changed() => {
                    info!("Watch runner cancelled");
                    return Ok(())
                }
                // Normal branch, calculates affected tasks from the changed files
                Some(paths) = tokio_rx.recv() => {
                    let mut this = self.clone();
                    let app_tx = app_tx.clone();
                    info!("{} Changed files: {:?}", paths.len(), paths);
                    let mut changed_tasks = Vec::new();
                    for t in tasks.iter() {
                        if t.match_inputs(&paths) {
                            changed_tasks.push(t.name.clone())
                        }
                    }
                    if changed_tasks.len() > 0 {
                        info!("Changed tasks: {:?}", changed_tasks);
                        let task_graph = this
                            .task_graph
                            .transitive_closure(&changed_tasks, Direction::Incoming)?;
                        let affected_tasks = task_graph.sort()?;
                        info!(
                        "Affected tasks: {:?}",
                        affected_tasks.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
                    );
                        for t in affected_tasks.iter() {
                            info!("Cancelling task: {}", t.name);
                            if let Err(err) = cancel_txs.get(&t.name).unwrap().send(()) {
                                warn!("Failed to send cancel task {:?}: {:?}", &t.name, err);
                            }
                            manager.stop_by_label(&t.name).await;
                        }
                        info!("Cancelled all tasks");
                        tokio_spawn!("runner", { n = count }, async move {
                            this.run(task_graph, app_tx, count).await
                        });
                        count += 1;
                    }
                }
            }
        }
        info!("Watcher runner finished");
        Ok(())
    }

    pub async fn run(&mut self, task_graph: TaskGraph, mut app_tx: EventSender, num_runs: u64) -> anyhow::Result<()> {
        // Run visitor
        let Visitor {
            mut node_rx,
            cancel: cancel_visitor,
            future: mut visitor_fut,
        } = task_graph
            .visit(self.concurrency)
            .with_context(|| "Error while visiting task graph")?;
        debug!("Visitor started");

        // Canceller
        let mut cancel_rx = self.cancel_rx.clone();
        let signal_handler = self.signal_handler.clone();
        let manager = self.manager.clone();
        let cancel_visitor_cloned = cancel_visitor.clone();
        tokio_spawn!("runner-canceller", async move {
            let sbsc = signal_handler.subscribe().unwrap();
            tokio::select! {
                _ = cancel_rx.changed() => {}
                _ = sbsc.listen() => {}
            }
            manager.stop().await;
            if let Err(err) = cancel_visitor_cloned.send(()) {
                warn!("Failed to send cancel signal: {:?}", err);
            }
        });

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
            let mut cancel_rx = self.task_cancel_rxs.get(&task.name).unwrap().resubscribe();
            let task_name = task.name.clone();
            let cancel_visitor_cloned = cancel_visitor.clone();
            task_fut.push(tokio_spawn!("task", { name = task_name }, async move {
                // Skip the task if any dependency didn't finish successfully
                if !deps_ok {
                    info!("Task does not run as its dependency task failed");
                    app_tx.finish_task(TaskResult::BadDeps);
                    if let Err(e) = callback.send(CallbackMessage(Some(false))).await {
                        warn!("Failed to send callback event: {:?}", e)
                    }
                    return Ok::<(), anyhow::Error>(());
                }

                if task.is_up_to_date() {
                    info!("Task output files are newer than input files");
                    app_tx.finish_task(TaskResult::UpToDate);
                    if let Err(e) = callback.send(CallbackMessage(Some(false))).await {
                        warn!("Failed to send callback event: {:?}", e)
                    }
                    return Ok::<(), anyhow::Error>(());
                }

                info!(
                    "Task is starting.\nrestart: {:?}\nshell: {:?} {:?}\ncommand: {:?}\nenv: {:?}\nworking_dir: {:?}",
                    num_restart, task.shell, &task.shell_args, task.command, task.env, task.working_dir
                );

                app_tx = app_tx.clone();

                let process = match Self::spawn_process(task.clone(), manager.clone()).await {
                    Ok(Some(process)) => process,
                    Err(e) => anyhow::bail!("failed to spawn task {:?}: {:?}", task.name, e),
                    _ => anyhow::bail!("failed to spawn task {:?}", task.name),
                };
                let pid = process.pid().unwrap_or(0);

                // Notify the app the task started
                app_tx.start_task(task.name.clone(), pid, num_restart, num_runs);

                let mut node_result = false;
                if task.is_service {
                    // Service task branch
                    let (cancel_probe_tx, cancel_probe_rx) = watch::channel(());
                    let log_rx = app_tx.subscribe_output();
                    let mut task_fut = tokio_spawn!(
                        "process",
                        { name = task.name },
                        Self::run_process(task.clone(), process, app_tx.clone(), cancel_rx.resubscribe())
                    );
                    let mut probe_fut = tokio_spawn!(
                        "probe",
                        { name = task.name },
                        Self::run_probe(task.clone(), log_rx, cancel_probe_rx)
                    );

                    let mut task_finished = None;
                    let mut probe_finished = None;
                    while task_finished.is_none() || probe_finished.is_none() {
                        tokio::select! {
                            // Process branch
                            result = &mut task_fut, if task_finished.is_none() => {
                                // Service task process should not finish before probe
                                // So the node result is considered as `false`
                                let result = result.with_context(|| format!("task {:?} failed to run", task.name))?;

                                match result {
                                    Ok(Some(result)) => {
                                        app_tx.finish_task(result.clone());
                                    }
                                    _ => {}
                                }

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
                                        info!("Task failed to run: {:?}", e);
                                        false
                                    }
                                };
                                if should_restart {
                                    info!("Task should restart");
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
                            result = &mut probe_fut, if probe_finished.is_none() => {
                                let result = result.with_context(|| format!("task {:?} failed to run", task.name))?;
                                // The probe result is the node result
                                probe_finished = Some(result.unwrap_or(false));
                            }
                            // Cancel branch
                            _ = cancel_rx.recv() => {
                                info!("Task is canceled, stopping...");
                                if let Err(e) = cancel_probe_tx.send(()) {
                                    warn!("Failed to send cancel probe: {:?}", e)
                                }
                                if let Err(e) = cancel_visitor_cloned.send(()) {
                                    warn!("Failed to send cancel visitor: {:?}", e)
                                }
                                return Ok(());
                            }
                        }

                        if probe_finished.is_some() && task_finished.is_some() {
                            info!("Task is ready but finished and no restart");
                            break;
                        }
                        // If probe finished first
                        if let Some(probe_ok) = probe_finished {
                            if probe_ok {
                                // ...and is successful, wait for the process
                                info!("Task is ready");
                                app_tx.ready_task();
                                // Notify the visitor the task is ready
                                if let Err(e) = callback.send(CallbackMessage(Some(true))).await {
                                    warn!("Failed to send callback event: {:?}", e)
                                }
                                continue;
                            } else {
                                // ...and is failure, the process will be stopped eventually
                                info!("Task is not ready");
                                app_tx.finish_task(TaskResult::NotReady);
                                node_result = false;
                                break;
                            }
                        }
                        // If task finished before probe, consider it as failed regardless of the result
                        if let Some(_) = task_finished {
                            info!("Task finished before it become ready");
                            if let Err(e) = cancel_probe_tx.send(()) {
                                warn!("Failed to send cancel probe: {:?}", e)
                            }
                            app_tx.finish_task(TaskResult::NotReady);
                            node_result = false;
                            break;
                        }
                    }
                    node_result
                } else {
                    // Normal task branch
                    let result = tokio::select! {
                        task_result = Self::run_process(task.clone(), process, app_tx.clone(), cancel_rx.resubscribe()) => {
                            task_result?
                        }
                        _ = cancel_rx.recv() => {
                            return Ok(());
                        }
                    };
                    match result {
                        Some(result) => {
                            app_tx.finish_task(result.clone());
                        }
                        _ => {}
                    };
                    node_result = match result {
                        Some(TaskResult::Success) => true,
                        _ => false,
                    };
                    node_result
                };

                // Notify the visitor the task finished
                if let Err(e) = callback.send(CallbackMessage(Some(node_result))).await {
                    warn!("Failed to send callback event: {:?}", e)
                }

                manager.stop_by_pid(pid).await;

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
        // app_tx.stop().await;

        if let Err(err) = cancel_visitor.send(()) {
            warn!("Failed to send cancel visitor: {:?}", err);
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

    async fn spawn_process(task: Task, manager: ProcessManager) -> anyhow::Result<Option<Child>> {
        let mut args = Vec::new();
        args.extend(task.shell_args.clone());
        args.push(task.command.clone());

        let cmd = Command::new(task.shell.clone())
            .with_args(args)
            .with_envs(task.env.clone())
            .with_current_dir(task.working_dir.clone())
            .with_label(&task.name)
            .to_owned();

        let process = match manager.spawn(cmd, Duration::from_millis(500)).await {
            Some(Ok(child)) => child,
            Some(Err(e)) => anyhow::bail!("failed to spawn task {:?}: {:?}", task.name, e),
            _ => return Ok(None),
        };

        info!("Task has started. PID={}", process.pid().unwrap_or(0));

        Ok(Some(process))
    }

    async fn run_process(
        task: Task,
        mut process: Child,
        app_tx: EventSender,
        mut cancel_rx: broadcast::Receiver<()>,
    ) -> anyhow::Result<Option<TaskResult>> {
        let pid = process.pid().unwrap_or(0);

        // Transfer stdin of the process to the app
        if let Some(stdin) = process.stdin() {
            app_tx.set_stdin(task.name.clone(), stdin);
        }

        // Wait until complete
        info!("Task is waiting for output. PID={}", pid);
        tokio::select! {
            _ = cancel_rx.recv() => {
                info!("Task is canceled, stopping...");
                process.kill().await;
                return Ok(None);
            }
            result = process.wait_with_piped_outputs(app_tx.clone()) => {
                let result = match result {
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
                info!("Task has finished. PID={}", pid);
                return Ok(Some(result))
            }
        }
    }
}
