use crate::config::{
    DependsOnConfig, DependsOnConfigStruct, DynamicVarsInner, HealthCheckConfig, ProjectConfig, ServiceConfig,
    TaskConfig, VarsConfig,
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
use std::collections::HashMap;
use std::path::PathBuf;
use tera::Tera;
use tracing::{debug, info};

pub struct ConfigRenderer {
    root_config: ProjectConfig,
    child_configs: IndexMap<String, ProjectConfig>,
    vars: IndexMap<String, VarsConfig>,
}

pub const ROOT_DIR_CONTEXT_KEY: &str = "root_dir";
pub const PROJECT_DIRS_CONTEXT_KEY: &str = "project_dirs";
pub const PROJECT_DIR_CONTEXT_KEY: &str = "project_dir";
pub const PROJECT_CONTEXT_KEY: &str = "project";
pub const TASK_CONTEXT_KEY: &str = "task";

impl ProjectConfig {
    pub async fn context(
        &self,
        context: &tera::Context,
        vars: &IndexMap<String, VarsConfig>,
    ) -> anyhow::Result<tera::Context> {
        let mut tera = Tera::default();
        let mut context = context.clone();
        context.insert(PROJECT_CONTEXT_KEY, &self.name);
        context.insert(PROJECT_DIR_CONTEXT_KEY, &self.dir.as_os_str().to_str().unwrap_or(""));

        // Render project-level vars.
        // Argument vars override project-level vars.
        for (k, v) in self.vars.iter().chain(vars.iter()) {
            let rk = tera.render_str(&k, &context)?;
            if !rk.is_empty() {
                let v = match v {
                    VarsConfig::Dynamic(s) => {
                        let mut s = s.clone();
                        s.command = tera.render_str(&s.command, &context)?;
                        s.env = render_string_map(&s.env, &mut tera, &context)?;
                        s.env_files = s
                            .env_files
                            .iter()
                            .map(|f| tera.render_str(f, &context))
                            .collect::<anyhow::Result<Vec<_>, _>>()?;
                        s.working_dir = s.working_dir.map(|w| tera.render_str(&w, &context)).transpose()?;
                        s.inner = Some(DynamicVarsInner {
                            name: k.clone(),
                            command: s.command.clone(),
                            shell: s.shell.clone().unwrap_or(self.shell.clone()),
                            env: Env::new().with(&s.env_file_paths(&self.dir), &s.env).load()?,
                            working_dir: s.working_dir_path(&self.dir).unwrap_or(self.working_dir_path()),
                        });
                        VarsConfig::Dynamic(s)
                    }
                    VarsConfig::Static(_) => v.clone(),
                };
                let rv = render_value(&v, &mut tera, &context).await?;
                context.insert(rk, &rv);
            }
        }

        Ok(context)
    }

    pub async fn render(&self, context: &tera::Context) -> anyhow::Result<ProjectConfig> {
        let mut tera = Tera::default();

        let mut config = self.clone();

        // Render includes
        let mut rendered_includes = Vec::new();
        for f in config.includes.iter() {
            rendered_includes.push(tera.render_str(f, &context)?);
        }
        config.includes = rendered_includes;

        // Render working_dir
        config.working_dir = tera.render_str(&config.working_dir, &context)?;

        // Render env
        config.env = render_string_map(&config.env, &mut tera, context)?;

        // Render env_files
        config.env_files = config
            .env_files
            .iter()
            .map(|f| tera.render_str(f, &context))
            .collect::<anyhow::Result<Vec<_>, _>>()?;

        // Render tasks
        let mut rendered_tasks = IndexMap::new();

        for (task_name, task_config) in config.tasks.iter_mut() {
            let task_context = task_config.context(self, context).await?;
            let task_config = task_config.render(&task_context).await?;
            rendered_tasks.insert(task_name.clone(), task_config);
        }
        config.tasks = rendered_tasks;

        Ok(config)
    }
}

impl TaskConfig {
    pub async fn context(&self, config: &ProjectConfig, context: &tera::Context) -> anyhow::Result<tera::Context> {
        let mut tera = Tera::default();
        let mut context = context.clone();
        context.insert(TASK_CONTEXT_KEY, &self.full_orig_name());

        // Render task-level vars
        for (k, v) in self.vars.iter() {
            let rk = tera.render_str(&k, &context)?;
            if !rk.is_empty() {
                let v = match v {
                    VarsConfig::Dynamic(s) => {
                        let mut s = s.clone();
                        s.command = tera.render_str(&s.command, &context)?;
                        s.env = render_string_map(&s.env, &mut tera, &context)?;
                        s.env_files = s
                            .env_files
                            .iter()
                            .map(|f| tera.render_str(f, &context))
                            .collect::<anyhow::Result<Vec<_>, _>>()?;
                        s.working_dir = s.working_dir.map(|w| tera.render_str(&w, &context)).transpose()?;
                        s.inner = Some(DynamicVarsInner {
                            name: k.clone(),
                            command: s.command.clone(),
                            shell: s
                                .shell
                                .clone()
                                .unwrap_or(self.shell.clone().unwrap_or(config.shell.clone())),
                            env: Env::new().with(&s.env_file_paths(&self.dir), &s.env).load()?,
                            working_dir: s
                                .working_dir_path(&self.dir)
                                .unwrap_or(self.working_dir_path(&self.dir).unwrap_or(config.working_dir_path())),
                        });
                        VarsConfig::Dynamic(s)
                    }
                    VarsConfig::Static(_) => v.clone(),
                };
                let rv = render_value(&v, &mut tera, &context).await?;
                context.insert(rk, &rv);
            }
        }
        Ok(context)
    }

    pub async fn render(&self, context: &tera::Context) -> anyhow::Result<TaskConfig> {
        let mut config = self.clone();
        let mut tera = Tera::default();

        // Render task-level vars
        config.vars = render_value_map(&config.vars, &mut tera, context).await?;

        // Render label
        if let Some(l) = config.label {
            config.label = Some(tera.render_str(&l, &context)?);
        }

        // Render command
        config.command = config.command.map(|c| tera.render_str(&c, &context)).transpose()?;

        // Render working_dir
        config.working_dir = match config.working_dir {
            Some(w) => Some(tera.render_str(&w, &context)?),
            None => None,
        };

        // Render env
        config.env = render_string_map(&config.env, &mut tera, &context)?;

        // Render env_files
        let mut rendered_env_files = Vec::new();
        for f in config.env_files.iter() {
            rendered_env_files.push(tera.render_str(f, &context)?);
        }
        config.env_files = rendered_env_files;

        // Render service.healthcheck
        if let Some(service) = config.service {
            if let ServiceConfig::Struct(mut st) = service {
                if let Some(mut healthcheck) = st.healthcheck {
                    match healthcheck {
                        // Log Probe
                        HealthCheckConfig::Log(ref mut c) => {
                            // Render log
                            c.log = tera.render_str(&c.log, &context)?;
                        }
                        // Exec Probe
                        HealthCheckConfig::Exec(ref mut c) => {
                            // Render command
                            c.command = tera.render_str(&c.command, &context)?;

                            // Render working_dir
                            c.working_dir = match &c.working_dir {
                                Some(w) => Some(tera.render_str(&w, &context)?),
                                None => None,
                            };

                            // Render env
                            c.env = render_string_map(&c.env, &mut tera, context)?;

                            // Render env_files
                            c.env_files = c
                                .env_files
                                .iter()
                                .map(|f| tera.render_str(f, &context))
                                .collect::<anyhow::Result<Vec<_>, _>>()?;
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
                    let task = tera.render_str(task, &context)?;
                    // Ignore if rendered task name is empty
                    if !task.ends_with("#") {
                        rendered_depends_on.push(DependsOnConfig::String(task))
                    }
                }
                DependsOnConfig::Struct(dep) => {
                    let task = tera.render_str(&dep.task, &context)?;
                    // Ignore if rendered task name is empty
                    if !task.ends_with("#") {
                        let vars = render_value_map(&dep.vars, &mut tera, context).await?;
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
    ) -> Self {
        Self {
            root_config: root_config.clone(),
            child_configs: child_config.clone(),
            vars: vars.clone(),
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
        context
    }

    pub async fn render(&mut self) -> anyhow::Result<(ProjectConfig, IndexMap<String, ProjectConfig>)> {
        let context = self.base_context();
        let mut task_contexts = HashMap::new();
        let mut tasks = Vec::new();
        let mut num_variants = HashMap::new();

        // Root project task contexts
        let root_context = self.root_config.context(&context, &self.vars).await?;
        let mut root_config = self
            .root_config
            .render(&root_context)
            .await
            .with_context(|| "failed to render config of project root")?;
        for t in self.root_config.tasks.values() {
            tasks.push(t.full_name());
            task_contexts.insert(t.full_name(), t.context(&root_config, &root_context).await?);
        }

        // Project task contexts
        let mut child_configs = IndexMap::new();
        for (k, c) in self.child_configs.iter_mut() {
            let project_context = c.context(&context, &self.vars).await?;
            child_configs.insert(
                k.clone(),
                c.render(&project_context)
                    .await
                    .with_context(|| format!("failed to render config of project {:?}", c.name))?,
            );
            for t in c.tasks.values() {
                tasks.push(t.full_name());
                task_contexts.insert(t.full_name(), t.context(c, &project_context).await?);
            }
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
            )
            .await?;
        }

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
    async fn render_variant_tasks(
        task_name: &str,
        root_config: &mut ProjectConfig,
        child_configs: &mut IndexMap<String, ProjectConfig>,
        raw_root_config: &mut ProjectConfig,
        raw_child_configs: &mut IndexMap<String, ProjectConfig>,
        num_variants: &mut HashMap<String, usize>,
        contexts: &mut HashMap<String, tera::Context>,
    ) -> anyhow::Result<()> {
        // Get task config
        let (task_config, project_config) =
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

            // Get raw dependency task config
            let (dep_task, dep_project) = Self::get_task(&depends_on.task, raw_root_config, raw_child_configs)
                .context(format!(
                    "unknown dependency task {:?} for {:?}",
                    depends_on.task, task_name
                ))?;

            let mut variant_task = dep_task.clone();

            // Merge depends_on vars into dependency task vars.
            // Only the vars that already exist in the dependency task are merged to avoid unnecessary variant tasks.
            for (k, v) in depends_on.vars.iter().filter(|(k, _)| dep_task.vars.contains_key(*k)) {
                variant_task.vars.insert(k.clone(), v.clone());
            }

            // Create context from dependency task
            let dep_context = contexts
                .get(&dep_task.full_name())
                .context(format!("unknown task {:?}", dep_task.full_name()))?;
            let variant_context = variant_task.context(dep_project, dep_context).await?;

            // Render
            let mut rendered_variant_task = variant_task.render(&variant_context).await?;

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
                // Replace the depends_on task name with the variant with the same vars
                depends_on.task = same_variant.full_name();
                continue;
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

            // Replace the depends_on task name with the variant name
            depends_on.task = variant_task_name.clone();

            // Render dependency tasks recursively
            Self::render_variant_tasks(
                &variant_task_name,
                root_config,
                child_configs,
                raw_root_config,
                raw_child_configs,
                num_variants,
                contexts,
            )
            .await?;
        }

        Self::set_task(task_config, root_config, child_configs);

        Ok(())
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

async fn render_value_map(
    map: &IndexMap<String, VarsConfig>,
    tera: &mut Tera,
    context: &tera::Context,
) -> anyhow::Result<IndexMap<String, VarsConfig>> {
    let mut ret = IndexMap::new();
    for (k, v) in map.iter() {
        let rk = tera.render_str(k, context)?;
        if !rk.is_empty() {
            let rv = VarsConfig::Static(render_value(v, tera, context).await?);
            ret.insert(rk, rv);
        }
    }
    Ok(ret)
}

#[async_recursion]
async fn render_value(value: &VarsConfig, tera: &mut Tera, context: &tera::Context) -> anyhow::Result<JsonValue> {
    let rendered = match value {
        VarsConfig::Static(s) => match s {
            JsonValue::String(s) => {
                let str = tera
                    .render_str(s, context)
                    .context(format!("failed to render {:?}", s))?;
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
                    rendered_items.push(render_value(&VarsConfig::Static(item.clone()), tera, context).await?);
                }
                JsonValue::Array(rendered_items)
            }
            JsonValue::Object(map) => {
                let mut rendered_map = Map::with_capacity(map.len());
                for (k, v) in map.iter() {
                    rendered_map.insert(
                        k.clone(),
                        render_value(&VarsConfig::Static(v.clone()), tera, context).await?,
                    );
                }
                JsonValue::Object(rendered_map)
            }
        },
        VarsConfig::Dynamic(s) => {
            let inner = s.inner.clone().context("dynamic vars inner value should be present")?;
            let name = inner.name;
            let command = inner.command;
            let shell = inner.shell;
            let working_dir = inner.working_dir;
            let mut args = Vec::new();
            args.extend(shell.args);
            args.push(command);
            let command = Command::new(shell.command)
                .with_args(args)
                .with_envs(inner.env)
                .with_current_dir(PathBuf::from(working_dir))
                .to_owned();
            let output = execute_command(&command)
                .await
                .context(format!("failed to render dynamic var {:?}", name))?;
            let trimmed = output.trim().to_string();
            render_value(&VarsConfig::Static(JsonValue::from(trimmed)), tera, context).await?
        }
    };
    Ok(rendered)
}

/// Executes a command and returns the output as a string.
async fn execute_command(command: &Command) -> anyhow::Result<String> {
    let manager = ProcessManager::infer();
    let mut process = match manager.spawn(command.clone(), DYNAMIC_VAR_STOP_TIMEOUT).await {
        Some(Ok(child)) => child,
        Some(Err(e)) => anyhow::bail!("failed to spawn process: {:?}", e),
        _ => anyhow::bail!("failed to spawn process"),
    };
    let output_collector = OutputCollector::new();
    let exit = process.wait_with_piped_outputs(output_collector.clone()).await;
    Ok(match exit {
        Ok(Some(exit_status)) => match exit_status {
            ChildExit::Finished(Some(code)) if code == 0 => output_collector.take_output(),
            ChildExit::Finished(Some(code)) => anyhow::bail!("process exited with non-zero code {:?}", code),
            ChildExit::Finished(None) => anyhow::bail!("process exited with unknown exit code"),
            ChildExit::Killed | ChildExit::KilledExternal => anyhow::bail!("process is killed by signal"),
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
