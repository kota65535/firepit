use anyhow::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::thread::available_parallelism;
use std::{io, path};

const CONFIG_FILE: [&str; 2] = ["firepit.yml", "firepit.yaml"];

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectConfig {
    /// Child projects.
    /// Valid only in root project config.
    #[serde(default)]
    pub projects: HashMap<String, String>,

    /// Shell configuration for all tasks.
    #[serde(default = "default_shell")]
    pub shell: ShellConfig,

    /// Working directory for all tasks.
    #[serde(default = "default_working_dir")]
    pub working_dir: String,

    /// Environment variables of all tasks.
    /// If this is a child project, merged with the root project's one.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Dotenv files for all tasks.
    /// If this is a child project, merged with the root project's one.
    #[serde(default)]
    pub env_files: Vec<String>,

    /// Task definitions.
    #[serde(default)]
    pub tasks: HashMap<String, TaskConfig>,

    /// Task concurrency.
    /// Valid only in root project config.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,

    /// Log configuration.
    /// Valid only in root project config.
    #[serde(default = "default_log")]
    pub log: LogConfig,

    /// UI configuration.
    /// Valid only in root project config.
    #[serde(default = "default_ui")]
    pub ui: UI,

    /// Project directory.
    #[serde(skip)]
    pub dir: PathBuf,
}

pub fn default_shell() -> ShellConfig {
    ShellConfig {
        command: "bash".to_string(),
        args: vec!["-c".to_string()],
    }
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
    pub fn new_multi(
        dir: &Path,
    ) -> anyhow::Result<(ProjectConfig, HashMap<String, ProjectConfig>)> {
        let dir = path::absolute(dir)?;
        let root_config = ProjectConfig::find_root(&dir)?;
        let mut children = HashMap::new();
        if root_config.is_root() {
            // Multi project
            for (name, path) in &root_config.projects {
                let mut child_config = ProjectConfig::new(dir.join(path).as_path())?;
                child_config.env.extend(root_config.env.clone());
                child_config.env_files.extend(
                    root_config
                        .env_files
                        .iter()
                        .map(|f| dir.join(f).to_str().unwrap().to_string())
                        .collect::<Vec<_>>(),
                );

                children.insert(name.clone(), child_config);
            }
            Ok((root_config, children))
        } else {
            // Single project
            Ok((root_config, children))
        }
    }

    pub fn new(path: &Path) -> anyhow::Result<ProjectConfig> {
        let (file, path) = Self::open_file(&path.join(CONFIG_FILE[0]))
            .or_else(|_| Self::open_file(&path.join(CONFIG_FILE[1])))
            .with_context(|| {
                format!(
                    "cannot open config file ({} or {}) in directory {:?}",
                    CONFIG_FILE[0], CONFIG_FILE[1], path
                )
            })?;
        let reader = BufReader::new(file);
        let mut data: ProjectConfig = serde_yaml::from_reader(reader)
            .with_context(|| format!("cannot parse config file {:?} as YAML", path))?;
        data.dir = path
            .to_path_buf()
            .parent()
            .map(|p| p.to_path_buf())
            .with_context(|| format!("cannot get the directory of {:?}", path))?;

        Ok(data)
    }

    fn open_file(path: &Path) -> Result<(File, PathBuf), io::Error> {
        match File::open(path) {
            Ok(file) => Ok((file, path.to_owned())),
            Err(e) => Err(e),
        }
    }

    fn find_root(cwd: &Path) -> anyhow::Result<ProjectConfig> {
        let config = ProjectConfig::new(cwd)?;
        if config.is_root() {
            return Ok(config);
        }
        for current_dir in cwd.ancestors() {
            match ProjectConfig::new(current_dir) {
                Ok(root_candidate) => {
                    if root_candidate.is_root() && config.is_child(&root_candidate) {
                        return Ok(root_candidate);
                    }
                }
                Err(err) => {
                    if err.downcast_ref::<io::Error>().map(|e| e.kind())
                        == Some(io::ErrorKind::NotFound)
                    {
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
        root.projects.values().any(|p| Path::new(p) == self.dir)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskConfig {
    /// Command to run.
    pub command: String,

    /// Shell configuration.
    pub shell: Option<ShellConfig>,

    /// Working directory.
    pub working_dir: Option<String>,

    /// Environment variables.
    /// Merged with the project's env.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Dotenv files.
    /// Merged with the project's env.
    #[serde(default)]
    pub env_files: Vec<String>,

    /// Names of Tasks on which this task depends.
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Service configuration.
    pub service: Option<ServiceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ShellConfig {
    /// Shell command.
    pub command: String,

    /// Arguments of the shell command.
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    pub file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum HealthCheckConfig {
    Log(LogHealthCheckerConfig),
    Exec(ExecHealthCheckerConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LogHealthCheckerConfig {
    pub log: String,
    #[serde(default = "default_log_healthcheck_timeout")]
    pub timeout: u64,
    #[serde(default = "default_healthcheck_start_period")]
    pub start_period: u64,
}

pub fn default_log_healthcheck_timeout() -> u64 {
    120
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecHealthCheckerConfig {
    pub command: String,

    pub working_dir: Option<String>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    pub shell: Option<ShellConfig>,

    #[serde(default = "default_healthcheck_interval")]
    pub interval: u64,

    #[serde(default = "default_healthcheck_timeout")]
    pub timeout: u64,

    #[serde(default = "default_healthcheck_retries")]
    pub retries: u64,

    #[serde(default = "default_healthcheck_start_period")]
    pub start_period: u64,
}

pub fn default_healthcheck_interval() -> u64 {
    30
}
pub fn default_healthcheck_timeout() -> u64 {
    30
}
pub fn default_healthcheck_retries() -> u64 {
    3
}
pub fn default_healthcheck_start_period() -> u64 {
    0
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ServiceConfig {
    Bool(bool),
    Struct(ServiceConfigStruct),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ServiceConfigStruct {
    pub healthcheck: Option<HealthCheckConfig>,

    #[serde(default = "default_service_restart")]
    pub restart: Restart,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    Always,
    OnFailure,
    Never,
}

pub fn default_service_restart() -> Restart {
    Restart::Never
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub enum UI {
    #[serde(rename = "cui")]
    Cui,
    #[serde(rename = "tui")]
    Tui,
}
