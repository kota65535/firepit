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
    writers: Arc<Mutex<SinkWriters<W>>>,
    primary: Marginals,
    error: Marginals,
}

#[derive(Default)]
struct Marginals {
    header: Option<()>,
    footer: Option<()>,
}

pub struct OutputWriter<'a, W> {
    logger: &'a OutputClient<W>,
    destination: Destination,
    buffer: Vec<u8>,
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
enum Destination {
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
            OutputClientBehavior::InMemoryBuffer | OutputClientBehavior::Grouped => {
                Some(Default::default())
            }
        };
        let writers = self.writers.clone();
        OutputClient {
            behavior,
            buffer,
            writers,
            primary: Default::default(),
            error: Default::default(),
        }
    }
}

impl<W: Write> OutputClient<W> {
    /// A writer that will write to the underlying sink's out writer according
    /// to this client's behavior.
    pub fn stdout(&self) -> OutputWriter<W> {
        OutputWriter {
            logger: self,
            destination: Destination::Stdout,
            buffer: Vec::new(),
        }
    }

    /// A writer that will write to the underlying sink's err writer according
    /// to this client's behavior.
    pub fn stderr(&self) -> OutputWriter<W> {
        OutputWriter {
            logger: self,
            destination: Destination::Stderr,
            buffer: Vec::new(),
        }
    }

    /// Consume the client and flush any bytes to the underlying sink if
    /// necessary
    pub fn finish(self, use_error: bool) -> io::Result<Option<Vec<u8>>> {
        let Self {
            behavior,
            buffer,
            writers,
            primary,
            error,
        } = self;
        let buffers = buffer.map(|cell| cell.into_inner().expect("lock poisoned"));
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
        let buffer = self
            .buffer
            .as_ref()
            .expect("attempted to add line to nil buffer");
        buffer.write().expect("lock poisoned").push(bytes);
    }
}

impl<'a, W: Write> Write for OutputWriter<'a, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for line in buf.split_inclusive(|b| *b == b'\n') {
            self.buffer.extend_from_slice(line);
            // If the line doesn't end in a newline we assume it isn't finished and add it
            // to the buffer
            if line.ends_with(b"\n") {
                self.logger.handle_bytes(SinkBytes {
                    buffer: self.buffer.as_slice().into(),
                    destination: self.destination,
                })?;
                self.buffer.clear();
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.logger.handle_bytes(SinkBytes {
            buffer: self.buffer.as_slice().into(),
            destination: self.destination,
        })?;
        self.buffer.clear();
        Ok(())
    }
}
