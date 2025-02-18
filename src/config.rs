use anyhow::Context;
use lazy_static::lazy_static;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::thread::available_parallelism;
use std::{io, path};

const CONFIG_FILE: [&str; 2] = ["firepit.yml", "firepit.yaml"];

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ProjectConfig {
    /// Child projects.
    /// Valid only in root project config.
    #[serde(default)]
    pub projects: HashMap<String, String>,

    /// Shell configuration for all tasks
    #[serde(default = "default_shell")]
    pub shell: ShellConfig,

    /// Working directory for all tasks
    #[serde(default = "default_working_dir")]
    pub working_dir: String,

    /// Environment variables of all tasks.
    /// Merged with those of child projects.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Dotenv files for all tasks.
    /// Merged with those of child projects.
    /// If environment variable duplicates, the later one wins.
    #[serde(default)]
    pub env_files: Vec<String>,

    /// Task definitions
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

    /// project directory path (absolute)
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
    pub fn new_multi(dir: &Path) -> anyhow::Result<(ProjectConfig, HashMap<String, ProjectConfig>)> {
        let dir = path::absolute(dir)?;
        let root_config = ProjectConfig::find_root(&dir)?;
        let mut children = HashMap::new();
        if root_config.is_root() {
            // Multi project
            for (name, path) in &root_config.projects {
                if name.contains("#") {
                    anyhow::bail!("Project name must not contain '#'. Found: {:?}", name)
                }
                let mut child_config = ProjectConfig::new(root_config.dir.join(path).as_path())?;
                root_config.env.iter().for_each(|(k, v)| {
                    child_config.env.entry(k.clone()).or_insert(v.clone());
                });
                child_config.env_files = root_config
                    .env_files
                    .clone()
                    .iter()
                    .map(|f| root_config.dir.join(f).to_str().unwrap().to_string())
                    .chain(child_config.env_files)
                    .collect();
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
        let mut data: ProjectConfig =
            serde_yaml::from_reader(reader).with_context(|| format!("cannot parse config file {:?}.", path))?;

        // Get project dir
        data.dir = path
            .to_path_buf()
            .parent()
            .map(|p| p.to_path_buf())
            .with_context(|| format!("cannot read the parent directory of {:?}", path))?;

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

    pub fn env_files_paths(&self) -> Vec<PathBuf> {
        self.env_files.iter().map(|f| absolute_or_join(f, &self.dir)).collect()
    }

    pub fn schema() -> anyhow::Result<String> {
        let schema = schemars::schema_for!(ProjectConfig);
        serde_json::to_string_pretty(&schema).context("cannot create config scehma")
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TaskConfig {
    /// Command to run
    pub command: String,

    /// Shell configuration
    pub shell: Option<ShellConfig>,

    /// Working directory
    pub working_dir: Option<String>,

    /// Environment variables.
    /// Merged with the project's env.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Dotenv files
    /// Merged with the project's env.
    #[serde(default)]
    pub env_files: Vec<String>,

    /// Dependency task names
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Service configuration
    pub service: Option<ServiceConfig>,

    /// Inputs file glob patterns
    #[serde(default)]
    pub inputs: Vec<String>,

    /// Output file glob patterns
    #[serde(default)]
    pub outputs: Vec<String>,
}

impl TaskConfig {
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

fn absolute_or_join(path: &str, dir: &PathBuf) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ShellConfig {
    /// Shell command.
    pub command: String,

    /// Arguments of the shell command.
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    pub file: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum HealthCheckConfig {
    Log(LogProbeConfig),
    Exec(ExecProbeConfig),
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LogProbeConfig {
    pub log: String,
    #[serde(default = "default_log_healthcheck_timeout")]
    pub timeout: u64,
    #[serde(default = "default_healthcheck_start_period")]
    pub start_period: u64,
}

pub fn default_log_healthcheck_timeout() -> u64 {
    120
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExecProbeConfig {
    pub command: String,

    pub working_dir: Option<String>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    #[serde(default)]
    pub env_files: Vec<String>,

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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ServiceConfig {
    Bool(bool),
    Struct(ServiceConfigStruct),
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ServiceConfigStruct {
    pub healthcheck: Option<HealthCheckConfig>,

    #[serde(default = "default_service_restart")]
    pub restart: Restart,
}

#[derive(Debug, Clone, JsonSchema)]
pub enum Restart {
    Always(u64),
    OnFailure(u64),
    Never,
}

lazy_static! {
    pub static ref ALWAYS: Regex = Regex::new(r"^always(:(\d+))?$").unwrap();
    pub static ref ON_FAILURE: Regex = Regex::new(r"^on-failure(:(\d+))?$").unwrap();
}

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
                    .and_then(|m| m.as_str().parse::<u64>().ok())
                    .unwrap_or(0);
                Restart::Always(num)
            }
            s if ON_FAILURE.is_match(&s) => {
                let num = ON_FAILURE
                    .captures(s.as_str())
                    .and_then(|c| c.get(2))
                    .and_then(|m| m.as_str().parse::<u64>().ok())
                    .unwrap_or(0);
                Restart::Always(num)
            }
            s if s == "never" => Restart::Never,
            _ => return Err(serde::de::Error::custom(format!("invalid restart value: {}", s))),
        };
        Ok(r)
    }
}

pub fn default_service_restart() -> Restart {
    Restart::Never
}

#[derive(Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub enum UI {
    #[serde(rename = "cui")]
    Cui,
    #[serde(rename = "tui")]
    Tui,
}
