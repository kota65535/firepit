use assertables::assert_starts_with;
use firepit::config::{ProjectConfig, ShellConfig};
use indexmap::IndexMap;
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

fn assert_eq_env(actual: &IndexMap<String, String>, expected: &IndexMap<&str, &str>) {
    for (key, value) in expected {
        assert_eq!(actual.get(*key), Some(&value.to_string()));
    }
}

#[test]
fn test_multi() {
    let path = Path::new("tests/fixtures/config/multi");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let foo = children.get("foo").unwrap().clone();
    let bar = children.get("bar").unwrap().clone();

    // root
    assert_eq_env(&root.env, &IndexMap::from([("A", "a-root"), ("B", "b-root")]));
    assert_eq!(root.env_files, vec![".env.root"]);
    assert_eq!(
        root.shell,
        ShellConfig {
            command: "shell-root".to_string(),
            args: vec!["args-root".to_string()],
        }
    );

    // foo
    assert_eq_env(
        &children.get("foo").unwrap().env,
        &IndexMap::from([("A", "a-foo"), ("B", "b-foo")]),
    );
    assert_eq!(foo.env_files, vec![String::from(".env.foo")]);
    assert_eq!(
        foo.shell,
        ShellConfig {
            command: "shell-foo".to_string(),
            args: vec!["args-foo".to_string()],
        }
    );

    // bar
    assert_eq_env(
        &children.get("bar").unwrap().env,
        &IndexMap::from([("A", "a-bar"), ("B", "b-bar")]),
    );
    assert_eq!(bar.env_files, vec![String::from(".env.bar")]);
    assert_eq!(
        bar.shell,
        ShellConfig {
            command: "bash".to_string(),
            args: vec!["-c".to_string()],
        }
    );
}

#[test]
fn test_merge() {
    let path = Path::new("tests/fixtures/config/merge");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let foo = children.get("foo").unwrap().clone();
    let bar = children.get("bar").unwrap().clone();

    // root
    assert_eq_env(&root.env, &IndexMap::from([("A", "a"), ("B", "b")]));
    assert_eq!(root.env_files, vec![".env.a", ".env.b", ".env.root"]);
    assert_eq!(
        root.shell,
        ShellConfig {
            command: "shell-root".to_string(),
            args: vec!["-x".to_string(), "-c".to_string()],
        }
    );

    // foo
    assert_eq_env(
        &children.get("foo").unwrap().env,
        &IndexMap::from([("A", "a"), ("foo", "foo")]),
    );
    assert_eq!(foo.env_files, vec![String::from(".env.a"), String::from(".env.foo"),]);
    assert_eq!(
        foo.shell,
        ShellConfig {
            command: "shell-foo".to_string(),
            args: vec!["args-foo".to_string()],
        }
    );
    assert_eq!(foo.tasks.get("foo").unwrap().command, Some("echo \"foo\"".to_string()));
    assert_eq!(
        foo.tasks.get("foo-2").unwrap().command,
        Some("echo \"foo-2\"".to_string())
    );

    // bar
    assert_eq_env(
        &children.get("bar").unwrap().env,
        &IndexMap::from([("bar", "bar"), ("B", "bar")]),
    );
    assert_eq!(bar.env_files, vec![String::from(".env.b"), String::from(".env.bar"),]);
    assert_eq!(
        bar.shell,
        ShellConfig {
            command: "bash".to_string(),
            args: vec!["-x".to_string(), "-c".to_string()],
        }
    );
    assert_eq!(bar.tasks.get("bar").unwrap().command, Some("echo \"bar\"".to_string()));
    assert_eq!(
        bar.tasks.get("bar-2").unwrap().command,
        Some("echo \"bar-2\"".to_string())
    );
}

#[test]
fn test_bad_root() {
    let path = Path::new("tests/fixtures/config/somewhere");
    let err = ProjectConfig::new_multi(path).expect_err("");
    assert_starts_with!(err.to_string(), "cannot open config file");
}

#[test]
fn test_bad_child() {
    let path = Path::new("tests/fixtures/config/bad_child");
    let err = ProjectConfig::new_multi(path).expect_err("");
    assert_starts_with!(err.to_string(), "cannot open config file");
}

#[test]
fn test_bad_type() {
    let path = Path::new("tests/fixtures/config/bad_type");
    let err = ProjectConfig::new_multi(path).expect_err("");
    println!("{:?}", err);
    assert_starts_with!(err.to_string(), "cannot parse config file");
}
