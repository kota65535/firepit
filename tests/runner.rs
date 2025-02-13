use firepit::runner::TaskRunner;
use std::collections::HashMap;
use std::fs::File;
use std::future::Future;
use std::io::Write;
use std::path;
use std::path::Path;

use firepit::config::ProjectConfig;
use firepit::event::{Event, EventSender};
use firepit::project::Workspace;
use firepit::watcher::FileWatcher;
use std::sync::Once;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
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

#[tokio::test]
async fn test_basic() {
    setup();
    let path = Path::new("tests/fixtures/runner/01_basic");
    let tasks = vec!["foo".to_string()];

    let mut expected = HashMap::new();
    expected.insert(String::from("#foo"), String::from("Finished: Success"));
    expected.insert(String::from("#bar"), String::from("Finished: Success"));
    expected.insert(String::from("#baz"), String::from("Finished: Success"));

    run_task(path, tasks, expected, None, None, None).await;
}

#[tokio::test]
async fn test_dependency_failure() {
    setup();
    let path = Path::new("tests/fixtures/runner/02_fail");
    let tasks = vec!["foo".to_string()];

    let mut expected = HashMap::new();
    expected.insert(String::from("#foo"), String::from("Finished: Dependencies failed"));
    expected.insert(String::from("#bar"), String::from("Finished: Dependencies failed"));
    expected.insert(String::from("#baz"), String::from("Finished: Failure with code 1"));

    run_task(path, tasks, expected, None, None, None).await;
}

#[tokio::test]
async fn test_service() {
    setup();
    let path = Path::new("tests/fixtures/runner/11_service");
    let tasks = vec!["foo".to_string()];

    let mut expected = HashMap::new();
    expected.insert(String::from("#foo"), String::from("Finished: Success"));
    expected.insert(String::from("#bar"), String::from("Finished: Success"));
    expected.insert(String::from("#baz"), String::from("Ready"));
    expected.insert(String::from("#qux"), String::from("Ready"));

    run_task(path, tasks, expected, None, None, Some(20)).await;
}

#[tokio::test]
async fn test_bad_service() {
    setup();
    let path = Path::new("tests/fixtures/runner/12_bad_service");
    let tasks = vec!["foo".to_string()];

    let mut expected = HashMap::new();
    expected.insert(String::from("#foo"), String::from("Finished: Dependencies failed"));
    expected.insert(String::from("#bar"), String::from("Finished: Service not ready"));
    expected.insert(String::from("#baz"), String::from("Finished: Service not ready"));

    run_task(path, tasks, expected, None, None, Some(20)).await;
}

#[tokio::test]
async fn test_watch() {
    setup();
    let path = Path::new("tests/fixtures/runner/21_watch");
    let tasks = vec!["foo".to_string()];

    let mut status_expected = HashMap::new();
    status_expected.insert(String::from("#foo"), String::from("Finished: Success"));
    status_expected.insert(String::from("#bar"), String::from("Finished: Success"));
    status_expected.insert(String::from("#baz"), String::from("Finished: Success"));

    let mut runs_expected = HashMap::new();
    runs_expected.insert(String::from("#foo"), 1);
    runs_expected.insert(String::from("#bar"), 1);
    runs_expected.insert(String::from("#baz"), 0);

    run_task_with_watch(&path, tasks, status_expected, None, Some(runs_expected), None, async {
        File::create("tests/fixtures/runner/21_watch/bar.txt").ok();
    })
    .await;
}

#[tokio::test]
async fn test_watch_service() {
    setup();
    let path = Path::new("tests/fixtures/runner/22_watch_service");
    let tasks = vec!["foo".to_string()];

    let mut status_expected = HashMap::new();
    status_expected.insert(String::from("#foo"), String::from("Finished: Success"));
    status_expected.insert(String::from("#bar"), String::from("Ready"));

    let mut runs_expected = HashMap::new();
    runs_expected.insert(String::from("#foo"), 1);
    runs_expected.insert(String::from("#bar"), 1);

    {
        let mut f = File::create("tests/fixtures/runner/22_watch_service/bar.txt").unwrap();
        f.write_all(b"12001").unwrap();
    }
    tokio::time::sleep(Duration::from_secs(1)).await;

    run_task_with_watch(
        &path,
        tasks,
        status_expected,
        None,
        Some(runs_expected),
        Some(20),
        async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let mut f = File::create("tests/fixtures/runner/22_watch_service/bar.txt").unwrap();
            f.write_all(b"12000").unwrap();
        },
    )
    .await;
}

#[tokio::test]
async fn test_up_to_date() {
    setup();
    let path = path::absolute(Path::new("tests/fixtures/runner/31_up_to_date")).unwrap();
    let tasks = vec!["foo".to_string()];

    File::create(path.join("foo.out")).ok();

    let mut expected = HashMap::new();
    expected.insert(String::from("#foo"), String::from("Finished: Up to date"));
    expected.insert(String::from("#bar"), String::from("Finished: Success"));
    expected.insert(String::from("#baz"), String::from("Finished: Success"));

    run_task(&path, tasks, expected, None, None, None).await;
}

