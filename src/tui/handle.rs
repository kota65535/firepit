use anyhow::Context;
use tokio::sync::{mpsc, oneshot};
use crate::ui::sender::{TaskSender, UISender};
use super::{
    app::FRAME_RATE,
    event::{CacheResult, OutputLogs, PaneSize},
    Error, Event, TaskResult,
};

/// Struct for sending app events to TUI rendering
#[derive(Debug, Clone)]
pub struct TuiSender {
    tx: mpsc::UnboundedSender<Event>,
}

/// Struct for receiving app events
pub struct TuiReceiver {
    rx: mpsc::UnboundedReceiver<Event>,
}

pub fn app_event_channel() -> (TuiSender, TuiReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    (TuiSender::new(tx), TuiReceiver::new(rx))
}


impl TuiSender {
    pub fn new(tx: mpsc::UnboundedSender<Event>) -> Self {
        let tick_sender = tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(FRAME_RATE);
            loop {
                interval.tick().await;
                if tick_sender.send(Event::Tick).is_err() {
                    break;
                }
            }
        });
        Self {
            tx,
        }
    }
}

impl TuiSender {
    pub fn start_task(&self, task: String, output_logs: OutputLogs) {
        self.tx.send(Event::StartTask { task, output_logs }).ok();
    }

    pub fn end_task(&self, task: String, result: TaskResult) {
        self.tx.send(Event::EndTask { task, result }).ok();
    }

    pub fn status(&self, task: String, status: String, result: CacheResult) {
        self.tx.send(Event::Status {
                task,
                status,
                result,
            })
            .ok();
    }

    pub fn set_stdin(&self, task: String, stdin: Box<dyn std::io::Write + Send>) {
        self.tx.send(Event::SetStdin { task, stdin }).ok();
    }

    /// Construct a sender configured for a specific task
    pub fn task(&self, task: String) -> TaskSender {
        TaskSender {
            name: task,
            handle: UISender::Tui(self.clone()),
            logs: Default::default(),
        }
    }

    /// Stop rendering TUI and restore terminal to default configuration
    pub async fn stop(&self) {
        let (callback_tx, callback_rx) = oneshot::channel();
        // Send stop event, if receiver has dropped ignore error as
        // it'll be a no-op.
        self.tx.send(Event::Stop(callback_tx)).ok();
        // Wait for callback to be sent or the channel closed.
        callback_rx.await.ok();
    }

    /// Update the list of tasks displayed in the TUI
    pub fn update_tasks(&self, tasks: Vec<String>) -> anyhow::Result<()> {
        Ok(self.tx
            .send(Event::UpdateTasks { tasks })
            .map_err(|err| Error::Mpsc(err.to_string()))?)
    }

    pub fn output(&self, task: String, output: Vec<u8>) -> anyhow::Result<()> {
        Ok(self.tx
            .send(Event::TaskOutput { task, output })
            .map_err(|err| Error::Mpsc(err.to_string()))?)
    }

    /// Restart the list of tasks displayed in the TUI
    pub fn restart_tasks(&self, tasks: Vec<String>) -> anyhow::Result<()> {
        Ok(self.tx
            .send(Event::RestartTasks { tasks })
            .map_err(|err| Error::Mpsc(err.to_string()))?)
    }

    /// Fetches the size of the terminal pane
    pub async fn pane_size(&self) -> Option<PaneSize> {
        let (callback_tx, callback_rx) = oneshot::channel();
        // Send query, if no receiver to handle the request return None
        self.tx.send(Event::PaneSizeQuery(callback_tx)).ok()?;
        // Wait for callback to be sent
        callback_rx.await.ok()
    }
}

impl TuiReceiver {
    pub fn new(rx: mpsc::UnboundedReceiver<Event>) -> Self {
        Self {
            rx
        }
    }

    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}
