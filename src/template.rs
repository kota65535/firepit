use crate::config::{
    default_healthcheck_interval, DependsOnConfig, HealthCheckConfig, ProjectConfig, Restart, ServiceConfig, TaskConfig,
};
use crate::probe::{ExecProbe, LogLineProbe, Probe};
use crate::project::Task;
use anyhow::Context;
use nix::NixPath;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use tera::Tera;
use tracing::{debug, info};

pub struct ConfigRenderer {
    root_config: ProjectConfig,
    child_configs: HashMap<String, ProjectConfig>,
}

const ROOT_DIR_CONTEXT_KEY: &str = "root_dir";
const PROJECT_DIR_CONTEXT_KEY: &str = "project_dir";
const PROJECT_CONTEXT_KEY: &str = "project";
const TASK_CONTEXT_KEY: &str = "task";

impl ProjectConfig {
    pub fn context(&mut self, context: &tera::Context) -> anyhow::Result<tera::Context> {
        let mut tera = Tera::default();
        let mut context = context.clone();
        context.insert(PROJECT_CONTEXT_KEY, &self.name);

        // Render vars
        let mut rendered_vars = HashMap::new();
        for (k, v) in self.vars.iter() {
            rendered_vars.insert(k.clone(), tera.render_str(&v, &context)?);
        }

        // Update context with rendered vars
        for (k, v) in rendered_vars.iter() {
            context.insert(k, v)
        }
        Ok(context)
    }

    pub fn render(&self, context: &tera::Context) -> anyhow::Result<ProjectConfig> {
        let mut tera = Tera::default();

        let mut config = self.clone();

        // Render working_dir
        config.working_dir = tera.render_str(&config.working_dir, &context)?;

        // Render env
        let mut rendered_env = HashMap::new();
        for (k, v) in config.env.iter() {
            rendered_env.insert(k.clone(), tera.render_str(v, &context)?);
        }
        config.env = rendered_env;

        // Render env_files
        let mut rendered_env_files = Vec::new();
        for f in config.env_files.iter() {
            rendered_env_files.push(tera.render_str(f, &context)?);
        }
        config.env_files = rendered_env_files;

        // Render tasks
        let mut rendered_tasks = HashMap::new();

        for (task_name, task_config) in config.tasks.iter_mut() {
            let task_context = task_config.context(context)?;
            let task_config = task_config.render(&task_context)?;
            rendered_tasks.insert(task_name.clone(), task_config);
        }
        config.tasks = rendered_tasks;

        Ok(config)
    }
}

impl TaskConfig {
    pub fn context(&self, context: &tera::Context) -> anyhow::Result<tera::Context> {
        let mut tera = Tera::default();
        let mut context = context.clone();
        context.insert(TASK_CONTEXT_KEY, &self.name);

        // Render vars
        let mut rendered_vars = HashMap::new();
        for (k, v) in self.vars.iter() {
            rendered_vars.insert(k.clone(), tera.render_str(&v, &context)?);
        }

        // Update context with rendered vars
        for (k, v) in rendered_vars.iter() {
            context.insert(k, v)
        }
        Ok(context)
    }

