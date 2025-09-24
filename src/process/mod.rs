//! `process`
//!
//! This module contains the code that is responsible for running the commands
//! that are queued by run. It consists of a set of child processes that are
//! spawned and managed by the manager. The manager is responsible for
//! running these processes to completion, forwarding signals, and closing
//! them when the manager is closed.
//!
//! As of now, the manager will execute futures in a random order, and
//! must be either `wait`ed on or `stop`ped to drive state.

mod child;
mod command;

use std::{io, sync::Arc, time::Duration};

pub use command::Command;
use futures::stream::FuturesUnordered;
use futures::{Future, StreamExt};
use tokio::sync::{watch, Mutex};
use tokio::task::JoinSet;
use tracing::{debug, trace};

pub use self::child::{Child, ChildExit};

/// A process manager that is responsible for spawning and managing child
/// processes. When the manager is Open, new child processes can be spawned
/// using `spawn`. When the manager is Closed, all currently-running children
/// will be closed, and no new children can be spawned.
#[derive(Debug, Clone)]
pub struct ProcessManager {
    state: Arc<Mutex<ProcessManagerInner>>,
    use_pty: bool,
}

#[derive(Debug)]
struct ProcessManagerInner {
    is_closing: bool,
    children: Vec<Child>,
    size: Option<PtySize>,
    current_shutdown_priority: u8,
    shutdown_cancel_tx: Option<watch::Sender<u8>>,
}

#[derive(Debug, Clone, Copy)]
pub struct PtySize {
    rows: u16,
    cols: u16,
}

impl ProcessManager {
    pub fn new(use_pty: bool) -> Self {
        debug!("Spawning children with pty: {use_pty}");
        Self {
            state: Arc::new(Mutex::new(ProcessManagerInner {
                is_closing: false,
                children: Vec::new(),
                size: None,
                current_shutdown_priority: 0,
                shutdown_cancel_tx: None,
            })),
            use_pty,
        }
    }

    /// Construct a process manager and infer if pty should be used
    pub fn infer() -> Self {
        // Only use PTY if we're not on windows and we're currently hooked up to a
        // in a TTY
        let use_pty = atty::is(atty::Stream::Stdout);
        Self::new(use_pty)
    }

    /// Returns whether children will be spawned attached to a pseudoterminal
    #[allow(dead_code)]
    pub fn use_pty(&self) -> bool {
        self.use_pty
    }
}

impl ProcessManager {
    /// Spawn a new child process to run the given command.
    ///
    /// The handle of the child can be either waited or stopped by the caller,
    /// as well as the entire process manager.
    ///
    /// If spawn returns None, the process manager is closed and the child
    /// process was not spawned. If spawn returns Some(Err), the process
    /// manager is open, but the child process failed to spawn.
    pub async fn spawn(&self, command: Command, stop_timeout: Duration) -> Option<io::Result<Child>> {
        let label = command.label();
        trace!("acquiring lock for spawning {label}");
        let mut lock = self.state.lock().await;
        trace!("acquired lock for spawning {label}");
        if lock.is_closing {
            debug!("Process manager is closing, refuses to spawn");
            return None;
        }
        let pty_size = self.use_pty.then(|| lock.pty_size()).flatten();
        let child = Child::spawn(command, child::ShutdownStyle::Graceful(stop_timeout), pty_size);
        if let Ok(child) = &child {
            lock.children.push(child.clone());
        }
        trace!("releasing lock for spawning {label}");
        Some(child)
    }

    pub async fn stop_by_label(&self, label: &str) -> anyhow::Result<Vec<ChildExit>> {
        let mut children = {
            let lock = self.state.lock().await;
            lock.children
                .iter()
                .filter(|c| c.label() == label)
                .cloned()
                .collect::<Vec<_>>()
        };

        let results = FuturesUnordered::from_iter(children.iter_mut().map(|c| c.stop()))
            .filter_map(|r| async move { r })
            .collect()
            .await;

        Ok(results)
    }

    pub async fn stop_by_pid(&self, pid: u32) -> Option<ChildExit> {
        let child = {
            let mut lock = self.state.lock().await;
            lock.children.iter_mut().find(|c| c.pid() == Some(pid)).cloned()
        };
        if let Some(mut c) = child {
            c.stop().await
        } else {
            None
        }
    }

    /// Stop the process manager, closing all child processes. On posix
    /// systems this will send a SIGINT, and on windows it will just kill
    /// the process immediately.
    pub async fn stop(&self) {
        self.close_with_priority(1, |mut c| async move { c.stop().await }).await
    }

    pub async fn kill(&self) {
        self.close_with_priority(2, |mut c| async move { c.kill().await }).await
    }

