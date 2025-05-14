use crate::app::tui::FRAME_RATE;
use crate::tokio_spawn;
use std::io;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tracing::warn;

#[derive(strum::AsRefStr)]
pub enum AppCommand {
    ///
    /// Task Commands
    ///
    PlanTask {
        task: String,
    },
    StartTask {
        task: String,
        pid: u32,
        restart: u64,
        max_restart: Option<u64>,
        reload: u64,
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
    Done,

    ///
    /// UI Commands
    ///
    Up,
    Down,
    ScrollUp(ScrollSize),
    ScrollDown(ScrollSize),
    ToggleSidebar,
    Stop,
    Tick,

    ///
    /// Interaction Commands
    ///
    EnterInteractive,
    ExitInteractive,
    Input {
        bytes: Vec<u8>,
    },

    ///
    /// Mouse Commands
    ///
    Mouse(crossterm::event::MouseEvent),
    MouseMultiClick(crossterm::event::MouseEvent, usize),
    CopySelection,
    Resize {
        rows: u16,
        cols: u16,
    },

    ///
    /// Search Commands
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

#[derive(Debug, Clone)]
pub struct AppCommandChannel {
    pub tx: mpsc::UnboundedSender<AppCommand>,
    name: String,
    logs: Arc<Mutex<Vec<u8>>>,
    log_subscribers: Vec<mpsc::UnboundedSender<Vec<u8>>>,
}

impl AppCommandChannel {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<AppCommand>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let tick_sender = tx.clone();
        tokio_spawn!("tick", async move {
            let mut interval = tokio::time::interval(FRAME_RATE);
            loop {
                interval.tick().await;
                if tick_sender.send(AppCommand::Tick).is_err() {
                    break;
                }
            }
        });
        (
            Self {
                tx,
                name: "".to_string(),
                logs: Default::default(),
                log_subscribers: Default::default(),
            },
            rx,
        )
    }

    pub fn with_name(&mut self, name: &str) -> Self {
        self.name = name.to_string();
        self.to_owned()
    }

    pub fn plan_task(&self, task: &str) {
        self.send(AppCommand::PlanTask { task: task.to_string() })
    }

    pub fn start_task(&self, task: String, pid: u32, restart: u64, max_restart: Option<u64>, reload: u64) {
        self.send(AppCommand::StartTask {
            task,
            pid,
            restart,
            max_restart,
            reload,
        })
    }

    pub fn ready_task(&self) {
        self.send(AppCommand::ReadyTask {
            task: self.name.to_string(),
        })
    }

    pub fn finish_task(&self, result: TaskResult) {
        self.send(AppCommand::FinishTask {
            task: self.name.clone(),
            result,
        })
    }

    pub fn output(&self, task: String, output: Vec<u8>) {
        self.send(AppCommand::TaskOutput { task, output })
    }

    pub fn set_stdin(&self, task: String, stdin: Box<dyn Write + Send>) {
        self.send(AppCommand::SetStdin { task, stdin })
    }

    pub async fn done(&self) {
        self.send(AppCommand::Done);
    }

    pub async fn stop(&self) {
        self.send(AppCommand::Stop);
    }

    pub async fn pane_size(&self) -> Option<PaneSize> {
        let (callback_tx, callback_rx) = oneshot::channel();
        self.send(AppCommand::PaneSizeQuery(callback_tx));
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

    pub fn send(&self, event: AppCommand) {
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

impl Write for AppCommandChannel {
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
    Running(TaskRun),
    Ready,
    Finished(TaskResult),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TaskRun {
    pub pid: u32,
    pub restart: u64,
    pub max_restart: Option<u64>,
    pub reload: u64,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum TaskResult {
    /// Finished successfully
    Success,

    /// Finished with non-zero exit code
    Failure(i32),

    /// Killed by signal
    Stopped,

    /// Not run because dependency task failed
    BadDeps,

    /// Not run because it is up-to-date
    UpToDate,

    /// Stopped because it does not become ready
    NotReady,

    /// Task is restarting due to the change of input
    Reloading,

    /// Unknown status
    Unknown,
}

impl TaskResult {
    pub fn is_success(&self) -> bool {
        matches!(self, TaskResult::Success | TaskResult::UpToDate)
    }

    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            TaskResult::Failure(_)
                | TaskResult::Stopped
                | TaskResult::BadDeps
                | TaskResult::NotReady
                | TaskResult::Unknown
        )
    }
}
