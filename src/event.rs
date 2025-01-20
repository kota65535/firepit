use crate::tui::event::PaneSize;
use tokio::sync::{mpsc, oneshot};

pub enum TaskEvent {
    PaneSizeQuery(oneshot::Sender<PaneSize>),
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum TaskResult {
    Success,
    Skipped,
    Stopped,
    Failure,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct TaskEventSender {
    tx: mpsc::UnboundedSender<TaskEvent>,
}

impl TaskEventSender {
    pub fn new(tx: mpsc::UnboundedSender<TaskEvent>) -> Self {
        Self {
            tx,
        }
    }

    pub async fn pane_size(&self) -> anyhow::Result<Option<PaneSize>> {
        let (callback_tx, callback_rx) = oneshot::channel();
        self.tx.send(TaskEvent::PaneSizeQuery(callback_tx))
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        Ok(callback_rx.await.ok())
    }

    fn send(&self, event: TaskEvent) -> anyhow::Result<()> {
        self.tx.send(event).map_err(|e| anyhow::anyhow!("Failed to send event: {:?}", e))
    }
}

#[derive(Debug)]
pub struct TaskEventReceiver {
    rx: mpsc::UnboundedReceiver<TaskEvent>,
}

impl TaskEventReceiver {
    pub fn new(rx: mpsc::UnboundedReceiver<TaskEvent>) -> Self {
        Self {
            rx
        }
    }

    pub async fn recv(&mut self) -> Option<TaskEvent> {
        self.rx.recv().await
    }
}
