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

use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

pub use command::Command;
use futures::Future;
use log::{debug, log_enabled, trace};
use tokio::task::JoinSet;

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
}

#[derive(Debug, Clone, Copy)]
pub struct PtySize {
    rows: u16,
    cols: u16,
}

impl ProcessManager {
    pub fn new(use_pty: bool) -> Self {
        debug!("spawning children with pty: {use_pty}");
        Self {
            state: Arc::new(Mutex::new(ProcessManagerInner {
                is_closing: false,
                children: Vec::new(),
                size: None,
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
    pub fn spawn(&self, command: Command, stop_timeout: Duration) -> Option<io::Result<Child>> {
        let label = log_enabled!(log::Level::Trace)
            .then(|| command.label())
            .unwrap_or_default();
        trace!("acquiring lock for spawning {label}");
        let mut lock = self.state.lock().unwrap();
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

    /// Stop the process manager, closing all child processes. On posix
    /// systems this will send a SIGINT, and on windows it will just kill
    /// the process immediately.
    pub async fn stop(&self) {
        self.close(|mut c| async move { c.stop().await }).await
    }

    /// Stop the process manager, waiting for all child processes to exit.
    ///
    /// If you want to set a timeout, use `tokio::time::timeout` and
    /// `Self::stop` if the timeout elapses.
    pub async fn wait(&self) {
        self.close(|mut c| async move { c.wait().await }).await
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
        let mut set = JoinSet::new();

        {
            let mut lock = self.state.lock().expect("not poisoned");
            lock.is_closing = true;
            for child in lock.children.iter() {
                let child = child.clone();
                set.spawn(async move { callback(child).await });
            }
        }

        debug!("Waiting for {} processes to exit", set.len());

        while let Some(out) = set.join_next().await {
            trace!("process exited: {:?}", out);
        }

        {
            let mut lock = self.state.lock().expect("not poisoned");

            // just allocate a new vec rather than clearing the old one
            lock.children = vec![];
        }
    }

    pub fn is_closed(&self) -> bool {
        self.state.lock().expect("not poisoned").is_closing
    }

    pub fn set_pty_size(&self, rows: u16, cols: u16) {
        self.state.lock().expect("not poisoned").size = Some(PtySize { rows, cols });
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
