use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::thread::available_parallelism;
use std::io;

const CONFIG_FILE: [&str; 2] = ["fire.yml", "fire.yaml"];

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
    pub concurrency: Option<usize>,

    /// Project directory.
    #[serde(skip)]
    pub dir: PathBuf,
}

pub fn default_shell() -> ShellConfig {
    ShellConfig {
        command: "bash".to_string(),
        args: vec!("-c".to_string())
    }
}

pub fn default_concurrency() -> usize {
    available_parallelism().unwrap().get()
}

impl ProjectConfig {
    pub fn new_multi(dir: &Path) -> anyhow::Result<(ProjectConfig, HashMap<String, ProjectConfig>)> {
        let dir = dir.to_path_buf();
        let mut root_config = ProjectConfig::find_root(&dir)?;
        root_config.shell.get_or_insert(default_shell());
        root_config.concurrency.get_or_insert(default_concurrency());
        let mut children = HashMap::new();
        if root_config.is_root() {
            // Multi project
            for (name, path) in &root_config.projects {
                let mut child_config = ProjectConfig::new(dir.join(path).as_path())?;
                child_config.envs.extend(root_config.envs.clone());
                child_config.env_files.extend(root_config.env_files.clone());
                child_config.shell.get_or_insert(root_config.shell.clone().expect("should be default value"));
                child_config.concurrency.get_or_insert(root_config.concurrency.expect("should be default value"));

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
            .with_context(|| format!("Failed to open config file ({} or {}) in directory {:?}", CONFIG_FILE[0], CONFIG_FILE[1], path))?;
        let reader = BufReader::new(file);
        let mut data: ProjectConfig = serde_yaml::from_reader(reader)
            .with_context(|| format!("Failed to parse config file {:?} as YAML", path))?;
        data.dir = path.to_path_buf().parent()
            .map(|p| p.to_path_buf())
            .with_context(|| format!("Failed to get the directory of {:?}", path))?;

        Ok(data)
    }
    
    fn open_file(path: &Path) -> Result<(File, PathBuf), io::Error> {
        match File::open(path) {
            Ok(file) => {
                Ok((file, path.to_owned()))
            },
            Err(e) => {
                Err(e)
            }
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

    /// Input files of this task.
    #[serde(default)]
    pub inputs: Vec<String>,

    /// Output files of this task.
    #[serde(default)]
    pub outputs: Vec<String>,

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
pub struct ReadinessProbeConfig {
    pub log_line: Option<LogLineReadinessProbeConfig>,
    pub exec: Option<ExecReadinessProbeConfig>,
    pub initial_delay_seconds: Option<u64>,
    pub period_seconds: Option<u64>,
    pub timeout_seconds: Option<u64>,
    pub success_threshold: Option<u64>,
    pub failure_threshold: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LogLineReadinessProbeConfig {
    pub pattern: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExecReadinessProbeConfig {
    pub command: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServiceConfig {
    pub readiness_probe: Option<ReadinessProbeConfig>,
}

