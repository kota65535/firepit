use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::thread::available_parallelism;

const CONFIG_FILE: [&str; 2] = ["firepit.yml", "firepit.yaml"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum UI {
    #[serde(rename = "cui")]
    Cui,
    #[serde(rename = "tui")]
    Tui,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProjectConfig {
    /// Child projects.
    /// Valid only in root project config.
    #[serde(default)]
    pub projects: HashMap<String, String>,

    /// Task definitions.
    #[serde(default)]
    pub tasks: HashMap<String, TaskConfig>,

    /// Environment variables set during execution of all tasks.
    /// Merged with the root project's one if this is a child project.
    #[serde(default)]
    pub envs: HashMap<String, String>,

    /// Files from where environment variables are loaded and set during execution of all tasks.
    /// Merged with the root project's one if this is a child project.
    #[serde(default)]
    pub env_files: Vec<String>,

    /// Shell configuration for all tasks.
    pub shell: Option<ShellConfig>,

    /// Task concurrency.
    /// Valid only in root project config.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,

    /// Log configuration.
    #[serde(default = "default_log")]
    pub log: LogConfig,

    /// UI configuration.
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

pub fn default_log() -> LogConfig {
    LogConfig {
        level: default_log_level(),
        file: None,
    }
}

pub fn default_log_level() -> String {
    "info".to_string()
}

pub fn default_concurrency() -> usize {
    available_parallelism().unwrap().get()
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
        let dir = dir.to_path_buf();
        let mut root_config = ProjectConfig::find_root(&dir)?;
        root_config.shell.get_or_insert(default_shell());
        let mut children = HashMap::new();
        if root_config.is_root() {
            // Multi project
            for (name, path) in &root_config.projects {
                let mut child_config = ProjectConfig::new(dir.join(path).as_path())?;
                child_config.envs.extend(root_config.envs.clone());
                child_config.env_files.extend(root_config.env_files.clone());
                child_config
                    .shell
                    .get_or_insert(root_config.shell.clone().expect("should be default value"));

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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskConfig {
    /// Command to run.
    pub command: String,

    /// Environment variables set during this task.
    /// Merged with the project environment variables.
    #[serde(default)]
    pub envs: HashMap<String, String>,

    /// Task names on which this task depends.
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Service configuration.
    pub service: Option<ServiceConfig>,

    /// Working directory of this task.
    pub working_dir: Option<PathBuf>,

    /// Shell configuration for this task.
    pub shell: Option<ShellConfig>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ShellConfig {
    pub command: String,

    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    pub file: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReadinessProbeConfig {
    pub log_line: Option<String>,
    pub exec: Option<ExecReadinessProbeConfig>,

    #[serde(default = "default_initial_delay_seconds")]
    pub initial_delay_seconds: u64,
    #[serde(default = "default_period_seconds")]
    pub period_seconds: u64,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_success_threshold")]
    pub success_threshold: u64,
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u64,
}

fn default_initial_delay_seconds() -> u64 {
    0
}
fn default_period_seconds() -> u64 {
    10
}
fn default_timeout_seconds() -> u64 {
    1
}
fn default_success_threshold() -> u64 {
    1
}
fn default_failure_threshold() -> u64 {
    3
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExecReadinessProbeConfig {
    pub command: String,
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub envs: HashMap<String, String>,
    pub shell: Option<ShellConfig>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum ServiceConfig {
    Bool(bool),
    Struct(ServiceConfigStruct),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServiceConfigStruct {
    pub readiness_probe: Option<ReadinessProbeConfig>,
    pub availability: Option<AvailabilityConfig>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AvailabilityConfig {
    pub restart: bool,
    pub backoff_seconds: u64,
    pub max_restarts: u64,
}
