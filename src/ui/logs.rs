use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
};
use std::path::Path;
use anyhow::Context;
use tracing::{debug, warn};

/// Receives logs and multiplexes them to a log file and/or a prefixed
/// writer
pub struct LogWriter<W> {
    log_file: Option<BufWriter<File>>,
    writer: Option<W>,
}

/// Derive didn't work here.
/// (we don't actually need `W` to implement `Default` here)
impl<W> Default for LogWriter<W> {
    fn default() -> Self {
        Self {
            log_file: None,
            writer: None,
        }
    }
}

impl<W: Write> LogWriter<W> {
    pub fn with_log_file(&mut self, log_file_path: &Path) -> anyhow::Result<()> {
        let log_file = File::create(log_file_path).context("error creating log file")?;
        self.log_file = Some(BufWriter::new(log_file));
        
        Ok(())
    }

    pub fn with_writer(&mut self, writer: W) {
        self.writer = Some(writer);
    }
}

impl<W: Write> Write for LogWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match (&mut self.log_file, &mut self.writer) {
            (Some(log_file), Some(prefixed_writer)) => {
                let _ = prefixed_writer.write(buf)?;
                log_file.write(buf)
            }
            (Some(log_file), None) => log_file.write(buf),
            (None, Some(prefixed_writer)) => prefixed_writer.write(buf),
            (None, None) => {
                // Should this be an error or even a panic?
                debug!("no log file or prefixed writer");
                Ok(0)
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(log_file) = &mut self.log_file {
            log_file.flush()?;
        }
        if let Some(prefixed_writer) = &mut self.writer {
            prefixed_writer.flush()?;
        }

        Ok(())
    }
}
