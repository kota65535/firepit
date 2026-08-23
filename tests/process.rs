use firepit::log::OutputCollector;
use firepit::process::{ChildExit, Command, ProcessManager};
use rstest::rstest;
use std::path::Path;
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

/// Killing a child must kill its descendants too. They inherit the output
/// pipes, so a surviving descendant keeps the pipes open and makes waiting for
/// the child output hang forever.
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn test_stop_kills_descendants(#[case] use_pty: bool) {
    setup();
    let stop_timeout = Duration::from_millis(500);
    let pid_file = std::env::temp_dir().join(format!("firepit-test-descendants-{use_pty}.pid"));
    std::fs::remove_file(&pid_file).ok();

    let manager = ProcessManager::new(use_pty);
    let cmd = Command::new("bash")
        .with_args(vec![
            String::from("-c"),
            // The `sleep` is a grandchild of firepit and ignores nothing, but it
            // is never signaled unless the whole process group is killed
            format!("trap '' INT; sleep 60 & echo $! > {}; wait", pid_file.to_string_lossy()),
        ])
        .with_label("leaves-grandchild")
        .to_owned();

    let mut child = manager.spawn(cmd, stop_timeout).await.unwrap().unwrap();
    let grandchild_pid = wait_for_pid(&pid_file).await;

    let exit = child.stop().await;
    assert_eq!(exit, Some(ChildExit::Killed));

    // Waiting for the output must not hang, which it does while a descendant
    // holds the output pipes open
    let collector = OutputCollector::new();
    let waited = tokio::time::timeout(
        Duration::from_secs(5),
        child.wait_with_piped_outputs(collector.clone(), collector.clone()),
    )
    .await;
    assert!(waited.is_ok(), "waiting for the child output timed out");

    let killed = tokio::time::timeout(Duration::from_secs(5), async {
        while is_alive(grandchild_pid) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    if killed.is_err() {
        // Do not leak the process into the rest of the test run
        unsafe { libc::kill(grandchild_pid, libc::SIGKILL) };
        panic!("grandchild process {grandchild_pid} survived the kill");
    }

    std::fs::remove_file(&pid_file).ok();
}

/// Signal 0 sends nothing and only checks that the process still exists.
fn is_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Waits until the given file contains a PID and returns it.
async fn wait_for_pid(pid_file: &Path) -> i32 {
    for _ in 0..50 {
        if let Ok(content) = std::fs::read_to_string(pid_file) {
            if let Ok(pid) = content.trim().parse::<i32>() {
                return pid;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("PID file was not written: {pid_file:?}");
}
