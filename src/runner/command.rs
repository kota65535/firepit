use crate::app::FRAME_RATE;
use crate::tokio_spawn;
use std::io;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::warn;

#[derive(Debug, Clone, strum::AsRefStr)]
pub enum RunnerCommand {
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
