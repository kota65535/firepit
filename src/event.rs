use std::io;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub enum TaskEvent {
    Plan {
        targets: Vec<String>,
        deps: Vec<String>,
    },
    Start {
        task: String
    },
    Skip {
        task: String
    },
    Output {
        task: String,
        output: Vec<u8>,
    },
    SetStdin {
        task: String,
        stdin: Box<dyn Write + Send>,
    },
    Finish {
        task: String,
        result: TaskResult
    },
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
    name: String,
    tx: mpsc::UnboundedSender<TaskEvent>,
    logs: Arc<Mutex<Vec<u8>>>,
}

impl TaskEventSender {
    pub fn new(tx: mpsc::UnboundedSender<TaskEvent>) -> Self {
        Self {
            name: "".to_string(),
            tx,
            logs: Default::default(),
        }
    }

    pub fn with_name(&self, name: &str) -> Self {
        let mut this = self.clone();
        this.name = name.to_string();
        this
    }

    pub fn plan(&self, targets: Vec<String>, deps: Vec<String>) -> anyhow::Result<()> {
        self.send(TaskEvent::Plan { targets, deps })
    }

    pub fn start(&self, task: &str) -> anyhow::Result<()> {
        self.send(TaskEvent::Start { task: task.to_string() })
    }

    pub fn finish(&self, task: &str, result: TaskResult) -> anyhow::Result<()> {
        self.send(TaskEvent::Finish { task: task.to_string(), result })
    }

    pub fn set_stdin(&self, task: &str,  stdin: Box<dyn Write + Send>) -> anyhow::Result<()> {
        self.send(TaskEvent::SetStdin { task: task.to_string(), stdin })
    }

    fn send(&self, event: TaskEvent) -> anyhow::Result<()> {
        self.tx.send(event).map_err(|e| anyhow::anyhow!("Failed to send event: {:?}", e))
    }
}

impl Write for TaskEventSender {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let task = self.name.clone();
        {
            self.logs
                .lock()
                .expect("should not poisoned")
                .extend_from_slice(buf);
        }

        self.send(TaskEvent::Output {
            task,
            output: buf.to_vec(),
        }).map_err(|_| io::Error::new(io::ErrorKind::Other, "receiver dropped"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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
