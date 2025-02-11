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
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::watch;
use tracing::{debug, info, warn};

#[derive(Debug)]
pub struct TaskRunner {
    pub target_tasks: Vec<String>,
    pub tasks: Vec<Task>,
    pub task_graph: TaskGraph,
    pub manager: ProcessManager,
    pub signal_handler: SignalHandler,
    pub concurrency: usize,
}

impl TaskRunner {
    pub fn new(ws: &Workspace, target_tasks: &Vec<String>, dir: &Path) -> anyhow::Result<TaskRunner> {
        let all_tasks = ws.tasks();
        let target_tasks = ws.target_tasks(target_tasks, dir)?;

        let task_graph = TaskGraph::new(&all_tasks)?.transitive_closure(&target_tasks)?;
        let tasks = task_graph.sort()?;
        debug!("Task graph:\n{:?}", task_graph);

        let manager = ProcessManager::infer();

        let signal = get_signal()?;
        let signal_handler = SignalHandler::new(signal);
        if let Some(subscriber) = signal_handler.subscribe() {
            let manager = manager.clone();
            tokio_spawn!("signal runner canceller", async move {
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
            concurrency: ws.concurrency,
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
        debug!("Visitor started");

        // Cancel visitor if we received any signal
        if let Some(subscriber) = self.signal_handler.subscribe() {
            tokio_spawn!("signal visitor canceller", async move {
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

            let task_name = task.name.clone();
            task_fut.push(tokio_spawn!("task", { task = task_name }, async move {
                // Skip the task if any dependency didn't finish successfully
                if !deps_ok {
                    info!("Task does not run as its dependency task failed");
                    app_tx.finish_task(TaskResult::BadDeps);
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

                let process = match Self::spawn_process(task.clone(), manager.clone()) {
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
                    let (cancel_probe_tx, cancel_probe_rx) = watch::channel(());
                    let log_rx = app_tx.subscribe_output();
                    let mut task_fut = tokio_spawn!(
                        "process",
                        { task = task.name },
                        Self::run_process(task.clone(), process, app_tx.clone())
                    );
                    let mut probe_fut = tokio_spawn!(
                        "probe",
                        { task = task.name },
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
                    let task_result = Self::run_process(task.clone(), process, app_tx.clone()).await?;
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

    fn spawn_process(task: Task, manager: ProcessManager) -> anyhow::Result<Option<Child>> {
        let mut args = Vec::new();
        args.extend(task.shell_args.clone());
        args.push(task.command.clone());

        let cmd = Command::new(task.shell.clone())
            .with_args(args)
            .with_envs(task.env.clone())
            .with_current_dir(task.working_dir.clone())
            .with_label(&task.name)
            .to_owned();

        let process = match manager.spawn(cmd, Duration::from_millis(500)) {
            Some(Ok(child)) => child,
            Some(Err(e)) => anyhow::bail!("failed to spawn task {:?}: {:?}", task.name, e),
            _ => return Ok(None),
        };
        let pid = process.pid().unwrap_or(0);

        info!("Task has started. PID={}", pid);

        Ok(Some(process))
    }

    async fn run_process(task: Task, mut process: Child, app_tx: EventSender) -> anyhow::Result<Option<TaskResult>> {
        let pid = process.pid().unwrap_or(0);

        // Transfer stdin of the process to the app
        if let Some(stdin) = process.stdin() {
            app_tx.set_stdin(task.name.clone(), stdin);
        }

        // Wait until complete
        info!("Task is waiting for output. PID={}", pid);
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

        info!("Task has finished. PID={}", pid);

        // Notify the app the task ended
        app_tx.finish_task(result.clone());

        Ok(Some(result))
    }
}
