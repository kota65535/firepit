use crate::tokio_spawn;
use crate::tui::app::FRAME_RATE;
use std::fmt::{Display, Formatter};
use std::io;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tracing::warn;

#[derive(strum::AsRefStr)]

pub enum Event {
    ///
    /// Task Events
    ///
    StartTask {
        task: String,
        pid: u32,
        restart: u64,
    },
    TaskOutput {
        task: String,
        output: Vec<u8>,
    },
    ReadyTask {
        task: String,
    },
    FinishTask {
        task: String,
        result: TaskResult,
    },
    SetStdin {
        task: String,
        stdin: Box<dyn Write + Send>,
    },
    PaneSizeQuery(oneshot::Sender<PaneSize>),
    Stop(oneshot::Sender<()>),
    // Stop initiated by the TUI itself
    InternalStop,

    ///
    /// UI Events
    ///
    Up,
    Down,
    ScrollUp(ScrollSize),
    ScrollDown(ScrollSize),
    ToggleSidebar,
    Tick,

    ///
    /// Interaction Events
    ///
    EnterInteractive,
    ExitInteractive,
    Input {
        bytes: Vec<u8>,
    },

    ///
    /// Mouse Events
    ///
    Mouse(crossterm::event::MouseEvent),
    MouseMultiClick(crossterm::event::MouseEvent, usize),
    CopySelection,
    Resize {
        rows: u16,
        cols: u16,
    },

    ///
    /// Search Events
    ///
    EnterSearch,
    SearchInputChar(char),
    SearchBackspace,
    SearchRun,
    SearchNext,
    SearchPrevious,
    ExitSearch,
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy)]
pub enum ScrollSize {
    One,
    Half,
    Full,
    Edge,
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
    pub tx: mpsc::UnboundedSender<Event>,
    name: String,
    logs: Arc<Mutex<Vec<u8>>>,
    log_subscribers: Vec<mpsc::UnboundedSender<Vec<u8>>>,
}

impl EventSender {
    pub fn new(tx: mpsc::UnboundedSender<Event>) -> Self {
        let tick_sender = tx.clone();
        tokio_spawn!("tick", async move {
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
            log_subscribers: Default::default(),
        }
    }

    pub fn with_name(&mut self, name: &str) -> Self {
        self.name = name.to_string();
        self.to_owned()
    }

    pub fn start_task(&self, task: String, pid: u32, restart: u64) {
        self.send(Event::StartTask { task, pid, restart })
    }

    pub fn ready_task(&self) {
        self.send(Event::ReadyTask {
            task: self.name.to_string(),
        })
    }

    pub fn finish_task(&self, result: TaskResult) {
        self.send(Event::FinishTask {
            task: self.name.clone(),
            result,
        })
    }

    pub fn output(&self, task: String, output: Vec<u8>) {
        self.send(Event::TaskOutput { task, output })
    }

    pub fn set_stdin(&self, task: String, stdin: Box<dyn Write + Send>) {
        self.send(Event::SetStdin { task, stdin })
    }

    pub async fn stop(&self) {
        let (callback_tx, callback_rx) = oneshot::channel();
        self.send(Event::Stop(callback_tx));
        if let Err(e) = callback_rx.await {
            warn!("Failed to receive callback of Stop event: {:?}", e)
        }
    }

    pub async fn pane_size(&self) -> Option<PaneSize> {
        let (callback_tx, callback_rx) = oneshot::channel();
        self.send(Event::PaneSizeQuery(callback_tx));
        match callback_rx.await {
            Ok(size) => Some(size),
            Err(e) => {
                warn!("Failed to receive callback of PaneSizeQuery event: {:?}", e);
                None
            }
        }
    }

    pub fn subscribe_output(&mut self) -> mpsc::UnboundedReceiver<Vec<u8>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.log_subscribers.push(tx);
        rx
    }

    pub fn send(&self, event: Event) {
        match self.tx.send(event) {
            Err(e) => {
                warn!("Task {:?} failed to send {} event: {:?}", self.name, e.0.as_ref(), e);
            }
            Ok(_) => {}
        }
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

impl Write for EventSender {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let task = self.name.clone();
        {
            self.logs.lock().expect("should not poisoned").extend_from_slice(buf);
        }

        self.output(task, buf.to_vec());
        for tx in self.log_subscribers.iter() {
            tx.send(buf.to_vec()).ok();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaskStatus {
    Planned,
    Running(TaskRunning),
    Ready,
    Finished(TaskResult),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TaskRunning {
    pub pid: u32,
    pub restart: u64,
}

impl Display for TaskStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Planned => write!(f, "Waiting"),
            TaskStatus::Running(r) => write!(f, "Running (PID: {}, Restart: {})", r.pid, r.restart),
            TaskStatus::Ready => write!(f, "Ready"),
            TaskStatus::Finished(result) => match result {
                TaskResult::Success => write!(f, "Finished"),
                TaskResult::Failure(code) => write!(f, "Exited with code {code}"),
                TaskResult::BadDeps => write!(f, "Dependencies failed"),
                TaskResult::NotReady => write!(f, "Service not ready"),
                TaskResult::Stopped => write!(f, "Killed"),
                TaskResult::Unknown => write!(f, "Unknown"),
            },
            TaskStatus::Unknown => write!(f, "Unknown"),
        }
    }
}
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum TaskResult {
    /// Run successfully.
    Success,

    /// Exited with non-zero code.
    Failure(i32),

    /// Killed by signal.
    Stopped,

    /// Dependencies not satisfied
    BadDeps,

    /// Service not ready
    NotReady,

    /// The other reason.
    Unknown,
}

impl Display for TaskResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskResult::Success => write!(f, "Success"),
            TaskResult::Failure(code) => write!(f, "Failure with code {code}"),
            TaskResult::BadDeps => write!(f, "Dependencies failed"),
            TaskResult::Stopped => write!(f, "Stopped"),
            TaskResult::NotReady => write!(f, "Service not ready"),
            TaskResult::Unknown => write!(f, "Unknown"),
        }
    }
}
