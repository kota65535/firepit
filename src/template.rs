use crate::config::{
    DependsOnConfig, DependsOnConfigStruct, DynamicVarsInner, FinalizedByConfig, FinalizedByConfigStruct,
    HealthCheckConfig, ProjectConfig, ServiceConfig, TaskConfig, VarsConfig, WaitForConfig, WaitForConfigStruct,
};
use crate::log::OutputCollector;
use crate::process::{ChildExit, Command, ProcessManager};
use crate::project::{Env, Task};
use crate::DYNAMIC_VAR_STOP_TIMEOUT;
use anyhow::Context;
use async_recursion::async_recursion;
use indexmap::IndexMap;
use serde_json::{Map, Value as JsonValue};
use serde_yaml::Value;
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};
use tera::Tera;
use tracing::{debug, info, warn};

pub struct ConfigRenderer {
    root_config: ProjectConfig,
    child_configs: IndexMap<String, ProjectConfig>,
    vars: IndexMap<String, VarsConfig>,
    watch: bool,
}

pub const ROOT_DIR_CONTEXT_KEY: &str = "root_dir";
pub const PROJECT_DIRS_CONTEXT_KEY: &str = "project_dirs";
pub const PROJECT_DIR_CONTEXT_KEY: &str = "project_dir";
pub const PROJECT_CONTEXT_KEY: &str = "project";
pub const TASK_CONTEXT_KEY: &str = "task";
pub const WATCH_CONTEXT_KEY: &str = "watch";

/// How long the repeated runs of one dynamic variable command have to take in total before the
/// user is told about `cache: true`. Below this, running the command again is not worth a warning.
const DYNAMIC_VAR_HINT_THRESHOLD: Duration = Duration::from_secs(1);

/// Outputs of the dynamic variable commands that have already run, so that a variable with
/// `cache: true` included by several projects or tasks runs its command only once.
///
/// The cache lives for a single [`ConfigRenderer::render`] call. Re-rendering the config, which is
/// what happens on every watch event, starts from an empty cache and runs the commands again.
///
/// Uncached commands are timed as well, so that a variable that would benefit from `cache: true`
/// can be pointed out by [`DynamicVarCache::warn_uncached`].
#[derive(Debug, Default)]
pub struct DynamicVarCache {
    outputs: HashMap<DynamicVarKey, String>,
    /// Keyed by variable name as well, so that the runs reported to the user all come from the
    /// one variable named in the report, and not from another that happens to run the same
    /// command. The working directory stays out of the key: a variable shared by several projects
    /// runs in a different directory in each, which is exactly the case worth reporting.
    runs: HashMap<(String, DynamicVarKey), DynamicVarRuns>,
}

/// How often one dynamic variable command has run, and how much time it has taken in total.
#[derive(Debug)]
struct DynamicVarRuns {
    count: usize,
    elapsed: Duration,
}

impl DynamicVarCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self, key: &DynamicVarKey) -> Option<&String> {
        self.outputs.get(key)
    }

    fn insert(&mut self, key: DynamicVarKey, output: String) {
        self.outputs.insert(key, output);
    }

    /// Records that the uncached command of `key` has run once more, taking `elapsed`.
    ///
    /// Cached runs are not recorded: the key does not tell a cached variable from an uncached one
    /// with the same command, so counting both would report a variable that is already cached.
    fn record_run(&mut self, key: &DynamicVarKey, name: &str, elapsed: Duration) {
        self.runs
            .entry((name.to_string(), key.clone()))
            .and_modify(|r| {
                r.count += 1;
                r.elapsed += elapsed;
            })
            .or_insert(DynamicVarRuns { count: 1, elapsed });
    }

    /// Returns the name and the runs of the uncached variables that ran their command more than
    /// once and took long enough for caching to be worth considering.
    fn uncached_hints(&self) -> Vec<(&str, &DynamicVarRuns)> {
        self.runs
            .iter()
            .filter(|(_, r)| r.count > 1 && r.elapsed >= DYNAMIC_VAR_HINT_THRESHOLD)
            .map(|((name, _), r)| (name.as_str(), r))
            .collect()
    }

    /// Tells the user about the variables worth caching, which they have no other way of noticing
    /// than the config taking a long time to render.
    ///
    /// Whether caching is actually correct depends on the command, which nothing here can tell, so
    /// the condition is stated rather than the advice given outright: a repeated `cat VERSION`
    /// shows up here too, and reusing its output across projects would be wrong.
    fn warn_uncached(&self) {
        for (name, runs) in self.uncached_hints() {
            warn!(
                "Dynamic var {:?} ran its command {} times, taking {:.1}s in total. \
                 If its output does not depend on the directory it runs in, set `cache: true` \
                 on it to run the command once and reuse its output.",
                name,
                runs.count,
                runs.elapsed.as_secs_f64(),
            );
        }
    }
}

