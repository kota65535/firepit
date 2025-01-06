use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::thread::available_parallelism;

const CONFIG_FILE: &str = "fire.yml";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProjectConfig {
    pub projects: Option<HashMap<String, String>>,
    pub tasks: Option<HashMap<String, TaskConfig>>,
    
    #[serde(default = "default_interpreter")]
    pub interpreter: Option<String>,
    #[serde(default = "default_concurrency")]
    pub concurrency: Option<usize>,

    #[serde(skip)]
    pub dir: PathBuf,
}

pub fn default_interpreter() -> Option<String> {
    Some("bash".to_string())
}

pub fn default_concurrency() -> Option<usize> {
    Some(available_parallelism().unwrap().get())
}


impl ProjectConfig {
    pub fn new_multi(dir: &Path) -> anyhow::Result<(ProjectConfig, HashMap<String, ProjectConfig>)> {
        let config = ProjectConfig::find_root(dir)?;
        let mut children = HashMap::new();
        if config.is_root() {
            // Multi project
            if let Some(projects) = config.projects.as_ref() {
                for (name, path) in projects {
                    let child_config = ProjectConfig::new(Path::new(dir).join(path).as_path())?;
                    children.insert(name.clone(), child_config);
                }
            }
            Ok((config, children))
        } else {
            // Single project
            Ok((config, children))
        }
    }

    pub fn new(path: &Path) -> anyhow::Result<ProjectConfig> {
        let file = File::open(path.join(CONFIG_FILE))
            .with_context(|| format!("Failed to open file {:?}", path))?;
        let reader = BufReader::new(file);
        let data: ProjectConfig = serde_yaml::from_reader(reader)
            .with_context(|| format!("Failed to parse YAML file {:?}", path))?;
        Ok(data)
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
                    if err.downcast_ref::<std::io::Error>().map(|e| e.kind()) == Some(std::io::ErrorKind::NotFound) {
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
        self.projects.is_some()
    }

    pub fn is_child(&self, root: &ProjectConfig) -> bool {
        match &root.projects {
            Some(projects) => {
                projects.values().any(|p| Path::new(p) == self.dir)
            }
            None => false
        }
    }
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
    pub pattern: String
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExecReadinessProbeConfig {
    pub command: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServiceConfig {
    pub readiness_probe: Option<ReadinessProbeConfig>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskConfig {
    pub command: String,
    pub depends_on: Option<Vec<String>>,
    pub service: Option<ServiceConfig>,
    pub inputs: Option<Vec<String>>,
    pub outputs: Option<Vec<String>>,
    pub working_dir: Option<PathBuf>,
}
