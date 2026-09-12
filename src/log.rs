use crate::app::command::{AppCommand, AppCommandChannel};
use crate::config::LogConfig;
use anyhow::Context;
use std::fmt::Write as _;
use std::fs::File;
use std::io;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Event, Id, Level, Subscriber};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::layer::{Context as LayerContext, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

/// A record of the firepit log on its way to the UI.
#[derive(Debug, Clone)]
pub struct LogRecord {
    /// The task the record is about, or `None` when it is about firepit itself
    pub task: Option<String>,
    pub level: Level,
    /// Where in firepit the record was made, as `firepit::probe`
    pub module: String,
    pub message: String,
}

impl LogRecord {
    /// The lines to show, opening with the level and the module.
    ///
    /// The output of a task says `INFO` too, so a record has to look like one of
    /// firepit's -- as a log line, not as another name beside the task's.
    pub fn lines(&self) -> impl Iterator<Item = String> + '_ {
        let opening = format!("{} {}: ", self.level.as_str(), self.module);
        self.message.lines().map(move |line| format!("{opening}{line}"))
    }
}

/// Where the log goes on its way to the UI.
///
/// The UI does not exist yet when the logger is installed, so the records made
/// until it does are held rather than dropped.
#[derive(Clone, Default)]
pub struct LogSink(Arc<Mutex<SinkState>>);

#[derive(Default)]
enum SinkState {
    #[default]
    Held,
    Holding(Vec<LogRecord>),
    Connected(AppCommandChannel),
}

impl LogSink {
    /// Sends everything held so far, and everything after it, to the app.
    pub fn connect(&self, app_tx: &AppCommandChannel) {
        let mut state = self.0.lock().expect("log sink poisoned");
        if let SinkState::Holding(held) = std::mem::take(&mut *state) {
            for record in held {
                Self::send(app_tx, record);
            }
        }
        *state = SinkState::Connected(app_tx.clone());
    }

    fn record(&self, record: LogRecord) {
        let mut state = self.0.lock().expect("log sink poisoned");
        match &mut *state {
            SinkState::Connected(app_tx) => Self::send(app_tx, record),
            SinkState::Holding(held) => held.push(record),
            SinkState::Held => *state = SinkState::Holding(vec![record]),
        }
    }

    /// Not through `AppCommandChannel::send`, which logs when the channel is
    /// closed: that record would come back here, fail to send, and log again.
    fn send(app_tx: &AppCommandChannel, record: LogRecord) {
        app_tx.tx.send(AppCommand::Log(record)).ok();
    }
}

/// Puts every record the filter lets through into the `LogSink`, with the task
/// it belongs to: the one named by the `name` field of the `task` span it is in.
struct AppLayer {
    sink: LogSink,
}

/// The name of the task a span is about, kept on the span so that the events
/// inside it can find it.
struct TaskName(String);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for AppLayer {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: LayerContext<'_, S>) {
        if attrs.metadata().name() != "task" {
            return;
        }
        let mut visitor = FieldVisitor::new("name");
        attrs.record(&mut visitor);
        if let (Some(value), Some(span)) = (visitor.value, ctx.span(id)) {
            span.extensions_mut().insert(TaskName(value));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: LayerContext<'_, S>) {
        let task = ctx.event_scope(event).and_then(|scope| {
            scope
                .from_root()
                .find_map(|span| span.extensions().get::<TaskName>().map(|t| t.0.clone()))
        });
        let mut visitor = FieldVisitor::new("message");
        event.record(&mut visitor);
        let Some(message) = visitor.value else {
            return;
        };
        self.sink.record(LogRecord {
            task,
            level: *event.metadata().level(),
            module: event.metadata().target().to_string(),
            message,
        });
    }
}

/// Reads one field by name, keeping the rest as `key=value`.
struct FieldVisitor {
    wanted: &'static str,
    value: Option<String>,
    extra: String,
}

impl FieldVisitor {
    fn new(wanted: &'static str) -> Self {
        Self {
            wanted,
            value: None,
            extra: String::new(),
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == self.wanted {
            self.value = Some(format!("{value:?}"));
        } else {
            write!(self.extra, " {}={:?}", field.name(), value).ok();
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == self.wanted {
            self.value = Some(value.to_string());
        } else {
            write!(self.extra, " {}={}", field.name(), value).ok();
        }
    }
}

/// Installs the logger and returns the sink to connect to the app once it exists.
///
/// A file is somewhere to keep the log as well, not somewhere to put it instead:
/// configuring one must not take it out of the UI.
pub fn init_logger(log: &LogConfig, tokio_console: bool) -> anyhow::Result<LogSink> {
    let sink = LogSink::default();
    let app_layer = AppLayer { sink: sink.clone() }.with_filter(EnvFilter::new(&log.level));

    let file_layer = match &log.file {
        Some(file_path) => {
            let file = File::create(file_path).with_context(|| format!("failed to create log file {}", file_path))?;
            let file = Mutex::new(file);
            let file_writer = BoxMakeWriter::new(move || {
                let file = file.lock().unwrap();
                file.try_clone().expect("file handle should be cloneable")
            });
            Some(
                tracing_subscriber::fmt::layer()
                    .with_writer(file_writer)
                    .with_ansi(false)
                    .with_filter(EnvFilter::new(&log.level)),
            )
        }
        None => None,
    };

    let console = tokio_console.then(|| console_subscriber::spawn().with_filter(LevelFilter::TRACE));
    tracing_subscriber::registry()
        .with(app_layer)
        .with(file_layer)
        .with(console)
        .init();
    Ok(sink)
}

#[derive(Debug, Clone)]
pub struct OutputCollector {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl OutputCollector {
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn take_output(&self) -> String {
        let mut buf = self.buffer.lock().expect("buffer poisoned");
        let contents = String::from_utf8_lossy(&buf).into_owned();
        buf.clear();
        contents
    }
}

impl Default for OutputCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for OutputCollector {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut buffer = self.buffer.lock().expect("buffer poisoned");
        buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