/// What makes two `cache: true` dynamic variables share a command output.
///
/// The working directory is deliberately left out: a variable shared by several projects runs in
/// each project directory, so including it would make the cache never hit in the case it exists
/// for. Opting in with `cache: true` is the declaration that the output does not depend on where
/// the command runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DynamicVarKey {
    shell_command: String,
    shell_args: Vec<String>,
    command: String,
    // BTreeMap, not HashMap, because HashMap is not hashable
    env: BTreeMap<String, String>,
}

impl DynamicVarKey {
    fn new(inner: &DynamicVarsInner) -> Self {
        Self {
            shell_command: inner.shell.command.clone(),
            shell_args: inner.shell.args.clone(),
            command: inner.command.clone(),
            env: inner.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        }
    }
}

impl ProjectConfig {
    pub async fn context(
        &self,
        context: &tera::Context,
        vars: &IndexMap<String, VarsConfig>,
        cache: &mut DynamicVarCache,
    ) -> anyhow::Result<tera::Context> {
        let mut tera = new_tera();
        let mut context = context.clone();
        context.insert(PROJECT_CONTEXT_KEY, &self.name);
        context.insert(PROJECT_DIR_CONTEXT_KEY, &self.dir.as_os_str().to_str().unwrap_or(""));

        // Render project-level vars.
        // CLI Argument vars override project-level vars.
        for (k, v) in vars
            .iter()
            .chain(self.vars.iter().filter(|(k, _)| !vars.contains_key(*k)))
        {
            let rk = tera.render_str(k, &context)?;
            if !rk.is_empty() {
                let v = match v {
                    VarsConfig::Dynamic(s) => {
                        let mut s = s.clone();
                        s.command = tera.render_str(&s.command, &context)?;
                        s.env = render_string_map(&s.env, &mut tera, &context)?;
                        s.env_files = render_env_files(&s.env_files, &mut tera, &context)?;
                        s.working_dir = s.working_dir.map(|w| tera.render_str(&w, &context)).transpose()?;
                        s.inner = Some(DynamicVarsInner {
                            name: k.clone(),
                            command: s.command.clone(),
                            shell: s.shell.clone().unwrap_or(self.shell.clone()),
                            env: Env::new().with(&s.env_file_paths(&self.dir), &s.env).load()?,
                            working_dir: s.working_dir_path(&self.working_dir_path()),
                            cache: s.cache,
                        });
                        VarsConfig::Dynamic(s)
                    }
                    VarsConfig::Static(_) => v.clone(),
                };
                let rv = render_value(&v, &mut tera, &context, cache).await?;
                context.insert(rk, &rv);
            }
        }

        Ok(context)
    }

    /// Renders the project config, along with the context of each of its tasks. A task context
    /// runs the commands of the task's dynamic vars, so it is returned to be reused instead of
    /// being built a second time.
    pub async fn render(
        &self,
        context: &tera::Context,
        cache: &mut DynamicVarCache,
    ) -> anyhow::Result<(ProjectConfig, HashMap<String, tera::Context>)> {
        let mut tera = new_tera();

        let mut config = self.clone();

        // Render includes
        let mut rendered_includes = Vec::new();
        for f in config.includes.iter() {
            rendered_includes.push(tera.render_str(f, context)?);
        }
        config.includes = rendered_includes;

        // Render working_dir
        config.working_dir = tera.render_str(&config.working_dir, context)?;

        // Render env
        config.env = render_string_map(&config.env, &mut tera, context)?;

        // Render env_files
        config.env_files = render_env_files(&config.env_files, &mut tera, context)?;

        // Render tasks.
        // A task inherits from the project config rendered above, ex: its working_dir.
        let project = config.clone();
        let mut rendered_tasks = IndexMap::new();
        let mut task_contexts = HashMap::new();

        for (task_name, task_config) in project.tasks.iter() {
            let task_context = task_config.context(&project, context, cache).await?;
            rendered_tasks.insert(task_name.clone(), task_config.render(&task_context, cache).await?);
            task_contexts.insert(task_config.full_name(), task_context);
        }
        config.tasks = rendered_tasks;

        Ok((config, task_contexts))
    }
}

