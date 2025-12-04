use firepit::runner::TaskRunner;
use std::collections::HashMap;
use std::fs::File;
use std::future::Future;
use std::io::Write;
use std::path::Path;
use std::{env, path};

use firepit::app::command::{AppCommand, AppCommandChannel};
use firepit::config::ProjectConfig;
use firepit::project::Workspace;
use firepit::runner::command::RunnerCommandChannel;
use indexmap::IndexMap;
use rstest::rstest;
use serde_json::Value;
use std::sync::{LazyLock, Once};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::info;
use tracing_subscriber::EnvFilter;

static INIT: Once = Once::new();

pub fn setup() {
    INIT.call_once(|| {
        // init_logger(
        //     &LogConfig {
        //         level: "debug".to_string(),
        //         file: Some("a.log".to_string()),
        //     },
        //     true,
        // )
        // .unwrap();
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

    run_task(&path, tasks, statuses, Some(outputs), false).await.unwrap();
}

#[tokio::test]
async fn test_basic_empty() {
    setup();
    let path = BASE_PATH.join("basic_empty");
    let tasks = vec![String::from("foo")];

    let mut statuses = HashMap::new();
    statuses.insert(String::from("#foo"), String::from("Finished: Success"));
    statuses.insert(String::from("#bar"), String::from("Finished: Success"));
    statuses.insert(String::from("#baz"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#bar"), String::from("bar"));
    outputs.insert(String::from("#baz"), String::from("baz"));

    run_task(&path, tasks, statuses, Some(outputs), false).await.unwrap();
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

    run_task(&path, tasks, statuses, Some(outputs), false).await.unwrap();
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

    run_task(&path, tasks, statuses, Some(outputs), false).await.unwrap();

    // With unqualified task name
    let tasks = vec![String::from("foo")];

    let mut statuses = HashMap::new();
    statuses.insert(String::from("foo#foo"), String::from("Finished: Success"));
    statuses.insert(String::from("bar#bar"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("foo#foo"), String::from("foo"));
    outputs.insert(String::from("bar#bar"), String::from("bar"));

    run_task(&path, tasks, statuses, Some(outputs), false).await.unwrap();
}

#[tokio::test]
async fn test_vars() {
    setup();

    let path = BASE_PATH.join("vars");
    let tasks = vec![
        String::from("number"),
        String::from("string"),
        String::from("boolean"),
        String::from("array"),
        String::from("map"),
    ];

    let mut stats = HashMap::new();
    stats.insert(String::from("#number"), String::from("Finished: Success"));
    stats.insert(String::from("#string"), String::from("Finished: Success"));
    stats.insert(String::from("#boolean"), String::from("Finished: Success"));
    stats.insert(String::from("#array"), String::from("Finished: Success"));
    stats.insert(String::from("#map"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#number"), String::from("1\nok"));
    outputs.insert(String::from("#string"), String::from("bar\nok"));
    outputs.insert(String::from("#boolean"), String::from("true\nok"));
    outputs.insert(String::from("#array"), String::from("1,2\nok"));
    outputs.insert(String::from("#map"), String::from("1,2\nok"));

    let vars = IndexMap::from([("offset".to_string(), Value::from(10))]);
    run_task_with_vars(&path, tasks, stats, Some(outputs), vars, false)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_vars_from_cli() {
    setup();

    let path = BASE_PATH.join("vars_cli");
    let tasks = vec![
        String::from("number"),
        String::from("string"),
        String::from("string2"),
        String::from("boolean"),
    ];

    let mut stats = HashMap::new();
    stats.insert(String::from("#number"), String::from("Finished: Success"));
    stats.insert(String::from("#string"), String::from("Finished: Success"));
    stats.insert(String::from("#string2"), String::from("Finished: Success"));
    stats.insert(String::from("#boolean"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#number"), String::from("2\nok"));
    outputs.insert(String::from("#string"), String::from("baz\nok"));
    outputs.insert(String::from("#string2"), String::from("piyo\nok"));
    outputs.insert(String::from("#boolean"), String::from("false\nok"));

    let vars = IndexMap::from([
        ("cli_number".to_string(), Value::from(1)),
        ("string".to_string(), Value::from("baz")),
        ("string2".to_string(), Value::from("piyo")),
        ("boolean".to_string(), Value::from(false)),
    ]);
    run_task_with_vars(&path, tasks, stats, Some(outputs), vars, false)
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
    outputs.insert(String::from("foo#foo"), String::from("foo 10\nroot"));
    outputs.insert(String::from("bar#bar"), String::from("bar 2\nfoo"));
    outputs.insert(String::from("#baz"), String::from("baz 3\nbar"));

    run_task(&path, tasks, stats, Some(outputs), false).await.unwrap();
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
    stats.insert(String::from("#qux-1"), String::from("Finished: Success"));
    stats.insert(String::from("#qux-2"), String::from("Finished: Success"));
    stats.insert(String::from("#quux-1"), String::from("Finished: Success"));
    stats.insert(String::from("#quux-2"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#foo"), String::from("foo 1"));
    outputs.insert(String::from("#bar"), String::from("bar 2"));
    outputs.insert(String::from("#baz"), String::from("baz 3"));
    outputs.insert(String::from("#baz-1"), String::from("baz 4"));
    outputs.insert(String::from("#qux-1"), String::from("qux 4 6"));
    outputs.insert(String::from("#qux-2"), String::from("qux 5 5"));
    outputs.insert(String::from("#quux-1"), String::from("quux 3"));
    outputs.insert(String::from("#quux-2"), String::from("quux 4"));

    run_task(&path, tasks, stats, Some(outputs), false).await.unwrap();
}

#[tokio::test]
async fn test_vars_dep_multi() {
    setup();

    let path = BASE_PATH.join("vars_dep_multi");
    let tasks = vec![String::from("foo")];

    let mut stats = HashMap::new();
    stats.insert(String::from("p1#foo"), String::from("Finished: Success"));
    stats.insert(String::from("p1#bar-1"), String::from("Finished: Success"));
    stats.insert(String::from("p2#baz-1"), String::from("Finished: Success"));
    stats.insert(String::from("p2#baz-2"), String::from("Finished: Success"));
    stats.insert(String::from("p2#qux"), String::from("Finished: Success"));
    stats.insert(String::from("p2#qux-1"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("p1#foo"), String::from("foo 2"));
    outputs.insert(String::from("p1#bar-1"), String::from("bar 3"));
    outputs.insert(String::from("p2#baz-1"), String::from("baz 4"));
    outputs.insert(String::from("p2#baz-2"), String::from("baz 5"));
    outputs.insert(String::from("p2#qux"), String::from("qux 5"));
    outputs.insert(String::from("p2#qux-1"), String::from("qux 4"));

    let vars = IndexMap::from([("A".to_string(), Value::from(2))]);

    run_task_with_vars(&path, tasks, stats, Some(outputs), vars, false)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_vars_dep_same() {
    setup();

    let path = BASE_PATH.join("vars_dep_same");
    let tasks = vec![String::from("foo"), String::from("bar")];

    let mut stats = HashMap::new();
    stats.insert(String::from("#foo"), String::from("Finished: Success"));
    stats.insert(String::from("#bar"), String::from("Finished: Success"));
    stats.insert(String::from("#baz-1"), String::from("Finished: Success"));
    stats.insert(String::from("#qux-1"), String::from("Finished: Success"));
    stats.insert(String::from("#qux-2"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#foo"), String::from("foo"));
    outputs.insert(String::from("#bar"), String::from("bar 2"));
    outputs.insert(String::from("#baz-1"), String::from("baz 4"));
    outputs.insert(String::from("#qux-1"), String::from("qux 6"));
    outputs.insert(String::from("#qux-2"), String::from("qux 5"));

    run_task(&path, tasks, stats, Some(outputs), false).await.unwrap();
}

#[tokio::test]
async fn test_cyclic() {
    setup();
    let path = BASE_PATH.join("cyclic");
    let tasks = vec![String::from("foo")];

    let err = run_task(&path, tasks, HashMap::new(), None, false)
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
    stats.insert(String::from("#bar"), String::from("Ready"));
    stats.insert(String::from("#baz"), String::from("Ready"));
    stats.insert(String::from("#qux"), String::from("Ready"));

    run_task(&path, tasks, stats, None, false).await.unwrap();
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

    run_task(&path, tasks, stats, None, false).await.unwrap();
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
    stats.insert(String::from("#qux"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#foo"), String::from("foo\nfoo"));
    outputs.insert(String::from("#bar"), String::from("bar\nbar"));
    outputs.insert(String::from("#baz"), String::from("baz"));
    outputs.insert(String::from("#qux"), String::from("qux\nqux"));

    let mut runs = HashMap::new();
    runs.insert(String::from("#foo"), 1);
    runs.insert(String::from("#bar"), 1);
    runs.insert(String::from("#baz"), 0);
    runs.insert(String::from("#qux"), 1);

    run_task_with_watch(
        &path,
        tasks,
        stats,
        Some(outputs),
        None,
        Some(runs),
        None,
        false,
        async {
            info!("Creating files");
            let mut f = File::create(BASE_PATH.join("watch").join("bar.txt")).unwrap();
            f.write_all(b"bar").unwrap();
            let mut f = File::create(BASE_PATH.join("watch").join("qux.txt")).unwrap();
            f.write_all(b"qux").unwrap();
        },
    )
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

    run_task_with_watch(&path, tasks, stats, None, None, Some(runs), Some(20), false, async {
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

    run_task(&path, tasks, stats, None, false).await.unwrap();
}

#[tokio::test]
async fn test_env_precedence() {
    env::set_var("key1", "os1");
    env::set_var("key2", "os2");
    env::set_var("key3", "os3");

    setup();

    let path = BASE_PATH.join("env");
    let tasks = vec![String::from("foo")];

    let mut stats = HashMap::new();
    stats.insert(String::from("#foo"), String::from("Finished: Success"));

    let mut outputs = HashMap::new();
    outputs.insert(String::from("#foo"), String::from("file1 env2 os3"));

    run_task(&path, tasks, stats, Some(outputs), false).await.unwrap();
}

async fn run_task(
    path: &Path,
    tasks: Vec<String>,
    status_expected: HashMap<String, String>,
    outputs_expected: Option<HashMap<String, String>>,
    force: bool,
) -> anyhow::Result<()> {
    run_task_inner(
        path,
        tasks,
        status_expected,
        outputs_expected,
        None,
        None,
        None,
        IndexMap::new(),
        force,
    )
    .await
}

async fn run_task_with_vars(
    path: &Path,
    tasks: Vec<String>,
    status_expected: HashMap<String, String>,
    outputs_expected: Option<HashMap<String, String>>,
    vars: IndexMap<String, Value>,
    force: bool,
) -> anyhow::Result<()> {
    run_task_inner(
        path,
        tasks,
        status_expected,
        outputs_expected,
        None,
        None,
        None,
        vars,
        force,
    )
    .await
}

async fn run_task_inner(
    path: &Path,
    tasks: Vec<String>,
    status_expected: HashMap<String, String>,
    outputs_expected: Option<HashMap<String, String>>,
    restarts_expected: Option<HashMap<String, u64>>,
    runs_expected: Option<HashMap<String, u64>>,
    timeout_seconds: Option<u64>,
    vars: IndexMap<String, Value>,
    force: bool,
) -> anyhow::Result<()> {
    let path = path::absolute(path)?;
    let (root, children) = ProjectConfig::new_multi(&path)?;
    let ws = Workspace::new(&root, &children, &tasks, &path, &vars, force, false, Some(false))?;
    // Create runner
    let mut runner = TaskRunner::new(&ws)?;
    let (app_tx, app_rx) = AppCommandChannel::new();

    let runner_tx = runner.command_tx.clone();

    // Start runner
    let runner_fut = tokio::spawn(async move {
        runner.run(&app_tx, false).await.ok();
    });

    // Handle events and assert task statuses
    let events_fut = handle_events(
        app_rx,
        runner_tx,
        status_expected,
        outputs_expected,
        restarts_expected,
        runs_expected,
        timeout_seconds,
    );

    runner_fut.await?;
    events_fut.await?;
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
    force: bool,
    f: F,
) where
    F: Future<Output = ()> + Send + 'static,
{
    let path = path::absolute(path).unwrap();
    let ws = Workspace::new(
        &ProjectConfig::new("", &path).unwrap(),
        &IndexMap::new(),
        &tasks,
        &path,
        &IndexMap::new(),
        force,
        true,
        Some(false),
    )
    .unwrap();

    // Create runner
    let mut runner = TaskRunner::new(&ws).unwrap();
    let (app_tx, app_rx) = AppCommandChannel::new();

    let runner_tx = runner.command_tx.clone();

    // Start runner
    let runner_fut = tokio::spawn(async move { runner.run(&app_tx, false).await.ok() });

    // Ensure to run file watcher before runner
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Do something in this closure, ex: create or update files
    tokio::spawn(async move { f.await });

    // Handle events and assert task statuses
    handle_events(
        app_rx,
        runner_tx,
        status_expected,
        outputs_expected,
        restarts_expected,
        runs_expected,
        timeout_seconds,
    )
    .await
    .unwrap();

    runner_fut.await.ok();
}

const DEFAULT_TEST_TIMEOUT_SECONDS: u64 = 10;

fn handle_events(
    mut app_rx: UnboundedReceiver<AppCommand>,
    runner_tx: RunnerCommandChannel,
    statuses_expected: HashMap<String, String>,
    outputs_expected: Option<HashMap<String, String>>,
    restarts_expected: Option<HashMap<String, u64>>,
    runs_expected: Option<HashMap<String, u64>>,
    timeout_seconds: Option<u64>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut statuses = HashMap::new();
        let mut raw_outputs = HashMap::<String, String>::new();
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
                Some(event) = app_rx.recv() => {
                    match event {
                        AppCommand::StartTask { task, pid: _, restart, max_restart: _, reload, datetime: _ } => {
                            restarts.insert(task.clone(), restart);
                            runs.insert(task.clone(), reload);
                        }
                        AppCommand::ReadyTask { task } => {
                            statuses.insert(task, String::from("Ready"));
                        }
                        AppCommand::FinishTask { task, result, datetime: _ } => {
                            statuses.insert(task, format!("Finished: {:?}", result));
                        }
                        AppCommand::Quit => {
                            break;
                        }
                        AppCommand::TaskOutput { task, output } => {
                            let str = String::from_utf8(output.clone()).unwrap();
                            let str_trimmed = str.trim().to_string();
                            match raw_outputs.get(&task) {
                                Some(t) => {
                                    let s = format!("{}{}", t, str);
                                    let st = format!("{}{}", t, str_trimmed);
                                    raw_outputs.insert(task.clone(), s);
                                    outputs.insert(task.clone(), st);
                                }
                                None => {
                                    raw_outputs.insert(task.clone(), str);
                                    outputs.insert(task.clone(), str_trimmed);
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
        runner_tx.quit();

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
