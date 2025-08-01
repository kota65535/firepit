use crate::config::{DependsOnConfig, HealthCheckConfig, ProjectConfig, Restart, ServiceConfig, TaskConfig, UI};
use crate::probe::{ExecProbe, LogLineProbe, Probe};
use crate::template::ConfigRenderer;
use anyhow::Context;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: Project,
    pub children: HashMap<String, Project>,
    pub tasks: Vec<Task>,
    pub target_tasks: Vec<String>,
    pub labels: HashMap<String, String>,
    pub ui: UI,
    pub use_pty: bool,
    pub concurrency: usize,
    pub force: bool,
    pub dir: PathBuf,
}

impl Workspace {
    pub fn new(
        root_config: &ProjectConfig,
        child_configs: &HashMap<String, ProjectConfig>,
        tasks: &Vec<String>,
        current_dir: &Path,
        vars: &HashMap<String, String>,
        env: &HashMap<String, String>,
        force: bool,
    ) -> anyhow::Result<Workspace> {
        let mut target_tasks = Vec::new();
        for task in tasks.iter() {
            let (project_name, task_name) = Task::split_name(task);
            match project_name {
                // Full name
                Some(project_name) => {
                    let task = if project_name.is_empty() {
                        root_config.task(task_name)?
                    } else {
                        child_configs
                            .get(project_name)
                            .with_context(|| format!("project {:?} is not defined", project_name))?
                            .task(task_name)?
                    };
                    target_tasks.push(task.full_name());
                }
                // Simple name
                None => {
                    if current_dir == root_config.dir {
                        // Select the task if exists in the root project.
                        // If not, select all tasks with the name in the child projects.
                        let tasks = match root_config.task(task_name) {
                            Ok(task) => vec![task],
                            Err(_) => child_configs.values().map(|c| c.task(task_name)).flatten().collect(),
                        };
                        if tasks.is_empty() {
                            anyhow::bail!("task {:?} is not defined.", task)
                        }
                        target_tasks.extend(tasks.iter().map(|t| t.full_name()));
                    } else {
                        let task = child_configs
                            .values()
                            .find(|c| current_dir == c.dir)
                            .with_context(|| format!("project {:?} is not defined", project_name))?
                            .task(task_name)?;
                        target_tasks.push(task.full_name());
                    }
                }
            }
        }

        let mut root_config = root_config.clone();
        let mut child_configs = child_configs.clone();

        for t in target_tasks.iter() {
            let (project_name, task_name) = Task::split_name(t);
            if let Some(project_name) = project_name {
                let task = if project_name.is_empty() {
                    root_config.task_mut(task_name)?
                } else {
                    child_configs
                        .get_mut(project_name)
                        .with_context(|| format!("project {:?} is not defined", project_name))?
                        .task_mut(task_name)?
                };
                task.vars.extend(vars.clone());
                task.env.extend(env.clone());
            }
        }

        let mut renderer = ConfigRenderer::new(&root_config, &child_configs);
        let (root_config, child_config) = renderer.render()?;

        let root = Project::new("", &root_config)?;
        let mut children = HashMap::new();
        for (k, v) in child_config.iter() {
            children.insert(k.clone(), Project::new(k, v)?);
        }

        let mut tasks = root.tasks.values().cloned().collect::<Vec<_>>();
        for p in children.values() {
            tasks.extend(p.tasks.values().cloned().collect::<Vec<_>>());
        }

        let labels = tasks
            .iter()
            .cloned()
            .map(|t| (t.name, t.label))
            .collect::<HashMap<_, _>>();

        // Determine UI.
        // - If stdout is not a TTY, use CUI
        // - If UI is explicitly set in the CLI option or the root config, use it
        // - If UI is not set, determine based on tasks
        let ui = if !atty::is(atty::Stream::Stdout) {
            UI::Cui
        } else {
            match root_config.ui {
                Some(ui) => ui,
                None => {
                    if tasks.iter().any(|t| target_tasks.contains(&t.name) && t.is_service) {
                        UI::Tui
                    } else {
                        UI::Cui
                    }
                }
            }
        };

        let use_pty = match ui {
            UI::Tui => true,
            UI::Cui => false,
        };

        Ok(Self {
            root,
            children,
            tasks,
            target_tasks,
            labels,
            ui,
            use_pty,
            concurrency: root_config.concurrency,
            force,
            dir: current_dir.to_owned(),
        })
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
            tasks: Task::new_multi(name, &root)?,
            dir: root.dir.clone(),
        })
    }

    pub fn task(&self, name: &String) -> Option<Task> {
        self.tasks.get(&Task::qualified_name(&self.name, name)).cloned()
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    /// Unique task name
    pub name: String,

    /// Label
    pub label: String,

    /// Command to run
    pub command: String,

    /// Shell command
    pub shell: String,

    /// Shell command arguments
    pub shell_args: Vec<String>,

    /// Environment variables
    pub env: HashMap<String, String>,

    /// Dependency task names
    pub depends_on: Vec<DependsOn>,

    /// Task working directory path (absolute).
    pub working_dir: PathBuf,

    /// Whether this task is a service or not
    pub is_service: bool,

    /// Health checker
    pub probe: Probe,

    /// Restart setting
    pub restart: Restart,

    /// Input files
    pub inputs: Vec<PathBuf>,

    /// Output files
    pub outputs: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct DependsOn {
    pub task: String,

    pub cascade: bool,
}

impl Task {
    pub fn new_multi(project_name: &str, config: &ProjectConfig) -> anyhow::Result<HashMap<String, Task>> {
        let mut ret = HashMap::new();
        for (task_name, task_config) in config.tasks.iter() {
            let task = Self::new(project_name, &config, task_name, task_config)?;
            ret.insert(task.name.clone(), task);
        }

        Ok(ret)
    }

    pub fn new(
        project_name: &str,
        config: &ProjectConfig,
        task_name: &str,
        task_config: &TaskConfig,
    ) -> anyhow::Result<Task> {
        if task_name.contains("#") {
            anyhow::bail!("Task name must not contain '#'. Found: {:?}", task_name)
        }

        let task_name = Task::qualified_name(project_name, task_name);

        // Shell
        let task_shell = task_config.shell.clone().unwrap_or(config.shell.clone());

        // Working directory
        let task_working_dir = task_config
            .working_dir_path(&config.dir)
            .unwrap_or(config.working_dir_path());

        // Environment variables
        // Priority:
        // 1. Root project env file
        // 2. Root project env
        // 3. Project env file
        // 4. Project env
        // 5. Task env file
        // 6. Task env
        let project_env = Self::merge_env(Self::load_env_files(&config.env_files_paths())?, config.env.clone())?;
        let task_env = Self::merge_env(
            Self::load_env_files(&task_config.env_file_paths(&config.dir))?,
            task_config.env.clone(),
        )?;
        let merged_task_env = Self::merge_env(project_env, task_env)?;

        // Input files
        let inputs = task_config
            .input_paths(&config.dir)
            .into_iter()
            .chain(task_config.env_file_paths(&config.dir).into_iter())
            .collect::<Vec<_>>();

        // Output files
        let outputs = task_config
            .output_paths(&config.dir)
            .into_iter()
            .chain(task_config.env_file_paths(&config.dir).into_iter())
            .collect::<Vec<_>>();

        // Probes
        let (is_service, probe, restart) = match task_config.service.clone() {
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
                            )),
                            // Exec Probe
                            HealthCheckConfig::Exec(c) => {
                                // Shell
                                let hc_shell = c.shell.clone().unwrap_or(task_shell.clone());
                                // Working directory
                                let hc_working_dir =
                                    c.working_dir_path(&config.dir).unwrap_or(task_working_dir.clone());
                                // Environment variables
                                let hc_env = Self::merge_env(
                                    Self::load_env_files(&c.env_files_paths(&config.dir))?,
                                    c.env.clone(),
                                )?;
                                let merged_hc_env = Self::merge_env(merged_task_env.clone(), hc_env)?;

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

        Ok(Self {
            name: Task::qualified_name(project_name, &task_name),
            label: task_config.label.clone().unwrap_or(task_name),
            command: task_config.command.clone().unwrap_or("".to_string()),
            shell: task_shell.command,
            shell_args: task_shell.args,
            working_dir: task_working_dir,
            env: merged_task_env,
            depends_on: task_config
                .depends_on
                .iter()
                .map(|s| match s {
                    DependsOnConfig::String(s) => DependsOn {
                        task: Task::qualified_name(project_name, s),
                        cascade: true,
                    },
                    DependsOnConfig::Struct(s) => DependsOn {
                        task: Task::qualified_name(project_name, &s.task),
                        cascade: s.cascade,
                    },
                })
                .collect(),
            is_service,
            probe,
            restart,
            inputs,
            outputs,
        })
    }

    fn merge_env(a: HashMap<String, String>, b: HashMap<String, String>) -> anyhow::Result<HashMap<String, String>> {
        Ok(a.into_iter().chain(b.into_iter()).collect::<HashMap<_, _>>())
    }

    fn load_env_files(files: &Vec<PathBuf>) -> anyhow::Result<HashMap<String, String>> {
        let mut ret = HashMap::new();
        for f in files.iter() {
            for item in dotenvy::from_path_iter(f).with_context(|| format!("cannot read env file {:?}", f))? {
                let (key, value) = item.with_context(|| format!("cannot parse env file {:?}", f))?;
                ret.insert(key, value);
            }
        }
        Ok(ret)
    }

    pub fn split_name(task_name: &str) -> (Option<&str>, &str) {
        if task_name.contains('#') {
            if let Some((p, t)) = task_name.split_once('#') {
                return (Some(p), t);
            }
        }
        (None, task_name)
    }

    pub fn qualified_name(project_name: &str, task_name: &str) -> String {
        if task_name.contains('#') {
            task_name.to_string()
        } else {
            format!("{}#{}", project_name, task_name)
        }
    }

    pub fn match_inputs(&self, paths: &HashSet<PathBuf>) -> bool {
        self.inputs
            .iter()
            .map(|i| {
                self.match_glob(i.to_str().unwrap_or(""), paths).unwrap_or_else(|e| {
                    warn!("{:?}", e);
                    false
                })
            })
            .any(|b| b)
    }

    pub fn is_up_to_date(&self) -> bool {
        if self.is_service {
            return false;
        }
        if self.inputs.is_empty() || self.outputs.is_empty() {
            return false;
        }
        let mut input_modified_time: u64 = 0;
        for p in self.inputs.iter() {
            let paths = self.glob(p).unwrap_or_else(|e| {
                warn!("{:?}", e);
                Vec::new()
            });
            let modified_time = self.latest_modified_time(&paths);
            if modified_time > input_modified_time {
                input_modified_time = modified_time;
            }
        }
        let mut output_modified_time: u64 = 0;
        for p in self.outputs.iter() {
            let paths = self.glob(p).unwrap_or_else(|e| {
                warn!("{:?}", e);
                Vec::new()
            });
            let modified_time = self.latest_modified_time(&paths);
            if modified_time > output_modified_time {
                output_modified_time = modified_time;
            }
        }
        input_modified_time < output_modified_time
    }

    fn latest_modified_time(&self, paths: &Vec<PathBuf>) -> u64 {
        let timestamps = paths
            .iter()
            .map(|p| self.modified_time(p))
            .collect::<anyhow::Result<Vec<_>>>()
            .unwrap_or_else(|e| {
                warn!("{:?}", e);
                Vec::new()
            });
        timestamps.into_iter().flatten().max().unwrap_or(0)
    }

    fn match_glob(&self, pattern: &str, path: &HashSet<PathBuf>) -> anyhow::Result<bool> {
        let glob = globmatch::Builder::new(pattern)
            .build_glob()
            .map_err(|e| anyhow::anyhow!("cannot build glob pattern: {:?}", e))?;
        Ok(path.iter().any(|p| glob.is_match(p)))
    }

    fn modified_time(&self, path: &Path) -> anyhow::Result<Option<u64>> {
        let metadata = std::fs::metadata(&path).with_context(|| format!("failed to get metadata of {:?}", path))?;
        if !metadata.is_file() {
            return Ok(None);
        }
        let modified_time = metadata
            .modified()
            .with_context(|| format!("failed to get modified time of {:?}", path))?;
        let duration_since_epoch = modified_time.duration_since(std::time::UNIX_EPOCH)?;
        let timestamp = duration_since_epoch.as_secs();
        Ok(Some(timestamp))
    }

    fn glob(&self, pattern: &PathBuf) -> anyhow::Result<Vec<PathBuf>> {
        let file_name = pattern.file_name().map(|f| f.to_string_lossy());
        let dir_name = pattern.parent();

        match (file_name, dir_name) {
            (Some(file_name), Some(dir_name)) => {
                let matcher = globmatch::Builder::new(file_name.as_ref())
                    .build(dir_name)
                    .map_err(|e| anyhow::anyhow!("cannot build glob pattern: {:?}", e))?;
                Ok(matcher.into_iter().flatten().collect::<Vec<_>>())
            }
            _ => Ok(vec![]),
        }
    }
}