async fn run_task(
    path: &Path,
    tasks: Vec<String>,
    status_expected: HashMap<String, String>,
    restarts_expected: Option<HashMap<String, u64>>,
    runs_expected: Option<HashMap<String, u64>>,
    timeout_seconds: Option<u64>,
) {
    let path = path::absolute(path).unwrap();
    let ws = Workspace::new(&ProjectConfig::new(&path).unwrap(), &HashMap::new()).unwrap();

    // Create runner
    let mut runner = TaskRunner::new(&ws, &tasks, Path::new(&path)).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let sender = EventSender::new(tx);

    let manager = runner.manager.clone();
    let cancel_tx = runner.cancel_tx.clone();

    // Start runner
    let runner_fut = tokio::spawn(async move {
        runner.start(sender).await.ok();
    });

    // Handle events and assert task statuses
    handle_events(
        rx,
        cancel_tx,
        status_expected,
        restarts_expected,
        runs_expected,
        timeout_seconds,
    )
    .await
    .unwrap();

    // Stop processes to forcing runner to finish
    manager.stop().await;
    runner_fut.await.ok();
}

async fn run_task_with_watch<F>(
    path: &Path,
    tasks: Vec<String>,
    status_expected: HashMap<String, String>,
    restarts_expected: Option<HashMap<String, u64>>,
    runs_expected: Option<HashMap<String, u64>>,
    timeout_seconds: Option<u64>,
    f: F,
) where
    F: Future<Output = ()> + Send + 'static,
{
    let path = path::absolute(path).unwrap();
    let ws = Workspace::new(&ProjectConfig::new(&path).unwrap(), &HashMap::new()).unwrap();

    // Create file watcher
    let mut file_watcher = FileWatcher::new().unwrap();
    let fwh = file_watcher.run(&path, Duration::from_millis(500)).unwrap();

    // Create runner
    let mut runner = TaskRunner::new(&ws, &tasks, Path::new(&path)).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let runner_app_tx = EventSender::new(tx);

    // Create watch runner
    let mut watch_runner = runner.clone();
    let watch_app_tx = runner_app_tx.clone();

    let manager = runner.manager.clone();
    let cancel_tx = runner.cancel_tx.clone();

    // Start runner
    let runner_fut = tokio::spawn(async move { runner.start(runner_app_tx).await.ok() });

    // Start watch runner
    let watcher_fut = tokio::spawn(async move { watch_runner.watch(fwh.rx, watch_app_tx).await.ok() });

    // Do something in this closure, ex: create or update files
    tokio::spawn(async move { f.await });

    // Handle events and assert task statuses
    handle_events(
        rx,
        cancel_tx,
        status_expected,
        restarts_expected,
        runs_expected,
        timeout_seconds,
    )
    .await
    .unwrap();

    // Stop processes to forcing runner to finish
    manager.stop().await;
    runner_fut.await.ok();
    watcher_fut.await.ok();
}

const DEFAULT_TEST_TIMEOUT_SECONDS: u64 = 20;

fn handle_events(
    mut rx: UnboundedReceiver<Event>,
    cancel_tx: watch::Sender<()>,
    status_expected: HashMap<String, String>,
    restarts_expected: Option<HashMap<String, u64>>,
    runs_expected: Option<HashMap<String, u64>>,
    timeout_seconds: Option<u64>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut status = HashMap::new();
        let mut restarts = HashMap::new();
        let mut runs = HashMap::new();

        let (timeout_tx, mut timeout_rx) = watch::channel(());
        let timeout_seconds = timeout_seconds.unwrap_or(DEFAULT_TEST_TIMEOUT_SECONDS);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(timeout_seconds)).await;
            timeout_tx.send(()).unwrap();
        });

        loop {
            tokio::select! {
                _ = timeout_rx.changed() => {
                    break
                }
                Some(event) = rx.recv() => {
                    match event {
                        Event::StartTask { task, pid, restart, run } => {
                            restarts.insert(task.clone(), restart);
                            runs.insert(task.clone(), run);
                        }
                        Event::ReadyTask { task } => {
                            status.insert(task, String::from("Ready"));
                        }
                        Event::FinishTask { task, result } => {
                            status.insert(task, format!("Finished: {}", result));
                        }
                        Event::Stop(callback) => {
                            callback.send(()).ok();
                            break;
                        }
                        _ => {}
                    }
                }
            }
            if status_expected == status
                && match restarts_expected.clone() {
                    Some(expected) => expected == restarts,
                    None => true,
                }
                && match runs_expected.clone() {
                    Some(expected) => expected == runs,
                    None => true,
                }
            {
                break;
            }
        }
        cancel_tx.send(()).unwrap();
        assert_eq!(status_expected, status);
        if let Some(restarts_expected) = restarts_expected {
            assert_eq!(restarts_expected, restarts);
        }
        if let Some(runs_expected) = runs_expected {
            assert_eq!(runs_expected, runs);
        }
    })
}
