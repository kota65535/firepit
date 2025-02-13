use crate::config::LogConfig;
use anyhow::Context;
use std::fs::File;
use std::sync::Mutex;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

pub fn init_logger(log: &LogConfig) -> anyhow::Result<()> {
    if let Some(file_path) = &log.file {
        let file = File::create(file_path).with_context(|| format!("failed to create log file {}", file_path))?;
        let file = Mutex::new(file);
        let file_writer = BoxMakeWriter::new(move || {
            let file = file.lock().unwrap();
            file.try_clone().expect("file handle should be cloneable")
        });

        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(file_writer)
            .with_ansi(false)
            .with_filter(EnvFilter::new(&log.level));

        tracing_subscriber::registry()
            .with(file_layer)
            .with(console_subscriber::spawn().with_filter(LevelFilter::TRACE))
            .init();
    }
    Ok(())
}
