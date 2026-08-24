use crate::tokio_spawn;
use futures::{Stream, StreamExt};
use nix::sys::signal::Signal;
use tokio::sync::broadcast;
use tracing::{debug, warn};

/// Capacity of the broadcast channel used to deliver signals. Signals are
/// rare and only used to drive the shutdown, so a small buffer is enough.
const SIGNAL_CHANNEL_SIZE: usize = 8;

/// SignalHandler watches a signal source for the whole lifetime of the
/// process and broadcasts every signal it receives to all subscribers.
///
/// Delivering every signal (not just the first one) lets subscribers escalate
/// a graceful shutdown into a forced one when the user interrupts again.
#[derive(Debug, Clone)]
pub struct SignalHandler {
    tx: broadcast::Sender<i32>,
}

/// Build a stream that yields every SIGINT and SIGTERM the process receives.
///
/// It deliberately keeps yielding after the first signal so that a second
/// Ctrl-C can be observed while the graceful shutdown is still in progress.
fn get_signal() -> anyhow::Result<impl Stream<Item = i32>> {
    use tokio::signal::unix;
    let sigint = unix::signal(unix::SignalKind::interrupt())?;
    let sigterm = unix::signal(unix::SignalKind::terminate())?;

    Ok(futures::stream::unfold(
        (sigint, sigterm),
        |(mut sigint, mut sigterm)| async move {
            let signal_num = tokio::select! {
                _ = sigint.recv() => libc::SIGINT,
                _ = sigterm.recv() => libc::SIGTERM,
            };
            Some((signal_num, (sigint, sigterm)))
        },
    ))
}

fn log_signal(signal_num: i32) {
    match Signal::try_from(signal_num) {
        Ok(signal) => debug!("Got signal {:?}({})", signal, signal_num),
        Err(e) => warn!("Unexpected signal {}: {:?})", signal_num, e),
    }
}

impl SignalHandler {
    pub fn infer() -> anyhow::Result<SignalHandler> {
        Ok(SignalHandler::new(get_signal()?))
    }

    /// Construct a new SignalHandler that forwards every item yielded by
    /// `signal_source` to all subscribers.
    pub fn new(signal_source: impl Stream<Item = i32> + Send + 'static) -> Self {
        let (tx, _) = broadcast::channel(SIGNAL_CHANNEL_SIZE);
        let worker_tx = tx.clone();
        tokio_spawn!("signal-handler", async move {
            let mut signal_source = Box::pin(signal_source);
            while let Some(signal_num) = signal_source.next().await {
                log_signal(signal_num);
                // We don't care if nobody is subscribed at the moment.
                let _ = worker_tx.send(signal_num);
            }
        });

        Self { tx }
    }

    /// Subscribe to all signals received from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<i32> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// Turn a channel into a signal stream so tests can deliver signals on demand.
    fn signal_stream(mut rx: mpsc::UnboundedReceiver<i32>) -> impl Stream<Item = i32> {
        futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
    }

    #[tokio::test]
    async fn every_signal_is_delivered_to_subscribers() {
        let (signal_tx, signal_rx) = mpsc::unbounded_channel();
        let handler = SignalHandler::new(signal_stream(signal_rx));
        let mut signals = handler.subscribe();

        // The handler must keep watching the signal source after the first
        // signal, otherwise a second Ctrl-C would be silently dropped and the
        // shutdown could never be escalated into a forced kill.
        for expected in [libc::SIGINT, libc::SIGINT, libc::SIGTERM] {
            signal_tx.send(expected).unwrap();
            let signal_num = tokio::time::timeout(Duration::from_secs(5), signals.recv())
                .await
                .expect("signal was not delivered")
                .expect("signal channel closed");
            assert_eq!(signal_num, expected);
        }
    }

    #[tokio::test]
    async fn late_subscribers_receive_subsequent_signals() {
        let (signal_tx, signal_rx) = mpsc::unbounded_channel();
        let handler = SignalHandler::new(signal_stream(signal_rx));

        // A subscriber created via a clone shares the same signal feed.
        let mut signals = handler.clone().subscribe();

        signal_tx.send(libc::SIGINT).unwrap();
        let signal_num = tokio::time::timeout(Duration::from_secs(5), signals.recv())
            .await
            .expect("signal was not delivered")
            .expect("signal channel closed");
        assert_eq!(signal_num, libc::SIGINT);
    }
}
