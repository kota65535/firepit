use crate::config::{
    default_stop_timeout, DependsOnConfig, HealthCheckConfig, ProjectConfig, Restart, ServiceConfig, TaskConfig, UI,
};
use crate::probe::{ExecProbe, LogLineProbe, Probe};
use crate::template::ConfigRenderer;
use crate::vars::VarsConfig;
use anyhow::Context;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: Project,
    pub children: HashMap<String, Project>,
    pub target_tasks: Vec<String>,
    pub finalizer_tasks: Vec<String>,
    pub concurrency: usize,
    pub force: bool,
    pub watch: bool,
    pub use_pty: bool,
    pub fail_fast: bool,
    pub dir: PathBuf,
}

impl Workspace {
    // Public constructor aggregating many independent config inputs; refactoring
    // into a builder/struct would change the public API without real benefit.
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        root_config: &ProjectConfig,
        child_configs: &IndexMap<String, ProjectConfig>,
        tasks: &[String],
        current_dir: &Path,
        vars: &IndexMap<String, VarsConfig>,
        force: bool,
        watch: bool,
        fail_fast: Option<bool>,
        use_pty: Option<bool>,
    ) -> anyhow::Result<Workspace> {
        // List targe tasks
        let mut target_tasks = Vec::new();
        for task in tasks.iter() {
            let (project_name, task_name) = Task::split_name(task);
            match project_name {
                // Full name
                Some(_) => {
                    target_tasks.push(Self::task_config(root_config, child_configs, task)?.full_name());
                }
                // Simple name
                None => {
                    if current_dir == root_config.dir {
                        // Select the task if exists in the root project.
                        // If not, select all tasks with the name in the child projects.
                        let tasks = match root_config.task(task_name) {
                            Ok(task) => vec![task],
                            Err(_) => child_configs.values().filter_map(|c| c.task(task_name).ok()).collect(),
                        };
                        if tasks.is_empty() {
                            anyhow::bail!("task {:?} does not exist in any project", task)
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

        // Override vars for target tasks
        let mut root_config = root_config.clone();
        let mut child_configs = child_configs.clone();
        for t in target_tasks.iter() {
            let task = Self::task_config_mut(&mut root_config, &mut child_configs, t)?;
            // A typed task var keeps its type, so the CLI value is interpreted according to it.
            for (k, v) in vars.iter() {
                if let Some(declared) = task.vars.get_mut(k) {
                    *declared = declared.with_value(v);
                }
            }
        }

        let mut renderer = ConfigRenderer::new(&root_config, &child_configs, vars, watch);
        let (mut root_config, mut child_configs) = renderer.render().await?;
        ProjectConfig::validate_multi(&root_config, &child_configs)?;
        let finalizer_tasks = Self::apply_finalized_by(&mut root_config, &mut child_configs, &target_tasks, force)?;
        Self::validate_vars(&root_config, &child_configs, &target_tasks, &finalizer_tasks, vars)?;

        let root = Project::new("", &root_config)?;
        let mut children = HashMap::new();
        for (k, v) in child_configs.iter() {
            children.insert(k.clone(), Project::new(k, v)?);
        }

        let use_pty = match use_pty {
            Some(u) => u,
            None => match root_config.ui {
                UI::Tui => true,
                UI::Cui => false,
            },
        };

        let fail_fast = match fail_fast {
            Some(f) => f,
            None => match root_config.ui {
                UI::Tui => false,
                UI::Cui => true,
            },
        };

        Ok(Self {
            root,
            children,
            target_tasks,
            finalizer_tasks,
            concurrency: root_config.concurrency,
            force,
            watch,
            use_pty,
            fail_fast,
            dir: current_dir.to_owned(),
        })
    }

    /// Looks up a task config by its full name, ex: `#foo` or `project#foo`.
    fn task_config<'a>(
        root_config: &'a ProjectConfig,
        child_configs: &'a IndexMap<String, ProjectConfig>,
        name: &str,
    ) -> anyhow::Result<&'a TaskConfig> {
        let (project_name, task_name) = Task::split_name(name);
        match project_name {
            Some("") | None => root_config.task(task_name),
            Some(p) => child_configs
                .get(p)
                .with_context(|| format!("project {:?} is not defined", p))?
                .task(task_name),
        }
    }

    /// Mutable counterpart of [`Self::task_config`].
    fn task_config_mut<'a>(
        root_config: &'a mut ProjectConfig,
        child_configs: &'a mut IndexMap<String, ProjectConfig>,
        name: &str,
    ) -> anyhow::Result<&'a mut TaskConfig> {
        let (project_name, task_name) = Task::split_name(name);
        match project_name {
            Some("") | None => root_config.task_mut(task_name),
            Some(p) => child_configs
                .get_mut(p)
                .with_context(|| format!("project {:?} is not defined", p))?
                .task_mut(task_name),
        }
    }

    fn apply_finalized_by(
        root_config: &mut ProjectConfig,
        child_configs: &mut IndexMap<String, ProjectConfig>,
        target_tasks: &[String],
        force: bool,
    ) -> anyhow::Result<Vec<String>> {
        let mut finalizer_tasks = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = target_tasks.to_vec();
        while let Some(name) = queue.pop() {
            if !visited.insert(name.clone()) {
                continue;
            }
            let task = Self::task_config(root_config, child_configs, &name)?;
            // Dependencies are not run under `force`, so neither are their finalizers
            if !force {
                queue.extend(task.depends_on.iter().map(|d| d.task().to_string()));
            }
            let finalizers = task
                .finalized_by
                .iter()
                .map(|f| f.task().to_string())
                .collect::<Vec<_>>();
            for post in finalizers {
                queue.push(post.clone());
                if !target_tasks.contains(&post) && !finalizer_tasks.contains(&post) {
                    finalizer_tasks.push(post.clone());
                }
                let finalizer = Self::task_config_mut(root_config, child_configs, &post)?;
                // A finalizer that also depends on the task already waits for it, and the
                // dependency is stricter: it requires the task to succeed. Keep that one, which
                // also keeps the graph free of parallel edges.
                let already_waits =
                    finalizer.finalizes.contains(&name) || finalizer.depends_on.iter().any(|d| d.task() == name);
                if !already_waits {
                    finalizer.finalizes.push(name.clone());
                }
            }
        }
        Ok(finalizer_tasks)
    }

    /// Ensures that every var involved in the run has a value.
    ///
    /// A var declared without a value, ex: `foo:`, has no default value, so it must be given one
    /// before the task runs. Two kinds of declarations are checked:
    /// - vars of the target tasks and their dependency tasks
    /// - project vars of the projects those tasks belong to, unless set by the CLI argument
    ///
    /// The other tasks and projects are not involved in the run, so their unset vars are ignored.
    fn validate_vars(
        root_config: &ProjectConfig,
        child_configs: &IndexMap<String, ProjectConfig>,
        target_tasks: &[String],
        finalizer_tasks: &[String],
        cli_vars: &IndexMap<String, VarsConfig>,
    ) -> anyhow::Result<()> {
        let (task_vars, project_vars) =
            Self::collect_unset_vars(root_config, child_configs, target_tasks, finalizer_tasks, cli_vars);
        if task_vars.is_empty() && project_vars.is_empty() {
            return Ok(());
        }
        anyhow::bail!(Self::unset_vars_message(&task_vars, &project_vars, target_tasks))
    }

    /// Collects the unset vars involved in the run:
    /// per-task vars (with whether each var can be set by the CLI argument, which is the case
    /// only for a target task's var, not a finalizer's) and per-project vars of the involved
    /// projects.
    fn collect_unset_vars(
        root_config: &ProjectConfig,
        child_configs: &IndexMap<String, ProjectConfig>,
        target_tasks: &[String],
        finalizer_tasks: &[String],
        cli_vars: &IndexMap<String, VarsConfig>,
    ) -> (UnsetTaskVars, UnsetProjectVars) {
        let task_configs = std::iter::once(root_config)
            .chain(child_configs.values())
            .flat_map(|c| c.tasks.values().map(|t| (t.full_name(), t)))
            .collect::<HashMap<_, _>>();

        // Walk the target tasks, the finalizers and their dependency tasks
        let target_task_set = target_tasks.iter().collect::<HashSet<_>>();
        let mut visited = HashSet::new();
        let mut involved_projects = HashSet::new();
        let mut queue = target_tasks.iter().chain(finalizer_tasks).cloned().collect::<Vec<_>>();
        let mut task_vars: UnsetTaskVars = Vec::new();
        while let Some(task_name) = queue.pop() {
            if !visited.insert(task_name.clone()) {
                continue;
            }
            let Some(task_config) = task_configs.get(&task_name) else {
                continue;
            };
            involved_projects.insert(task_config.project.clone());
            let is_target = target_task_set.contains(&task_name);
            let names = task_config
                .vars
                .iter()
                .filter(|(_, v)| v.is_unset())
                .map(|(k, _)| (k.clone(), is_target))
                .collect::<Vec<_>>();
            if !names.is_empty() {
                task_vars.push((task_name.clone(), names));
            }
            queue.extend(
                task_config
                    .depends_on
                    .iter()
                    .map(|d| d.task())
                    .filter(|d| !d.is_empty())
                    .map(|d| Task::qualified_name(&task_config.project, d)),
            );
        }

        // Unset project vars of the involved projects. The CLI argument always sets a project
        // var, so vars given by the CLI are not errors.
        let mut project_vars: UnsetProjectVars = Vec::new();
        for project_name in involved_projects.iter() {
            let Some(config) = (if project_name.is_empty() {
                Some(root_config)
            } else {
                child_configs.get(project_name)
            }) else {
                continue;
            };
            let names = config
                .vars
                .iter()
                .filter(|(k, v)| v.is_unset() && !cli_vars.contains_key(*k))
                .map(|(k, _)| k.clone())
                .collect::<Vec<_>>();
            if !names.is_empty() {
                project_vars.push((project_name.clone(), names));
            }
        }

        task_vars.sort();
        project_vars.sort();
        (task_vars, project_vars)
    }

    /// Builds the error message for the unset vars: one line per project/task, followed by
    /// an example command with the CLI-settable vars and a hint for the dependency task vars.
    fn unset_vars_message(
        task_vars: &[(String, Vec<(String, bool)>)],
        project_vars: &[(String, Vec<String>)],
        target_tasks: &[String],
    ) -> String {
        let quote =
            |names: &mut dyn Iterator<Item = &String>| names.map(|n| format!("{:?}", n)).collect::<Vec<_>>().join(", ");
        let mut msg = project_vars
            .iter()
            .map(|(project, names)| {
                let project = if project.is_empty() {
                    "the root project".to_string()
                } else {
                    format!("project {:?}", project)
                };
                format!(
                    "{} requires vars that are not set: {}",
                    project,
                    quote(&mut names.iter())
                )
            })
            .chain(task_vars.iter().map(|(task, names)| {
                format!(
                    "task {:?} requires vars that are not set: {}",
                    task,
                    quote(&mut names.iter().map(|(n, _)| n))
                )
            }))
            .collect::<Vec<_>>()
            .join("\n");

        // Example command reproducing what the user ran, with the missing vars appended.
        // The "#" prefix of root project tasks is internal, so strip it.
        let cli_settable = project_vars
            .iter()
            .flat_map(|(_, names)| names.iter())
            .chain(
                task_vars
                    .iter()
                    .flat_map(|(_, names)| names.iter().filter(|(_, s)| *s).map(|(n, _)| n)),
            )
            .map(|n| format!("{}=<value>", n))
            .collect::<Vec<_>>();
        if !cli_settable.is_empty() {
            let tasks = target_tasks
                .iter()
                .map(|t| t.strip_prefix('#').unwrap_or(t))
                .collect::<Vec<_>>()
                .join(" ");
            msg = format!("{}\nSet them like: fire {} {}", msg, tasks, cli_settable.join(" "));
        }
        if task_vars.iter().any(|(_, names)| names.iter().any(|(_, s)| !s)) {
            msg = format!(
                "{}\nVars of a dependency task can be set with the dependent task's `depends_on.vars`",
                msg
            );
        }
        msg
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

    pub fn labels(&self) -> HashMap<String, String> {
        self.tasks()
            .into_iter()
            .map(|t| (t.name, t.label))
            .collect::<HashMap<_, _>>()
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
            tasks: Task::new_multi(name, root)?,
            dir: root.dir.clone(),
        })
    }

    pub fn task(&self, name: &str) -> Option<Task> {
        self.tasks.get(&Task::qualified_name(&self.name, name)).cloned()
    }
}