impl TaskConfig {
    pub async fn context(
        &self,
        config: &ProjectConfig,
        context: &tera::Context,
        cache: &mut DynamicVarCache,
    ) -> anyhow::Result<tera::Context> {
        let mut tera = new_tera();
        let mut context = context.clone();
        context.insert(TASK_CONTEXT_KEY, &self.full_orig_name());

        // Render task-level vars
        for (k, v) in self.vars.iter() {
            let rk = tera.render_str(k, &context)?;
            if !rk.is_empty() {
                // A task-level var without a value never inherits: declaring it shadows the
                // project-level var of the same name. The value must be given explicitly, by the
                // `<name>=<value>` CLI argument for the tasks being run or by the dependent
                // task's `depends_on.vars`. Until then it stays null, and is reported as an error
                // when the task is actually run.
                if v.is_unset() {
                    context.insert(rk, &JsonValue::Null);
                    continue;
                }
                let v = match v {
                    VarsConfig::Dynamic(s) => {
                        let mut s = s.clone();
                        s.command = tera.render_str(&s.command, &context)?;
                        s.env = render_string_map(&s.env, &mut tera, &context)?;
                        s.env_files = render_env_files(&s.env_files, &mut tera, &context)?;
                        s.working_dir = s.working_dir.map(|w| tera.render_str(&w, &context)).transpose()?;
                        s.inner = Some(DynamicVarsInner {
                            name: k.clone(),
                            command: s.command.clone(),
                            shell: s
                                .shell
                                .clone()
                                .unwrap_or(self.shell.clone().unwrap_or(config.shell.clone())),
                            env: Env::new().with(&s.env_file_paths(&config.dir), &s.env).load()?,
                            working_dir: s.working_dir_path(&self.working_dir_path(&config.working_dir_path())),
                            cache: s.cache,
                        });
                        VarsConfig::Dynamic(s)
                    }
                    VarsConfig::Static(_) => v.clone(),
                };
                let rv = render_value(&v, &mut tera, &context, cache).await?;
                context.insert(rk, &rv);
            }
        }
        Ok(context)
    }

    pub async fn render(&self, context: &tera::Context, cache: &mut DynamicVarCache) -> anyhow::Result<TaskConfig> {
        let mut config = self.clone();
        let mut tera = new_tera();

        // Render task-level vars
        config.vars = render_value_map(&config.vars, &mut tera, context, DynamicVar::FromContext, cache).await?;

        // Render label
        if let Some(l) = config.label {
            config.label = Some(tera.render_str(&l, context)?);
        }

        // Render command
        config.command = config.command.map(|c| tera.render_str(&c, context)).transpose()?;

        // Render working_dir
        config.working_dir = match config.working_dir {
            Some(w) => Some(tera.render_str(&w, context)?),
            None => None,
        };

        // Render env
        config.env = render_string_map(&config.env, &mut tera, context)?;

        // Render env_files
        config.env_files = render_env_files(&config.env_files, &mut tera, context)?;

        // Render service.healthcheck
        if let Some(service) = config.service {
            if let ServiceConfig::Struct(mut st) = service {
                if let Some(mut healthcheck) = st.healthcheck {
                    match healthcheck {
                        // Log Probe
                        HealthCheckConfig::Log(ref mut c) => {
                            // Render log
                            c.log = tera.render_str(&c.log, context)?;
                        }
                        // Exec Probe
                        HealthCheckConfig::Exec(ref mut c) => {
                            // Render command
                            c.command = tera.render_str(&c.command, context)?;

                            // Render working_dir
                            c.working_dir = match &c.working_dir {
                                Some(w) => Some(tera.render_str(w, context)?),
                                None => None,
                            };

                            // Render env
                            c.env = render_string_map(&c.env, &mut tera, context)?;

                            // Render env_files
                            c.env_files = render_env_files(&c.env_files, &mut tera, context)?;
                        }
                    }
                    st.healthcheck = Some(healthcheck);
                }
                config.service = Some(ServiceConfig::Struct(st));
            } else {
                config.service = Some(service);
            }
        }

        // Render depends_on task and vars
        let mut rendered_depends_on = Vec::new();
        for depends_on in config.depends_on.iter() {
            match depends_on {
                DependsOnConfig::String(task) => {
                    let task = tera.render_str(task, context)?;
                    // Ignore if rendered task name is empty
                    if !task.ends_with("#") {
                        rendered_depends_on.push(DependsOnConfig::String(task))
                    }
                }
                DependsOnConfig::Struct(dep) => {
                    let task = tera.render_str(&dep.task, context)?;
                    // Ignore if rendered task name is empty
                    if !task.ends_with("#") {
                        let vars = render_value_map(&dep.vars, &mut tera, context, DynamicVar::Rejected, cache).await?;
                        rendered_depends_on.push(DependsOnConfig::Struct(DependsOnConfigStruct {
                            task,
                            vars,
                            cascade: dep.cascade,
                        }));
                    }
                }
            }
        }
        config.depends_on = rendered_depends_on;

        // Render wait_for task and vars
        let mut rendered_wait_for = Vec::new();
        for wait_for in config.wait_for.iter() {
            let task = tera.render_str(wait_for.task(), context)?;
            // Ignore if rendered task name is empty
            if task.ends_with("#") {
                continue;
            }
            match wait_for {
                WaitForConfig::String(_) => rendered_wait_for.push(WaitForConfig::String(task)),
                WaitForConfig::Struct(w) => {
                    let vars = render_value_map(&w.vars, &mut tera, context, DynamicVar::Rejected, cache).await?;
                    rendered_wait_for.push(WaitForConfig::Struct(WaitForConfigStruct { task, vars }));
                }
            }
        }
        config.wait_for = rendered_wait_for;

        // Render finalized_by task and vars
        let mut rendered_finalized_by = Vec::new();
        for finalized_by in config.finalized_by.iter() {
            let task = tera.render_str(finalized_by.task(), context)?;
            // Ignore if rendered task name is empty
            if task.ends_with("#") {
                continue;
            }
            match finalized_by {
                FinalizedByConfig::String(_) => rendered_finalized_by.push(FinalizedByConfig::String(task)),
                FinalizedByConfig::Struct(f) => {
                    let vars = render_value_map(&f.vars, &mut tera, context, DynamicVar::Rejected, cache).await?;
                    rendered_finalized_by.push(FinalizedByConfig::Struct(FinalizedByConfigStruct { task, vars }));
                }
            }
        }
        config.finalized_by = rendered_finalized_by;

        Ok(config)
    }

