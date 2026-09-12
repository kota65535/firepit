use assertables::{assert_err, assert_ok};
use firepit::config::ProjectConfig;
use firepit::project::Workspace;
use firepit::vars::VarsConfig;
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
    // A file that is not there is how an optional dotenv file looks, so it is skipped without a
    // word and the files around it are still read
    let ws = assert_ok!(result);
    assert_eq_env(
        &ws.root.task("foo").unwrap().env.load().unwrap(),
        &HashMap::from([("PRESENT", "yes")]),
    );
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
async fn test_unset_task_var_shadows_project_var() {
    // The project level `env` has a value, but the task declares `env` without a value,
    // which shadows the project value, so an explicit value is required
    let path = Path::new("tests/fixtures/project/required_vars");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("#shadow")],
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
async fn test_unset_task_var_shadowing_project_var_given_by_cli() {
    let path = Path::new("tests/fixtures/project/required_vars");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#shadow")],
        &std::env::current_dir().unwrap(),
        &IndexMap::from([(String::from("env"), VarsConfig::Static(serde_json::Value::from("prod")))]),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    assert_eq!(ws.task("#shadow").unwrap().command, String::from("echo \"prod\""));
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
async fn test_unset_dep_var_is_not_given_by_cli() {
    // The CLI argument sets only target task vars and project vars,
    // so it must not reach the dependency task's unset var
    let path = Path::new("tests/fixtures/project/required_vars");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("#dependent_nocli")],
        &std::env::current_dir().unwrap(),
        &IndexMap::from([(
            String::from("region"),
            VarsConfig::Static(serde_json::Value::from("us-east-1")),
        )]),
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
        &[String::from("#plain")],
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
async fn test_unset_project_var_without_cli() {
    // The project level `env` is unset, so running any task of the project
    // without the CLI argument is an error, even if the task does not declare it
    let path = Path::new("tests/fixtures/project/required_project_var");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("#bar")],
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
        &[String::from("#bar")],
        &std::env::current_dir().unwrap(),
        &IndexMap::from([(String::from("env"), VarsConfig::Static(serde_json::Value::from("prod")))]),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    assert_eq!(ws.task("#bar").unwrap().command, String::from("echo \"prod\""));
}

#[tokio::test]
async fn test_unset_project_var_of_other_project_is_ignored() {
    // Project `b` has an unset project var, but only project `a` is involved in the run
    let path = Path::new("tests/fixtures/project/required_project_var_multi");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("a#build")],
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
async fn test_unset_project_var_of_involved_project() {
    let path = Path::new("tests/fixtures/project/required_project_var_multi");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("b#deploy")],
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
async fn test_unset_task_var_with_unset_project_var() {
    // The `foo` task declares `env` without a value and the project level `env` is also unset,
    // so there is nothing to inherit
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
async fn test_unset_task_var_given_by_cli() {
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
async fn test_undeclared_cli_var() {
    // `regoin` is a typo of `region`, matching no var declaration, so it is an error
    let path = Path::new("tests/fixtures/project/required_vars");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("#plain")],
        &std::env::current_dir().unwrap(),
        &IndexMap::from([(
            String::from("regoin"),
            VarsConfig::Static(serde_json::Value::from("us-east-1")),
        )]),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await;
    assert_err!(result);
}

#[tokio::test]
async fn test_args_cli_var_needs_no_declaration() {
    // The `args` var receives the arguments after `--`, so it is accepted undeclared
    let path = Path::new("tests/fixtures/project/required_vars");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("#plain")],
        &std::env::current_dir().unwrap(),
        &IndexMap::from([(
            String::from("args"),
            VarsConfig::Static(serde_json::Value::from("--nocapture")),
        )]),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await;
    assert_ok!(result);
}

