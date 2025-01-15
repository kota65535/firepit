use std::{
    fmt::Debug,
    future::Future,
    sync::{Arc, Mutex},
};
use anyhow::Context;
use futures::{stream::FuturesUnordered, StreamExt};
use log::{debug, info, warn};
use nix::sys::signal::Signal;
use tokio::sync::{mpsc, oneshot};

/// SignalHandler provides a mechanism to subscribe to a future and get alerted
/// whenever the future completes or the handler gets a close message.
#[derive(Debug, Clone)]
pub struct SignalHandler {
    state: Arc<Mutex<HandlerState>>,
    close: mpsc::Sender<()>,
}

#[derive(Debug, Default)]
struct HandlerState {
    subscribers: Vec<oneshot::Sender<oneshot::Sender<()>>>,
    is_closing: bool,
}

pub struct SignalSubscriber(oneshot::Receiver<oneshot::Sender<()>>);

/// SubscriberGuard should be kept until a subscriber is done processing the
/// signal
pub struct SubscriberGuard(oneshot::Sender<()>);

pub fn get_signal() -> anyhow::Result<impl Future<Output = Option<i32>>> {
    use tokio::signal::unix;
    let mut sigint = unix::signal(unix::SignalKind::interrupt())?;
    let mut sigterm = unix::signal(unix::SignalKind::terminate())?;

    Ok(async move {
        tokio::select! {
            _ = sigint.recv() => {
                Some(libc::SIGINT)
            }
            _ = sigterm.recv() => {
                Some(libc::SIGTERM)
            }
        }
    })
}

impl SignalHandler {
    /// Construct a new SignalHandler that will alert any subscribers when
    /// `signal_source` completes or `close` is called on it.
    pub fn new(signal_source: impl Future<Output = Option<i32>> + Send + 'static) -> Self {
        // think about channel size
        let state = Arc::new(Mutex::new(HandlerState::default()));
        let worker_state = state.clone();
        let (close, mut rx) = mpsc::channel::<()>(1);
        tokio::spawn(async move {
            tokio::select! {
                // We don't care if we get a signal or if we are unable to receive signals
                // Either way we start the shutdown.
                Some(signal_num) = signal_source => {
                    match Signal::try_from(signal_num) {
                        Ok(signal) => {
                            debug!("Got signal: {:?}({})", signal, signal_num)
                        }
                        Err(e) => {
                            warn!("Unexpected signal ({})", signal_num)
                        }
                    }

                },
                // We don't care if a close message was sent or if all handlers are dropped.
                // Either way start the shutdown process.
                _ = rx.recv() => {}
            }

            let mut callbacks = {
                let mut state = worker_state.lock().expect("lock poisoned");
                // Mark ourselves as closing to prevent any additional subscribers from being
                // added
                state.is_closing = true;
                state
                    .subscribers
                    .drain(..)
                    .filter_map(|callback| {
                        let (tx, rx) = oneshot::channel();
                        // If the subscriber is no longer around we don't wait for the callback
                        callback.send(tx).ok()?;
                        Some(rx)
                    })
                    .collect::<FuturesUnordered<_>>()
            };

            // We don't care if callback gets dropped or if the done signal is sent.
            while let Some(_fut) = callbacks.next().await {}
        });

        Self { state, close }
    }

    /// Register a new subscriber
    /// Will return `None` if SignalHandler is in the process of shutting down
    /// or if it has already shut down.
    pub fn subscribe(&self) -> Option<SignalSubscriber> {
        self.state
            .lock()
            .expect("poisoned lock")
            .add_subscriber()
            .map(SignalSubscriber)
    }

    /// Send message to signal handler that it should shut down and alert
    /// subscribers
    pub async fn close(&self) {
        if self.close.send(()).await.is_err() {
            // watcher has already closed
            return;
        }
        self.done().await;
    }

    /// Wait until handler is finished and all subscribers finish their cleanup
    /// work
    pub async fn done(&self) {
        // Receiver is dropped once the worker task completes
        self.close.closed().await;
    }

    // Check if the worker thread is done, only meant to be used for assertions in
    // testing
    #[cfg(test)]
    fn is_done(&self) -> bool {
        self.close.is_closed()
    }
}

impl SignalSubscriber {
    /// Wait until signal is received by the signal handler
    pub async fn listen(self) -> SubscriberGuard {
        let callback = self
            .0
            .await
            .expect("signal handler worker thread exited without alerting subscribers");
        SubscriberGuard(callback)
    }
}

impl HandlerState {
    fn add_subscriber(&mut self) -> Option<oneshot::Receiver<oneshot::Sender<()>>> {
        (!self.is_closing).then(|| {
            let (tx, rx) = oneshot::channel();
            self.subscribers.push(tx);
            rx
        })
    }
}