    pub fn is_variant(&self, other: &TaskConfig) -> bool {
        self.project == other.project && self.orig_name == other.orig_name
    }
}

impl ConfigRenderer {
    pub fn new(
        root_config: &ProjectConfig,
        child_config: &IndexMap<String, ProjectConfig>,
        vars: &IndexMap<String, VarsConfig>,
        watch: bool,
    ) -> Self {
        Self {
            root_config: root_config.clone(),
            child_configs: child_config.clone(),
            vars: vars.clone(),
            watch,
        }
    }

    fn base_context(&self) -> tera::Context {
        let mut context = tera::Context::new();
        let root_dir = self.root_config.dir.as_os_str().to_str().unwrap_or("");
        context.insert(ROOT_DIR_CONTEXT_KEY, root_dir);
        if self.child_configs.is_empty() {
            context.insert(PROJECT_DIR_CONTEXT_KEY, root_dir);
        } else {
            let project_dirs = self
                .child_configs
                .iter()
                .map(|(k, v)| (k.as_str(), v.dir.as_os_str().to_str().unwrap_or("")))
                .collect::<HashMap<_, _>>();
            context.insert(PROJECT_DIRS_CONTEXT_KEY, &project_dirs);
        }
        context.insert(WATCH_CONTEXT_KEY, &self.watch);
        context
    }

    pub async fn render(&mut self) -> anyhow::Result<(ProjectConfig, IndexMap<String, ProjectConfig>)> {
        let context = self.base_context();
        let mut cache = DynamicVarCache::new();
        let mut task_contexts = HashMap::new();
        let mut tasks = Vec::new();
        let mut num_variants = HashMap::new();

        // Root project task contexts
        let root_context = self.root_config.context(&context, &self.vars, &mut cache).await?;
        let (mut root_config, root_task_contexts) = self
            .root_config
            .render(&root_context, &mut cache)
            .await
            .with_context(|| "failed to render config of project root")?;
        tasks.extend(root_task_contexts.keys().cloned());
        task_contexts.extend(root_task_contexts);

        // Project task contexts
        let mut child_configs = IndexMap::new();
        for (k, c) in self.child_configs.iter_mut() {
            let project_context = c.context(&context, &self.vars, &mut cache).await?;
            let (child_config, child_task_contexts) = c
                .render(&project_context, &mut cache)
                .await
                .with_context(|| format!("failed to render config of project {:?}", c.name))?;
            child_configs.insert(k.clone(), child_config);
            tasks.extend(child_task_contexts.keys().cloned());
            task_contexts.extend(child_task_contexts);
        }

        tasks.sort();
        for t in tasks.iter() {
            Self::render_variant_tasks(
                t,
                &mut root_config,
                &mut child_configs,
                &mut self.root_config,
                &mut self.child_configs,
                &mut num_variants,
                &mut task_contexts,
                &mut cache,
            )
            .await?;
        }

        cache.warn_uncached();

        Ok((root_config.clone(), child_configs.clone()))
    }

