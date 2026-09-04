use crate::project::Task;
use crate::template::new_tera;
use crate::template::ROOT_DIR_CONTEXT_KEY;
use crate::util::merge_yaml;
use crate::vars::{check_var_value, validate_var_declaration, VarSchema, VarType};
use anyhow::Context;
use derivative::Derivative;
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use regex::Regex;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::Value;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::thread::available_parallelism;
use std::{io, iter, path};
use tracing::info;

const CONFIG_FILE: [&str; 2] = ["firepit.yml", "firepit.yaml"];

#[derive(Clone, Deserialize, Serialize, JsonSchema, Derivative)]
#[derivative(Debug)]
pub struct ProjectConfig {
    /// Project name
    #[serde(skip)]
    pub name: String,

    /// Child projects.
    /// Valid only in a root project config.
    /// ```yaml
    /// projects:
    ///   client: packages/client
    ///   server: packages/server
    /// ```
    #[serde(default)]
    pub projects: IndexMap<String, String>,

    /// **Deprecated**: Use [`defaults`](https://kota65535.github.io/firepit/schema.html#defaults) instead.
    ///
    /// Shell configuration for all the project tasks.
    /// ```yaml
    /// shell:
    ///   command: "bash"
    ///   args: ["-eux", "-c"]
    /// ```
    #[serde(default = "default_shell")]
    #[schemars(extend("deprecated" = true))]
    pub shell: ShellConfig,

    /// **Deprecated**: Use [`defaults`](https://kota65535.github.io/firepit/schema.html#defaults) instead.
    ///
    /// Working directory for all the project tasks.
    /// ```yaml
    /// working_dir: src
    /// ```
    #[serde(default = "default_working_dir")]
    #[schemars(extend("x-template" = true, "deprecated" = true))]
    pub working_dir: String,

    /// Template variables for all the project tasks.
    /// A variable declared without a value has no default, so it is required: running any task of
    /// the project without giving it a value by the `<name>=<value>` CLI argument is an error.
    /// ```yaml
    /// vars:
    ///   registry: docker.io/example
    ///   image: "{{ registry }}/server"
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub vars: IndexMap<String, VarsConfig>,

    /// **Deprecated**: Use [`defaults`](https://kota65535.github.io/firepit/schema.html#defaults) instead.
    ///
    /// Environment variables for all the project tasks.
    /// ```yaml
    /// env:
    ///   TZ: Asia/Tokyo
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true, "deprecated" = true))]
    pub env: IndexMap<String, String>,

    /// **Deprecated**: Use [`defaults`](https://kota65535.github.io/firepit/schema.html#defaults) instead.
    ///
    /// Dotenv files for all the project tasks.
    /// In case of duplicated environment variables, the latter one takes precedence.
    /// ```yaml
    /// env_files:
    ///   - .env
    ///   - .env.local
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true, "deprecated" = true))]
    pub env_files: Vec<String>,

    /// **Deprecated**: Use [`defaults`](https://kota65535.github.io/firepit/schema.html#defaults) instead.
    ///
    /// Dependency tasks for all the project tasks.
    /// ```yaml
    /// depends_on:
    ///   - '#install'
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true, "deprecated" = true))]
    pub depends_on: Vec<DependsOnConfig>,

    /// Default settings applied to tasks matching a selector.
    /// ```yaml
    /// defaults:
    ///   - tasks: "^(build|test)"
    ///     depends_on:
    ///       - install
    /// ```
    #[serde(default)]
    pub defaults: Vec<DefaultsConfig>,

    /// Task definitions.
    #[serde(default)]
    pub tasks: IndexMap<String, TaskConfig>,

    /// Task concurrency.
    /// Valid only in a root project config.
    /// ```yaml
    /// concurrency: 4
    /// ```
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,

    /// Log configuration.
    /// Valid only in a root project config.
    /// ```yaml
    /// log:
    ///   level: debug
    ///   file: "{{ root_dir }}/firepit.log"
    /// ```
    #[serde(default = "default_log")]
    pub log: LogConfig,

    /// Gantt chart output file path.
    /// Valid only in a root project config.
    /// ```yaml
    /// gantt_file: gantt.svg
    /// ```
    pub gantt_file: Option<String>,

    /// UI configuration.
    /// Valid only in a root project config.
    /// ```yaml
    /// ui: cui
    /// ```
    #[serde(default = "default_ui")]
    pub ui: UI,

    /// Additional config files to be included.
    /// ```yaml
    /// includes:
    ///   - common-vars.yml
    ///   - common-tasks.yml
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub includes: Vec<String>,

    /// Project file path (absolute)
    #[serde(skip)]
    pub path: PathBuf,

    /// Project directory path (absolute)
    #[serde(skip)]
    pub dir: PathBuf,

    /// Raw YAML data
    #[serde(skip)]
    #[derivative(Debug = "ignore")]
    pub raw: Value,
}

pub fn default_shell() -> ShellConfig {
    ShellConfig {
        command: default_shell_command(),
        args: default_shell_args(),
    }
}

pub fn default_shell_command() -> String {
    "bash".to_string()
}

