use regex::Regex;
use crate::event::{Event, EventReceiver, EventSender};

pub trait Prober {
    async fn probe(&self) -> bool;
}
pub struct LogLineProber {
    regex: Option<Regex>,
    rx: EventReceiver,
    tx: EventSender
}

impl LogLineProber {

    pub fn new(regex: Option<Regex>, rx: EventReceiver, tx: EventSender) -> Self {
        Self {
            regex,
            rx,
            tx
        }
    }
    
    pub async fn probe(&mut self) {
        while let Some(event) = self.rx.recv().await {
            match event {
                Event::TaskOutput { task, output } => {
                    self.tx.output(task.clone(), output.clone()).ok();
                    let line = String::from_utf8(output).unwrap_or_default();
                    if let Some(regex) = &self.regex {
                        if regex.is_match(&line) {
                            self.tx.ready_task(task.clone());
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

pub struct ExecProber {
    
}