    /// Stop the process manager, waiting for all child processes to exit.
    ///
    /// If you want to set a timeout, use `tokio::time::timeout` and
    /// `Self::stop` if the timeout elapses.
    pub async fn wait(&self) {
        self.close(|mut c| async move { c.wait().await }).await
    }

    /// Close the process manager with priority handling
    ///
    /// Higher priority operations can interrupt lower priority ones
    async fn close_with_priority<F, C>(&self, priority: u8, callback: F)
    where
        F: Fn(Child) -> C + Sync + Send + Copy + 'static,
        C: Future<Output = Option<ChildExit>> + Sync + Send + 'static,
    {
        let (cancel_rx, should_start) = {
            let mut lock = self.state.lock().await;

            // Check if we should override the current operation
            let should_start = if lock.current_shutdown_priority == 0 {
                // No current operation
                true
            } else if priority > lock.current_shutdown_priority {
                // Higher priority operation overrides current one
                if let Some(sender) = &lock.shutdown_cancel_tx {
                    let _ = sender.send(priority);
                }
                true
            } else if priority == lock.current_shutdown_priority {
                // Same priority operation, don't start a new one
                false
            } else {
                // Lower priority operation, ignore
                return;
            };

            if should_start {
                lock.is_closing = true;
                lock.current_shutdown_priority = priority;

                let (cancel_tx, cancel_rx) = watch::channel(priority);
                lock.shutdown_cancel_tx = Some(cancel_tx);
                (cancel_rx, true)
            } else {
                // Wait for existing operation to complete
                if let Some(sender) = &lock.shutdown_cancel_tx {
                    let cancel_rx = sender.subscribe();
                    (cancel_rx, false)
                } else {
                    return;
                }
            }
        };

        if !should_start {
            // Wait for existing operation to complete or be cancelled
            let mut rx = cancel_rx;
            let _ = rx.changed().await;
            return;
        }

        self.execute_shutdown(priority, callback, cancel_rx).await;

        // Clear operation state
        {
            let mut lock = self.state.lock().await;
            lock.current_shutdown_priority = 0;
            lock.shutdown_cancel_tx = None;
            lock.children = vec![];
        }
    }

    async fn execute_shutdown<F, C>(&self, priority: u8, callback: F, mut cancel_rx: watch::Receiver<u8>)
    where
        F: Fn(Child) -> C + Sync + Send + Copy + 'static,
        C: Future<Output = Option<ChildExit>> + Sync + Send + 'static,
    {
        let children = {
            let lock = self.state.lock().await;
            lock.children.clone()
        };

        let mut set = JoinSet::new();
        for child in children {
            set.spawn(async move { callback(child).await });
        }

        debug!("Waiting for {} processes to exit with priority {}", set.len(), priority);

        loop {
            tokio::select! {
                // Check for cancellation/override
                Ok(()) = cancel_rx.changed() => {
                    let new_priority = *cancel_rx.borrow();
                    if new_priority != priority {
                        debug!("Shutdown operation priority {} cancelled by priority {}", priority, new_priority);
                        // Abort remaining tasks and let the new operation take over
                        set.abort_all();
                        return;
                    }
                }
                // Wait for processes to exit
                Some(out) = set.join_next() => {
                    trace!("process exited: {:?}", out);
                    if set.is_empty() {
                        break;
                    }
                }
                else => break,
            }
        }

        debug!("All processes exited for priority {}", priority);
    }

    /// Close the process manager, running the given callback on each child
    ///
    /// note: this is designed to be called multiple times, ie calling close
    /// with two different strategies will propagate both signals to the child
    /// processes. clearing the task queue and re-enabling spawning are both
    /// idempotent operations
    async fn close<F, C>(&self, callback: F)
    where
        F: Fn(Child) -> C + Sync + Send + Copy + 'static,
        C: Future<Output = Option<ChildExit>> + Sync + Send + 'static,
    {
        self.close_with_priority(1, callback).await
    }

    pub async fn is_closed(&self) -> bool {
        let lock = self.state.lock().await;
        lock.is_closing || lock.current_shutdown_priority != 0
    }

    pub async fn set_pty_size(&self, rows: u16, cols: u16) {
        self.state.lock().await.size = Some(PtySize { rows, cols });
    }
}

impl ProcessManagerInner {
    fn pty_size(&mut self) -> Option<PtySize> {
        if self.size.is_none() {
            self.size = PtySize::from_tty();
        }
        self.size
    }
}

impl PtySize {
    fn from_tty() -> Option<Self> {
        console::Term::stdout()
            .size_checked()
            .map(|(rows, cols)| Self { rows, cols })
    }
}

impl Default for PtySize {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}
