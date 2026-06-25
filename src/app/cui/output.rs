use std::{
    borrow::Cow,
    io::{self, Write},
    sync::{Arc, Mutex, RwLock},
};

/// OutputSink represent a sink for outputs that can be written to from multiple
/// threads through the use of Loggers.
pub struct OutputSink<W> {
    writers: Arc<Mutex<SinkWriters<W>>>,
}

struct SinkWriters<W> {
    out: W,
    err: W,
}

/// OutputClient allows for multiple threads to write to the same OutputSink
pub struct OutputClient<W> {
    behavior: OutputClientBehavior,
    // We could use a RefCell if we didn't use this with async code.
    // Any locals held across an await must implement Sync and RwLock lets us achieve this
    buffer: Option<RwLock<Vec<SinkBytes<'static>>>>,
    line_buffers: LineBuffers,
    writers: Arc<Mutex<SinkWriters<W>>>,
}

struct LineBuffers {
    out: RwLock<Vec<u8>>,
    err: RwLock<Vec<u8>>,
}

pub struct OutputWriter<'a, W> {
    logger: &'a OutputClient<W>,
    destination: Destination,
}

/// Enum for controlling the behavior of the client
#[derive(Debug, Clone, Copy)]
pub enum OutputClientBehavior {
    /// Every line sent to the client will get immediately sent to the sink
    Passthrough,
    /// Every line sent to the client will get immediately sent to the sink,
    /// but a buffer will be built up as well and returned when finish is called
    InMemoryBuffer,
    // Every line sent to the client will get tracked in the buffer only being
    // sent to the sink once finish is called.
    Grouped,
}

#[derive(Debug, Clone, Copy)]
pub enum Destination {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone)]
struct SinkBytes<'a> {
    buffer: Cow<'a, [u8]>,
    destination: Destination,
}

impl<W: Write> OutputSink<W> {
    /// Produces a new sink with the corresponding out and err writers
    pub fn new(out: W, err: W) -> Self {
        Self {
            writers: Arc::new(Mutex::new(SinkWriters { out, err })),
        }
    }

    /// Produces a new client that will send all bytes that it receives to the
    /// underlying sink. Behavior of how these bytes are sent is controlled
    /// by the behavior parameter. Note that OutputClient intentionally doesn't
    /// implement Sync as if you want to write to the same sink
    /// from multiple threads, then you should create a logger for each thread.
    pub fn logger(&self, behavior: OutputClientBehavior) -> OutputClient<W> {
        let buffer = match behavior {
            OutputClientBehavior::Passthrough => None,
            OutputClientBehavior::InMemoryBuffer | OutputClientBehavior::Grouped => Some(Default::default()),
        };
        let writers = self.writers.clone();
        OutputClient {
            behavior,
            buffer,
            line_buffers: LineBuffers {
                out: RwLock::new(Vec::new()),
                err: RwLock::new(Vec::new()),
            },
            writers,
        }
    }
}

