pub mod color;
pub mod lib;
pub mod line;
pub mod output;
pub mod prefixed;

use crate::app::command::AppCommand;
use crate::app::command::AppCommandChannel;
use crate::app::cui::color::ColorSelector;
use crate::app::cui::lib::{ColorConfig, BOLD_RED, RED};
use crate::app::cui::output::{OutputClient, OutputClientBehavior, OutputSink};
use crate::app::cui::prefixed::PrefixedWriter;
use crate::app::signal::SignalHandler;
use crate::runner::command::RunnerCommandChannel;
use crate::tokio_spawn;
use anyhow::Context;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::io::{stdout, Stdout, Write};
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

pub struct CuiApp {
    color_selector: ColorSelector,
    output_clients: Arc<RwLock<HashMap<String, OutputClient<PrefixedWriter<Stdout>>>>>,
    command_tx: AppCommandChannel,
    command_rx: mpsc::UnboundedReceiver<AppCommand>,
    signal_handler: SignalHandler,
    target_tasks: Vec<String>,
    /// Finalizers run while quitting, so their failures count unlike the tasks stopped by it
    finalizer_tasks: HashSet<String>,
    labels: HashMap<String, String>,
    quit_on_done: bool,
    fail_fast: bool,
    no_log_prefix: bool,
}

impl CuiApp {
    pub fn new(
        target_tasks: &[String],
        finalizer_tasks: &[String],
        labels: &HashMap<String, String>,
        quit_on_done: bool,
        fail_fast: bool,
        no_log_prefix: bool,
    ) -> anyhow::Result<Self> {
        let (command_tx, command_rx) = AppCommandChannel::new();
        Ok(Self {
            color_selector: ColorSelector::default(),
            output_clients: Arc::new(RwLock::new(HashMap::new())),
            command_tx,
            command_rx,
            signal_handler: SignalHandler::infer()?,
            target_tasks: target_tasks.to_vec(),
            finalizer_tasks: finalizer_tasks.iter().cloned().collect(),
            labels: labels.clone(),
            fail_fast,
            quit_on_done,
            no_log_prefix,
        })
    }

    fn register_output_client(&mut self, task: &str) {
        let task = task.to_string();
        let prefix = if self.no_log_prefix {
            ""
        } else {
            self.labels.get(&task).unwrap_or(&task)
        };
        let out = PrefixedWriter::new(
            ColorConfig::infer(),
            self.color_selector.string_with_color(prefix, prefix),
            stdout(),
        );
        let err = PrefixedWriter::new(
            ColorConfig::infer(),
            self.color_selector.string_with_color(prefix, prefix),
            stdout(),
        );
        let output_client = OutputSink::new(out, err).logger(OutputClientBehavior::Passthrough);
        self.output_clients
            .write()
            .expect("lock poisoned")
            .insert(task, output_client);
    }

    pub fn command_tx(&self) -> AppCommandChannel {
        self.command_tx.clone()
    }

    pub async fn run(&mut self, runner_tx: &RunnerCommandChannel) -> anyhow::Result<i32> {
        // Translate every signal into a quit command. The app forwards each one
        // to the runner, which turns a repeated quit into a forced kill.
        let mut signals = self.signal_handler.subscribe();
        let command_tx = self.command_tx.clone();
        tokio_spawn!("app-canceller", async move {
            // A lagged receiver still means signals arrived, so treat it the same.
            while let Ok(_) | Err(RecvError::Lagged(_)) = signals.recv().await {
                command_tx.quit().await;
            }
        });

        let ret = self.run_inner(runner_tx).await;

        if let Err(err) = ret {
            error!("Error: {}", err);
            // `run_inner` has returned early without stopping the runner.
            runner_tx.quit();
            return Err(err);
        }

        info!("App is exiting");
        ret
    }

    pub async fn run_inner(&mut self, runner_tx: &RunnerCommandChannel) -> anyhow::Result<i32> {
        let mut tasks_remaining = self.target_tasks.iter().cloned().collect::<HashSet<_>>();
        let mut failed_tasks = IndexMap::new();
        let mut quitting = false;
        while let Some(event) = self.command_rx.recv().await {
            match event {
                AppCommand::StartTask { task, .. } => self.register_output_client(&task),
                AppCommand::TaskOutput { task, output } => {
                    let output_clients = self.output_clients.read().expect("lock poisoned");
                    let output_client = output_clients.get(&task).context("output client not found")?;
                    output_client
                        .stdout()
                        .write_all(output.as_slice())
                        .context("failed to write to stdout")?;
                }
                AppCommand::FinishTask {
                    task,
                    result,
                    datetime: _,
                } => {
                    debug!("Task {:?} finished", task);

                    // Tasks stopped by the quit are not failures, but the finalizers run through it
                    if result.is_failure() && (!quitting || self.finalizer_tasks.contains(&task)) {
                        failed_tasks.insert(task.clone(), result);
                        eprintln!(
                            "{}",
                            RED.apply_to(result.long_message(self.labels.get(&task).unwrap_or(&task)).to_string())
                        );
                    }
                    tasks_remaining.remove(&task);
                    debug!("Target tasks remaining: {:?}", tasks_remaining);
                }
                AppCommand::Quit => {
                    // Keep processing output while the runner shuts down. The
                    // runner kills the tasks when it receives a second quit, and
                    // sends `Done` once it has finished either way.
                    quitting = true;
                    runner_tx.quit();
                }
                AppCommand::Done if self.quit_on_done || quitting => break,
                _ => {}
            }
            // Quit once the targets are done. The runner then runs the finalizers and sends `Done`
            if self.quit_on_done && !quitting && tasks_remaining.is_empty() {
                debug!("Target tasks all done");
                quitting = true;
                runner_tx.quit();
            }
        }

        // Stop the runner unless quitting, in which case it has been told already
        // and has finished by now.
        if !quitting {
            runner_tx.quit();
        }

        if !failed_tasks.is_empty() {
            if self.fail_fast {
                let (task, result) = failed_tasks.iter().next().unwrap();
                eprintln!();
                eprintln!(
                    "{}",
                    RED.apply_to(format!(
                        "FAILURE: {}",
                        result.long_message(self.labels.get(task).unwrap_or(task))
                    ))
                );
            } else {
                eprintln!();
                eprintln!(
                    "{}",
                    BOLD_RED.apply_to(format!("FAILURE: {} tasks failed", failed_tasks.len()))
                );
                let max_label_len = failed_tasks
                    .keys()
                    .map(|t| self.labels.get(t).unwrap_or(t).len())
                    .max()
                    .unwrap_or(0);
                for (t, r) in failed_tasks.iter() {
                    if r.is_failure() {
                        eprintln!(
                            "{}",
                            RED.apply_to(format!(
                                "* {:max_label_len$} : {}",
                                self.labels.get(t).unwrap_or(t),
                                r.short_message()
                            ))
                        );
                    }
                }
            }
        }

        let exit_code = if !failed_tasks.is_empty() { 1 } else { 0 };
        Ok(exit_code)
    }
}