    pub fn render(&self, context: &tera::Context) -> anyhow::Result<(TaskConfig)> {
        let mut config = self.clone();
        let mut tera = Tera::default();

        // Render vars
        let mut rendered_vars = HashMap::new();
        for (k, v) in config.vars.iter() {
            rendered_vars.insert(k.clone(), tera.render_str(&v, &context)?);
        }
        config.vars = rendered_vars;

        // Render label
        if let Some(l) = config.label {
            config.label = Some(tera.render_str(&l, &context)?);
        }

        // Render command
        config.command = tera.render_str(&config.command, &context)?;

        // Render working_dir
        config.working_dir = match config.working_dir {
            Some(w) => Some(tera.render_str(&w, &context)?),
            None => None,
        };

        // Render env
        let mut rendered_env = HashMap::new();
        for (k, v) in config.env.iter() {
            rendered_env.insert(tera.render_str(k, &context)?, tera.render_str(v, &context)?);
        }
        config.env = rendered_env;

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
                            let mut rendered_env = HashMap::new();
                            for (k, v) in c.env.iter() {
                                rendered_env.insert(tera.render_str(k, &context)?, tera.render_str(v, &context)?);
                            }
                            c.env = rendered_env;

                            // Render env_files
                            let mut rendered_env_files = Vec::new();
                            for f in c.env_files.iter() {
                                rendered_env_files.push(tera.render_str(f, &context)?);
                            }
                            c.env_files = rendered_env_files;
                        }
                    }
                    st.healthcheck = Some(healthcheck);
                }
                config.service = Some(ServiceConfig::Struct(st));
            } else {
                config.service = Some(service);
            }
        }

        // Render depends_on[*].vars
        for depends_on in config.depends_on.iter_mut() {
            if let DependsOnConfig::Struct(ref mut dep) = depends_on {
                // Render vars
                let mut rendered_vars = HashMap::new();
                for (k, v) in dep.vars.iter() {
                    rendered_vars.insert(k.clone(), tera.render_str(&v, &context)?);
                }
                dep.vars = rendered_vars;
            }
        }

        debug!("Task {:?} context: {:?}", self.name, context);

        Ok(config)
    }

    pub fn is_maybe_same(&self, config: TaskConfig) -> bool {
        self.orig_name == config.orig_name && self.vars == config.vars
    }
}

impl ConfigRenderer {
    pub fn new(root_config: &ProjectConfig, child_config: &HashMap<String, ProjectConfig>) -> Self {
        Self {
            root_config: root_config.clone(),
            child_configs: child_config.clone(),
        }
    }

    fn base_context(&self) -> tera::Context {
        let mut context = tera::Context::new();
        let root_dir = self.root_config.dir.as_os_str().to_str().unwrap_or("");
        context.insert(ROOT_DIR_CONTEXT_KEY, root_dir);
        if self.child_configs.is_empty() {
            context.insert(PROJECT_DIR_CONTEXT_KEY, root_dir);
        } else {
            let project_dir = self
                .child_configs
                .iter()
                .map(|(k, v)| (k.as_str(), v.dir.as_os_str().to_str().unwrap_or("")))
                .collect::<HashMap<_, _>>();
            context.insert(PROJECT_DIR_CONTEXT_KEY, &project_dir);
        }
        context
    }

    pub fn render(&mut self) -> anyhow::Result<(ProjectConfig, HashMap<String, ProjectConfig>)> {
        let context = self.base_context();
        let mut task_contexts = HashMap::new();
        let mut tasks = Vec::new();
        let mut variant_vars_map = HashMap::new();

        // Root project task contexts
        let root_context = self.root_config.context(&context)?;
        let mut root_config = self.root_config.render(&root_context)?;
        for t in self.root_config.tasks.values() {
            tasks.push(t.full_name());
            task_contexts.insert(t.full_name(), t.context(&root_context)?);
        }
        root_config.tasks = HashMap::new();

        // Project task contexts
        let mut child_configs = HashMap::new();
        for (k, c) in self.child_configs.iter_mut() {
            let project_context = c.context(&root_context)?;
            child_configs.insert(k.clone(), c.render(&project_context)?);
            for t in c.tasks.values() {
                tasks.push(t.full_name());
                task_contexts.insert(t.full_name(), t.context(&project_context)?);
            }
            c.tasks = HashMap::new();
        }

        for t in tasks.iter() {
            Self::render_task_variants(
                t,
                &mut root_config,
                &mut child_configs,
                &mut self.root_config,
                &mut self.child_configs,
                &mut variant_vars_map,
                &mut task_contexts,
            )?;
        }

        Ok((root_config.clone(), child_configs.clone()))
    }