/// Unset vars grouped by task, with whether each var can be set by the CLI argument
type UnsetTaskVars = Vec<(String, Vec<(String, bool)>)>;

/// Unset vars grouped by project
type UnsetProjectVars = Vec<(String, Vec<String>)>;

#[derive(Debug, Clone)]
pub struct Task {
    /// Unique task name
    pub name: String,

    /// Task name as written in the config, shared by all variants of the task.
    /// Unlike `name`, it carries no internal variant suffix (-1, -2, ...)
    pub orig_name: String,

    /// Label
    pub label: String,

    /// Command to run
    pub command: String,

    /// Shell command
    pub shell: String,

    /// Shell command arguments
    pub shell_args: Vec<String>,

    /// Environment variables
    pub env: Env,

    /// Dependency task names
    pub depends_on: Vec<DependsOn>,

    /// Tasks to run after, without depending on them
    pub wait_for: Vec<WaitFor>,

    /// Resolved template variables of this task.
    /// A variant of a task differs from its siblings only in these, so they are what
    /// `wait_for` compares against to pick the variants to wait for.
    pub vars: IndexMap<String, JsonValue>,

    /// Task working directory path (absolute).
    pub working_dir: PathBuf,

    /// Whether this task is a service or not
    pub is_service: bool,

    /// Health checker
    pub probe: Probe,

