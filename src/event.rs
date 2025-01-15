use crate::process::ChildExit;
use anyhow::Context;
use std::io;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum TaskEvent {
    Start {
        task: String
    },
    Output {
        task: String,
        output: Vec<u8>,
    },
    Finish {
        task: String,
        reason: ChildExit
    },
}

pub fn task_event_channel() -> (TaskEventSender, TaskEventReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    (TaskEventSender::new("".to_string(), tx), TaskEventReceiver::new(rx))
}

#[derive(Debug, Clone)]
pub struct TaskEventSender {
    name: String,
    tx: mpsc::UnboundedSender<TaskEvent>,
    logs: Arc<Mutex<Vec<u8>>>,
}

impl TaskEventSender {
    pub fn new(name: String, tx: mpsc::UnboundedSender<TaskEvent>) -> Self {
        Self {
            name,
            tx,
            logs: Default::default(),
        }
    }

    pub fn clone_with_name(&self, name: &str) -> Self {
        let mut this = self.clone();
        this.name = name.to_string();
        this
    }

    pub fn start(&self, task: &str) -> anyhow::Result<()> {
        self.send(TaskEvent::Start { task: task.to_string() })
    }

    pub fn finish(&self, task: &str, reason: ChildExit) -> anyhow::Result<()> {
        self.send(TaskEvent::Finish { task: task.to_string(), reason })
    }

    fn send(&self, event: TaskEvent) -> anyhow::Result<()> {
        self.tx.send(event).with_context(|| "failed to send event")
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

    pub async fn recv(&mut self) -> anyhow::Result<TaskEvent> {
        self.rx.recv().await.with_context(|| "failed to send event")
    }
}
