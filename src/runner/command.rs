use tokio::sync::broadcast;
use tracing::warn;

#[derive(Debug, Clone, strum::AsRefStr)]
pub enum RunnerCommand {
    StopTask { task: String },
    RestartTask { task: String, force: bool },
    Quit,
}

#[derive(Debug, Clone)]
pub struct RunnerCommandChannel {
    tx: broadcast::Sender<RunnerCommand>,
}

impl RunnerCommandChannel {
    pub fn new(size: usize) -> (Self, broadcast::Receiver<RunnerCommand>) {
        let (tx, rx) = broadcast::channel(size);
        (Self { tx }, rx)
    }

    pub fn stop_task(&self, task: &str) {
        self.send(RunnerCommand::StopTask { task: task.to_string() })
    }

    pub fn restart_task(&self, task: &str, force: bool) {
        self.send(RunnerCommand::RestartTask {
            task: task.to_string(),
            force,
        })
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
