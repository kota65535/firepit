use crate::cui::color::ColorSelector;
use crate::cui::lib::ColorConfig;
use crate::cui::output::{OutputClient, OutputClientBehavior, OutputSink};
use crate::cui::prefixed::PrefixedWriter;
use crate::event::{Event, EventReceiver};
use crate::event::{EventSender, TaskResult};
use crate::signal::SignalHandler;
use crate::tokio_spawn;
use anyhow::Context;
use std::collections::{HashMap, HashSet};
use std::io::{stdout, Stdout, Write};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tracing::debug;

pub struct CuiApp {
    color_selector: ColorSelector,
    output_clients: Arc<RwLock<HashMap<String, OutputClient<PrefixedWriter<Stdout>>>>>,
    sender: EventSender,
    receiver: EventReceiver,
    signal_handler: SignalHandler,
    target_tasks: Vec<String>,
    labels: HashMap<String, String>,
    no_quit: bool,
}

impl CuiApp {
    pub fn new(target_tasks: &Vec<String>, labels: &HashMap<String, String>, no_quit: bool) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Ok(Self {
            color_selector: ColorSelector::default(),
            output_clients: Arc::new(RwLock::new(HashMap::new())),
            sender: EventSender::new(tx),
            receiver: EventReceiver::new(rx),
            signal_handler: SignalHandler::infer()?,
            target_tasks: target_tasks.clone(),
            labels: labels.clone(),
            no_quit,
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

    pub async fn run(&mut self) -> anyhow::Result<i32> {
        let signal_handler = self.signal_handler.clone();
        let sender = self.sender.clone();
        tokio_spawn!("cui-canceller", async move {
            let subscriber = signal_handler.subscribe();
            if let Some(subscriber) = subscriber {
                let _guard = subscriber.listen().await;
                sender.stop().await;
            }
        });
        let mut failure = false;
        let mut task_remaining: HashSet<String> = self.target_tasks.iter().cloned().collect();
        while let Some(event) = self.receiver.recv().await {
            match event {
                Event::StartTask { task, .. } => self.register_output_client(&task),
                Event::TaskOutput { task, output } => {
                    let output_clients = self.output_clients.read().expect("lock poisoned");
                    let output_client = output_clients.get(&task).context("output client not found")?;
                    output_client
                        .stdout()
                        .write_all(output.as_slice())
                        .context("failed to write to stdout")?;
                }
                Event::FinishTask { task, result } => {
                    debug!("Task {:?} finished", task);
                    let message = match result {
                        TaskResult::Failure(code) => Some(format!("Process finished with exit code {code}")),
                        TaskResult::Stopped => Some("Process terminated".to_string()),
                        _ => None,
                    };
                    failure |= result.is_failure();
                    if let Some(message) = message {
                        eprintln!("{}", message);
                    }
                    task_remaining.remove(&task);
                    debug!("Target tasks remaining: {:?}", task_remaining);
                }
                Event::Stop => break,
                Event::Done if !self.no_quit => break,
                _ => {}
            }
            if !self.no_quit && task_remaining.is_empty() {
                debug!("Target tasks all done");
                break;
            }
        }
        let exit_code = if failure { 1 } else { 0 };
        Ok(exit_code)
    }

    pub fn sender(&self) -> EventSender {
        self.sender.clone()
    }
}
