use assertables::assert_starts_with;
use firepit::config::ProjectConfig;
use std::collections::HashMap;
use std::env;
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
    for (key, value) in expected {
        assert_eq!(actual.get(*key), Some(&value.to_string()));
    }
}

fn to_absolute_path(root: &Path, path: &str) -> String {
    env::current_dir()
        .unwrap()
        .join(root)
        .join(path)
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn test_multi() {
    let path = Path::new("tests/fixtures/config/multi");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let foo = &children.get("foo").unwrap();
    let bar = &children.get("bar").unwrap();

    // root
    assert_eq_env(&root.env, &HashMap::from([("A", "a"), ("B", "b")]));
    assert_eq!(root.env_files, vec![".env.root"]);
    assert_eq!(root.shell.command, "shell-root");
    assert_eq!(root.shell.args, vec!["args-root"]);

    // foo
    assert_eq_env(
        &children.get("foo").unwrap().env,
        &HashMap::from([("A", "a"), ("B", "b-foo"), ("C", "c")]),
    );
    assert_eq!(
        foo.env_files,
        vec![to_absolute_path(&path, ".env.root"), String::from(".env.foo")]
    );
    assert_eq!(foo.shell.command, "shell-foo");
    assert_eq!(foo.shell.args, vec!["args-foo"]);

    // bar
    assert_eq_env(
        &children.get("bar").unwrap().env,
        &HashMap::from([("A", "a"), ("B", "b-bar"), ("C", "c")]),
    );
    assert_eq!(
        bar.env_files,
        vec![to_absolute_path(&path, ".env.root"), String::from(".env.bar")]
    );
    assert_eq!(bar.shell.command, "shell-bar");
    assert_eq!(bar.shell.args, vec!["args-bar"]);
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