    /// Restart setting
    pub restart: Restart,

    /// Grace period between `SIGINT` and `SIGKILL` on stop
    pub stop_timeout: Duration,

    /// Input files
    pub inputs: Vec<PathBuf>,

    /// Output files
    pub outputs: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct DependsOn {
    pub task: String,

    pub cascade: bool,

    /// Run the dependent task even if this dependency fails: the dependent task is a finalizer
    pub always: bool,
}

#[derive(Debug, Clone)]
pub struct WaitFor {
    /// Name of the task to wait for, as written in the config, so it names every variant of it
    pub task: String,

    /// Variables narrowing down which variants to wait for.
    /// Only these are compared, so a variant may differ in the others. Empty means every variant.
    pub vars: IndexMap<String, JsonValue>,
}

impl WaitFor {
    /// Returns whether the given task is one this entry waits for.
    ///
    /// A var the task does not declare is ignored, as `depends_on.vars` ignores it when picking
    /// the variant to create. Comparing it instead would make an entry copied from a `depends_on`
    /// silently match nothing, losing the ordering it was written for.
    pub fn matches(&self, task: &Task) -> bool {
        if self.task != task.orig_name {
            return false;
        }
        self.vars
            .iter()
            .filter(|(k, _)| task.vars.contains_key(*k))
            .all(|(k, v)| task.vars.get(k) == Some(v))
    }
}

#[derive(Debug, Clone)]
pub struct Env {
    configs: Vec<EnvConfig>,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    pub fn new() -> Self {
        Self { configs: Vec::new() }
    }

