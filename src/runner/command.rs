use crate::app::FRAME_RATE;
use crate::tokio_spawn;
use std::io;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::warn;

#[derive(Debug, Clone, strum::AsRefStr)]
pub enum RunnerCommand {
    StopTask { task: String },
    RestartTask { task: String },
    Quit,
}

#[derive(Debug, Clone)]
pub struct RunnerCommandChannel {
    tx: broadcast::Sender<RunnerCommand>,
}

impl RunnerCommandChannel {
    pub fn new() -> (Self, broadcast::Receiver<RunnerCommand>) {
        let (tx, rx) = broadcast::channel(10);
        (Self { tx }, rx)
    }

    pub fn stop_task(&self, task: &str) {
        self.send(RunnerCommand::StopTask { task: task.to_string() })
    }

    pub fn restart_task(&self, task: &str) {
        self.send(RunnerCommand::RestartTask { task: task.to_string() })
    }

    pub fn quit(&self) {
        self.send(RunnerCommand::Quit);
    }

    fn send(&self, event: RunnerCommand) {
        match self.tx.send(event) {
            Err(e) => {
                warn!("Failed to send {} event: {:?}", e.0.as_ref(), e);
            }
            Ok(_) => {}
        }
    }
}