#[tokio::test]
async fn test_cli_var_declared_by_other_task() {
    // `region` is declared by the `required` task, which is not involved in this run.
    // The declaration is looked up across all projects and tasks, so this is not an error
    let path = Path::new("tests/fixtures/project/required_vars");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let result = Workspace::new(
        &root,
        &children,
        &[String::from("#plain")],
        &std::env::current_dir().unwrap(),
        &IndexMap::from([(
            String::from("region"),
            VarsConfig::Static(serde_json::Value::from("us-east-1")),
        )]),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await;
    assert_ok!(result);
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

#[tokio::test]
async fn test_env_file_template() {
    let path = Path::new("tests/fixtures/project/env_file_template");
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

    // The values of a dotenv file are templates rendered with the task context: a project var,
    // a task var, and a later file overriding an earlier one with a template of its own.
    let run = ws.root.task("run").unwrap();
    assert_eq_env(
        &run.env.load().unwrap(),
        &HashMap::from([
            ("SESSION", "abc"),
            ("OVERRIDE", "local-abc"),
            ("LITERAL", "{{x"),
            ("PORT", "3000"),
        ]),
    );
}

#[tokio::test]
async fn test_env_file_bad_template() {
    let path = Path::new("tests/fixtures/project/env_file_bad_template");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    // A bad template does not fail the workspace: a dotenv file is only parsed until its task runs
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

    // An undefined variable is named, along with the file and the key, but never the value: a
    // dotenv file holds secrets
    let msg = format!("{:?}", ws.root.task("undefined").unwrap().env.load().unwrap_err());
    assert!(msg.contains(".env.undefined"), "{msg}");
    assert!(msg.contains("SECRET"), "{msg}");
    assert!(msg.contains("missing"), "{msg}");
    assert!(!msg.contains("s3cr3t"), "{msg}");

    // A syntax error quotes the template, so it is dropped
    let msg = format!("{:?}", ws.root.task("syntax").unwrap().env.load().unwrap_err());
    assert!(msg.contains(".env.syntax"), "{msg}");
    assert!(msg.contains("BROKEN"), "{msg}");
    assert!(!msg.contains("s3cr3t"), "{msg}");
}

#[tokio::test]
async fn test_undeclared_args_renders_as_empty_string() {
    // `args` needs no declaration, so `{{ args }}` renders even without a value
    let path = Path::new("tests/fixtures/project/args_undeclared");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#plain")],
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    assert_eq!(ws.task("#plain").unwrap().command, String::from("echo \"[]\""));
}

#[tokio::test]
async fn test_undeclared_args_overridden_by_cli_var() {
    let path = Path::new("tests/fixtures/project/args_undeclared");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#plain")],
        &std::env::current_dir().unwrap(),
        &IndexMap::from([(
            String::from("args"),
            VarsConfig::Static(serde_json::Value::from("--nocapture")),
        )]),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    assert_eq!(
        ws.task("#plain").unwrap().command,
        String::from("echo \"[--nocapture]\"")
    );
}

#[tokio::test]
async fn test_declared_args_overrides_the_implicit_default() {
    // A task declaring `args` keeps its own default instead of the implicit empty string
    let path = Path::new("tests/fixtures/project/args_undeclared");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#declared")],
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
        ws.task("#declared").unwrap().command,
        String::from("echo \"[default]\"")
    );
}

#[tokio::test]
async fn test_dependency_override_of_undeclared_args() {
    // `args` needs no declaration, so a `depends_on.vars` override of it must still reach the
    // dependency instead of being dropped as an unknown var
    let path = Path::new("tests/fixtures/project/args_undeclared");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#dep_outer")],
        &std::env::current_dir().unwrap(),
        &IndexMap::new(),
        false,
        false,
        Some(false),
        Some(false),
    )
    .await
    .unwrap();

    // The override makes a variant of the dependency, so follow `depends_on` to the variant
    // rather than looking the task up by its name in the config
    let outer = ws.task("#dep_outer").unwrap();
    let dep = outer.depends_on.first().expect("dep_outer depends on dep_inner");
    let inner = ws.task(&dep.task).expect("the variant is part of the run");
    assert_eq!(inner.command, String::from("echo \"[-x]\""));
}

#[tokio::test]
async fn test_project_var_referencing_undeclared_args() {
    // The implicit declaration comes first in its scope, so another var of the same scope can
    // reference `args` without declaring it
    let path = Path::new("tests/fixtures/project/args_undeclared");
    let (root, children) = ProjectConfig::new_multi(path).unwrap();
    let ws = Workspace::new(
        &root,
        &children,
        &[String::from("#uses_project_var")],
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
        ws.task("#uses_project_var").unwrap().command,
        String::from("echo \"[pre  post]\"")
    );
}
