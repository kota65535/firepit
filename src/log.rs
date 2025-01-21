use crate::config::LogConfig;
use anyhow::Context;
use log::LevelFilter;
use std::fs::File;
use std::str::FromStr;

pub fn init_logger(config: &LogConfig) -> anyhow::Result<()> {
    let mut builder = env_logger::Builder::new();
    builder.filter_level(
        LevelFilter::from_str(&config.level)
            .with_context(|| format!("invalid log level: {}", &config.level))?,
    );
    match &config.file {
        Some(file) => {
            let target = Box::new(
                File::create(file).with_context(|| format!("cannot create log file {}", file))?,
            );
            builder.target(env_logger::Target::Pipe(target)).init();
            Ok(())
        }
        None => {
            builder.init();
            Ok(())
        },
    }
}
