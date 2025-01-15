use std::io;

use crate::ui::output::{OutputClient, OutputWriter};
use crate::ui::sender::TaskSender;
use either::Either;

/// Small wrapper over our two output types that defines a shared interface for
/// interacting with them.
pub enum TaskOutput<W> {
    Direct(OutputClient<W>),
    UI(TaskSender),
}

/// Struct for displaying information about task
impl<W: io::Write> TaskOutput<W> {
    pub fn finish(self, use_error: bool, is_cache_hit: bool) -> io::Result<Option<Vec<u8>>> {
        match self {
            TaskOutput::Direct(client) => client.finish(use_error),
            TaskOutput::UI(client) if use_error => Ok(Some(client.failed())),
            TaskOutput::UI(client) => Ok(Some(client.succeeded(is_cache_hit))),
        }
    }

    pub fn stdout(&self) -> Either<OutputWriter<W>, TaskSender> {
        match self {
            TaskOutput::Direct(client) => Either::Left(client.stdout()),
            TaskOutput::UI(client) => Either::Right(client.clone()),
        }
    }

    pub fn stderr(&self) -> Either<OutputWriter<W>, TaskSender> {
        match self {
            TaskOutput::Direct(client) => Either::Left(client.stderr()),
            TaskOutput::UI(client) => Either::Right(client.clone()),
        }
    }

    pub fn task_logs(&self) -> Either<OutputWriter<W>, TaskSender> {
        match self {
            TaskOutput::Direct(client) => Either::Left(client.stdout()),
            TaskOutput::UI(client) => Either::Right(client.clone()),
        }
    }
}

// A tiny enum that allows us to use the same type for stdout and stderr without
// the use of Box<dyn Write>
pub enum StdWriter {
    Out(io::Stdout),
    Err(io::Stderr),
    Null(io::Sink),
}

impl StdWriter {
    fn writer(&mut self) -> &mut dyn io::Write {
        match self {
            StdWriter::Out(out) => out,
            StdWriter::Err(err) => err,
            StdWriter::Null(null) => null,
        }
    }
}

impl From<io::Stdout> for StdWriter {
    fn from(value: io::Stdout) -> Self {
        Self::Out(value)
    }
}

impl From<io::Stderr> for StdWriter {
    fn from(value: io::Stderr) -> Self {
        Self::Err(value)
    }
}

impl From<io::Sink> for StdWriter {
    fn from(value: io::Sink) -> Self {
        Self::Null(value)
    }
}

impl io::Write for StdWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer().flush()
    }
}
