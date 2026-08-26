use assertables::{assert_err, assert_ok};
use firepit::config::{ProjectConfig, VarsConfig};
use firepit::project::Workspace;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Once;
use std::time::Duration;
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

fn assert_eq_env(actual: &HashMap<String, String>, expected: &HashMap<&str, &str>) {
    assert_eq!(actual.len(), expected.len());
    for (key, value) in expected {
        assert_eq!(actual.get(*key), Some(&value.to_string()));
    }
}

#[tokio::test]
async fn test_env_file_not_found() {
    let path = Path::new("tests/fixtures/project/no_env_file");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &Vec::new(),
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await;
    assert_ok!(result);
}

#[tokio::test]
async fn test_bad_env_file() {
    let path = Path::new("tests/fixtures/project/bad_env_file");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &Vec::new(),
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await;
    assert_err!(result);
}

#[tokio::test]
async fn test_variant_label() {
    let path = Path::new("tests/fixtures/project/variant_label");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#foo")],
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    let labels = ws.labels();
    // Default labels do not include the internal variant suffix
    assert_eq!(labels.get("#foo"), Some(&String::from("#foo")));
    assert_eq!(labels.get("#bar-1"), Some(&String::from("#bar")));
    // Explicit labels are rendered with the variant vars
    assert_eq!(labels.get("#baz-1"), Some(&String::from("baz 2")));
}

#[tokio::test]
async fn test_empty_string_task_var_renders_as_string_in_label() {
    let path = Path::new("tests/fixtures/project/task_empty_args_label");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#tf")],
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    let labels = ws.labels();
    assert_eq!(labels.get("#tf"), Some(&String::from("#tf ")));
}

#[tokio::test]
async fn test_unset_task_var_inherits_project_var() {
    let path = Path::new("tests/fixtures/project/required_vars");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#inherit")],
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    assert_eq!(ws.task("#inherit").unwrap().command, String::from("echo \"dev\""));
}

#[tokio::test]
async fn test_unset_task_var_given_by_dependent_task() {
    let path = Path::new("tests/fixtures/project/required_vars");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#dependent")],
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    assert_eq!(
        ws.task("#required-1").unwrap().command,
        String::from("echo \"us-east-1\"")
    );
}

#[tokio::test]
async fn test_unset_task_var_without_value() {
    let path = Path::new("tests/fixtures/project/required_vars");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("#required")],
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await;
    assert_err!(result);
}

#[tokio::test]
async fn test_unset_task_var_of_other_task_is_ignored() {
    // The `required` task has an unset var, but it is not run, so it must not be an error
    let path = Path::new("tests/fixtures/project/required_vars");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("#inherit")],
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await;
    assert_ok!(result);
}

#[tokio::test]
async fn test_unset_project_var_without_value() {
    let path = Path::new("tests/fixtures/project/required_project_var");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("#foo")],
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await;
    assert_err!(result);
}

#[tokio::test]
async fn test_unset_project_var_given_by_cli() {
    let path = Path::new("tests/fixtures/project/required_project_var");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#foo")],
        &std::env::current_dir().unwrap(),
        &IndexMap::from([(String::from("env"), VarsConfig::Static(serde_json::Value::from("prod")))]),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    assert_eq!(ws.task("#foo").unwrap().command, String::from("echo \"prod\""));
}

#[tokio::test]
async fn test_multi() {
    let path = Path::new("tests/fixtures/project/multi");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &Vec::new(),
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    let root = ws.root.task("root").unwrap();
    assert_eq_env(
        &root.env.load().unwrap(),
        &HashMap::from([("A", "a-x"), ("B", "b-x-x")]),
    );
    assert_eq!(
        root.depends_on.iter().map(|s| s.task.clone()).collect::<Vec<_>>(),
        vec!["#install".to_string()]
    );
    assert_eq!(root.command, "echo \"root x\"".to_string());

    let install = ws.root.task("install").unwrap();
    assert_eq_env(
        &install.env.load().unwrap(),
        &HashMap::from([("A", "a-x"), ("B", "b-x-x"), ("C", "c")]),
    );
    assert!(install.depends_on.is_empty());

    let _foo = ws.children.get("foo").unwrap().task("foo").unwrap();
}

#[tokio::test]
async fn test_stop_timeout() {
    let path = Path::new("tests/fixtures/project/stop_timeout");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &Vec::new(),
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    // Omitted: falls back to the default grace period
    assert_eq!(ws.root.task("default").unwrap().stop_timeout, Duration::from_secs(10));
    // Explicit task-level value
    assert_eq!(ws.root.task("explicit").unwrap().stop_timeout, Duration::from_secs(30));
    // Services are covered too
    assert_eq!(
        ws.root.task("service_explicit").unwrap().stop_timeout,
        Duration::from_secs(20)
    );
}