    fn set_task<'a>(
        task_config: TaskConfig,
        root_config: &'a mut ProjectConfig,
        child_configs: &'a mut HashMap<String, ProjectConfig>,
    ) {
        if task_config.project.is_empty() {
            root_config.tasks.insert(task_config.name.clone(), task_config);
        } else if let Some(c) = child_configs.get_mut(&task_config.name) {
            c.tasks.insert(task_config.name.clone(), task_config);
        }
    }

    fn get_mut(&mut self, task_name: &str) -> Option<&mut TaskConfig> {
        if let Some((p, t)) = task_name.split_once("#") {
            if p.is_empty() {
                return self.root_config.tasks.get_mut(t);
            }
            if let Some(c) = self.child_configs.get_mut(p) {
                return c.tasks.get_mut(t);
            }
        }
        None
    }

    fn get_task<'a>(
        task_name: &str,
        root_config: &'a mut ProjectConfig,
        child_configs: &'a mut HashMap<String, ProjectConfig>,
    ) -> Option<&'a mut TaskConfig> {
        if let Some((p, t)) = task_name.split_once("#") {
            if p.is_empty() {
                return root_config.tasks.get_mut(t);
            }
            if let Some(c) = child_configs.get_mut(p) {
                return c.tasks.get_mut(t);
            }
        }
        None
    }

    /// When a dependency task is specified with vars, it is considered as a different task.
    /// Task variants is managed internally with sequentially numbered suffixes, ex: {name}-1, {name}-2.
    fn render_task_variants(
        task_name: &str,
        root_config: &mut ProjectConfig,
        child_configs: &mut HashMap<String, ProjectConfig>,
        raw_root_config: &mut ProjectConfig,
        raw_child_configs: &mut HashMap<String, ProjectConfig>,
        num_variants: &mut HashMap<String, usize>,
        contexts: &mut HashMap<String, tera::Context>,
    ) -> anyhow::Result<()> {
        debug!("Variants for {:?} dependency tasks", task_name);

        let task_config = Self::get_task(task_name, raw_root_config, raw_child_configs)
            .context(format!("Unknown task {:?}", task_name))?;
        let context = contexts
            .get(task_name)
            .context(format!("Unknown task {:?}", task_name))?;
        info!("{}, context: {:?}", task_name, context);

        let mut task_config = task_config.render(context)?;

        for depends_on in task_config.depends_on.iter_mut() {
            // Struct notation
            let DependsOnConfig::Struct(depends_on) = depends_on else {
                continue;
            };
            // With vars
            if depends_on.vars.is_empty() {
                continue;
            };

            let dep_task = Self::get_task(&depends_on.task, raw_root_config, raw_child_configs)
                .context(format!("Unknown task {:?}", task_name))?;

            let mut variant_task = dep_task.clone();
            let suffix = num_variants
                .entry(dep_task.full_orig_name())
                .and_modify(|v| *v += 1)
                .or_insert(1);
            let variant_task_name = format!("{}-{}", dep_task.full_name(), suffix);

            let mut variant_dep_task_vars = dep_task.vars.clone();
            variant_dep_task_vars.extend(depends_on.vars.clone());

            variant_task.name = Task::simple_name(&variant_task_name);
            variant_task.vars = variant_dep_task_vars.clone();

            info!(
                "{} -> {} ({}) vars: {:?} + {:?} = {:?}",
                task_name,
                dep_task.full_name(),
                variant_task_name,
                dep_task.vars,
                depends_on.vars,
                variant_dep_task_vars
            );

            // Create context from dependency task
            let dep_context = contexts
                .get(&dep_task.full_name())
                .context(format!("Unknown task {:?}", dep_task.full_name()))?;
            let variant_context = variant_task.context(dep_context)?;

            info!(
                "{} -> {} ({}) context: {:?}",
                task_name,
                dep_task.full_name(),
                variant_task_name,
                variant_context
            );

            // Render
            let rendered_variant_task = variant_task.render(&variant_context)?;

            contexts.insert(variant_task_name.clone(), variant_context);

            // Add task variant config
            Self::set_task(variant_task, raw_root_config, raw_child_configs);
            Self::set_task(rendered_variant_task, root_config, child_configs);

            // Replace the dep task name to the suffixed one
            depends_on.task = variant_task_name.clone();

            Self::render_task_variants(
                &variant_task_name,
                root_config,
                child_configs,
                raw_root_config,
                raw_child_configs,
                num_variants,
                contexts,
            )?;
        }

        Self::set_task(task_config, root_config, child_configs);

        Ok(())
    }
}