impl<W: Write> OutputClient<W> {
    /// A writer that will write to the underlying sink's out writer according
    /// to this client's behavior.
    pub fn stdout(&self) -> OutputWriter<'_, W> {
        OutputWriter {
            logger: self,
            destination: Destination::Stdout,
        }
    }

    /// A writer that will write to the underlying sink's err writer according
    /// to this client's behavior.
    pub fn stderr(&self) -> OutputWriter<'_, W> {
        OutputWriter {
            logger: self,
            destination: Destination::Stderr,
        }
    }

    /// Consume the client and flush any bytes to the underlying sink if
    /// necessary
    pub fn finish(self) -> io::Result<Option<Vec<u8>>> {
        let Self { buffer, .. } = self;
        let buffers = buffer.map(|cell| cell.into_inner().expect("should not poisoned"));
        Ok(buffers.map(|buffers| {
            // TODO: it might be worth the list traversal to calculate length so we do a
            // single allocation
            let mut bytes = Vec::new();
            for SinkBytes { buffer, .. } in buffers {
                bytes.extend_from_slice(&buffer[..]);
            }
            bytes
        }))
    }

    fn handle_bytes(&self, bytes: SinkBytes) -> io::Result<()> {
        if matches!(
            self.behavior,
            OutputClientBehavior::InMemoryBuffer | OutputClientBehavior::Grouped
        ) {
            // This reconstruction is necessary to change the type of bytes from
            // SinkBytes<'a> to SinkBytes<'static>
            let bytes = SinkBytes {
                destination: bytes.destination,
                buffer: bytes.buffer.to_vec().into(),
            };
            self.add_bytes_to_buffer(bytes);
        }
        if matches!(
            self.behavior,
            OutputClientBehavior::Passthrough | OutputClientBehavior::InMemoryBuffer
        ) {
            self.write_bytes(bytes)
        } else {
            // If we only wrote to the buffer, then we consider it a successful write
            Ok(())
        }
    }

    fn write_bytes(&self, bytes: SinkBytes) -> io::Result<()> {
        let SinkBytes {
            buffer: line,
            destination,
        } = bytes;
        let mut writers = self.writers.lock().expect("writer lock poisoned");
        let writer = match destination {
            Destination::Stdout => &mut writers.out,
            Destination::Stderr => &mut writers.err,
        };
        writer.write_all(&line)
    }

    fn add_bytes_to_buffer(&self, bytes: SinkBytes<'static>) {
        let buffer = self.buffer.as_ref().expect("attempted to add line to nil buffer");
        buffer.write().expect("lock poisoned").push(bytes);
    }

    fn line_buffer(&self, destination: Destination) -> &RwLock<Vec<u8>> {
        match destination {
            Destination::Stdout => &self.line_buffers.out,
            Destination::Stderr => &self.line_buffers.err,
        }
    }
}

impl<'a, W: Write> Write for OutputWriter<'a, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut ready = Vec::new();
        let line_buffer = self.logger.line_buffer(self.destination);
        {
            let mut buffer = line_buffer.write().expect("lock poisoned");

            for line in buf.split_inclusive(|b| *b == b'\n') {
                buffer.extend_from_slice(line);
                // If the line doesn't end in a newline we assume it isn't finished and add it
                // to the buffer
                if line.ends_with(b"\n") {
                    ready.push(std::mem::take(&mut *buffer));
                }
            }
        }

        for line in ready {
            self.logger.handle_bytes(SinkBytes {
                buffer: line.into(),
                destination: self.destination,
            })?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let line = {
            let line_buffer = self.logger.line_buffer(self.destination);
            let mut buffer = line_buffer.write().expect("lock poisoned");
            if buffer.is_empty() {
                None
            } else {
                Some(std::mem::take(&mut *buffer))
            }
        };
        if let Some(line) = line {
            self.logger.handle_bytes(SinkBytes {
                buffer: line.into(),
                destination: self.destination,
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{OutputClientBehavior, OutputSink};
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct SharedWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl SharedWriter {
        fn bytes(&self) -> Vec<u8> {
            self.bytes.lock().expect("lock poisoned").clone()
        }
    }

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.lock().expect("lock poisoned").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn passthrough_keeps_partial_stdout_across_writers() {
        let stdout = SharedWriter::default();
        let sink = OutputSink::new(stdout.clone(), SharedWriter::default());
        let client = sink.logger(OutputClientBehavior::Passthrough);

        client.stdout().write_all(b"Enter a value: ").unwrap();
        assert!(stdout.bytes().is_empty());

        client.stdout().write_all(b"\n").unwrap();
        assert_eq!(stdout.bytes(), b"Enter a value: \n");
    }

    #[test]
    fn passthrough_flushes_partial_stdout() {
        let stdout = SharedWriter::default();
        let sink = OutputSink::new(stdout.clone(), SharedWriter::default());
        let client = sink.logger(OutputClientBehavior::Passthrough);

        let mut writer = client.stdout();
        writer.write_all(b"Enter a value: ").unwrap();
        writer.flush().unwrap();

        assert_eq!(stdout.bytes(), b"Enter a value: ");
    }
}
