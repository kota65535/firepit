pub mod color;
pub mod lib;
pub mod line;
pub mod output;
pub mod prefixed;

use crate::app::command::AppCommand;
use crate::app::command::AppCommandChannel;
use crate::app::cui::color::ColorSelector;
use crate::app::cui::lib::{ColorConfig, GREY, RED, YELLOW};
use crate::app::cui::output::{OutputClient, OutputClientBehavior, OutputSink};
use crate::app::cui::prefixed::PrefixedWriter;
use crate::app::print_failure_summary;
use crate::app::signal::SignalHandler;
use crate::log::LogRecord;
use crate::runner::command::RunnerCommandChannel;
use crate::tokio_spawn;
use anyhow::Context;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::io::{stdout, Stdout, Write};
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tracing::{debug, error, Level};

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
    no_log_prefix: bool,
}

impl CuiApp {
    pub fn new(
        target_tasks: &[String],
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
            labels: labels.clone(),
            quit_on_done,
            fail_fast,
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

    fn print_log(&mut self, record: &LogRecord) {
        let style = match record.level {
            Level::ERROR => RED.clone(),
            Level::WARN => YELLOW.clone(),
            _ => GREY.clone(),
        };
        let prefix = match &record.task {
            Some(task) if !self.no_log_prefix => {
                let label = self.labels.get(task).unwrap_or(task);
                self.color_selector.string_with_color(label, label).to_string()
            }
            _ => String::new(),
        };
        for line in record.lines() {
            eprintln!("{}{}", prefix, style.apply_to(line));
        }
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
            error!("CUI failed: {}", err);
            // `run_inner` has returned early without stopping the runner.
            runner_tx.quit();
            return Err(err);
        }

        debug!("App is exiting");
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
                AppCommand::Log(record) => self.print_log(&record),
                AppCommand::FinishTask {
                    task,
                    result,
                    datetime: _,
                } => {
                    debug!("Task {:?} finished", task);

                    if result.is_failure() {
                        eprintln!(
                            "{}",
                            RED.apply_to(result.long_message(self.labels.get(&task).unwrap_or(&task)).to_string())
                        );
                        failed_tasks.insert(task.clone(), result);
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

        let failed = failed_tasks
            .iter()
            .map(|(t, r)| (self.labels.get(t).unwrap_or(t).clone(), r.clone()))
            .collect::<Vec<_>>();
        print_failure_summary(&failed, self.fail_fast);

        let exit_code = if !failed_tasks.is_empty() { 1 } else { 0 };
        Ok(exit_code)
    }
}
