use crate::project::Task;
use crate::runner::command::RunnerCommandChannel;
use crate::tokio_spawn;
use notify::Watcher;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct FileWatcher {
    tasks: Vec<Task>,
    dir: PathBuf,
    debounce_duration: Duration,
    inner: Arc<Mutex<FileWatcherState>>,
}

#[derive(Clone)]
pub struct FileWatcherState {
    is_closing: bool,
}

#[derive(Debug, Clone)]
pub enum WatcherCommand {
    Stop,
}

pub struct FileWatcherHandle {
    pub watcher_tx: broadcast::Sender<WatcherCommand>,
    pub future: JoinHandle<()>,
}

impl FileWatcher {
    pub fn new(tasks: &Vec<Task>, dir: &Path, debounce_duration: Duration) -> FileWatcher {
        FileWatcher {
            inner: Arc::new(Mutex::new(FileWatcherState { is_closing: false })),
            tasks: tasks.clone(),
            dir: dir.to_path_buf(),
            debounce_duration,
        }
    }

    pub fn run(&mut self, runner_tx: &RunnerCommandChannel) -> anyhow::Result<FileWatcherHandle> {
        let (std_tx, std_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();

        let mut watcher = notify::recommended_watcher(std_tx)?;
        watcher.watch(&self.dir, notify::RecursiveMode::Recursive)?;

        let (watcher_tx, mut watcher_rx) = broadcast::channel(1024);

        // Cancel the file watcher if cancel is sent
        let state = self.inner.clone();
        tokio_spawn!("watcher-canceller", async move {
            while let Ok(event) = watcher_rx.recv().await {
                match event {
                    WatcherCommand::Stop => {
                        info!("Stopping watcher");
                        let mut state = state.lock().expect("not poisoned");
                        state.is_closing = true;
                        break;
                    }
                }
            }
        });

        let state = self.inner.clone();
        let tasks = self.tasks.clone();
        let dir = self.dir.clone();
        let debounce_duration = self.debounce_duration.clone();
        let runner_tx = runner_tx.clone();
        let future = std::thread::spawn(move || {
            let _guard = watcher;
            let mut event_buffer = Vec::new();
            info!("Start watching files under {:?}", dir);
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
                            debug!("Event buffer {:?}", event_buffer);
                            let paths = event_buffer
                                .iter()
                                .filter(|e| e.kind.is_create() || e.kind.is_modify() || e.kind.is_remove())
                                .flat_map(|e| e.paths.clone())
                                .collect::<HashSet<_>>();

                            info!("{} Changed files: {:?}", paths.len(), paths);
                            let mut changed_tasks = Vec::new();
                            for t in tasks.iter() {
                                if t.match_inputs(&paths) {
                                    changed_tasks.push(t.name.clone())
                                }
                            }

                            for task in changed_tasks.iter() {
                                runner_tx.restart_task(task, false);
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
                    return;
                }
            }
            info!("Watcher finished");
        });

        let wrapped_future = tokio::task::spawn_blocking(move || future.join().expect("thread panicked"));

        Ok(FileWatcherHandle {
            watcher_tx,
            future: wrapped_future,
        })
    }
}
