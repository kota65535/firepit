use crate::app::command::AppCommand;
use crate::app::input::encode_key;
use crate::tokio_spawn;
use crossterm::event::EventStream;
use futures::StreamExt;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct InputHandler {}

impl InputHandler {
    pub fn new() -> Self {
        Self {}
    }

    pub fn start(&self) -> mpsc::Receiver<crossterm::event::Event> {
        let (tx, rx) = mpsc::channel(1024);

        if atty::is(atty::Stream::Stdin) {
            let mut events = EventStream::new();
            tokio_spawn!("input-handler", async move {
                while let Some(Ok(event)) = events.next().await {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            });
        }

        rx
    }

    pub fn handle(&mut self, event: crossterm::event::Event) -> Option<AppCommand> {
        match event {
            crossterm::event::Event::Key(e) => Some(AppCommand::Input { bytes: encode_key(e) }),
            _ => None,
        }
    }
}
