use crate::event::TaskResult;
use tokio::sync::oneshot;

pub enum Event {
    StartTask {
        task: String,
    },
    TaskOutput {
        task: String,
        output: Vec<u8>,
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
    Mouse(crossterm::event::MouseEvent),
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
