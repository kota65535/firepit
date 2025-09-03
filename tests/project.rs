use assertables::assert_ok;
use firepit::config::ProjectConfig;
use firepit::project::Workspace;
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

#[test]
fn test_env_file_not_found() {
    let path = Path::new("tests/fixtures/project/no_env_file");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &Vec::new(),
        &std::env::current_dir().unwrap(),
        &HashMap::new(),
        &HashMap::new(),
        false,
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
        &HashMap::new(),
        &HashMap::new(),
        false,
    );
    assert_ok!(result);
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
        &HashMap::new(),
        &HashMap::new(),
        false,
    )
    .unwrap();

    let foo = ws.children.get("foo").unwrap().task("foo").unwrap();
    assert_eq!(
        foo.depends_on.iter().map(|s| s.task.clone()).collect::<Vec<_>>(),
        vec!["#install".to_string()]
    );

    let foo2 = ws.children.get("foo").unwrap().task("foo-2").unwrap();
    assert_eq!(
        foo2.depends_on.iter().map(|s| s.task.clone()).collect::<Vec<_>>(),
        vec!["#install".to_string(), "foo#foo".to_string()]
    );
}
