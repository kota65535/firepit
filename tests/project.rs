use assertables::assert_starts_with;
use firepit::config::ProjectConfig;
use firepit::project::Workspace;
use log::LevelFilter;
use std::path::Path;
use std::sync::Once;

static INIT: Once = Once::new();

pub fn setup() {
    INIT.call_once(|| {
        env_logger::builder().filter_level(LevelFilter::Debug).try_init();
    });
}

#[test]
fn test_env_file_not_found() {
    let path = Path::new("tests/fixtures/project/no_env_file");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let err = Workspace::new(&root, &children).unwrap_err();
    assert_starts_with!(err.to_string(), "cannot read env file");
}

#[test]
fn test_bad_env_file() {
    let path = Path::new("tests/fixtures/project/bad_env_file");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let err = Workspace::new(&root, &children).unwrap_err();
    assert_starts_with!(err.to_string(), "cannot parse env file");
}
