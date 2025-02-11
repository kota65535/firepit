use crate::config::{HealthCheckConfig, ProjectConfig, Restart, ServiceConfig};
use crate::probe::{ExecProbe, LogLineProbe, Probe};
use anyhow::Context;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: Project,
    pub children: HashMap<String, Project>,
    pub concurrency: usize,
}

impl Workspace {
    pub fn new(
        root_config: &ProjectConfig,
        child_config: &HashMap<String, ProjectConfig>,
    ) -> anyhow::Result<Workspace> {
        let root = Project::new("", root_config)?;
        let mut children = HashMap::new();
        for (k, v) in child_config.iter() {
            children.insert(k.clone(), Project::new(k, v)?);
        }

        Self::validate_projects(&root, &children)?;

        Ok(Self {
            root,
            children,
            concurrency: root_config.concurrency,
        })
    }

    fn validate_projects(root: &Project, children: &HashMap<String, Project>) -> anyhow::Result<()> {
        let mut tasks = root.tasks.values().map(|t| t.name.clone()).collect::<HashSet<_>>();
        for p in children.values() {
            tasks.extend(p.tasks.values().map(|t| t.name.clone()));
        }

        let mut deps = root
            .tasks
            .values()
            .flat_map(|t| t.depends_on.iter())
            .collect::<HashSet<_>>();
        for p in children.values() {
            deps.extend(
                p.tasks
                    .values()
                    .flat_map(|t| t.depends_on.iter())
                    .collect::<HashSet<_>>(),
            );
        }

        for d in deps.iter() {
            if !tasks.contains(*d) {
                anyhow::bail!("task {:?} is not defined.", d);
            }
        }

        Ok(())
    }

    pub fn tasks(&self) -> Vec<Task> {
        // All tasks
        let mut tasks = self.root.tasks.values().cloned().collect::<Vec<_>>();
        for p in self.children.values() {
            tasks.extend(p.tasks.values().cloned().collect::<Vec<_>>());
        }
        tasks
    }

    pub fn target_tasks(&self, tasks: &Vec<String>, current_dir: &Path) -> anyhow::Result<Vec<String>> {
        if self.root.dir == current_dir {
            let mut target_tasks = Vec::new();
            for t in tasks.iter() {
                let target = match self.root.task(t) {
                    Some(t) => vec![t.name],
                    None => {
                        let child_tasks = self
                            .children
                            .values()
                            .filter_map(|p| p.task(t))
                            .map(|t| t.name)
                            .collect::<Vec<_>>();
                        if child_tasks.is_empty() {
                            anyhow::bail!("task {:?} is not defined in any project", t);
                        }
                        child_tasks
                    }
                };
                target_tasks.extend(target);
            }
            Ok(target_tasks)
        } else {
            let child = self
                .children
                .values()
                .find(|c| c.dir == current_dir)
                .with_context(|| format!("directory {:?} is not part of any projects", current_dir))?;
            for t in tasks.iter() {
                child
                    .task(t)
                    .with_context(|| format!("task {:?} is not defined in project {:?}", t, child.name))?;
            }
            Ok(tasks.clone())
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    /// Project name.
    pub name: String,

    /// Project tasks.
    pub tasks: HashMap<String, Task>,

    /// Absolute path of the project directory.
    pub dir: PathBuf,
}

impl Project {
    pub fn new(name: &str, root: &ProjectConfig) -> anyhow::Result<Project> {
        Ok(Project {
            name: name.to_owned(),
            tasks: Task::from_project_config(name, &root)?,
            dir: root.dir.clone(),
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

    pub probe: Probe,

    pub restart: Restart,
}

impl Task {
    pub fn from_project_config(project_name: &str, config: &ProjectConfig) -> anyhow::Result<HashMap<String, Task>> {
        let mut ret = HashMap::new();
        for (task_name, task_config) in config.tasks.iter() {
            if task_name.contains("#") {
                anyhow::bail!("Task name must not contain '#'. Found: {:?}", task_name)
            }
            let task_config = task_config.clone();
            let task_name = Task::qualified_name(project_name, task_name);

            // Shell
            let task_shell = task_config.shell.clone().unwrap_or(config.shell.clone());

            // Working directory
            let task_working_dir = task_config
                .working_dir_path(&config.dir)
                .unwrap_or(config.working_dir_path());

            // Environment variables
            let project_env = Self::load_env_files(&config.env_files_paths())?
                .into_iter()
                .chain(config.env.clone().into_iter())
                .collect::<HashMap<_, _>>();
            let task_env = Self::load_env_files(&task_config.env_files_paths(&config.dir))?
                .into_iter()
                .chain(task_config.env.clone().into_iter())
                .collect::<HashMap<_, _>>();
            let merged_task_env = project_env
                .into_iter()
                .chain(task_env.clone().into_iter())
                .collect::<HashMap<_, _>>();

            // Probes
            let (is_service, probe, restart) = match task_config.service {
                Some(service) => match service {
                    ServiceConfig::Bool(bool) => (bool, Probe::None, Restart::Never),
                    ServiceConfig::Struct(st) => {
                        let probe = match st.healthcheck {
                            Some(healthcheck) => match healthcheck {
                                // Log Probe
                                HealthCheckConfig::Log(c) => Probe::LogLine(LogLineProbe::new(
                                    &task_name,
                                    Regex::new(&c.log).with_context(|| format!("invalid regex pattern {:?}", c.log))?,
                                    c.timeout,
                                    c.start_period,
                                )),
                                // Exec Probe
                                HealthCheckConfig::Exec(c) => {
                                    // Shell
                                    let hc_shell = c.shell.clone().unwrap_or(task_shell.clone());
                                    // Working directory
                                    let hc_working_dir =
                                        c.working_dir_path(&config.dir).unwrap_or(task_working_dir.clone());
                                    // Environment variables
                                    let hc_env = Self::load_env_files(&c.env_files_paths(&config.dir))?
                                        .into_iter()
                                        .chain(c.env.clone().into_iter())
                                        .collect::<HashMap<_, _>>();
                                    let merged_hc_env = task_env
                                        .into_iter()
                                        .chain(hc_env.into_iter())
                                        .collect::<HashMap<_, _>>();

                                    Probe::Exec(ExecProbe::new(
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
                            None => Probe::None,
                        };
                        (true, probe, st.restart)
                    }
                },
                _ => (false, Probe::None, Restart::Never),
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
                    probe,
                    restart,
                },
            );
        }
        Ok(ret)
    }

    pub fn load_env_files(files: &Vec<PathBuf>) -> anyhow::Result<HashMap<String, String>> {
        let mut ret = HashMap::new();
        for f in files.iter() {
            for item in dotenvy::from_path_iter(f).with_context(|| format!("cannot read env file {:?}", f))? {
                let (key, value) = item.with_context(|| format!("cannot parse env file {:?}", f))?;
                ret.insert(key, value);
            }
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
