use assertables::assert_starts_with;
use firepit::config::{DependsOnConfig, FinalizedByConfig, ProjectConfig, ShellConfig};
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

fn depends_on_names(task: &firepit::config::TaskConfig) -> Vec<String> {
    task.depends_on
        .iter()
        .map(|d| match d {
            DependsOnConfig::String(s) => s.clone(),
            DependsOnConfig::Struct(s) => s.task.clone(),
        })
        .collect()
}

fn finalized_by_names(task: &firepit::config::TaskConfig) -> Vec<String> {
    task.finalized_by
        .iter()
        .map(|f| match f {
            FinalizedByConfig::String(s) => s.clone(),
            FinalizedByConfig::Struct(s) => s.task.clone(),
        })
        .collect()
}

#[test]
fn test_defaults_finalized_by() {
    let path = Path::new("tests/fixtures/config/defaults_finalized_by");
    let (root, _) = ProjectConfig::new_multi(path).unwrap();

    // build and test should have finalized_by cleanup from defaults
    let build = root.tasks.get("build").unwrap();
    assert!(finalized_by_names(build).iter().any(|f| f.contains("cleanup")));

    let test = root.tasks.get("test").unwrap();
    assert!(finalized_by_names(test).iter().any(|f| f.contains("cleanup")));

    // lint should NOT have finalized_by
    let lint = root.tasks.get("lint").unwrap();
    assert!(finalized_by_names(lint).is_empty());
}

#[test]
fn test_defaults_regex() {
    let path = Path::new("tests/fixtures/config/defaults_regex");
    let (root, _) = ProjectConfig::new_multi(path).unwrap();

    // build and test should match "^(build|test)" regex
    let build = root.tasks.get("build").unwrap();
    assert_eq!(
        build.shell,
        Some(ShellConfig {
            command: "node".to_string(),
            args: vec!["-e".to_string()],
        })
    );
    assert_eq!(build.env.get("NODE_ENV"), Some(&"development".to_string()));
    assert!(depends_on_names(build).iter().any(|d| d.contains("install")));

    let test = root.tasks.get("test").unwrap();
    // Task-specific env should override defaults
    assert_eq!(test.env.get("NODE_ENV"), Some(&"test".to_string()));
    assert!(depends_on_names(test).iter().any(|d| d.contains("install")));

    // install and lint should NOT match
    let install = root.tasks.get("install").unwrap();
    assert_eq!(install.shell, None);
    assert!(install.env.get("NODE_ENV").is_none());

    let lint = root.tasks.get("lint").unwrap();
    assert_eq!(lint.shell, None);
    assert!(lint.env.get("NODE_ENV").is_none());
}

#[test]
fn test_defaults_list() {
    let path = Path::new("tests/fixtures/config/defaults_list");
    let (root, _) = ProjectConfig::new_multi(path).unwrap();

    // build and test should match the explicit list
    let build = root.tasks.get("build").unwrap();
    assert_eq!(build.env.get("NODE_ENV"), Some(&"development".to_string()));
    assert!(depends_on_names(build).iter().any(|d| d.contains("install")));

    let test = root.tasks.get("test").unwrap();
    assert_eq!(test.env.get("NODE_ENV"), Some(&"development".to_string()));

    // install and lint should NOT match
    let install = root.tasks.get("install").unwrap();
    assert!(install.env.get("NODE_ENV").is_none());

    let lint = root.tasks.get("lint").unwrap();
    assert!(lint.env.get("NODE_ENV").is_none());
}

