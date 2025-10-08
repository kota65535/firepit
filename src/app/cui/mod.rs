pub mod color;
pub mod lib;
pub mod line;
pub mod logs;
pub mod output;
pub mod prefixed;

use crate::app::command::AppCommand;
use crate::app::command::{AppCommandChannel, TaskResult};
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
use tokio::sync::mpsc;
use tracing::{debug, error, info};

pub struct CuiApp {
    color_selector: ColorSelector,
    output_clients: Arc<RwLock<HashMap<String, OutputClient<PrefixedWriter<Stdout>>>>>,
    command_tx: AppCommandChannel,
    command_rx: mpsc::UnboundedReceiver<AppCommand>,
    signal_handler: SignalHandler,
    target_tasks: Vec<String>,
    labels: HashMap<String, String>,
    quit_on_done: bool,
    fail_fast: bool,
}

impl CuiApp {
    pub fn new(
        target_tasks: &Vec<String>,
        labels: &HashMap<String, String>,
        quit_on_done: bool,
        fail_fast: bool,
    ) -> anyhow::Result<Self> {
        let (command_tx, command_rx) = AppCommandChannel::new();
        Ok(Self {
            color_selector: ColorSelector::default(),
            output_clients: Arc::new(RwLock::new(HashMap::new())),
            command_tx,
            command_rx,
            signal_handler: SignalHandler::infer()?,
            target_tasks: target_tasks.clone(),
            labels: labels.clone(),
            fail_fast,
            quit_on_done,
        })
    }

    fn register_output_client(&mut self, task: &str) {
        let task = task.to_string();
        let prefix = self.labels.get(&task).unwrap_or(&task);
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
        let signal_handler = self.signal_handler.clone();
        let command_tx = self.command_tx.clone();
        tokio_spawn!("app-canceller", async move {
            let subscriber = signal_handler.subscribe();
            if let Some(subscriber) = subscriber {
                let _guard = subscriber.listen().await;
                command_tx.quit().await;
            }
        });

        let ret = self.run_inner().await;
        runner_tx.quit();

        if let Err(err) = ret {
            error!("Error: {}", err);
            return Err(err);
        }

        info!("App is exiting");
        Ok(ret?)
    }

    pub async fn run_inner(&mut self) -> anyhow::Result<i32> {
        let mut tasks_remaining = self.target_tasks.iter().cloned().collect::<HashSet<_>>();
        let mut failed_tasks = IndexMap::new();
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
                AppCommand::FinishTask { task, result, datetime } => {
                    debug!("Task {:?} finished", task);

                    if result.is_failure() {
                        failed_tasks.insert(task.clone(), result);
                        eprintln!(
                            "{}",
                            RED.apply_to(format!(
                                "{}",
                                result.long_message(self.labels.get(&task).unwrap_or(&task))
                            ))
                        );
                    }
                    tasks_remaining.remove(&task);
                    debug!("Target tasks remaining: {:?}", tasks_remaining);
                }
                AppCommand::Quit => break,
                AppCommand::Done if self.quit_on_done => break,
                _ => {}
            }
            if self.quit_on_done && tasks_remaining.is_empty() {
                debug!("Target tasks all done");
                break;
            }
        }

        if failed_tasks.len() > 0 {
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

        let exit_code = if failed_tasks.len() > 0 { 1 } else { 0 };
        Ok(exit_code)
    }
}
