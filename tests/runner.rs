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
use rstest::rstest;
use std::sync::{LazyLock, Once};
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

static BASE_PATH: LazyLock<&Path> = LazyLock::new(|| Path::new("tests/fixtures/runner"));

#[tokio::test]
async fn test_basic_single() {
    setup();
    let path = BASE_PATH.join("basic_single");
    let tasks = vec![String::from("foo")];

    let mut statuses = HashMap::new();
    statuses.insert(String::from("#foo"), String::from("Finished: Success"));
    statuses.insert(String::from("#bar"), String::from("Finished: Success"));
    statuses.insert(String::from("#baz"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#foo"), String::from("foo"));
    outputs.insert(String::from("#bar"), String::from("bar"));
    outputs.insert(String::from("#baz"), String::from("baz"));

    run_task(&path, tasks, statuses, Some(outputs), None, None, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_basic_failure() {
    setup();
    let path = BASE_PATH.join("basic_failure");
    let tasks = vec![String::from("foo")];

    let mut statuses = HashMap::new();
    statuses.insert(String::from("#foo"), String::from("Finished: BadDeps"));
    statuses.insert(String::from("#bar"), String::from("Finished: BadDeps"));
    statuses.insert(String::from("#baz"), String::from("Finished: Failure(1)"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#baz"), String::from("baz"));

    run_task(&path, tasks, statuses, Some(outputs), None, None, None)
        .await
        .unwrap();
}

#[tokio::test]
#[rstest]
#[case("")]
#[case("foo")]
async fn test_basic_multi(#[case] dir: &str) {
    setup();
    let path = BASE_PATH.join("basic_multi").join(dir);

    // With qualified task name
    let tasks = vec![String::from("#baz")];

    let mut statuses = HashMap::new();
    statuses.insert(String::from("foo#foo"), String::from("Finished: Success"));
    statuses.insert(String::from("bar#bar"), String::from("Finished: Success"));
    statuses.insert(String::from("#baz"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("foo#foo"), String::from("foo"));
    outputs.insert(String::from("bar#bar"), String::from("bar"));
    outputs.insert(String::from("#baz"), String::from("baz"));

    run_task(&path, tasks, statuses, Some(outputs), None, None, None)
        .await
        .unwrap();

    // With unqualified task name
    let tasks = vec![String::from("foo")];

    let mut statuses = HashMap::new();
    statuses.insert(String::from("foo#foo"), String::from("Finished: Success"));
    statuses.insert(String::from("bar#bar"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("foo#foo"), String::from("foo"));
    outputs.insert(String::from("bar#bar"), String::from("bar"));

    run_task(&path, tasks, statuses, Some(outputs), None, None, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_vars() {
    setup();

    let path = BASE_PATH.join("vars");
    let tasks = vec![String::from("foo")];

    let mut stats = HashMap::new();
    stats.insert(String::from("#foo"), String::from("Finished: Success"));
    stats.insert(String::from("#bar"), String::from("Finished: Success"));
    stats.insert(String::from("#baz"), String::from("Finished: Success"));
    stats.insert(String::from("#qux"), String::from("Ready"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#foo"), String::from("foo 1"));
    outputs.insert(String::from("#bar"), String::from("bar 2"));
    outputs.insert(String::from("#baz"), String::from("baz 3"));
    outputs.insert(String::from("#qux"), String::from("qux 12001"));

    run_task(&path, tasks, stats, Some(outputs), None, None, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_vars_multi() {
    setup();
    let path = BASE_PATH.join("vars_multi");
    let tasks = vec![String::from("#baz")];

    let mut stats = HashMap::new();
    stats.insert(String::from("foo#foo"), String::from("Finished: Success"));
    stats.insert(String::from("bar#bar"), String::from("Finished: Success"));
    stats.insert(String::from("#baz"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("foo#foo"), String::from("foo 10root"));
    outputs.insert(String::from("bar#bar"), String::from("bar 2foo"));
    outputs.insert(String::from("#baz"), String::from("baz 3bar"));

    run_task(&path, tasks, stats, Some(outputs), None, None, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_vars_dep() {
    setup();

    let path = BASE_PATH.join("vars_dep");
    let tasks = vec![String::from("foo")];

    let mut stats = HashMap::new();
    stats.insert(String::from("#foo"), String::from("Finished: Success"));
    stats.insert(String::from("#bar"), String::from("Finished: Success"));
    stats.insert(String::from("#baz"), String::from("Finished: Success"));
    stats.insert(String::from("#baz-1"), String::from("Finished: Success"));
    stats.insert(String::from("#qux"), String::from("Finished: Success"));
    stats.insert(String::from("#quux-1"), String::from("Finished: Success"));
    stats.insert(String::from("#quux-2"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#foo"), String::from("foo 1"));
    outputs.insert(String::from("#bar"), String::from("bar 2"));
    outputs.insert(String::from("#baz"), String::from("baz 3"));
    outputs.insert(String::from("#baz-1"), String::from("baz 4"));
    outputs.insert(String::from("#qux"), String::from("qux 4"));
    outputs.insert(String::from("#quux-1"), String::from("quux 3"));
    outputs.insert(String::from("#quux-2"), String::from("quux 4"));

    run_task(&path, tasks, stats, Some(outputs), None, None, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_cyclic() {
    setup();
    let path = BASE_PATH.join("cyclic");
    let tasks = vec![String::from("foo")];

    let err = run_task(&path, tasks, HashMap::new(), None, None, None, None)
        .await
        .expect_err("should fail");
    assert!(err.to_string().contains("cyclic dependency"));
}

#[tokio::test]
async fn test_service() {
    setup();
    let path = BASE_PATH.join("service");
    let tasks = vec![String::from("foo")];

    let mut stats = HashMap::new();
    stats.insert(String::from("#foo"), String::from("Finished: Success"));
    stats.insert(String::from("#bar"), String::from("Finished: Success"));
    stats.insert(String::from("#baz"), String::from("Ready"));
    stats.insert(String::from("#qux"), String::from("Ready"));

    run_task(&path, tasks, stats, None, None, None, Some(20)).await.unwrap();
}

#[tokio::test]
async fn test_service_failure() {
    setup();
    let path = BASE_PATH.join("service_failure");
    let tasks = vec![String::from("foo")];

    let mut stats = HashMap::new();
    stats.insert(String::from("#foo"), String::from("Finished: BadDeps"));
    stats.insert(String::from("#bar"), String::from("Finished: NotReady"));
    stats.insert(String::from("#baz"), String::from("Finished: NotReady"));

    run_task(&path, tasks, stats, None, None, None, Some(20)).await.unwrap();
}

#[tokio::test]
async fn test_watch() {
    setup();
    let path = BASE_PATH.join("watch");
    let tasks = vec![String::from("foo")];

    let mut stats = HashMap::new();
    stats.insert(String::from("#foo"), String::from("Finished: Success"));
    stats.insert(String::from("#bar"), String::from("Finished: Success"));
    stats.insert(String::from("#baz"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#foo"), String::from("foofoo"));
    outputs.insert(String::from("#bar"), String::from("barbar"));
    outputs.insert(String::from("#baz"), String::from("baz"));

    let mut runs = HashMap::new();
    runs.insert(String::from("#foo"), 1);
    runs.insert(String::from("#bar"), 1);
    runs.insert(String::from("#baz"), 0);

    run_task_with_watch(&path, tasks, stats, Some(outputs), None, Some(runs), None, async {
        File::create(BASE_PATH.join("watch").join("bar.txt")).ok();
    })
    .await;
}

#[tokio::test]
async fn test_watch_service() {
    setup();
    let path = BASE_PATH.join("watch_service");
    let tasks = vec![String::from("foo")];

    let mut stats = HashMap::new();
    stats.insert(String::from("#foo"), String::from("Finished: Success"));
    stats.insert(String::from("#bar"), String::from("Ready"));

    let mut runs = HashMap::new();
    runs.insert(String::from("#foo"), 1);
    runs.insert(String::from("#bar"), 1);

    {
        let mut f = File::create(path.join("bar.txt")).unwrap();
        f.write_all(b"12001").unwrap();
    }
    tokio::time::sleep(Duration::from_secs(1)).await;

    run_task_with_watch(&path, tasks, stats, None, None, Some(runs), Some(20), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut f = File::create(BASE_PATH.join("watch_service").join("bar.txt")).unwrap();
        f.write_all(b"12000").unwrap();
    })
    .await;
}

#[tokio::test]
async fn test_up_to_date() {
    setup();
    let path = BASE_PATH.join("up_to_date");
    let tasks = vec![String::from("foo")];

    File::create(path.join("foo.out")).ok();

    let mut stats = HashMap::new();
    stats.insert(String::from("#foo"), String::from("Finished: UpToDate"));
    stats.insert(String::from("#bar"), String::from("Finished: Success"));
    stats.insert(String::from("#baz"), String::from("Finished: Success"));

    run_task(&path, tasks, stats, None, None, None, None).await.unwrap();
}

async fn run_task(
    path: &Path,
    tasks: Vec<String>,
    status_expected: HashMap<String, String>,
    outputs_expected: Option<HashMap<String, String>>,
    restarts_expected: Option<HashMap<String, u64>>,
    runs_expected: Option<HashMap<String, u64>>,
    timeout_seconds: Option<u64>,
) -> anyhow::Result<()> {
    let path = path::absolute(path)?;
    let (root, children) = ProjectConfig::new_multi(&path)?;
    let ws = Workspace::new(&root, &children)?;

    // Create runner
    let mut runner = TaskRunner::new(&ws, &tasks, Path::new(&path))?;
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
        outputs_expected,
        restarts_expected,
        runs_expected,
        timeout_seconds,
    )
    .await?;

    // Stop processes to forcing runner to finish
    manager.stop().await;
    runner_fut.await?;
    Ok(())
}

async fn run_task_with_watch<F>(
    path: &Path,
    tasks: Vec<String>,
    status_expected: HashMap<String, String>,
    outputs_expected: Option<HashMap<String, String>>,
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
        outputs_expected,
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

const DEFAULT_TEST_TIMEOUT_SECONDS: u64 = 10;

fn handle_events(
    mut rx: UnboundedReceiver<Event>,
    cancel_tx: watch::Sender<()>,
    statuses_expected: HashMap<String, String>,
    outputs_expected: Option<HashMap<String, String>>,
    restarts_expected: Option<HashMap<String, u64>>,
    runs_expected: Option<HashMap<String, u64>>,
    timeout_seconds: Option<u64>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut statuses = HashMap::new();
        let mut outputs = HashMap::<String, String>::new();
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
                        Event::StartTask { task, pid: _, restart, max_restart: _, reload } => {
                            restarts.insert(task.clone(), restart);
                            runs.insert(task.clone(), reload);
                        }
                        Event::ReadyTask { task } => {
                            statuses.insert(task, String::from("Ready"));
                        }
                        Event::FinishTask { task, result } => {
                            statuses.insert(task, format!("Finished: {:?}", result));
                        }
                        Event::Stop(callback) => {
                            callback.send(()).ok();
                            break;
                        }
                        Event::TaskOutput { task, output } => {
                            let str = String::from_utf8(output.clone()).unwrap();
                            match outputs.get(&task) {
                                Some(t) => {
                                    let s = format!("{}{}", t, str);
                                    outputs.insert(task.clone(), normalize_str(&s));
                                }
                                None => {
                                    outputs.insert(task.clone(), normalize_str(&str));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            if statuses_expected == statuses
                && match outputs_expected.clone() {
                    Some(expected) => expected == outputs,
                    None => true,
                }
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
        assert_eq!(statuses_expected, statuses);
        if let Some(outputs_expected) = outputs_expected {
            assert_eq!(outputs_expected, outputs);
        }
        if let Some(restarts_expected) = restarts_expected {
            assert_eq!(restarts_expected, restarts);
        }
        if let Some(runs_expected) = runs_expected {
            assert_eq!(runs_expected, runs);
        }
    })
}

fn normalize_str(s: &str) -> String {
    s.trim().chars().filter(|&c| !c.is_control()).collect()
}
