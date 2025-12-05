use crate::project::Task;
use crate::template::ROOT_DIR_CONTEXT_KEY;
use crate::util::merge_yaml;
use anyhow::Context;
use derivative::Derivative;
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use regex::Regex;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::Value;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::thread::available_parallelism;
use std::{io, iter, path};
use tera::Tera;
use tracing::info;

const CONFIG_FILE: [&str; 2] = ["firepit.yml", "firepit.yaml"];

#[derive(Clone, Deserialize, Serialize, JsonSchema, Derivative)]
#[derivative(Debug)]
pub struct ProjectConfig {
    /// Project name
    #[serde(skip)]
    pub name: String,

    /// Child projects.
    /// Valid only in a root project config.
    /// ```yaml
    /// projects:
    ///   client: packages/client
    ///   server: packages/server
    /// ```
    #[serde(default)]
    pub projects: IndexMap<String, String>,

    /// Shell configuration for all the project tasks.
    /// ```yaml
    /// shell:
    ///   command: "bash"
    ///   args: ["-eux", "-c"]
    /// ```
    #[serde(default = "default_shell")]
    pub shell: ShellConfig,

    /// Working directory for all the project tasks.
    /// ```yaml
    /// working_dir: src
    /// ```
    #[serde(default = "default_working_dir")]
    #[schemars(extend("x-template" = true))]
    pub working_dir: String,

    /// Template variables for all the project tasks.
    /// ```yaml
    /// vars:
    ///   aws_account_id: 123456789012
    ///   aws_region: ap-northeast-1
    ///   ecr_registry: "{{ aws_account_id }}.dkr.ecr.{{ aws_region }}.amazonaws.com"
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub vars: IndexMap<String, JsonValue>,

    /// Environment variables for all the project tasks.
    /// ```yaml
    /// env:
    ///   TZ: Asia/Tokyo
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env: IndexMap<String, String>,

    /// Dotenv files for all the project tasks.
    /// In case of duplicated environment variables, the latter one takes precedence.
    /// ```yaml
    /// env_files:
    ///   - .env
    ///   - .env.local
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env_files: Vec<String>,

    /// Dependency tasks for all the project tasks.
    /// ```yaml
    /// depends_on:
    ///   - '#install'
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub depends_on: Vec<DependsOnConfig>,

    /// Task definitions.
    #[serde(default)]
    pub tasks: IndexMap<String, TaskConfig>,

    /// Task concurrency.
    /// Valid only in a root project config.
    /// ```yaml
    /// concurrency: 4
    /// ```
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,

    /// Log configuration.
    /// Valid only in a root project config.
    /// ```yaml
    /// log:
    ///   level: debug
    ///   file: "{{ root_dir }}/firepit.log"
    /// ```
    #[serde(default = "default_log")]
    pub log: LogConfig,

    /// Gantt chart output file path.
    /// Valid only in a root project config.
    /// ```yaml
    /// gantt_file: gantt.svg
    /// ```
    pub gantt_file: Option<String>,

    /// UI configuration.
    /// Valid only in a root project config.
    /// ```yaml
    /// ui: cui
    /// ```
    #[serde(default = "default_ui")]
    pub ui: UI,

    /// Additional config files to be included.
    /// ```yaml
    /// includes:
    ///   - common-vars.yml
    ///   - common-tasks.yml
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub includes: Vec<String>,

    /// Project file path (absolute)
    #[serde(skip)]
    pub path: PathBuf,

    /// Project directory path (absolute)
    #[serde(skip)]
    pub dir: PathBuf,

    /// Raw YAML data
    #[serde(skip)]
    #[derivative(Debug = "ignore")]
    pub raw: Value,
}

pub fn default_shell() -> ShellConfig {
    ShellConfig {
        command: default_shell_command(),
        args: default_shell_args(),
    }
}

pub fn default_shell_command() -> String {
    "bash".to_string()
}

pub fn default_shell_args() -> Vec<String> {
    vec!["-c".to_string()]
}

pub fn default_working_dir() -> String {
    ".".to_string()
}

pub fn default_concurrency() -> usize {
    available_parallelism().unwrap().get()
}

pub fn default_log() -> LogConfig {
    LogConfig {
        level: default_log_level(),
        file: None,
    }
}

pub fn default_log_level() -> String {
    "info".to_string()
}

pub fn default_ui() -> UI {
    if atty::is(atty::Stream::Stdout) {
        UI::Tui
    } else {
        UI::Cui
    }
}

