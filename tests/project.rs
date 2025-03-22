use assertables::assert_starts_with;
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
    let err = Workspace::new(
        &root,
        &children,
        &Vec::new(),
        &std::env::current_dir().unwrap(),
        &HashMap::new(),
        &HashMap::new(),
    )
    .unwrap_err();
    assert_starts_with!(err.to_string(), "cannot read env file");
}

#[test]
fn test_bad_env_file() {
    let path = Path::new("tests/fixtures/project/bad_env_file");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let err = Workspace::new(
        &root,
        &children,
        &Vec::new(),
        &std::env::current_dir().unwrap(),
        &HashMap::new(),
        &HashMap::new(),
    )
    .unwrap_err();
    assert_starts_with!(err.to_string(), "cannot parse env file");
}
