use firepit::runner::TaskRunner;
use std::collections::HashMap;
use std::path::Path;

use firepit::config::ProjectConfig;
use firepit::event::{Event, EventSender};
use log::LevelFilter;
use std::sync::Once;
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

static INIT: Once = Once::new();

pub fn setup() {
    INIT.call_once(|| {
        env_logger::builder().filter_level(LevelFilter::Debug).try_init();
    });
}

#[tokio::test]
async fn test_basic() {
    setup();
    let path = Path::new("tests/fixtures/basic");
    let tasks = vec!["foo".to_string()];

    let status = run_task(path, tasks, 2).await;

    let mut expected = HashMap::new();
    expected.insert(String::from("#foo"), String::from("Finished: Success"));
    expected.insert(String::from("#bar"), String::from("Finished: Success"));

    assert_eq!(status, expected);
}

#[tokio::test]
async fn test_service() {
    setup();
    let path = Path::new("tests/fixtures/service");
    let tasks = vec!["foo".to_string()];

    let status = run_task(path, tasks, 4).await;

    let mut expected = HashMap::new();
    expected.insert(String::from("#foo"), String::from("Finished: Success"));
    expected.insert(String::from("#bar"), String::from("Ready"));
    expected.insert(String::from("#baz"), String::from("Ready"));
    expected.insert(String::from("#qux"), String::from("Ready"));

    assert_eq!(status, expected);
}

#[tokio::test]
async fn test_bad_service() {
    setup();
    let path = Path::new("tests/fixtures/bad_service");
    let tasks = vec!["foo".to_string()];

    let status = run_task(path, tasks, 3).await;

    let mut expected = HashMap::new();
    expected.insert(String::from("#foo"), String::from("Finished: Dependencies failed"));
    expected.insert(String::from("#bar"), String::from("Finished: Service not ready"));
    expected.insert(String::from("#baz"), String::from("Finished: Service not ready"));

    assert_eq!(status, expected);
}

async fn run_task(path: &Path, tasks: Vec<String>, num_ready_tasks: usize) -> HashMap<String, String> {
    let config = ProjectConfig::new(path).unwrap();
    let mut runner = TaskRunner::new(&config, &HashMap::new(), &tasks, Path::new(path)).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let sender = EventSender::new(tx);

    let manager = runner.manager.clone();

    let runner_fut = tokio::spawn(async move { runner.run(sender).await });
    let status = handle_events(rx, num_ready_tasks).await.unwrap();

    // Stop processes to forcing runner to finish
    manager.stop().await;
    runner_fut.await;

    status
}

fn handle_events(mut rx: UnboundedReceiver<Event>, num_tasks: usize) -> JoinHandle<HashMap<String, String>> {
    tokio::spawn(async move {
        let mut status = HashMap::new();
        while let Some(event) = rx.recv().await {
            match event {
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
            if status.len() >= num_tasks {
                break;
            }
        }
        status
    })
}
