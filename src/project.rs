use crate::config::{HealthCheckConfig, ProjectConfig, Restart, ServiceConfig};
use crate::probe::{ExecProber, LogLineProber, Prober};
use anyhow::Context;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Project {
    /// Project name.
    pub name: String,

    /// Project tasks.
    pub tasks: HashMap<String, Task>,

    /// Absolute path of the project directory.
    pub dir: PathBuf,

    /// Task concurrency.
    pub concurrency: usize,
}

impl Project {
    pub fn new(name: &str, config: &ProjectConfig) -> anyhow::Result<Project> {
        let config = config.clone();

        Ok(Project {
            name: name.to_owned(),
            tasks: Task::from_project_config(name, &config)?,
            dir: config.dir.clone(),
            concurrency: config.concurrency,
        })
    }

    pub fn task(&self, name: &String) -> Option<Task> {
        self.tasks.get(&Task::qualified_name(&self.name, name)).cloned()
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,

    pub command: String,

    pub shell: String,

    pub shell_args: Vec<String>,

    pub env: HashMap<String, String>,

    pub depends_on: Vec<String>,

    /// Task working directory path (absolute).
    pub working_dir: PathBuf,

    pub is_service: bool,

    pub prober: Prober,

    pub restart: Restart,
}

pub fn load_env_files(files: &Vec<String>) -> anyhow::Result<HashMap<String, String>> {
    let mut ret = HashMap::new();
    for f in files.iter() {
        for item in dotenvy::from_path_iter(Path::new(f)).with_context(|| format!("cannot read env file {:?}", f))? {
            let (key, value) = item.with_context(|| format!("cannot parse env file {:?}", f))?;
            ret.insert(key, value);
        }
    }
    Ok(ret)
}

impl Task {
    pub fn from_project_config(project_name: &str, config: &ProjectConfig) -> anyhow::Result<HashMap<String, Task>> {
        let mut ret = HashMap::new();
        for (task_name, task_config) in config.tasks.iter() {
            let task_config = task_config.clone();
            let task_name = Task::qualified_name(project_name, task_name);

            // Shell
            let task_shell = task_config.shell.clone().unwrap_or(config.shell.clone());

            // Working directory
            let mut task_working_dir = PathBuf::from(task_config.working_dir.unwrap_or(config.working_dir.clone()));
            task_working_dir = if task_working_dir.is_absolute() {
                task_working_dir
            } else {
                config.dir.join(task_working_dir)
            };

            // Environment variables
            let project_env = load_env_files(&config.env_files)?
                .into_iter()
                .chain(config.env.clone().into_iter())
                .collect::<HashMap<_, _>>();
            let task_env = load_env_files(&task_config.env_files)?
                .into_iter()
                .chain(task_config.env.clone().into_iter())
                .collect::<HashMap<_, _>>();
            let merged_task_env = project_env
                .into_iter()
                .chain(task_env.clone().into_iter())
                .collect::<HashMap<_, _>>();

            // Probes
            let (is_service, prober, restart) = match task_config.service {
                Some(service) => match service {
                    ServiceConfig::Bool(bool) => (bool, Prober::None, Restart::Never),
                    ServiceConfig::Struct(st) => {
                        let prober = match st.healthcheck {
                            Some(healthcheck) => match healthcheck {
                                // Log Probe
                                HealthCheckConfig::Log(c) => Prober::LogLine(LogLineProber::new(
                                    &task_name,
                                    Regex::new(&c.log).with_context(|| format!("invalid regex pattern {:?}", c.log))?,
                                    c.timeout,
                                    c.start_period,
                                )),
                                // Exec Probe
                                HealthCheckConfig::Exec(c) => {
                                    // Shell
                                    let hc_shell = c.shell.unwrap_or(task_shell.clone());
                                    // Working directory
                                    let mut hc_working_dir =
                                        c.working_dir.map(PathBuf::from).unwrap_or(task_working_dir.clone());
                                    hc_working_dir = if hc_working_dir.is_absolute() {
                                        hc_working_dir
                                    } else {
                                        config.dir.join(hc_working_dir)
                                    };
                                    // Environment variables
                                    let hc_env = load_env_files(&c.env_files)?
                                        .into_iter()
                                        .chain(c.env.clone().into_iter())
                                        .collect::<HashMap<_, _>>();
                                    let merged_hc_env = task_env
                                        .into_iter()
                                        .chain(hc_env.into_iter())
                                        .collect::<HashMap<_, _>>();

                                    Prober::Exec(ExecProber::new(
                                        &task_name,
                                        &c.command,
                                        &hc_shell.command,
                                        hc_shell.args,
                                        hc_working_dir,
                                        merged_hc_env,
                                        c.interval,
                                        c.timeout,
                                        c.retries,
                                        c.start_period,
                                    ))
                                }
                            },
                            None => Prober::None,
                        };
                        (true, prober, st.restart)
                    }
                },
                _ => (false, Prober::None, Restart::Never),
            };

            ret.insert(
                task_name.clone(),
                Task {
                    name: task_name.clone(),
                    command: task_config.command,
                    shell: task_shell.command,
                    shell_args: task_shell.args,
                    working_dir: task_working_dir,
                    env: merged_task_env,
                    depends_on: task_config
                        .depends_on
                        .iter()
                        .map(|s| Task::qualified_name(project_name, s))
                        .collect(),
                    is_service,
                    prober,
                    restart,
                },
            );
        }
        Ok(ret)
    }
    pub fn qualified_name(project_name: &str, task_name: &str) -> String {
        if task_name.contains('#') {
            task_name.to_string()
        } else {
            format!("{}#{}", project_name, task_name)
        }
    }
}
