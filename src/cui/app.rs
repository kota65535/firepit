use crate::cui::color::ColorSelector;
use crate::cui::lib::ColorConfig;
use crate::cui::output::{OutputClient, OutputClientBehavior, OutputSink};
use crate::cui::prefixed::PrefixedWriter;
use crate::event::EventSender;
use crate::event::{Event, EventReceiver};
use anyhow::Context;
use std::collections::HashMap;
use std::io::{stdout, Stdout, Write};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;

pub struct CuiApp {
    color_selector: ColorSelector,
    output_clients: Arc<RwLock<HashMap<String, OutputClient<PrefixedWriter<Stdout>>>>>,
    sender: EventSender,
    receiver: EventReceiver,
}

impl CuiApp {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            color_selector: ColorSelector::default(),
            output_clients: Arc::new(RwLock::new(HashMap::new())),
            sender: EventSender::new(tx),
            receiver: EventReceiver::new(rx),
        }
    }

    fn register_output_client(&mut self, prefix: &str) {
        let out = PrefixedWriter::new(
            ColorConfig::infer(),
            self.color_selector.string_with_color(prefix, prefix),
            stdout(),
        );
        let err = PrefixedWriter::new(
            ColorConfig::infer(),
            self.color_selector.string_with_color(prefix, prefix),
            stdout(),
        );
        let output_client = OutputSink::new(out, err).logger(OutputClientBehavior::Passthrough);
        self.output_clients
            .write()
            .expect("lock poisoned")
            .insert(prefix.to_string(), output_client);
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        while let Some(event) = self.receiver.recv().await {
            match event {
                Event::StartTask { task } => self.register_output_client(&task),
                Event::TaskOutput { task, output } => {
                    let output_clients = self.output_clients.read().expect("lock poisoned");
                    let output_client = output_clients
                        .get(&task)
                        .with_context(|| "Output client not found")?;
                    output_client
                        .stdout()
                        .write_all(output.as_slice())
                        .context("failed to write to stdout")?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn sender(&self) -> EventSender {
        self.sender.clone()
    }
}