#[test]
fn test_defaults_multi() {
    let path = Path::new("tests/fixtures/config/defaults_multi");
    let (root, _) = ProjectConfig::new_multi(path).unwrap();

    // All tasks should have LOG_LEVEL from ".*" defaults
    for (_, task) in root.tasks.iter() {
        assert_eq!(task.env.get("LOG_LEVEL"), Some(&"info".to_string()));
    }

    // build and build-storybook should match "^build" and have depends_on install + NODE_ENV
    let build = root.tasks.get("build").unwrap();
    assert_eq!(build.env.get("NODE_ENV"), Some(&"production".to_string()));
    assert!(depends_on_names(build).iter().any(|d| d.contains("install")));

    let build_sb = root.tasks.get("build-storybook").unwrap();
    assert_eq!(build_sb.env.get("NODE_ENV"), Some(&"production".to_string()));
    assert!(depends_on_names(build_sb).iter().any(|d| d.contains("install")));

    // test should NOT have NODE_ENV or depends_on from "^build"
    let test = root.tasks.get("test").unwrap();
    assert!(test.env.get("NODE_ENV").is_none());
    assert!(depends_on_names(test).iter().all(|d| !d.contains("install")));
}

#[test]
fn test_defaults_override() {
    let path = Path::new("tests/fixtures/config/defaults_override");
    let (root, _) = ProjectConfig::new_multi(path).unwrap();

    // install: only first defaults entry matches (all tasks)
    let install = root.tasks.get("install").unwrap();
    assert_eq!(install.env.get("LOG_LEVEL"), Some(&"info".to_string()));
    assert_eq!(install.env.get("NODE_ENV"), Some(&"development".to_string()));
    assert_eq!(
        install.shell,
        Some(ShellConfig {
            command: "bash".to_string(),
            args: vec!["-c".to_string()],
        })
    );
    assert_eq!(install.env_files, vec![".env.base"]);
    assert!(depends_on_names(install).iter().any(|d| d.contains("install")));
    assert!(!depends_on_names(install).iter().any(|d| d.contains("lint")));

    // build: both defaults entries match; second overrides scalar/map values
    let build = root.tasks.get("build").unwrap();
    assert_eq!(build.env.get("LOG_LEVEL"), Some(&"info".to_string()));
    // NODE_ENV should be "production" from 2nd entry, NOT "development" from 1st
    assert_eq!(build.env.get("NODE_ENV"), Some(&"production".to_string()));
    // shell should be node from 2nd entry, NOT bash from 1st
    assert_eq!(
        build.shell,
        Some(ShellConfig {
            command: "node".to_string(),
            args: vec!["-e".to_string()],
        })
    );
    // env_files should be concatenated: base + build
    assert_eq!(build.env_files, vec![".env.base", ".env.build"]);
    // depends_on should be concatenated: install + lint
    assert!(depends_on_names(build).iter().any(|d| d.contains("install")));
    assert!(depends_on_names(build).iter().any(|d| d.contains("lint")));

    // test: only first defaults entry matches, task-level env overrides
    let test = root.tasks.get("test").unwrap();
    assert_eq!(test.env.get("LOG_LEVEL"), Some(&"info".to_string()));
    // task-level NODE_ENV: test overrides defaults NODE_ENV: development
    assert_eq!(test.env.get("NODE_ENV"), Some(&"test".to_string()));
    assert_eq!(
        test.shell,
        Some(ShellConfig {
            command: "bash".to_string(),
            args: vec!["-c".to_string()],
        })
    );
}

#[test]
fn test_defaults_bad_regex() {
    let path = Path::new("tests/fixtures/config/defaults_bad_regex");
    let err = ProjectConfig::new_multi(path).expect_err("");
    assert!(err.to_string().contains("invalid regex pattern"));
}

#[test]
fn test_defaults_no_selector() {
    let path = Path::new("tests/fixtures/config/defaults_no_selector");
    let (root, _) = ProjectConfig::new_multi(path).unwrap();

    // No tasks field = all tasks should have LOG_LEVEL
    for (_, task) in root.tasks.iter() {
        assert_eq!(task.env.get("LOG_LEVEL"), Some(&"info".to_string()));
    }

    // tasks: "" (empty regex) = no tasks should match
    for (_, task) in root.tasks.iter() {
        assert!(task.env.get("EMPTY_REGEX").is_none());
    }

    // tasks: [] (empty list) = no tasks should match
    for (_, task) in root.tasks.iter() {
        assert!(task.env.get("EMPTY_LIST").is_none());
    }
}