    fn set_task<'a>(
        task_config: TaskConfig,
        root_config: &'a mut ProjectConfig,
        child_configs: &'a mut IndexMap<String, ProjectConfig>,
    ) {
        if task_config.project.is_empty() {
            root_config.tasks.insert(task_config.name.clone(), task_config);
        } else if let Some(c) = child_configs.get_mut(&task_config.project) {
            c.tasks.insert(task_config.name.clone(), task_config);
        }
    }

    fn get_task<'a>(
        task_name: &str,
        root_config: &'a ProjectConfig,
        child_configs: &'a IndexMap<String, ProjectConfig>,
    ) -> Option<(&'a TaskConfig, &'a ProjectConfig)> {
        if let Some((p, t)) = task_name.split_once("#") {
            if p.is_empty() {
                return match root_config.tasks.get(t) {
                    Some(t) => Some((t, root_config)),
                    None => None,
                };
            }
            if let Some(c) = child_configs.get(p) {
                return match c.tasks.get(t) {
                    Some(t) => Some((t, c)),
                    None => None,
                };
            }
        }
        None
    }

    fn get_variant_tasks<'a>(
        orig_name: &str,
        root_config: &'a ProjectConfig,
        child_configs: &'a IndexMap<String, ProjectConfig>,
    ) -> Vec<&'a TaskConfig> {
        if let Some((p, orig_name)) = orig_name.split_once("#") {
            if p.is_empty() {
                return root_config
                    .tasks
                    .values()
                    .filter(|t| t.orig_name == orig_name)
                    .collect::<Vec<_>>();
            }
            if let Some(c) = child_configs.get(p) {
                return c
                    .tasks
                    .values()
                    .filter(|t| t.orig_name == orig_name)
                    .collect::<Vec<_>>();
            }
        }
        Vec::new()
    }

    #[async_recursion]
    #[allow(clippy::too_many_arguments)]
    async fn render_variant_tasks(
        task_name: &str,
        root_config: &mut ProjectConfig,
        child_configs: &mut IndexMap<String, ProjectConfig>,
        raw_root_config: &mut ProjectConfig,
        raw_child_configs: &mut IndexMap<String, ProjectConfig>,
        num_variants: &mut HashMap<String, usize>,
        contexts: &mut HashMap<String, tera::Context>,
        cache: &mut DynamicVarCache,
    ) -> anyhow::Result<()> {
        // Get task config
        let (task_config, _project_config) =
            Self::get_task(task_name, root_config, child_configs).context(format!("unknown task {:?}", task_name))?;
        let context = contexts
            .get(task_name)
            .context(format!("unknown task {:?}", task_name))?;
        debug!(
            "Task: {:?}\ncontext: {:#?}\nvars: {:#?}",
            task_name, context, task_config.vars
        );

        let mut task_config = task_config.clone();

        // Render task variants.
        // When a dependency task is specified with vars, it is considered as a different task.
        // Task variants are managed internally with sequentially numbered suffixes, ex: {name}-1, {name}-2.
        for depends_on in task_config.depends_on.iter_mut() {
            // With struct notation
            let DependsOnConfig::Struct(depends_on) = depends_on else {
                continue;
            };
            // With vars
            if depends_on.vars.is_empty() {
                continue;
            };
            // Replace the depends_on task name with the variant name
            depends_on.task = Self::render_variant_task(
                task_name,
                &depends_on.task,
                &depends_on.vars,
                root_config,
                child_configs,
                raw_root_config,
                raw_child_configs,
                num_variants,
                contexts,
                cache,
            )
            .await?;
        }

        // A finalizer specified with vars is a variant as well
        for finalized_by in task_config.finalized_by.iter_mut() {
            let FinalizedByConfig::Struct(finalized_by) = finalized_by else {
                continue;
            };
            if finalized_by.vars.is_empty() {
                continue;
            };
            finalized_by.task = Self::render_variant_task(
                task_name,
                &finalized_by.task,
                &finalized_by.vars,
                root_config,
                child_configs,
                raw_root_config,
                raw_child_configs,
                num_variants,
                contexts,
                cache,
            )
            .await?;
        }

        Self::set_task(task_config, root_config, child_configs);

        Ok(())
    }

    /// Renders the variant of the task `dep_task_name` with the given vars merged, for the task
    /// `task_name` that refers to it, and returns the variant name. An existing variant with the
    /// same vars is reused.
    #[async_recursion]
    #[allow(clippy::too_many_arguments)]
    async fn render_variant_task(
        task_name: &str,
        dep_task_name: &str,
        vars: &IndexMap<String, VarsConfig>,
        root_config: &mut ProjectConfig,
        child_configs: &mut IndexMap<String, ProjectConfig>,
        raw_root_config: &mut ProjectConfig,
        raw_child_configs: &mut IndexMap<String, ProjectConfig>,
        num_variants: &mut HashMap<String, usize>,
        contexts: &mut HashMap<String, tera::Context>,
        cache: &mut DynamicVarCache,
    ) -> anyhow::Result<String> {
        // Get the raw config of the referenced task
        let (dep_task, dep_project) = Self::get_task(dep_task_name, raw_root_config, raw_child_configs).context(
            format!("unknown task {:?} referred to by {:?}", dep_task_name, task_name),
        )?;

        let mut variant_task = dep_task.clone();

        // Merge the given vars into the task vars.
        // Only the vars that already exist in the task are merged to avoid unnecessary variant tasks.
        for (k, v) in vars.iter().filter(|(k, _)| dep_task.vars.contains_key(*k)) {
            variant_task.vars.insert(k.clone(), v.clone());
        }

        // Create context from dependency task
        let dep_context = contexts
            .get(&dep_task.full_name())
            .context(format!("unknown task {:?}", dep_task.full_name()))?;
        let variant_context = variant_task.context(dep_project, dep_context, cache).await?;

        // Render
        let mut rendered_variant_task = variant_task.render(&variant_context, cache).await?;

        debug!(
            "Variant?: {:?}, dependent: {:?}\ncontext: {:#?}\nvars: {:#?}",
            rendered_variant_task.full_name(),
            task_name,
            variant_context,
            rendered_variant_task.vars,
        );

        // Two variants are equal when their original names and contexts are same
        if let Some(same_variant) =
            Self::get_variant_tasks(&rendered_variant_task.full_name(), root_config, child_configs)
                .iter()
                .find(|t| {
                    contexts
                        .get(&t.full_name())
                        .map(|c| *c == variant_context)
                        .unwrap_or(false)
                })
        {
            // Reuse the variant with the same vars
            return Ok(same_variant.full_name());
        }

        // Name
        let suffix = num_variants
            .entry(dep_task.full_orig_name())
            .and_modify(|v| *v += 1)
            .or_insert(1);
        let variant_task_name = format!("{}-{}", dep_task.full_name(), suffix);
        rendered_variant_task.name = Task::split_name(&variant_task_name).1.to_string();

        info!(
            "Variant: {:?}, dependent: {:?}\ncontext: {:#?}\nvars: {:#?}",
            rendered_variant_task.full_name(),
            task_name,
            variant_context,
            rendered_variant_task.vars
        );

        contexts.insert(variant_task_name.clone(), variant_context);

        // Add task variant config
        Self::set_task(rendered_variant_task, root_config, child_configs);

        // Render dependency tasks recursively
        Self::render_variant_tasks(
            &variant_task_name,
            root_config,
            child_configs,
            raw_root_config,
            raw_child_configs,
            num_variants,
            contexts,
            cache,
        )
        .await?;

        Ok(variant_task_name)
    }
}

