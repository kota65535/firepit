use crate::signal::{get_signal, SignalHandler};
use crate::tokio_spawn;
use log::{info, warn};
use notify::Watcher;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

#[derive(Clone)]
pub struct FileWatcher {
    inner: Arc<Mutex<FileWatcherState>>,
    signal_handler: SignalHandler,
}

#[derive(Clone)]
pub struct FileWatcherState {
    is_closing: bool,
}

pub struct FileWatcherHandle {
    pub rx: mpsc::UnboundedReceiver<HashSet<PathBuf>>,
    pub cancel: watch::Sender<()>,
    pub future: std::thread::JoinHandle<()>,
}

impl FileWatcher {
    pub fn new() -> anyhow::Result<FileWatcher> {
        Ok(FileWatcher {
            inner: Arc::new(Mutex::new(FileWatcherState { is_closing: false })),
            signal_handler: SignalHandler::new(get_signal()?),
        })
    }

    pub fn run(&mut self, path: &Path, debounce_duration: Duration) -> anyhow::Result<FileWatcherHandle> {
        let (tokio_tx, tokio_rx) = mpsc::unbounded_channel::<HashSet<PathBuf>>();
        let (std_tx, std_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();

        let mut watcher = notify::recommended_watcher(std_tx)?;
        watcher.watch(&path, notify::RecursiveMode::Recursive)?;

        let state = self.inner.clone();
        // Cancel file watcher when got signal
        if let Some(subscriber) = self.signal_handler.subscribe() {
            tokio_spawn!("watcher-canceller-signal", async move {
                let _guard = subscriber.listen().await;
                state.lock().expect("not poisoned").is_closing = true
            });
        }

        let tx = tokio_tx.clone();
        let (cancel_tx, mut cancel_rx) = watch::channel(());

        // Cancel file watcher if cancel is sent
        let state = self.inner.clone();
        tokio::spawn(async move {
            while let Ok(_) = cancel_rx.changed().await {
                state.lock().expect("not poisoned").is_closing = true;
            }
        });

        let state = self.inner.clone();
        let future = std::thread::spawn(move || {
            let _guard = watcher;
            let mut event_buffer = Vec::new();
            loop {
                match std_rx.recv_timeout(debounce_duration) {
                    Ok(event) => match event {
                        Ok(event) => {
                            event_buffer.push(event);
                        }
                        Err(e) => {
                            warn!("Failed to recv file event: {:?}", e);
                        }
                    },
                    Err(RecvTimeoutError::Timeout) => {
                        if !event_buffer.is_empty() {
                            let paths = event_buffer
                                .iter()
                                .filter(|e| e.kind.is_create() || e.kind.is_modify() || e.kind.is_remove())
                                .flat_map(|e| e.paths.clone())
                                .collect::<HashSet<_>>();
                            if let Err(e) = tx.send(paths) {
                                warn!("Failed to send file events: {:?}", e);
                            }
                            event_buffer.clear();
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        info!("Stopping watcher since sender dropped");
                        break;
                    }
                }
                if state.lock().expect("not poisoned").is_closing {
                    info!("Watcher finished");
                    return;
                }
            }
        });

        Ok(FileWatcherHandle {
            rx: tokio_rx,
            cancel: cancel_tx,
            future,
        })
    }
}