pub fn default_shell_args() -> Vec<String> {
    vec!["-c".to_string()]
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
    pub fn new_multi(dir: &Path) -> anyhow::Result<(ProjectConfig, IndexMap<String, ProjectConfig>)> {
        let dir = path::absolute(dir)?;
        let mut root_config = ProjectConfig::find_root(&dir)?;
        let mut children = IndexMap::new();

        // Tera context is used for merge
        let mut context = tera::Context::new();
        context.insert(
            ROOT_DIR_CONTEXT_KEY,
            &root_config.dir.as_os_str().to_str().unwrap_or(""),
        );

        if root_config.is_root() {
            // Multi project
            for (name, path) in &root_config.projects {
                if name.contains("#") {
                    anyhow::bail!("Project name must not contain '#'. Found: {:?}", name)
                }
                let mut child_config = ProjectConfig::new(name, root_config.dir.join(path).as_path())?;
                for t in child_config.tasks.values_mut() {
                    t.project = name.clone();
                }
                child_config = child_config.merge(&context)?;
                child_config.apply_defaults()?;
                children.insert(name.clone(), child_config);
            }
        } else {
            // Single project
            root_config.name = "".to_string();
        }

        root_config = root_config.merge(&context)?;
        root_config.apply_defaults()?;

        Ok((root_config, children))
    }

    pub fn validate_multi(root: &ProjectConfig, children: &IndexMap<String, ProjectConfig>) -> anyhow::Result<()> {
        let all_tasks = iter::once(root)
            .chain(children.values())
            .flat_map(|p| p.tasks.values())
            .collect::<Vec<_>>();
        let tasks = all_tasks.iter().map(|t| t.full_name()).collect::<HashSet<_>>();
        let services = all_tasks
            .iter()
            .filter(|t| t.is_service())
            .map(|t| t.full_name())
            .collect::<HashSet<_>>();
        for config in iter::once(root).chain(children.values()) {
            config
                .validate(&tasks, &services)
                .context(format!("invalid config file: {:?}", config.path))?;
        }
        Ok(())
    }

    fn validate(&self, tasks: &HashSet<String>, services: &HashSet<String>) -> anyhow::Result<()> {
        for (_, t) in self.tasks.iter() {
            for d in t.depends_on.iter().map(|d| d.task()) {
                if !tasks.contains(d) {
                    anyhow::bail!("tasks.{}.depends_on: task {:?} is not defined.", t.name, d);
                }
            }
            for w in t.wait_for.iter().map(|w| w.task()) {
                if !tasks.contains(w) {
                    anyhow::bail!("tasks.{}.wait_for: task {:?} is not defined.", t.name, w);
                }
            }
            for f in t.finalized_by.iter().map(|f| f.task()) {
                if !tasks.contains(f) {
                    anyhow::bail!("tasks.{}.finalized_by: task {:?} is not defined.", t.name, f);
                }
                // A finalizer must finish, which a service does not do on its own
                if services.contains(f) {
                    anyhow::bail!("tasks.{}.finalized_by: task {:?} must not be a service.", t.name, f);
                }
            }
        }
        Ok(())
    }

    pub fn new_from_str(name: &str, str: &str, path: &Path, dir: &Path) -> anyhow::Result<ProjectConfig> {
        let mut data = serde_yaml::from_str::<ProjectConfig>(str)?;

        // Project dir
        data.path = path.to_owned();
        data.dir = dir.to_owned();

        // Name
        data.name = name.to_string();

        // Var declarations
        for (k, v) in data.vars.iter() {
            v.validate().with_context(|| format!("vars.{}", k))?;
        }
        for (t, task) in data.tasks.iter() {
            for (k, v) in task.vars.iter() {
                v.validate().with_context(|| format!("tasks.{}.vars.{}", t, k))?;
            }
        }
        for (i, default) in data.defaults.iter().enumerate() {
            for (k, v) in default.vars.iter() {
                v.validate().with_context(|| format!("defaults[{}].vars.{}", i, k))?;
            }
        }

        // Task name & dependency task name
        for (k, v) in data.tasks.iter_mut() {
            v.name = k.clone();
            v.orig_name = k.clone();
            v.project = name.to_string();
            v.depends_on = v
                .depends_on
                .iter()
                .map(|d| match d {
                    DependsOnConfig::String(s) => DependsOnConfig::String(Task::qualified_name(name, s)),
                    DependsOnConfig::Struct(s) => DependsOnConfig::Struct(DependsOnConfigStruct {
                        task: Task::qualified_name(&data.name, &s.task),
                        vars: s.vars.clone(),
                        cascade: s.cascade,
                    }),
                })
                .collect();
            v.wait_for = v
                .wait_for
                .iter()
                .map(|w| w.with_task(Task::qualified_name(name, w.task())))
                .collect();
            v.finalized_by = v
                .finalized_by
                .iter()
                .map(|f| f.with_task(Task::qualified_name(name, f.task())))
                .collect();
        }

        // Save raw data
        let raw_data = serde_yaml::from_str::<Value>(str)?;
        data.raw = raw_data;

        Ok(data)
    }

    pub fn new(name: &str, dir: &Path) -> anyhow::Result<ProjectConfig> {
        let (mut file, path) = Self::open_file(&dir.join(CONFIG_FILE[0]))
            .or_else(|_| Self::open_file(&dir.join(CONFIG_FILE[1])))
            .with_context(|| {
                format!(
                    "cannot open config file ({} or {}) in directory {:?}",
                    CONFIG_FILE[0], CONFIG_FILE[1], dir
                )
            })?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        Self::new_from_str(name, &buf, path.as_path(), dir)
            .with_context(|| format!("cannot parse config file {:?}", path))
    }

    pub fn merge(&self, context: &tera::Context) -> anyhow::Result<Self> {
        // Render includes only
        let mut tera = new_tera();
        let mut rendered_includes = Vec::new();
        for f in self.includes.iter() {
            rendered_includes.push(tera.render_str(f, context)?);
        }

        // Start from empty value
        let mut ret = Value::Null;

        // Merge included files first
        for incl in rendered_includes.iter() {
            info!("Config file {:?} includes {:?}", self.dir, incl);
            let path = absolute_or_join(incl, &self.dir);
            let (file, _) = Self::open_file(&self.dir.join(incl))
                .with_context(|| format!("cannot open included file {:?}", path))?;
            let reader = BufReader::new(file);
            let raw_yaml: Value =
                serde_yaml::from_reader(reader).with_context(|| format!("cannot read included file {:?}.", path))?;
            merge_yaml(&mut ret, &raw_yaml, true)
        }

        // Merge the main file
        merge_yaml(&mut ret, &self.raw, true);

        // Convert back to ProjectConfig
        let merged_str = serde_yaml::to_string(&ret)?;
        let merged = Self::new_from_str(&self.name, &merged_str, &self.path, &self.dir)?;
        Ok(merged)
    }

    fn open_file(path: &Path) -> Result<(File, PathBuf), io::Error> {
        match File::open(path) {
            Ok(file) => Ok((file, path.to_owned())),
            Err(e) => Err(e),
        }
    }

    fn find_root(cwd: &Path) -> anyhow::Result<ProjectConfig> {
        let config = ProjectConfig::new("", cwd)?;
        if config.is_root() {
            return Ok(config);
        }
        for current_dir in cwd.ancestors() {
            match ProjectConfig::new("", current_dir) {
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

    pub fn env_file_paths(&self) -> Vec<PathBuf> {
        self.env_files.iter().map(|f| absolute_or_join(f, &self.dir)).collect()
    }

    pub fn schema() -> anyhow::Result<String> {
        let schema = schemars::schema_for!(ProjectConfig);
        serde_json::to_string_pretty(&schema).context("cannot create config schema")
    }

    pub fn task(&self, name: &str) -> anyhow::Result<&TaskConfig> {
        self.tasks
            .get(name)
            .with_context(|| anyhow::anyhow!("task {:?} is not defined", name))
    }

    pub fn task_mut(&mut self, name: &str) -> anyhow::Result<&mut TaskConfig> {
        self.tasks
            .get_mut(name)
            .with_context(|| anyhow::anyhow!("task {:?} is not defined", name))
    }

    pub fn relative_path_from(&self, path: &Path) -> PathBuf {
        self.dir.strip_prefix(path).unwrap_or(&self.dir).to_path_buf()
    }

    /// Collect deprecation warnings for project-level task settings.
    /// Returns a list of warning messages for fields that should be migrated to `defaults`.
    pub fn deprecated_warnings(&self) -> Vec<String> {
        let file = self.path.display();
        let mut warnings = Vec::new();
        let fields: &[(&str, bool)] = &[
            ("shell", self.shell != default_shell()),
            ("working_dir", self.working_dir != default_working_dir()),
            ("env", !self.env.is_empty()),
            ("env_files", !self.env_files.is_empty()),
            ("depends_on", !self.depends_on.is_empty()),
        ];
        for (field, used) in fields {
            if *used {
                warnings.push(format!(
                    "{}: project-level `{}` is deprecated. Use `defaults` instead. See https://kota65535.github.io/firepit/schema.html#defaults",
                    file, field
                ));
            }
        }
        warnings
    }

    /// Apply `defaults` entries to all matching tasks.
    /// For each task, all matching defaults are merged in order (later entries override earlier
    /// for scalars and maps, arrays are concatenated), then the merged result is applied to the
    /// task as a base layer (task-specific values take precedence).
    pub fn apply_defaults(&mut self) -> anyhow::Result<()> {
        // Validate regex patterns and qualify depends_on upfront
        let qualified_defaults: Vec<DefaultsConfig> = self
            .defaults
            .iter()
            .map(|d| {
                if let Some(TaskSelector::Regex(ref pattern)) = d.tasks {
                    Regex::new(pattern).with_context(|| format!("defaults: invalid regex pattern {:?}", pattern))?;
                }
                Ok(DefaultsConfig {
                    depends_on: d
                        .depends_on
                        .iter()
                        .map(|dep| match dep {
                            DependsOnConfig::String(s) => DependsOnConfig::String(Task::qualified_name(&self.name, s)),
                            DependsOnConfig::Struct(s) => DependsOnConfig::Struct(DependsOnConfigStruct {
                                task: Task::qualified_name(&self.name, &s.task),
                                vars: s.vars.clone(),
                                cascade: s.cascade,
                            }),
                        })
                        .collect(),
                    wait_for: d
                        .wait_for
                        .iter()
                        .map(|w| w.with_task(Task::qualified_name(&self.name, w.task())))
                        .collect(),
                    ..d.clone()
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        for (task_name, task_config) in self.tasks.iter_mut() {
            // Merge all matching defaults in order (later entries override earlier)
            let mut eff_shell: Option<ShellConfig> = None;
            let mut eff_working_dir: Option<String> = None;
            let mut eff_service: Option<ServiceConfig> = None;
            let mut eff_stop_timeout: Option<u64> = None;
            let mut eff_vars: IndexMap<String, VarsConfig> = IndexMap::new();
            let mut eff_env: IndexMap<String, String> = IndexMap::new();
            let mut eff_env_files: Vec<String> = Vec::new();
            let mut eff_depends_on: Vec<DependsOnConfig> = Vec::new();
            let mut eff_wait_for: Vec<WaitForConfig> = Vec::new();
            let mut eff_inputs: Vec<String> = Vec::new();
            let mut eff_outputs: Vec<String> = Vec::new();
            let mut matched = false;

            for default in &qualified_defaults {
                let is_match = match &default.tasks {
                    None => true,
                    Some(selector) => selector.matches(task_name),
                };
                if !is_match {
                    continue;
                }
                matched = true;

                // Scalars: later wins
                if default.shell.is_some() {
                    eff_shell = default.shell.clone();
                }
                if default.working_dir.is_some() {
                    eff_working_dir = default.working_dir.clone();
                }
                if default.service.is_some() {
                    eff_service = default.service.clone();
                }
                if default.stop_timeout.is_some() {
                    eff_stop_timeout = default.stop_timeout;
                }
                // Maps: later wins on key conflict
                eff_vars.extend(default.vars.clone());
                eff_env.extend(default.env.clone());
                // Arrays: concatenate in order
                eff_env_files.extend(default.env_files.clone());
                eff_depends_on.extend(default.depends_on.clone());
                eff_wait_for.extend(default.wait_for.clone());
                eff_inputs.extend(default.inputs.clone());
                eff_outputs.extend(default.outputs.clone());
            }

            if !matched {
                continue;
            }

            // Apply merged defaults to task (task-specific values take precedence)
            if task_config.shell.is_none() {
                task_config.shell = eff_shell;
            }
            if task_config.working_dir.is_none() {
                task_config.working_dir = eff_working_dir;
            }
            if task_config.service.is_none() {
                task_config.service = eff_service;
            }
            if task_config.stop_timeout.is_none() {
                task_config.stop_timeout = eff_stop_timeout;
            }

            eff_vars.extend(task_config.vars.clone());
            task_config.vars = eff_vars;

            eff_env.extend(task_config.env.clone());
            task_config.env = eff_env;

            eff_env_files.append(&mut task_config.env_files);
            task_config.env_files = eff_env_files;

            eff_depends_on.append(&mut task_config.depends_on);
            task_config.depends_on = eff_depends_on;

            eff_wait_for.append(&mut task_config.wait_for);
            task_config.wait_for = eff_wait_for;

            eff_inputs.append(&mut task_config.inputs);
            task_config.inputs = eff_inputs;

            eff_outputs.append(&mut task_config.outputs);
            task_config.outputs = eff_outputs;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskConfig {
    /// Name
    #[serde(skip)]
    pub name: String,

    /// Original name.
    /// Used for tracking the original of the task variant.
    #[serde(skip)]
    pub orig_name: String,

    /// Project name
    #[serde(skip)]
    pub project: String,

    /// Label to display instead of the task name.
    #[schemars(extend("x-template" = true))]
    pub label: Option<String>,

    /// Description
    pub description: Option<String>,

    /// Command to run
    #[schemars(extend("x-template" = true))]
    pub command: Option<String>,

    /// Shell configuration
    pub shell: Option<ShellConfig>,

    /// Working directory
    /// ```yaml
    /// working_dir: dist
    /// ```
    #[schemars(extend("x-template" = true))]
    pub working_dir: Option<String>,

    /// Template variables. A task variable shadows the project variable of the same name,
    /// and the `<name>=<value>` CLI argument overrides both.
    /// Can be used at `label`, `command`, `working_dir`, `env`, `env_files`, `depends_on`, `depends_on.{task, vars}`,
    /// `wait_for`, `wait_for.{task, vars}`,
    /// `service.healthcheck.log` and `service.healthcheck.exec.{command, working_dir, env, env_files}`
    ///
    /// A variable declared without a value has no default, so it is required: give it a value
    /// with the CLI argument or the dependent task's `depends_on.vars`.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub vars: IndexMap<String, VarsConfig>,

    /// Environment variables. Merged with the project `env`.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env: IndexMap<String, String>,

    /// Dotenv files. Merged with the project `env_files`.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env_files: Vec<String>,

    /// Dependency tasks
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub depends_on: Vec<DependsOnConfig>,

    /// Tasks to run after, without depending on them.
    ///
    /// Unlike `depends_on`, the listed tasks are not added to the run.
    /// They only order this task after them when they are going to run anyway.
    /// Naming a task orders this one after every variant of it. Write an entry in object form
    /// to wait only for the variants whose vars match the given ones.
    /// ```yaml
    /// wait_for:
    ///   - lint
    ///   - task: migrate
    ///     vars:
    ///       database: app
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub wait_for: Vec<WaitForConfig>,

    /// Tasks to run after this task finishes, whether it succeeds or fails.
    /// They run only when this task is part of the run (as a target or a dependency),
    /// so running a finalizer on its own does not run the task it finalizes.
    /// Write an entry in object form to override the finalizer's `vars`, as with `depends_on`.
    /// ```yaml
    /// finalized_by:
    ///   - db-down
    ///   - task: notify
    ///     vars:
    ///       channel: ci
    /// ```
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub finalized_by: Vec<FinalizedByConfig>,

    /// Tasks this task finalizes, filled per run by the workspace: those whose `finalized_by`
    /// lists this task. This task runs after all of them finish, whether they succeed or fail.
    #[serde(skip)]
    pub finalizes: Vec<String>,

    /// Service configurations
    pub service: Option<ServiceConfig>,

    /// Grace period in seconds given to the task process after `SIGINT` is sent,
    /// before it is forcibly killed with `SIGKILL`.
    /// ```yaml
    /// stop_timeout: 30
    /// ```
    pub stop_timeout: Option<u64>,

    /// Inputs file glob patterns
    #[serde(default)]
    pub inputs: Vec<String>,

    /// Output file glob patterns
    #[serde(default)]
    pub outputs: Vec<String>,
}

impl TaskConfig {
    pub fn full_name(&self) -> String {
        format!("{}#{}", self.project, self.name)
    }

    pub fn is_service(&self) -> bool {
        matches!(
            self.service,
            Some(ServiceConfig::Bool(true)) | Some(ServiceConfig::Struct(_))
        )
    }

    pub fn full_orig_name(&self) -> String {
        format!("{}#{}", self.project, self.orig_name)
    }

    pub fn working_dir_path(&self, dir: &Path) -> PathBuf {
        match self.working_dir.clone() {
            Some(wd) => absolute_or_join(&wd, dir),
            None => dir.to_path_buf(),
        }
    }

    pub fn env_file_paths(&self, dir: &Path) -> Vec<PathBuf> {
        self.env_files.iter().map(|f| absolute_or_join(f, dir)).collect()
    }

    pub fn input_paths(&self, dir: &Path) -> Vec<PathBuf> {
        self.inputs.iter().map(|f| absolute_or_join(f, dir)).collect()
    }

    pub fn output_paths(&self, dir: &Path) -> Vec<PathBuf> {
        self.outputs.iter().map(|f| absolute_or_join(f, dir)).collect()
    }
}

fn absolute_or_join(path: &str, dir: &Path) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    }
}

/// Vars config
///
/// A variable is one of:
/// - a scalar value (`foo: bar`), whose type is inferred from the value
/// - a typed declaration (`foo: { type: array, default: [a, b] }`), see [`TypedVars`]
/// - a dynamic variable (`foo: { command: ... }`), see [`DynamicVars`]
///
/// Array and object values are only accepted as the `default` of a typed declaration, so that an
/// object is never ambiguous between a value and a declaration.
///
/// An object with `command` is dynamic (tried first, so `{ type, command }` is a typed dynamic
/// variable), otherwise an object with `type` is a typed declaration.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(
    untagged,
    expecting = "a scalar value, a typed declaration with `type` (and optionally `default`), or a dynamic variable with `command`"
)]
pub enum VarsConfig {
    Dynamic(Box<DynamicVars>),
    Typed(TypedVars),
    Static(
        #[serde(deserialize_with = "deserialize_scalar")]
        #[schemars(schema_with = "scalar_schema")]
        JsonValue,
    ),
}

impl VarsConfig {
    /// Returns whether the variable is declared without a value, ex: `foo:` or
    /// `foo: { type: string }`.
    /// Such a variable has no default, so it is required: it must be given a value before the
    /// task runs, by the `<name>=<value>` CLI argument or the dependent task's `depends_on.vars`.
    pub fn is_unset(&self) -> bool {
        match self {
            VarsConfig::Static(JsonValue::Null) => true,
            VarsConfig::Typed(t) => t.default.as_ref().is_none_or(JsonValue::is_null),
            _ => false,
        }
    }

    /// Returns the config to use when `value` overrides this declaration (from the CLI argument
    /// or `depends_on.vars`): a typed declaration keeps its type, so the value is interpreted
    /// according to it; otherwise the value replaces the declaration as is.
    pub fn with_value(&self, value: &VarsConfig) -> VarsConfig {
        let declared = match self {
            VarsConfig::Typed(t) => Some((t.r#type, &t.schema)),
            VarsConfig::Dynamic(d) => d.r#type.map(|t| (t, &d.schema)),
            VarsConfig::Static(_) => None,
        };
        match (declared, value) {
            (Some((r#type, schema)), VarsConfig::Static(v)) => VarsConfig::Typed(TypedVars {
                r#type,
                default: Some(v.clone()),
                schema: schema.clone(),
            }),
            _ => value.clone(),
        }
    }

    /// Checks the declaration itself: the JSON Schema keywords must be known, require `type`,
    /// and form a valid schema.
    ///
    /// # Errors
    ///
    /// Returns an error describing the offending keyword.
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            VarsConfig::Typed(t) => validate_var_declaration(Some(t.r#type), &t.schema),
            VarsConfig::Dynamic(d) => validate_var_declaration(d.r#type, &d.schema),
            VarsConfig::Static(_) => Ok(()),
        }
    }
}

/// Accepts only scalar values; arrays and objects must be declared with `type`.
fn deserialize_scalar<'de, D: Deserializer<'de>>(deserializer: D) -> Result<JsonValue, D::Error> {
    let value = JsonValue::deserialize(deserializer)?;
    if value.is_array() || value.is_object() {
        return Err(de::Error::custom(
            "array and object values must be declared with `type`",
        ));
    }
    Ok(value)
}

fn scalar_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["string", "number", "boolean", "null"],
        "description": "Scalar value. Arrays and objects must be declared with `type`."
    })
}

/// Typed variable declaration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TypedVars {
    /// Type of the variable, following JSON Schema
    #[serde(rename = "type")]
    pub r#type: VarType,

    /// Default value. Without it the variable is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<JsonValue>,

    /// Any other JSON Schema keyword (`enum`, `pattern`, `minimum`, `items`, ...) validates the value.
    #[serde(flatten)]
    pub schema: VarSchema,
}

impl TypedVars {
    /// Checks `value` against the JSON Schema keywords of the declaration.
    pub fn check(&self, value: &JsonValue) -> anyhow::Result<()> {
        check_var_value(self.r#type, &self.schema, value)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DynamicVars {
    /// Command
    #[schemars(extend("x-template" = true))]
    pub command: String,

    /// Type of the variable, following JSON Schema. The command output is interpreted as this
    /// type; without it, the type is inferred from the output.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<VarType>,

    /// Any other JSON Schema keyword (`enum`, `pattern`, `minimum`, `items`, ...) validates the
    /// output. Requires `type`.
    #[serde(flatten)]
    pub schema: VarSchema,

    /// Shell configuration
    pub shell: Option<ShellConfig>,

    /// Environment variables
    #[serde(default, deserialize_with = "deserialize_hash_map")]
    #[schemars(extend("x-template" = true))]
    pub env: IndexMap<String, String>,

    /// Dotenv files
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env_files: Vec<String>,

    /// Working directory
    #[schemars(extend("x-template" = true))]
    pub working_dir: Option<String>,

    /// Whether the command output is reused by the other variables running the same command in
    /// the same working directory. A variable shared by several projects runs in each project
    /// directory, so sharing one run across them takes an explicit `working_dir`. Leave it off
    /// for a command that must run every time, ex: allocating a resource.
    #[serde(default)]
    pub cache: bool,

    #[serde(skip)]
    pub inner: Option<DynamicVarsInner>,
}

impl DynamicVars {
    pub fn env_file_paths(&self, dir: &Path) -> Vec<PathBuf> {
        self.env_files.iter().map(|f| absolute_or_join(f, dir)).collect()
    }

    pub fn working_dir_path(&self, dir: &Path) -> PathBuf {
        match self.working_dir.clone() {
            Some(wd) => absolute_or_join(&wd, dir),
            None => dir.to_path_buf(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DynamicVarsInner {
    pub name: String,
    pub command: String,
    pub shell: ShellConfig,
    pub working_dir: PathBuf,
    pub env: HashMap<String, String>,
    pub cache: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct ShellConfig {
    /// Shell command.
    #[serde(default = "default_shell_command")]
    pub command: String,

    /// Arguments of the shell command.
    #[serde(default = "default_shell_args")]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    /// Log level. Valid values: error, warn, info, debug, trace
    pub level: String,

    /// Log file path.
    pub file: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(extend("x-template" = true))]
pub enum DependsOnConfig {
    String(String),
    Struct(DependsOnConfigStruct),
}

impl DependsOnConfig {
    pub fn task(&self) -> &str {
        match self {
            DependsOnConfig::String(s) => s,
            DependsOnConfig::Struct(s) => &s.task,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DependsOnConfigStruct {
    /// Dependency task name
    #[schemars(extend("x-template" = true))]
    pub task: String,

    /// Variables to override the dependency task vars.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub vars: IndexMap<String, VarsConfig>,

    /// Whether the task restarts if this dependency task restarts.
    #[serde(default = "default_cascade")]
    pub cascade: bool,
}

fn default_cascade() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(extend("x-template" = true))]
pub enum WaitForConfig {
    String(String),
    Struct(WaitForConfigStruct),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WaitForConfigStruct {
    /// Name of the task to wait for
    #[schemars(extend("x-template" = true))]
    pub task: String,

    /// Variables narrowing down which variants of the task to wait for.
    /// Only the variables given here are compared, so the variants may differ in the others.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub vars: IndexMap<String, VarsConfig>,
}

impl WaitForConfig {
    pub fn task(&self) -> &str {
        match self {
            WaitForConfig::String(s) => s,
            WaitForConfig::Struct(s) => &s.task,
        }
    }

    pub fn vars(&self) -> Option<&IndexMap<String, VarsConfig>> {
        match self {
            WaitForConfig::String(_) => None,
            WaitForConfig::Struct(s) => Some(&s.vars),
        }
    }

    /// Returns a copy of this entry with the task name replaced.
    pub fn with_task(&self, task: String) -> Self {
        match self {
            WaitForConfig::String(_) => WaitForConfig::String(task),
            WaitForConfig::Struct(s) => WaitForConfig::Struct(WaitForConfigStruct {
                task,
                vars: s.vars.clone(),
            }),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(extend("x-template" = true))]
pub enum FinalizedByConfig {
    String(String),
    Struct(FinalizedByConfigStruct),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FinalizedByConfigStruct {
    /// Finalizer task name
    #[schemars(extend("x-template" = true))]
    pub task: String,

    /// Variables to override the finalizer task vars.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub vars: IndexMap<String, VarsConfig>,
}

impl FinalizedByConfig {
    pub fn task(&self) -> &str {
        match self {
            FinalizedByConfig::String(s) => s,
            FinalizedByConfig::Struct(s) => &s.task,
        }
    }

    /// Returns a copy of this entry with the task name replaced.
    pub fn with_task(&self, task: String) -> Self {
        match self {
            FinalizedByConfig::String(_) => FinalizedByConfig::String(task),
            FinalizedByConfig::Struct(s) => FinalizedByConfig::Struct(FinalizedByConfigStruct {
                task,
                vars: s.vars.clone(),
            }),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum HealthCheckConfig {
    Log(LogProbeConfig),
    Exec(ExecProbeConfig),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LogProbeConfig {
    /// Log regex pattern to determine the task service is ready
    #[schemars(extend("x-template" = true))]
    pub log: String,

    /// Timeout in seconds
    #[serde(default = "default_log_healthcheck_timeout")]
    pub timeout: u64,
}

pub fn default_log_healthcheck_timeout() -> u64 {
    20
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExecProbeConfig {
    /// Command to check if the service is ready
    #[schemars(extend("x-template" = true))]
    pub command: String,

    /// Shell configuration
    pub shell: Option<ShellConfig>,

    /// Working directory
    #[schemars(extend("x-template" = true))]
    pub working_dir: Option<String>,

    /// Environment variables. Merged with the task `env`.
    #[serde(default, deserialize_with = "deserialize_hash_map")]
    #[schemars(extend("x-template" = true))]
    pub env: IndexMap<String, String>,

    /// Dotenv files. Merged with the task `env_files`.
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env_files: Vec<String>,

    /// Interval in seconds.
    /// The command will run interval seconds after the task is started,
    /// and then again interval seconds after each previous check completes.
    #[serde(default = "default_healthcheck_interval")]
    pub interval: u64,

    /// Timeout in seconds
    #[serde(default = "default_healthcheck_timeout")]
    pub timeout: u64,

    /// Number of consecutive readiness-check failures allowed before giving up.
    #[serde(default = "default_healthcheck_retries")]
    pub retries: u64,

    /// Initialization period in seconds.
    /// Probe failure during that period will not be counted towards the maximum number of retries.
    #[serde(default = "default_healthcheck_start_period")]
    pub start_period: u64,
}

impl ExecProbeConfig {
    pub fn working_dir_path(&self, dir: &Path) -> PathBuf {
        match self.working_dir.clone() {
            Some(wd) => {
                let wd = Path::new(&wd);
                if wd.is_absolute() {
                    wd.to_path_buf()
                } else {
                    dir.join(wd)
                }
            }
            None => dir.to_path_buf(),
        }
    }

    pub fn env_files_paths(&self, dir: &Path) -> Vec<PathBuf> {
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
    5
}
pub fn default_healthcheck_timeout() -> u64 {
    5
}
pub fn default_healthcheck_retries() -> u64 {
    3
}
pub fn default_healthcheck_start_period() -> u64 {
    0
}

/// Default grace period in seconds between `SIGINT` and `SIGKILL` for a task process.
pub fn default_stop_timeout() -> u64 {
    10
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ServiceConfig {
    Bool(bool),
    Struct(Box<ServiceConfigStruct>),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ServiceConfigStruct {
    /// Readiness probe configuration
    pub healthcheck: Option<HealthCheckConfig>,

    /// Restart policy
    #[serde(default = "default_service_restart")]
    pub restart: Restart,
}

#[derive(Debug, Clone)]
pub enum Restart {
    Always(Option<u64>),
    OnFailure(Option<u64>),
    Never,
}

impl Restart {
    pub fn max_restart(&self) -> Option<u64> {
        match self {
            Restart::Always(n) => *n,
            Restart::OnFailure(n) => *n,
            Restart::Never => Some(0),
        }
    }
}

pub static ALWAYS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^always(:(\d+))?$").unwrap());
pub static ON_FAILURE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^on-failure(:(\d+))?$").unwrap());

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
                    .and_then(|m| m.as_str().parse::<u64>().ok());
                Restart::Always(num)
            }
            s if ON_FAILURE.is_match(&s) => {
                let num = ON_FAILURE
                    .captures(s.as_str())
                    .and_then(|c| c.get(2))
                    .and_then(|m| m.as_str().parse::<u64>().ok());
                Restart::OnFailure(num)
            }
            s if s == "never" => Restart::Never,
            _ => return Err(serde::de::Error::custom(format!("invalid restart value: {}", s))),
        };
        Ok(r)
    }
}

impl Serialize for Restart {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Restart::Always(Some(num)) => serializer.serialize_str(&format!("always:{}", num)),
            Restart::Always(None) => serializer.serialize_str("always"),
            Restart::OnFailure(Some(num)) => serializer.serialize_str(&format!("on-failure:{}", num)),
            Restart::OnFailure(None) => serializer.serialize_str("on-failure"),
            Restart::Never => serializer.serialize_str("never"),
        }
    }
}

pub fn default_service_restart() -> Restart {
    Restart::Never
}

// cf. https://graham.cool/schemars/implementing/
impl JsonSchema for Restart {
    fn schema_name() -> Cow<'static, str> {
        "Restart".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Restart").into()
    }

    // JSON Schema designed to improve editor autocompletion.
    // Provide enum candidates for completion and allow numeric variants via pattern matching.
    fn json_schema(_gen: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "anyOf": [
                {
                  "type": "string",
                  "enum": [
                    "always",
                    "on-failure",
                    "never"
                  ]
                },
                {
                  "type": "string",
                  "pattern": "^(always(:\\d+)?|on-failure(:\\d+)?|never)$"
                }
            ]
        })
    }
}

/// Task selector for `defaults`.
/// A string value is treated as a regex pattern matched against the task name.
/// An array value is treated as an explicit list of task names.
/// If omitted, all tasks are matched.
/// ```yaml
/// defaults:
///   - tasks: "^build"        # regex
///     env:
///       NODE_ENV: production
///   - tasks: [test, lint]    # explicit list
///     depends_on:
///       - install
///   - env:                   # no tasks field = all tasks
///       LOG_LEVEL: info
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum TaskSelector {
    Regex(String),
    List(Vec<String>),
}

impl TaskSelector {
    /// Returns true if the given task name matches this selector.
    pub fn matches(&self, task_name: &str) -> bool {
        match self {
            TaskSelector::Regex(pattern) => {
                if pattern.is_empty() {
                    return false;
                }
                Regex::new(pattern).map(|re| re.is_match(task_name)).unwrap_or(false)
            }
            TaskSelector::List(names) => {
                if names.is_empty() {
                    return false;
                }
                names.iter().any(|n| n == task_name)
            }
        }
    }
}

/// Default settings applied to tasks matching the selector.
/// ```yaml
/// defaults:
///   - tasks: "^(build|test)"
///     depends_on:
///       - install
///     env:
///       NODE_ENV: development
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DefaultsConfig {
    /// Task selector. A string is a regex pattern, an array is an explicit list of task names.
    /// If omitted, all tasks are matched.
    pub tasks: Option<TaskSelector>,

    /// Shell configuration
    pub shell: Option<ShellConfig>,

    /// Working directory
    #[schemars(extend("x-template" = true))]
    pub working_dir: Option<String>,

    /// Template variables
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub vars: IndexMap<String, VarsConfig>,

    /// Environment variables
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env: IndexMap<String, String>,

    /// Dotenv files
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env_files: Vec<String>,

    /// Dependency tasks
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub depends_on: Vec<DependsOnConfig>,

    /// Tasks to run after, without depending on them
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub wait_for: Vec<WaitForConfig>,

    /// Service configurations
    pub service: Option<ServiceConfig>,

    /// Grace period in seconds given to the task process after `SIGINT` is sent,
    /// before it is forcibly killed with `SIGKILL`.
    pub stop_timeout: Option<u64>,

    /// Inputs file glob patterns
    #[serde(default)]
    pub inputs: Vec<String>,

    /// Output file glob patterns
    #[serde(default)]
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum UI {
    #[serde(rename = "cui")]
    Cui,
    #[serde(rename = "tui")]
    Tui,
}

/// Deserializes IndexMap while converting number to string, which is the default behavior.
/// This is necessary when using serde(untagged), as it strictly checks types.
fn deserialize_hash_map<'de, D>(deserializer: D) -> Result<IndexMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    let map: IndexMap<String, Value> = IndexMap::deserialize(deserializer)?;
    let mut new_map = IndexMap::new();

    for (key, value) in map {
        let value_str = match value {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => s,
            Value::Sequence(_) => {
                return Err(de::Error::invalid_type(
                    de::Unexpected::Seq,
                    &"a string, number, or boolean",
                ))
            }
            Value::Mapping(_) => {
                return Err(de::Error::invalid_type(
                    de::Unexpected::Map,
                    &"a string, number, or boolean",
                ))
            }
            Value::Tagged(t) => {
                return Err(de::Error::invalid_type(
                    de::Unexpected::Other(t.value.as_str().unwrap_or("unknown")),
                    &"a string, number, or boolean",
                ))
            }
        };
        new_map.insert(key, value_str);
    }

    Ok(new_map)
}