impl ProjectConfig {
    pub fn new_multi(dir: &Path) -> anyhow::Result<(ProjectConfig, IndexMap<String, ProjectConfig>)> {
        let dir = path::absolute(dir)?;
        let mut root_config = ProjectConfig::find_root(&dir)?;
        let mut children = IndexMap::new();

        // Tera context is used for merge
        let mut context = tera::Context::new();
        context.insert(
            ROOT_DIR_CONTEXT_KEY,
            &root_config.dir.as_os_str().to_str().unwrap_or(""),
        );

        if root_config.is_root() {
            // Multi project
            for (name, path) in &root_config.projects {
                if name.contains("#") {
                    anyhow::bail!("Project name must not contain '#'. Found: {:?}", name)
                }
                let mut child_config = ProjectConfig::new(name, root_config.dir.join(path).as_path())?;
                for t in child_config.tasks.values_mut() {
                    t.project = name.clone();
                }
                child_config = child_config.merge(&context)?;
                children.insert(name.clone(), child_config);
            }
        } else {
            // Single project
            root_config.name = "".to_string();
        }

        root_config = root_config.merge(&context)?;

        Ok((root_config, children))
    }

    pub fn validate_multi(root: &ProjectConfig, children: &IndexMap<String, ProjectConfig>) -> anyhow::Result<()> {
        let mut tasks = root.tasks.values().map(|t| t.full_name()).collect::<HashSet<_>>();
        for p in children.values() {
            tasks.extend(p.tasks.values().map(|t| t.full_name()).collect::<HashSet<_>>());
        }
        for config in iter::once(root).chain(children.values()) {
            config
                .validate(&tasks)
                .context(format!("invalid config file: {:?}", config.path))?;
        }
        Ok(())
    }

    fn validate(&self, tasks: &HashSet<String>) -> anyhow::Result<()> {
        for (_, t) in self.tasks.iter() {
            for d in t.depends_on.iter().map(|d| match d {
                DependsOnConfig::String(s) => s.clone(),
                DependsOnConfig::Struct(s) => s.task.clone(),
            }) {
                if !tasks.contains(&d) {
                    anyhow::bail!("tasks.{}.depends_on: task {:?} is not defined.", t.name, d);
                }
            }
        }
        Ok(())
    }

    pub fn new_from_str(name: &str, str: &str, path: &Path, dir: &Path) -> anyhow::Result<ProjectConfig> {
        let mut data = serde_yaml::from_str::<ProjectConfig>(str)?;

        // Project dir
        data.path = path.to_owned();
        data.dir = dir.to_owned();

        // Name
        data.name = name.to_string();

        // Task name & dependency task name
        for (k, v) in data.tasks.iter_mut() {
            v.name = k.clone();
            v.orig_name = k.clone();
            v.project = name.to_string();
            v.depends_on = v
                .depends_on
                .iter()
                .map(|d| match d {
                    DependsOnConfig::String(s) => DependsOnConfig::String(Task::qualified_name(&name, s)),
                    DependsOnConfig::Struct(s) => DependsOnConfig::Struct(DependsOnConfigStruct {
                        task: Task::qualified_name(&data.name, &s.task),
                        vars: s.vars.clone(),
                        cascade: s.cascade,
                    }),
                })
                .collect();
        }

        // Save raw data
        let raw_data = serde_yaml::from_str::<Value>(str)?;
        data.raw = raw_data;

        Ok(data)
    }

    pub fn new(name: &str, dir: &Path) -> anyhow::Result<ProjectConfig> {
        let (mut file, path) = Self::open_file(&dir.join(CONFIG_FILE[0]))
            .or_else(|_| Self::open_file(&dir.join(CONFIG_FILE[1])))
            .with_context(|| {
                format!(
                    "cannot open config file ({} or {}) in directory {:?}",
                    CONFIG_FILE[0], CONFIG_FILE[1], dir
                )
            })?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        Self::new_from_str(name, &buf, path.as_path(), dir)
            .with_context(|| format!("cannot parse config file {:?}", path))
    }

