use assertables::{assert_err, assert_ok};
use firepit::config::ProjectConfig;
use firepit::project::Workspace;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Once;
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

#[test]
fn test_env_file_not_found() {
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
    );
    assert_ok!(result);
}

#[test]
fn test_bad_env_file() {
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
    );
    assert_err!(result);
}

#[test]
fn test_multi() {
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
    )
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

    let foo = ws.children.get("foo").unwrap().task("foo").unwrap();
}