fn render_string_map(
    map: &IndexMap<String, String>,
    tera: &mut Tera,
    context: &tera::Context,
) -> anyhow::Result<IndexMap<String, String>> {
    let mut ret = IndexMap::new();
    for (k, v) in map.iter() {
        let rk = tera.render_str(k, context)?;
        if !rk.is_empty() {
            let rv = tera.render_str(v, context)?;
            ret.insert(rk, rv);
        }
    }
    Ok(ret)
}

fn render_env_files(env_files: &[String], tera: &mut Tera, context: &tera::Context) -> anyhow::Result<Vec<String>> {
    let mut ret = Vec::new();
    for env_file in env_files {
        let rendered = tera.render_str(env_file, context)?;
        if !rendered.is_empty() {
            ret.push(rendered);
        }
    }
    Ok(ret)
}

/// How [`render_value_map`] resolves a dynamic var.
enum DynamicVar {
    /// The var has already run while the task context was built, so its value is taken from the
    /// context instead of running the command a second time.
    FromContext,
    /// The var is passed to another task by `depends_on`, `wait_for` or `finalized_by`, which
    /// takes a value, not a command.
    Rejected,
}

async fn render_value_map(
    map: &IndexMap<String, VarsConfig>,
    tera: &mut Tera,
    context: &tera::Context,
    dynamic: DynamicVar,
    cache: &mut DynamicVarCache,
) -> anyhow::Result<IndexMap<String, VarsConfig>> {
    let mut ret = IndexMap::new();
    for (k, v) in map.iter() {
        let rk = tera.render_str(k, context)?;
        if !rk.is_empty() {
            let rv = match v {
                // Unset vars stay unset; they are reported as an error when the task is run
                _ if v.is_unset() => JsonValue::Null,
                VarsConfig::Dynamic(_) => match dynamic {
                    DynamicVar::FromContext => context
                        .get(&rk)
                        .cloned()
                        .with_context(|| format!("dynamic var {:?} has no rendered value", rk))?,
                    DynamicVar::Rejected => anyhow::bail!(
                        "var {:?} cannot be dynamic here. Declare it as a project-level or task-level var and pass its value instead",
                        rk
                    ),
                },
                VarsConfig::Static(_) => render_value(v, tera, context, cache).await?,
            };
            ret.insert(rk, VarsConfig::Static(rv));
        }
    }
    Ok(ret)
}

