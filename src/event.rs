use crate::tui::app::FRAME_RATE;
use log::{debug, info};
use std::fmt::{Display, Formatter};
use std::io;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

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
    SearchExit {
        restore_scroll: bool,
    },
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

    pub fn start_task(&self, task: String, pid: u32, restart: u64) {
        if let Err(e) = self.tx.send(Event::StartTask { task, pid, restart }) {
            debug!("failed to send StartTask event: {:?}", e)
        }
    }

    pub fn ready_task(&self, task: String) {
        if let Err(e) = self.tx.send(Event::ReadyTask { task }) {
            debug!("failed to send ReadyTask event: {:?}", e)
        }
    }

    pub fn finish_task(&self, task: String, result: TaskResult) {
        if let Err(e) = self.tx.send(Event::FinishTask { task, result }) {
            debug!("failed to send FinishTask event: {:?}", e)
        }
    }

    pub fn set_stdin(&self, task: String, stdin: Box<dyn Write + Send>) {
        if let Err(e) = self.tx.send(Event::SetStdin { task, stdin }) {
            debug!("failed to send SetStdin event: {:?}", e)
        }
    }

    /// Stop rendering TUI and restore terminal to default configuration
    pub async fn stop(&self) {
        let (callback_tx, callback_rx) = oneshot::channel();
        // Send stop event, if receiver has dropped ignore error as
        // it'll be a no-op.
        if let Err(e) = self.tx.send(Event::Stop(callback_tx)) {
            debug!("failed to send Stop event: {:?}", e)
        }
        // Wait for callback to be sent or the channel closed.
        if let Err(e) = callback_rx.await {
            debug!("failed to receive callback of Stop event: {:?}", e)
        }
    }

    pub fn output(&self, task: String, output: Vec<u8>) {
        if let Err(e) = self.tx.send(Event::TaskOutput { task, output }) {
            debug!("failed to send Output event: {:?}", e)
        }
    }

    /// Fetches the size of the terminal pane
    pub async fn pane_size(&self) -> Option<PaneSize> {
        let (callback_tx, callback_rx) = oneshot::channel();
        if let Err(e) = self.tx.send(Event::PaneSizeQuery(callback_tx)) {
            debug!("failed to send PaneSizeQuery event: {:?}", e)
        }
        // Wait for callback to be sent or the channel closed.
        match callback_rx.await {
            Ok(size) => Some(size),
            Err(e) => {
                debug!("failed to receive callback of PaneSizeQuery event: {:?}", e);
                None
            }
        }
    }
}

impl Write for EventSender {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let task = self.name.clone();
        {
            self.logs.lock().expect("should not poisoned").extend_from_slice(buf);
        }

        self.output(task, buf.to_vec());
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
                TaskResult::BadDeps => write!(f, "Dependency task failed"),
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
            TaskResult::BadDeps => write!(f, "Dependency task failed"),
            TaskResult::Stopped => write!(f, "Stopped"),
            TaskResult::NotReady => write!(f, "Service not ready in timeout"),
            TaskResult::Failure(code) => write!(f, "Failure with code {code}"),
            TaskResult::Unknown => write!(f, "Unknown"),
        }
    }
}
