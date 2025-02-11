use crate::config::{LogConfig, UI};
use std::fs::File;
use std::str::FromStr;
use std::sync::Mutex;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::writer::{BoxMakeWriter, MakeWriterExt};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

pub fn init_logger(log: &LogConfig, ui: &UI) -> anyhow::Result<()> {
    match &log.file {
        Some(f) => {
            let file = File::create(f).expect("Failed to create log file");
            let file = Mutex::new(file);
            let file_writer = BoxMakeWriter::new(move || {
                let file = file.lock().unwrap();
                file.try_clone().expect("Failed to clone file handle")
            });

            let file_layer = tracing_subscriber::fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false)
                .with_filter(EnvFilter::new(&log.level));

            tracing_subscriber::registry()
                .with(file_layer)
                .with(console_subscriber::spawn().with_filter(LevelFilter::TRACE))
                .init();
            Ok(())
        }
        None => {
            if *ui != UI::Tui {
                tracing_subscriber::fmt()
                    .with_max_level(tracing::Level::from_str(&log.level)?)
                    .init();
            }
            Ok(())
        }
    }
}
