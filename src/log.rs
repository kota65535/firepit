use crate::config::{LogConfig, UI};
use anyhow::Context;
use log::LevelFilter;
use std::fs::File;
use std::str::FromStr;

pub fn init_logger(log: &LogConfig, ui: &UI) -> anyhow::Result<()> {
    let mut builder = env_logger::Builder::new();
    builder
        .filter_level(LevelFilter::from_str(&log.level).with_context(|| format!("invalid log level: {}", &log.level))?);
    match &log.file {
        Some(file) => {
            let target = Box::new(File::create(file).with_context(|| format!("cannot create log file {}", file))?);
            builder.target(env_logger::Target::Pipe(target)).init();
            Ok(())
        }
        None => {
            if *ui != UI::Tui {
                builder.init();
            }
            Ok(())
        }
    }
}