    pub fn merge(&self, context: &tera::Context) -> anyhow::Result<Self> {
        // Render includes only
        let mut tera = Tera::default();
        let mut rendered_includes = Vec::new();
        for f in self.includes.iter() {
            rendered_includes.push(tera.render_str(f, &context)?);
        }

        // Start from empty value
        let mut ret = Value::Null;

        // Merge included files first
        for incl in rendered_includes.iter() {
            info!("Config file {:?} includes {:?}", self.dir, incl);
            let path = absolute_or_join(&incl, &self.dir);
            let (file, _) = Self::open_file(&self.dir.join(&incl))
                .with_context(|| format!("cannot open included file {:?}", path))?;
            let reader = BufReader::new(file);
            let raw_yaml: Value =
                serde_yaml::from_reader(reader).with_context(|| format!("cannot read included file {:?}.", path))?;
            merge_yaml(&mut ret, &raw_yaml, true)
        }

        // Merge the main file
        merge_yaml(&mut ret, &self.raw, true);

        // Convert back to ProjectConfig
        let merged_str = serde_yaml::to_string(&ret)?;
        let merged = Self::new_from_str(&self.name, &merged_str, &self.path, &self.dir)?;
        Ok(merged)
    }

    fn open_file(path: &Path) -> Result<(File, PathBuf), io::Error> {
        match File::open(path) {
            Ok(file) => Ok((file, path.to_owned())),
            Err(e) => Err(e),
        }
    }

    fn find_root(cwd: &Path) -> anyhow::Result<ProjectConfig> {
        let config = ProjectConfig::new("", cwd)?;
        if config.is_root() {
            return Ok(config);
        }
        for current_dir in cwd.ancestors() {
            match ProjectConfig::new("", current_dir) {
                Ok(root_candidate) => {
                    if root_candidate.is_root() && config.is_child(&root_candidate) {
                        return Ok(root_candidate);
                    }
                }
                Err(err) => {
                    if err.downcast_ref::<io::Error>().map(|e| e.kind()) == Some(io::ErrorKind::NotFound) {
                        continue; // Continue to the next ancestor directory if the config file is not found
                    } else {
                        return Err(err); // Return error if any other error
                    }
                }
            }
        }
        Ok(config)
    }

    pub fn is_root(&self) -> bool {
        !self.projects.is_empty()
    }

    pub fn is_child(&self, root: &ProjectConfig) -> bool {
        root.projects.values().any(|p| Path::join(&root.dir, p) == self.dir)
    }

    pub fn working_dir_path(&self) -> PathBuf {
        absolute_or_join(&self.working_dir, &self.dir)
    }

    pub fn env_file_paths(&self) -> Vec<PathBuf> {
        self.env_files.iter().map(|f| absolute_or_join(f, &self.dir)).collect()
    }

    pub fn schema() -> anyhow::Result<String> {
        let schema = schemars::schema_for!(ProjectConfig);
        serde_json::to_string_pretty(&schema).context("cannot create config schema")
    }

    pub fn task(&self, name: &str) -> anyhow::Result<&TaskConfig> {
        self.tasks
            .get(name)
            .with_context(|| anyhow::anyhow!("task {:?} is not defined", name))
    }

    pub fn task_mut(&mut self, name: &str) -> anyhow::Result<&mut TaskConfig> {
        self.tasks
            .get_mut(name)
            .with_context(|| anyhow::anyhow!("task {:?} is not defined", name))
    }