#[async_recursion]
async fn render_value(
    value: &VarsConfig,
    tera: &mut Tera,
    context: &tera::Context,
    cache: &mut DynamicVarCache,
) -> anyhow::Result<JsonValue> {
    let rendered = match value {
        VarsConfig::Static(s) => match s {
            JsonValue::String(s) => {
                let str = tera
                    .render_str(s, context)
                    .context(format!("failed to render {:?}", s))?;
                if str.is_empty() {
                    return Ok(JsonValue::String(str));
                }
                let yaml_value =
                    serde_yaml::from_str::<Value>(&str).context(format!("failed to read YAML value {:?}", str))?;
                match yaml_value {
                    Value::Null => JsonValue::Null,
                    Value::Bool(b) => JsonValue::Bool(b),
                    Value::Number(n) => yaml_number_to_json_number(&n).unwrap_or(JsonValue::Null),
                    Value::String(s) => JsonValue::String(s),
                    _ => JsonValue::String(str),
                }
            }
            JsonValue::Number(_) | JsonValue::Bool(_) | JsonValue::Null => s.clone(),
            JsonValue::Array(items) => {
                let mut rendered_items = Vec::with_capacity(items.len());
                for item in items {
                    rendered_items.push(render_value(&VarsConfig::Static(item.clone()), tera, context, cache).await?);
                }
                JsonValue::Array(rendered_items)
            }
            JsonValue::Object(map) => {
                let mut rendered_map = Map::with_capacity(map.len());
                for (k, v) in map.iter() {
                    rendered_map.insert(
                        k.clone(),
                        render_value(&VarsConfig::Static(v.clone()), tera, context, cache).await?,
                    );
                }
                JsonValue::Object(rendered_map)
            }
        },
        VarsConfig::Dynamic(s) => {
            let inner = s.inner.clone().context("dynamic vars inner value should be present")?;
            // The key is built even when the var is not cached, so that its runs can be counted
            let key = DynamicVarKey::new(&inner);
            let cached = inner.cache.then(|| cache.get(&key)).flatten();
            let trimmed = match cached {
                Some(cached) => {
                    debug!("Dynamic var {:?} reuses the cached output {:?}", inner.name, cached);
                    cached.clone()
                }
                None => {
                    let mut args = Vec::new();
                    args.extend(inner.shell.args.clone());
                    args.push(inner.command.clone());
                    let command = Command::new(inner.shell.command.clone())
                        .with_args(args)
                        .with_envs(inner.env.clone())
                        .with_current_dir(inner.working_dir.clone())
                        .to_owned();
                    let started = Instant::now();
                    let output = execute_command(&command)
                        .await
                        .context(format!("failed to render dynamic var {:?}", inner.name))?;
                    let trimmed = output.trim().to_string();
                    if inner.cache {
                        cache.insert(key, trimmed.clone());
                    } else {
                        cache.record_run(&key, &inner.name, started.elapsed());
                    }
                    trimmed
                }
            };
            render_value(&VarsConfig::Static(JsonValue::from(trimmed)), tera, context, cache).await?
        }
    };
    Ok(rendered)
}

/// Executes a command and returns the output as a string.
async fn execute_command(command: &Command) -> anyhow::Result<String> {
    let manager = ProcessManager::new(false);
    let mut process = match manager.spawn(command.clone(), DYNAMIC_VAR_STOP_TIMEOUT).await {
        Some(Ok(child)) => child,
        Some(Err(e)) => anyhow::bail!("failed to spawn process: {:?}", e),
        _ => anyhow::bail!("failed to spawn process"),
    };
    let stdout_collector = OutputCollector::new();
    let stderr_collector = OutputCollector::new();
    let exit = process
        .wait_with_piped_outputs(stdout_collector.clone(), stderr_collector)
        .await;
    let output = stdout_collector.take_output().trim().to_string();
    Ok(match exit {
        Ok(Some(exit_status)) => match exit_status {
            // Trim trailing newline
            ChildExit::Finished(Some(0)) => output,
            ChildExit::Finished(Some(code)) => {
                anyhow::bail!("process exited with non-zero code: {:?}, output: {:?}", code, output)
            }
            ChildExit::Finished(None) => {
                anyhow::bail!("process exited with unknown exit code. output: {:?}", output)
            }
            ChildExit::Killed | ChildExit::KilledExternal => {
                anyhow::bail!("process is killed by signal. output: {:?}", output)
            }
            ChildExit::Failed => anyhow::bail!("process failed"),
        },
        Ok(None) => anyhow::bail!("failed to get the exit code"),
        Err(e) => anyhow::bail!("error while waiting process: {:?}", e),
    })
}

/// Converts serde_yaml::Value::Number to serde_json::Value::Number
fn yaml_number_to_json_number(yaml_num: &serde_yaml::Number) -> Option<serde_json::Value> {
    if let Some(i) = yaml_num.as_i64() {
        Some(serde_json::Value::Number(serde_json::Number::from(i)))
    } else if let Some(u) = yaml_num.as_u64() {
        Some(serde_json::Value::Number(serde_json::Number::from(u)))
    } else if let Some(f) = yaml_num.as_f64() {
        serde_json::Number::from_f64(f).map(serde_json::Value::Number)
    } else {
        None
    }
}

/// Creates a [`Tera`] instance with the Firepit custom filters registered.
///
/// All template rendering must go through this function so that every template
/// field supports the same set of filters.
///
/// # Examples
///
/// ```
/// use firepit::template::new_tera;
///
/// let mut tera = new_tera();
/// let mut context = tera::Context::new();
/// context.insert("value", "a b");
/// assert_eq!(tera.render_str("echo {{ value | quote }}", &context).unwrap(), "echo 'a b'");
/// ```
pub fn new_tera() -> Tera {
    let mut tera = Tera::default();
    tera.register_filter("quote", quote_filter);
    tera
}