    pub fn with(&self, env_files: &[PathBuf], env: &IndexMap<String, String>) -> Self {
        let mut configs = self.configs.clone();
        configs.push(EnvConfig {
            env_files: env_files.to_vec(),
            env: env.clone(),
        });
        Self { configs }
    }

    pub fn verify(self) -> anyhow::Result<Self> {
        self.configs.iter().try_for_each(|e| e.load_env_files().map(|_| ()))?;
        Ok(self)
    }

    pub fn load(&self) -> anyhow::Result<HashMap<String, String>> {
        self.configs.iter().try_fold(HashMap::new(), |acc, config| {
            Ok(acc.into_iter().chain(config.merged_env()?).collect::<HashMap<_, _>>())
        })
    }
}

#[derive(Debug, Clone)]
pub struct EnvConfig {
    pub env_files: Vec<PathBuf>,
    pub env: IndexMap<String, String>,
}

impl EnvConfig {
    pub fn merged_env(&self) -> anyhow::Result<HashMap<String, String>> {
        Ok(self
            .load_env_files()?
            .into_iter()
            .chain(self.env.clone())
            .collect::<HashMap<_, _>>())
    }

    fn load_env_files(&self) -> anyhow::Result<HashMap<String, String>> {
        let mut ret = HashMap::new();
        for f in self.env_files.iter() {
            let iter = match dotenvy::from_path_iter(f) {
                Ok(it) => it,
                Err(e) => {
                    // Ignore if env file not found
                    info!("cannot read env file {:?}: {:?}", f, e);
                    continue;
                }
            };
            for item in iter {
                let (key, value) = item.with_context(|| format!("cannot parse env file {:?}", f))?;
                ret.insert(key, value);
            }
        }
        Ok(ret)
    }
}

impl Task {
    pub fn new_multi(project_name: &str, config: &ProjectConfig) -> anyhow::Result<HashMap<String, Task>> {
        let mut ret = HashMap::new();
        for (task_name, task_config) in config.tasks.iter() {
            let task = Self::new(project_name, config, task_name, task_config)?;
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
        let task_shell = task_config.clone().shell.unwrap_or(config.shell.clone());

        // Working directory
        let task_working_dir = task_config.working_dir_path(&config.working_dir_path());

        // Environment variables
        // Priority:
        // 1. Root project env file
        // 2. Root project env
        // 3. Project env file
        // 4. Project env
        // 5. Task env file
        // 6. Task env
        let env = Env::new()
            .with(&config.env_file_paths(), &config.env.clone())
            .with(&task_config.env_file_paths(&config.dir), &task_config.env.clone())
            .verify()?;

        // Depends On
        let depends_on = task_config
            .depends_on
            .iter()
            .chain(config.depends_on.iter())
            .collect::<Vec<_>>();

        // Input files
        let inputs = task_config
            .input_paths(&config.dir)
            .into_iter()
            .chain(task_config.env_file_paths(&config.dir))
            .collect::<Vec<_>>();

        // Output files
        let outputs = task_config
            .output_paths(&config.dir)
            .into_iter()
            .chain(task_config.env_file_paths(&config.dir))
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
                                Regex::new(&c.log).with_context(|| format!("invalid regex pattern {:?}", c.log))?,
                                c.timeout,
                            )),
                            // Exec Probe
                            HealthCheckConfig::Exec(c) => {
                                // Shell
                                let hc_shell = c.shell.clone().unwrap_or(task_shell.clone());
                                // Working directory
                                let hc_working_dir = c.working_dir_path(&task_working_dir);
                                // Environment variables
                                let env = env.with(&c.env_files_paths(&config.dir), &c.env).verify()?;

                                Probe::Exec(ExecProbe::new(
                                    &task_name,
                                    &c.command,
                                    &hc_shell.command,
                                    hc_shell.args,
                                    hc_working_dir,
                                    env,
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
            orig_name: Task::qualified_name(project_name, &task_config.orig_name),
            // Default to the original name so that task variants do not expose
            // their internal suffix (-1, -2, ...) in the UI
            label: task_config
                .label
                .clone()
                .unwrap_or_else(|| Task::qualified_name(project_name, &task_config.orig_name)),
            command: task_config.command.clone().unwrap_or("".to_string()),
            shell: task_shell.command,
            shell_args: task_shell.args,
            working_dir: task_working_dir,
            env,
            depends_on: depends_on
                .iter()
                .map(|s| match s {
                    DependsOnConfig::String(s) => DependsOn {
                        task: Task::qualified_name(project_name, s),
                        cascade: true,
                        always: false,
                    },
                    DependsOnConfig::Struct(s) => DependsOn {
                        task: Task::qualified_name(project_name, &s.task),
                        cascade: s.cascade,
                        always: false,
                    },
                })
                .chain(task_config.finalizes.iter().map(|t| DependsOn {
                    task: t.clone(),
                    cascade: true,
                    always: true,
                }))
                .filter(|d| d.task != task_name) // Exclude the task itself
                .collect(),
            wait_for: task_config
                .wait_for
                .iter()
                .map(|w| WaitFor {
                    task: Task::qualified_name(project_name, w.task()),
                    vars: w.vars().map(Self::resolved_vars).unwrap_or_default(),
                })
                .collect(),
            vars: Self::resolved_vars(&task_config.vars),
            is_service,
            probe,
            restart,
            stop_timeout: Duration::from_secs(task_config.stop_timeout.unwrap_or_else(default_stop_timeout)),
            inputs,
            outputs,
        })
    }

    /// Takes the values of rendered vars. Rendering resolves every var to a static value,
    /// so a var that is still dynamic here has not been rendered and has no value to compare.
    fn resolved_vars(vars: &IndexMap<String, VarsConfig>) -> IndexMap<String, JsonValue> {
        vars.iter()
            .filter_map(|(k, v)| match v {
                VarsConfig::Static(v) => Some((k.clone(), v.clone())),
                VarsConfig::Dynamic(_) | VarsConfig::Typed(_) => None,
            })
            .collect()
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
        self.inputs.iter().any(|i| {
            self.match_glob(i.to_str().unwrap_or(""), paths).unwrap_or_else(|e| {
                warn!("{:?}", e);
                false
            })
        })
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

    fn latest_modified_time(&self, paths: &[PathBuf]) -> u64 {
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
        let metadata = std::fs::metadata(path).with_context(|| format!("failed to get metadata of {:?}", path))?;
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

    fn glob(&self, pattern: &Path) -> anyhow::Result<Vec<PathBuf>> {
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
