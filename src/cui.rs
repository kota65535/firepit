use std::collections::HashMap;
use std::io::{stdout, Stdout, Write};
use std::sync::{Arc, RwLock};
use anyhow::Context;
use crate::event::{TaskEvent, TaskEventReceiver};
use crate::ui::color_selector::ColorSelector;
use crate::ui::lib::ColorConfig;
use crate::ui::output::{OutputClient, OutputClientBehavior, OutputSink};
use crate::ui::prefixed::PrefixedWriter;

pub struct CuiApp {
    color_selector: ColorSelector,
    output_clients: Arc<RwLock<HashMap<String, OutputClient<PrefixedWriter<Stdout>>>>>,
}

impl CuiApp {
    pub fn new() -> Self {
        Self {
            color_selector: ColorSelector::default(),
            output_clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn register_output_client(&mut self, prefix: &str) {
        let out = PrefixedWriter::new(ColorConfig::infer(), self.color_selector.string_with_color(prefix, prefix), stdout());
        let err = PrefixedWriter::new(ColorConfig::infer(), self.color_selector.string_with_color(prefix, prefix), stdout());
        let output_client = OutputSink::new(out, err).logger(OutputClientBehavior::Passthrough);
        self.output_clients.write().expect("lock poisoned").insert(prefix.to_string(), output_client);
    }

    pub async fn handle_events(&mut self, mut rx: TaskEventReceiver) -> anyhow::Result<()> {
        while let Some(event) = rx.recv().await {
            match event {
                TaskEvent::Start { task } => {
                    self.register_output_client(&task)
                }
                TaskEvent::Output { task, output } => {
                    let output_clients = self.output_clients.read().expect("lock poisoned");
                    let output_client = output_clients.get(&task).with_context(|| "Output client not found")?;
                    output_client.stdout().write_all(output.as_slice()).context("failed to write to stdout")?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}
