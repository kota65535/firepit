use std::fmt::{Display, Formatter};
use crate::tui::app::FRAME_RATE;
use std::io;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

pub enum Event {
    StartTask {
        task: String,
    },
    TaskOutput {
        task: String,
        output: Vec<u8>,
    },
    ReadyTask {
        task: String,
    },
    EndTask {
        task: String,
        result: TaskResult,
    },
    PaneSizeQuery(oneshot::Sender<PaneSize>),
    Stop(oneshot::Sender<()>),
    // Stop initiated by the TUI itself
    InternalStop,
    Tick,
    Up,
    Down,
    ScrollUp,
    ScrollDown,
    SetStdin {
        task: String,
        stdin: Box<dyn std::io::Write + Send>,
    },
    EnterInteractive,
    ExitInteractive,
    Input {
        bytes: Vec<u8>,
    },
    Resize {
        rows: u16,
        cols: u16,
    },
    ToggleSidebar,
}

pub enum Direction {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneSize {
    pub rows: u16,
    pub cols: u16,
}

pub struct EventReceiver {
    rx: mpsc::UnboundedReceiver<Event>,
}

impl EventReceiver {
    pub fn new(rx: mpsc::UnboundedReceiver<Event>) -> Self {
        Self { rx }
    }

    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

#[derive(Debug, Clone)]
pub struct EventSender {
    tx: mpsc::UnboundedSender<Event>,
    name: String,
    logs: Arc<Mutex<Vec<u8>>>,
}

impl EventSender {
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
            name: "".to_string(),
            logs: Default::default(),
        }
    }

    pub fn with_name(&mut self, name: &str) -> Self {
        self.name = name.to_string();
        self.to_owned()
    }

    pub fn start_task(&self, task: String) {
        self.tx.send(Event::StartTask { task }).ok();
    }

    pub fn ready_task(&self, task: String) {
        self.tx.send(Event::ReadyTask { task }).ok();
    }

    pub fn end_task(&self, task: String, result: TaskResult) {
        self.tx.send(Event::EndTask { task, result }).ok();
    }

    pub fn set_stdin(&self, task: String, stdin: Box<dyn std::io::Write + Send>) {
        self.tx.send(Event::SetStdin { task, stdin }).ok();
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

    pub fn output(&self, task: String, output: Vec<u8>) -> anyhow::Result<()> {
        self.tx
            .send(Event::TaskOutput { task, output })
            .map_err(|err| anyhow::anyhow!(err.to_string()))
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

impl Write for EventSender {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let task = self.name.clone();
        {
            self.logs
                .lock()
                .expect("should not poisoned")
                .extend_from_slice(buf);
        }

        self.output(task, buf.to_vec())
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum TaskResult {
    /// Run successfully.
    Success,
    /// Exited with non-zero code.
    Failure(i32),
    /// Skipped due to the failure of the deps.
    Skipped,
    /// Killed by someone else.
    Stopped,
    /// The other reason.
    Unknown,
}

impl Display for TaskResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskResult::Success => write!(f, "Success"),
            TaskResult::Skipped => write!(f, "Skipped"),
            TaskResult::Stopped => write!(f, "Stopped"),
            TaskResult::Failure(code) => write!(f, "Failure with code {code}"),
            TaskResult::Unknown => write!(f, "Unknown"),
        }
    }
}