/// Tera filter that escapes a value so that it can be safely embedded in a shell
/// command as a single argument.
///
/// Strings are quoted with [`shlex`], so whitespace, quotes, newlines and shell
/// metacharacters in the value cannot break out of the argument.
/// Numbers, booleans and `null` are quoted the same way after being stringified,
/// and arrays are quoted element-wise and joined with a single space, which is
/// handy for passing a list of arguments.
///
/// # Errors
///
/// Returns an error when the value is a map, when an array contains a nested
/// array or map, or when the value contains a nul byte, which cannot be
/// represented as a shell argument.
fn quote_filter(value: &JsonValue, _args: &HashMap<String, JsonValue>) -> tera::Result<JsonValue> {
    let quoted = match value {
        JsonValue::Array(items) => {
            let words = items.iter().map(scalar_to_string).collect::<tera::Result<Vec<_>>>()?;
            shlex::try_join(words.iter().map(|w| w.as_str()))
                .map_err(|e| tera::Error::msg(format!("failed to quote array for shell: {}", e)))?
        }
        _ => {
            let word = scalar_to_string(value)?;
            shlex::try_quote(&word)
                .map_err(|e| tera::Error::msg(format!("failed to quote {:?} for shell: {}", word, e)))?
                .into_owned()
        }
    };
    Ok(JsonValue::String(quoted))
}

/// Stringifies a scalar JSON value for the `quote` filter.
///
/// # Errors
///
/// Returns an error for arrays and maps, which have no single-argument shell
/// representation.
fn scalar_to_string(value: &JsonValue) -> tera::Result<String> {
    match value {
        JsonValue::String(s) => Ok(s.clone()),
        JsonValue::Number(n) => Ok(n.to_string()),
        JsonValue::Bool(b) => Ok(b.to_string()),
        JsonValue::Null => Ok(String::new()),
        JsonValue::Array(_) | JsonValue::Object(_) => Err(tera::Error::msg(
            "the `quote` filter accepts a string, number, boolean, null or an array of them",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(command: &str) -> DynamicVarKey {
        DynamicVarKey {
            shell_command: String::from("sh"),
            shell_args: vec![String::from("-c")],
            command: String::from(command),
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn test_uncached_hints() {
        let mut cache = DynamicVarCache::new();
        let long = DYNAMIC_VAR_HINT_THRESHOLD;

        // Ran once: caching it would save nothing, however slow it is
        cache.record_run(&key("once"), "once", long);
        // Ran twice but fast: not worth bothering the user about
        cache.record_run(&key("fast"), "fast", Duration::ZERO);
        cache.record_run(&key("fast"), "fast", Duration::ZERO);
        // Ran twice and slow in total, even though neither run reaches the threshold alone
        cache.record_run(&key("slow"), "slow", long / 2);
        cache.record_run(&key("slow"), "slow", long / 2);
        // Two variables sharing a command are counted apart, so neither reaches two runs and the
        // report cannot blame one of them for the other's execution
        cache.record_run(&key("shared"), "twin-a", long);
        cache.record_run(&key("shared"), "twin-b", long);

        let hints = cache.uncached_hints();
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].0, "slow");
        assert_eq!(hints[0].1.count, 2);
    }

    fn quote(value: JsonValue) -> tera::Result<String> {
        let args = HashMap::new();
        quote_filter(&value, &args).map(|v| v.as_str().unwrap_or_default().to_string())
    }

    #[test]
    fn test_quote_scalars() {
        assert_eq!(quote(json!("simple")).unwrap(), "simple");
        assert_eq!(quote(json!("a b'c")).unwrap(), r#""a b'c""#);
        assert_eq!(quote(json!("foo; date")).unwrap(), "'foo; date'");
        assert_eq!(quote(json!("line1\nline2")).unwrap(), "'line1\nline2'");
        assert_eq!(quote(json!("")).unwrap(), "''");
        assert_eq!(quote(json!(42)).unwrap(), "42");
        assert_eq!(quote(json!(true)).unwrap(), "true");
        // Null becomes an empty argument rather than being dropped
        assert_eq!(quote(JsonValue::Null).unwrap(), "''");
    }

    #[test]
    fn test_quote_array() {
        assert_eq!(quote(json!(["foo", "bar baz"])).unwrap(), "foo 'bar baz'");
        assert_eq!(quote(json!([])).unwrap(), "");
    }

    #[test]
    fn test_quote_errors() {
        // A nul byte cannot be represented as a shell argument
        assert!(quote(json!("a\0b")).is_err());
        // Maps and nested collections have no single-argument representation
        assert!(quote(json!({ "a": 1 })).is_err());
        assert!(quote(json!([["nested"]])).is_err());
    }
}