    pub fn relative_path_from(&self, path: &Path) -> PathBuf {
        self.dir.strip_prefix(path).unwrap_or(&self.dir).to_path_buf()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskConfig {
    /// Name
    #[serde(skip)]
    pub name: String,

    /// Original name.
    /// Used for tracking the original of the task variant.
    #[serde(skip)]
    pub orig_name: String,

    /// Project name
    #[serde(skip)]
    pub project: String,

    /// Label to display instead of the task name.
    #[schemars(extend("x-template" = true))]
    pub label: Option<String>,

    /// Description
    pub description: Option<String>,

    /// Command to run
    #[schemars(extend("x-template" = true))]
    pub command: Option<String>,

    /// Shell configuration
    pub shell: Option<ShellConfig>,

    /// Working directory
    /// ```yaml
    /// working_dir: dist
    /// ```
    #[schemars(extend("x-template" = true))]
    pub working_dir: Option<String>,

    /// Template variables. Merged with the project `vars`.
    /// Can be used at `label`, `command`, `working_dir`, `env`, `env_files`, `depends_on`, `depends_on.{task, vars}`,
    /// `service.healthcheck.log` and `service.healthcheck.exec.{command, working_dir, env, env_files}`
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub vars: IndexMap<String, JsonValue>,

    /// Environment variables. Merged with the project `env`.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env: IndexMap<String, String>,

    /// Dotenv files. Merged with the project `env_files`.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env_files: Vec<String>,

    /// Dependency tasks
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub depends_on: Vec<DependsOnConfig>,

    /// Service configurations
    pub service: Option<ServiceConfig>,

    /// Inputs file glob patterns
    #[serde(default)]
    pub inputs: Vec<String>,

    /// Output file glob patterns
    #[serde(default)]
    pub outputs: Vec<String>,
}

impl TaskConfig {
    pub fn full_name(&self) -> String {
        format!("{}#{}", self.project, self.name)
    }

    pub fn full_orig_name(&self) -> String {
        format!("{}#{}", self.project, self.orig_name)
    }

    pub fn working_dir_path(&self, dir: &PathBuf) -> Option<PathBuf> {
        match self.working_dir.clone() {
            Some(wd) => Some(absolute_or_join(&wd, dir)),
            None => None,
        }
    }

    pub fn env_file_paths(&self, dir: &PathBuf) -> Vec<PathBuf> {
        self.env_files.iter().map(|f| absolute_or_join(f, dir)).collect()
    }

    pub fn input_paths(&self, dir: &PathBuf) -> Vec<PathBuf> {
        self.inputs.iter().map(|f| absolute_or_join(f, dir)).collect()
    }

    pub fn output_paths(&self, dir: &PathBuf) -> Vec<PathBuf> {
        self.outputs.iter().map(|f| absolute_or_join(f, dir)).collect()
    }
}

fn absolute_or_join(path: &str, dir: &Path) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct ShellConfig {
    /// Shell command.
    #[serde(default = "default_shell_command")]
    pub command: String,

    /// Arguments of the shell command.
    #[serde(default = "default_shell_args")]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    /// Log level. Valid values: error, warn, info, debug, trace
    pub level: String,

    /// Log file path.
    pub file: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(extend("x-template" = true))]
pub enum DependsOnConfig {
    String(String),
    Struct(DependsOnConfigStruct),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DependsOnConfigStruct {
    /// Dependency task name
    #[schemars(extend("x-template" = true))]
    pub task: String,

    /// Variables to override the dependency task vars.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub vars: IndexMap<String, JsonValue>,

    /// Whether the task restarts if this dependency task restarts.
    #[serde(default = "default_cascade")]
    pub cascade: bool,
}

fn default_cascade() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum HealthCheckConfig {
    Log(LogProbeConfig),
    Exec(ExecProbeConfig),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LogProbeConfig {
    /// Log regex pattern to determine the task service is ready
    #[schemars(extend("x-template" = true))]
    pub log: String,

    /// Timeout in seconds
    #[serde(default = "default_log_healthcheck_timeout")]
    pub timeout: u64,
}

pub fn default_log_healthcheck_timeout() -> u64 {
    20
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExecProbeConfig {
    /// Command to check if the service is ready
    #[schemars(extend("x-template" = true))]
    pub command: String,

    /// Shell configuration
    pub shell: Option<ShellConfig>,

    /// Working directory
    #[schemars(extend("x-template" = true))]
    pub working_dir: Option<String>,

    /// Environment variables. Merged with the task `env`.
    #[serde(default, deserialize_with = "deserialize_hash_map")]
    #[schemars(extend("x-template" = true))]
    pub env: IndexMap<String, String>,

    /// Dotenv files. Merged with the task `env_files`.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env_files: Vec<String>,

    /// Interval in seconds.
    /// The command will run interval seconds after the task is started,
    /// and then again interval seconds after each previous check completes.
    #[serde(default = "default_healthcheck_interval")]
    pub interval: u64,

    /// Timeout in seconds
    #[serde(default = "default_healthcheck_timeout")]
    pub timeout: u64,

    /// Number of consecutive readiness-check failures allowed before giving up.
    #[serde(default = "default_healthcheck_retries")]
    pub retries: u64,

    /// Initialization period in seconds.
    /// Probe failure during that period will not be counted towards the maximum number of retries.
    #[serde(default = "default_healthcheck_start_period")]
    pub start_period: u64,
}

impl ExecProbeConfig {
    pub fn working_dir_path(&self, dir: &PathBuf) -> Option<PathBuf> {
        match self.working_dir.clone() {
            Some(wd) => {
                let wd = Path::new(&wd);
                if wd.is_absolute() {
                    Some(wd.to_path_buf())
                } else {
                    Some(dir.join(wd))
                }
            }
            None => None,
        }
    }

    pub fn env_files_paths(&self, dir: &PathBuf) -> Vec<PathBuf> {
        self.env_files
            .iter()
            .map(|f| {
                let p = Path::new(f);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    dir.join(p)
                }
            })
            .collect()
    }
}

pub fn default_healthcheck_interval() -> u64 {
    5
}
pub fn default_healthcheck_timeout() -> u64 {
    5
}
pub fn default_healthcheck_retries() -> u64 {
    3
}
pub fn default_healthcheck_start_period() -> u64 {
    0
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ServiceConfig {
    Bool(bool),
    Struct(ServiceConfigStruct),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ServiceConfigStruct {
    /// Readiness probe configuration
    pub healthcheck: Option<HealthCheckConfig>,

    /// Restart policy
    #[serde(default = "default_service_restart")]
    pub restart: Restart,
}

#[derive(Debug, Clone)]
pub enum Restart {
    Always(Option<u64>),
    OnFailure(Option<u64>),
    Never,
}

impl Restart {
    pub fn max_restart(&self) -> Option<u64> {
        match self {
            Restart::Always(n) => *n,
            Restart::OnFailure(n) => *n,
            Restart::Never => Some(0),
        }
    }
}

pub static ALWAYS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^always(:(\d+))?$").unwrap());
pub static ON_FAILURE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^on-failure(:(\d+))?$").unwrap());

impl<'de> Deserialize<'de> for Restart {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        let r = match s {
            s if ALWAYS.is_match(&s) => {
                let num = ALWAYS
                    .captures(s.as_str())
                    .and_then(|c| c.get(2))
                    .and_then(|m| m.as_str().parse::<u64>().ok());
                Restart::Always(num)
            }
            s if ON_FAILURE.is_match(&s) => {
                let num = ON_FAILURE
                    .captures(s.as_str())
                    .and_then(|c| c.get(2))
                    .and_then(|m| m.as_str().parse::<u64>().ok());
                Restart::OnFailure(num)
            }
            s if s == "never" => Restart::Never,
            _ => return Err(serde::de::Error::custom(format!("invalid restart value: {}", s))),
        };
        Ok(r)
    }
}

impl Serialize for Restart {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Restart::Always(Some(num)) => serializer.serialize_str(&format!("always:{}", num)),
            Restart::Always(None) => serializer.serialize_str("always"),
            Restart::OnFailure(Some(num)) => serializer.serialize_str(&format!("on-failure:{}", num)),
            Restart::OnFailure(None) => serializer.serialize_str("on-failure"),
            Restart::Never => serializer.serialize_str("never"),
        }
    }
}

pub fn default_service_restart() -> Restart {
    Restart::Never
}

// cf. https://graham.cool/schemars/implementing/
impl JsonSchema for Restart {
    fn schema_name() -> Cow<'static, str> {
        "Restart".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Restart").into()
    }

    // JSON Schema designed to improve editor autocompletion.
    // Provide enum candidates for completion and allow numeric variants via pattern matching.
    fn json_schema(_gen: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "anyOf": [
                {
                  "type": "string",
                  "enum": [
                    "always",
                    "on-failure",
                    "never"
                  ]
                },
                {
                  "type": "string",
                  "pattern": "^(always(:\\d+)?|on-failure(:\\d+)?|never)$"
                }
            ]
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum UI {
    #[serde(rename = "cui")]
    Cui,
    #[serde(rename = "tui")]
    Tui,
}

/// Deserializes IndexMap while converting number to string, which is the default behavior.
/// This is necessary when using serde(untagged), as it strictly checks types.
fn deserialize_hash_map<'de, D>(deserializer: D) -> Result<IndexMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    let map: IndexMap<String, Value> = IndexMap::deserialize(deserializer)?;
    let mut new_map = IndexMap::new();

    for (key, value) in map {
        let value_str = match value {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => s,
            Value::Sequence(_) => {
                return Err(de::Error::invalid_type(
                    de::Unexpected::Seq,
                    &"a string, number, or boolean",
                ))
            }
            Value::Mapping(_) => {
                return Err(de::Error::invalid_type(
                    de::Unexpected::Map,
                    &"a string, number, or boolean",
                ))
            }
            Value::Tagged(t) => {
                return Err(de::Error::invalid_type(
                    de::Unexpected::Other(t.value.as_str().unwrap_or("unknown")),
                    &"a string, number, or boolean",
                ))
            }
        };
        new_map.insert(key, value_str);
    }

    Ok(new_map)
}
