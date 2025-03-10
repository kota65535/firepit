use crate::config::{HealthCheckConfig, ProjectConfig, Restart, ServiceConfig};
use crate::cui::lib::BOLD;
use crate::probe::{ExecProbe, LogLineProbe, Probe};
use anyhow::Context;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tera::Tera;
use tracing::warn;

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

    pub fn task(&self, name: &str) -> Option<Task> {
        self.root
            .tasks
            .values()
            .find(|t| t.name == name)
            .or_else(|| {
                self.children
                    .values()
                    .flat_map(|c| c.tasks.values())
                    .find(|t| t.name == name)
            })
            .cloned()
    }

    pub fn target_tasks(&self, tasks: &Vec<String>, current_dir: &Path) -> anyhow::Result<Vec<String>> {
        let mut target_tasks = Vec::new();
        if self.root.dir == current_dir {
            // In root directory...
            for t in tasks.iter() {
                // If the name is qualified, simply find it
                if t.contains('#') {
                    let found = self.task(t).with_context(|| format!("task {:?} is not defined", t))?;
                    target_tasks.push(found.name);
                } else {
                    // If the name is not qualified, search first in root tasks
                    // An if not found, search in child projects
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
            }
        } else {
            // In child directory...
            for t in tasks.iter() {
                // If the name is qualified, simply find it
                if t.contains('#') {
                    let found = self.task(t).with_context(|| format!("task {:?} is not defined", t))?;
                    target_tasks.push(found.name);
                } else {
                    // If the name is not qualified, search in the project of the current directory
                    let child = self
                        .children
                        .values()
                        .find(|c| c.dir == current_dir)
                        .with_context(|| format!("directory {:?} is not part of any projects", current_dir))?;
                    let found = child
                        .task(t)
                        .with_context(|| format!("task {:?} is not defined in project {:?}", t, child.name))?;
                    target_tasks.push(found.name);
                }
            }
        }
        Ok(target_tasks)
    }

    pub fn print_info(&self) {
        let mut lines = Vec::new();
        lines.push(format!(
            "{}:\n  dir: {:?}\n  tasks: {:?}",
            BOLD.apply_to("root").to_string(),
            self.root.dir,
            self.root.tasks.keys().collect::<Vec<_>>()
        ));
        for (k, v) in self.children.iter() {
            lines.push(format!(
                "{}:\n  dir: {:?}\n  tasks: {:?}",
                BOLD.apply_to(k).to_string(),
                v.dir,
                v.tasks.keys().collect::<Vec<_>>()
            ))
        }
        println!("{}", lines.join("\n"));
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
    /// Unique task name
    pub name: String,

    /// Command to run
    pub command: String,

    /// Shell command
    pub shell: String,

    /// Shell command arguments
    pub shell_args: Vec<String>,

    /// Environment variables
    pub env: HashMap<String, String>,

    /// Dependency task names
    pub depends_on: Vec<String>,

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

impl Task {
    fn merge_env(a: HashMap<String, String>, b: HashMap<String, String>) -> anyhow::Result<HashMap<String, String>> {
        Ok(a.into_iter().chain(b.into_iter()).collect::<HashMap<_, _>>())
    }

    pub fn from_project_config(project_name: &str, config: &ProjectConfig) -> anyhow::Result<HashMap<String, Task>> {
        let mut ret = HashMap::new();
        let mut tera = Tera::default();
        let mut base_context = tera::Context::new();
        for (k, v) in config.vars.iter() {
            base_context.insert(k.clone(), v);
        }
        for (task_name, task_config) in config.tasks.iter() {
            if task_name.contains("#") {
                anyhow::bail!("Task name must not contain '#'. Found: {:?}", task_name)
            }

            let mut context = base_context.clone();
            for (k, v) in task_config.vars.iter() {
                context.insert(k.clone(), v);
            }

            let mut task_config = task_config.clone();
            let task_name = Task::qualified_name(project_name, task_name);

            // Shell
            let task_shell = task_config.shell.clone().unwrap_or(config.shell.clone());

            // Render template for command, working_dir, env and env_files
            task_config.command = tera.render_str(&task_config.command, &context)?;
            task_config.working_dir = match task_config.working_dir {
                Some(w) => Some(tera.render_str(&w, &context)?),
                None => None,
            };
            let mut rendered_env = HashMap::new();
            for (k, v) in task_config.env.iter() {
                rendered_env.insert(tera.render_str(k, &context)?, tera.render_str(v, &context)?);
            }
            task_config.env = rendered_env;
            let mut rendered_env_files = Vec::new();
            for f in task_config.env_files.iter() {
                rendered_env_files.push(tera.render_str(f, &context)?);
            }
            task_config.env_files = rendered_env_files;

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
            let (is_service, probe, restart) = match task_config.service {
                Some(service) => match service {
                    ServiceConfig::Bool(bool) => (bool, Probe::None, Restart::Never),
                    ServiceConfig::Struct(st) => {
                        let probe = match st.healthcheck {
                            Some(healthcheck) => match healthcheck {
                                // Log Probe
                                HealthCheckConfig::Log(c) => {
                                    let log = tera.render_str(&c.log, &context)?;
                                    Probe::LogLine(LogLineProbe::new(
                                        &task_name,
                                        Regex::new(&log)
                                            .with_context(|| format!("invalid regex pattern {:?}", c.log))?,
                                        c.timeout,
                                        c.start_period,
                                    ))
                                }
                                // Exec Probe
                                HealthCheckConfig::Exec(mut c) => {
                                    // Render template for command, working_dir, env and env_files
                                    c.command = tera.render_str(&c.command, &context)?;
                                    c.working_dir = match c.working_dir {
                                        Some(w) => Some(tera.render_str(&w, &context)?),
                                        None => None,
                                    };
                                    let mut rendered_env = HashMap::new();
                                    for (k, v) in c.env.iter() {
                                        rendered_env
                                            .insert(tera.render_str(k, &context)?, tera.render_str(v, &context)?);
                                    }
                                    c.env = rendered_env;
                                    let mut rendered_env_files = Vec::new();
                                    for f in c.env_files.iter() {
                                        rendered_env_files.push(tera.render_str(f, &context)?);
                                    }
                                    c.env_files = rendered_env_files;

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

            ret.insert(
                task_name.clone(),
                Task {
                    name: task_name.clone(),
                    command: task_config.command.clone(),
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
                    inputs,
                    outputs,
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
