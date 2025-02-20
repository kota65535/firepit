use crate::cui::color::ColorSelector;
use crate::cui::lib::ColorConfig;
use crate::cui::output::{OutputClient, OutputClientBehavior, OutputSink};
use crate::cui::prefixed::PrefixedWriter;
use crate::event::EventSender;
use crate::event::{Event, EventReceiver};
use crate::signal::SignalHandler;
use crate::tokio_spawn;
use anyhow::Context;
use std::collections::HashMap;
use std::io::{stdout, Stdout, Write};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tracing::warn;

pub struct CuiApp {
    color_selector: ColorSelector,
    output_clients: Arc<RwLock<HashMap<String, OutputClient<PrefixedWriter<Stdout>>>>>,
    sender: EventSender,
    receiver: EventReceiver,
    signal_handler: SignalHandler,
}

impl CuiApp {
    pub fn new() -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Ok(Self {
            color_selector: ColorSelector::default(),
            output_clients: Arc::new(RwLock::new(HashMap::new())),
            sender: EventSender::new(tx),
            receiver: EventReceiver::new(rx),
            signal_handler: SignalHandler::infer()?,
        })
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
        let signal_handler = self.signal_handler.clone();
        let sender = self.sender.clone();
        tokio_spawn!("cui-canceller", async move {
            let subscriber = signal_handler.subscribe();
            if let Some(subscriber) = subscriber {
                let _guard = subscriber.listen().await;
                sender.stop().await;
            }
        });
        while let Some(event) = self.receiver.recv().await {
            match event {
                Event::StartTask { task, .. } => self.register_output_client(&task),
                Event::TaskOutput { task, output } => {
                    let output_clients = self.output_clients.read().expect("lock poisoned");
                    let output_client = output_clients.get(&task).with_context(|| "Output client not found")?;
                    output_client
                        .stdout()
                        .write_all(output.as_slice())
                        .context("failed to write to stdout")?;
                }
                Event::Stop(callback) => {
                    if let Err(e) = callback.send(()) {
                        warn!("Failed to send callback event: {:?}", e)
                    }
                    return Ok(());
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
