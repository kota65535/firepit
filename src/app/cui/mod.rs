pub mod color;
pub mod lib;
pub mod line;
pub mod logs;
pub mod output;
pub mod prefixed;

pub mod input;

use crate::app::command::AppCommand;
use crate::app::command::{AppCommandChannel, TaskResult};
use crate::app::cui::color::ColorSelector;
use crate::app::cui::input::InputHandler;
use crate::app::cui::lib::{ColorConfig, BOLD_RED};
use crate::app::cui::line::LineWriter;
use crate::app::cui::output::{OutputClient, OutputClientBehavior, OutputSink};
use crate::app::cui::prefixed::PrefixedWriter;
use crate::app::signal::SignalHandler;
use crate::runner::command::RunnerCommandChannel;
use crate::tokio_spawn;
use anyhow::Context;
use std::collections::{HashMap, HashSet};
use std::io::{stderr, stdout, Stdout, Write};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

pub struct CuiApp {
    color_selector: ColorSelector,
    output_clients: Arc<RwLock<HashMap<String, OutputClient<PrefixedWriter<Stdout>>>>>,
    stdins: Arc<Mutex<HashMap<String, Box<dyn Write + Send>>>>,
    command_tx: AppCommandChannel,
    command_rx: mpsc::UnboundedReceiver<AppCommand>,
    crossterm_rx: mpsc::Receiver<crossterm::event::Event>,
    input_handler: InputHandler,
    signal_handler: SignalHandler,
    target_tasks: Vec<String>,
    labels: HashMap<String, String>,
    quit_on_done: bool,
}

impl CuiApp {
    pub fn new(
        target_tasks: &Vec<String>,
        labels: &HashMap<String, String>,
        quit_on_done: bool,
    ) -> anyhow::Result<Self> {
        let (command_tx, command_rx) = AppCommandChannel::new();
        let input_handler = InputHandler::new();
        let crossterm_rx = input_handler.start();

        Ok(Self {
            color_selector: ColorSelector::default(),
            output_clients: Arc::new(RwLock::new(HashMap::new())),
            stdins: Arc::new(Mutex::new(HashMap::new())),
            command_tx,
            command_rx,
            crossterm_rx,
            input_handler,
            signal_handler: SignalHandler::infer()?,
            target_tasks: target_tasks.clone(),
            labels: labels.clone(),
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
        let mut failure = false;
        let mut task_remaining: HashSet<String> = self.target_tasks.iter().cloned().collect();
        let mut finisher = LineWriter::new(stderr());

        while let Some(event) = self.poll().await? {
            match event {
                AppCommand::Input { bytes } => {
                    for (task, stdin) in self.stdins.lock().expect("lock poisoned").iter_mut() {
                        stdin
                            .write_all(&bytes)
                            .context(format!("failed writing to stdin of task {}", task))?;
                    }
                }
                AppCommand::SetStdin { task, stdin } => {
                    self.stdins
                        .lock()
                        .expect("lock poisoned")
                        .insert(task.to_string(), stdin);
                }
                AppCommand::StartTask { task, .. } => self.register_output_client(&task),
                AppCommand::TaskOutput { task, output } => {
                    let output_clients = self.output_clients.read().expect("lock poisoned");
                    let output_client = output_clients.get(&task).context("output client not found")?;
                    output_client
                        .stdout()
                        .write_all(output.as_slice())
                        .context("failed writing to stdout")?;
                }
                AppCommand::FinishTask { task, result } => {
                    debug!("Task {:?} finished", task);

                    let message = match result {
                        TaskResult::Failure(code) => Some(format!("Process finished with exit code {code}")),
                        TaskResult::Stopped => Some("Process is terminated".to_string()),
                        TaskResult::NotReady => Some("Task is not ready".to_string()),
                        _ => None,
                    };
                    failure |= result.is_failure();
                    if let Some(message) = message {
                        let line = BOLD_RED.apply_to(format!("{}: {}\n", task, message));
                        finisher
                            .write_all(line.to_string().as_bytes())
                            .context("failed writing to stderr")?;
                    }
                    task_remaining.remove(&task);
                    debug!("Target tasks remaining: {:?}", task_remaining);
                }
                AppCommand::Quit => break,
                AppCommand::Done if self.quit_on_done => break,
                _ => {}
            }
            if self.quit_on_done && task_remaining.is_empty() {
                debug!("Target tasks all done");
                break;
            }
        }
        let exit_code = if failure { 1 } else { 0 };
        Ok(exit_code)
    }

    /// Blocking poll for events, will only return None if app handle has been
    /// dropped
    async fn poll<'a>(&mut self) -> anyhow::Result<Option<AppCommand>> {
        let input_closed = self.crossterm_rx.is_closed();

        if input_closed {
            Ok(self.command_rx.recv().await)
        } else {
            let mut event = None;
            loop {
                tokio::select! {
                    e = self.crossterm_rx.recv() => {
                        if let Some(e) = e {
                            event = self.input_handler.handle(e);
                        }
                    }
                    e = self.command_rx.recv() => {
                        event = e;
                    }
                }
                if event.is_some() {
                    break;
                }
            }
            Ok(event)
        }
    }
}
