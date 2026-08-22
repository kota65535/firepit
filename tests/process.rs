use firepit::process::{ChildExit, Command, ProcessManager};
use rstest::rstest;
use std::sync::Once;
use std::time::{Duration, Instant};
use tracing_subscriber::EnvFilter;

static INIT: Once = Once::new();

pub fn setup() {
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("debug"))
            .with_ansi(false)
            .init();
    });
}

/// A process that ignores `SIGINT` must survive exactly the configured grace
/// period before it is forcibly killed, in both PTY and non-PTY mode.
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn test_stop_timeout_is_honored(#[case] use_pty: bool) {
    setup();
    let stop_timeout = Duration::from_secs(2);
    let manager = ProcessManager::new(use_pty);
    let cmd = Command::new("bash")
        .with_args(vec![
            String::from("-c"),
            String::from("trap '' INT; echo ready; sleep 60"),
        ])
        .with_label("ignores-sigint")
        .to_owned();

    let mut child = manager.spawn(cmd, stop_timeout).await.unwrap().unwrap();

    // Give the trap a chance to be installed before stopping
    tokio::time::sleep(Duration::from_millis(500)).await;

    let start = Instant::now();
    let exit = child.stop().await;
    let elapsed = start.elapsed();

    assert_eq!(exit, Some(ChildExit::Killed));
    // SIGINT is ignored, so the child cannot exit before the grace period elapses
    assert!(
        elapsed >= stop_timeout,
        "stopped after {elapsed:?}, expected at least {stop_timeout:?}"
    );
    // ...and it must not linger far beyond it
    assert!(
        elapsed < stop_timeout + Duration::from_secs(3),
        "stopped after {elapsed:?}, expected close to {stop_timeout:?}"
    );
}

/// A process that exits on `SIGINT` must not wait out the grace period.
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn test_stop_returns_early_when_sigint_is_handled(#[case] use_pty: bool) {
    setup();
    let manager = ProcessManager::new(use_pty);
    let cmd = Command::new("bash")
        .with_args(vec![String::from("-c"), String::from("echo ready; sleep 60")])
        .with_label("handles-sigint")
        .to_owned();

    let mut child = manager.spawn(cmd, Duration::from_secs(30)).await.unwrap().unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    let start = Instant::now();
    let exit = child.stop().await;
    let elapsed = start.elapsed();

    assert_eq!(exit, Some(ChildExit::Killed));
    assert!(
        elapsed < Duration::from_secs(5),
        "stopped after {elapsed:?}, expected it to exit on SIGINT without waiting out the grace period"
    );
}
