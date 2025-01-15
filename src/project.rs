use crate::config::ProjectConfig;
use crate::event::{TaskEventSender};
use crate::graph::TaskGraph;
use crate::process::{Command, ProcessManager};
use crate::signal::{get_signal, SignalHandler};
use anyhow::anyhow;
use futures::future::{join, join_all};
use log::info;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ProjectRunner {
    pub target_tasks: Vec<Task>,
    pub tasks: Vec<Task>,
    pub task_graph: TaskGraph,
    pub manager: ProcessManager,
    pub signal_handler: SignalHandler,
    pub concurrency: usize,
}

fn dir_contains(a: &PathBuf, b: &PathBuf) -> bool {
    while let Some(p) = b.parent() {
        if a == p {
            return true;
        }
    }
    false
}


impl ProjectRunner {
    pub fn new(root: &ProjectConfig,
               children: &HashMap<String, ProjectConfig>,
               target_tasks: &Vec<String>,
               dir: PathBuf,
    ) -> anyhow::Result<ProjectRunner> {
        let root_project = Project::new(&"".to_string(), root);
        let child_projects = children.iter()
            .map(|(k, v)| (k.clone(), Project::new(k, v)))
            .collect::<HashMap<_, _>>();

        let mut tasks = root_project.tasks.values().cloned().collect::<Vec<_>>();
        for p in child_projects.values() {
            tasks.extend(p.tasks.values().cloned().collect::<Vec<_>>());
        }

        let target_tasks = if root.dir == dir {
            target_tasks.iter()
                .flat_map(|t| child_projects.values()
                    .map(|p| p.task(t))
                    .flatten()
                    .collect::<Vec<_>>())
                .collect::<Vec<Task>>()
        } else if dir_contains(&root.dir, &dir) {
            target_tasks.iter()
                .flat_map(|t| child_projects.values()
                    .filter(|p| dir_contains(&dir, &p.dir))
                    .map(|p| p.task(t))
                    .flatten()
                    .collect::<Vec<_>>())
                .collect::<Vec<Task>>()
        } else {
            Vec::new()
        };

        let mut task_graph = TaskGraph::new(&tasks)?;
        task_graph = task_graph.transitive_closure(&target_tasks.iter()
            .map(|t| t.name.clone())
            .collect())?;
        tasks = task_graph.tasks();


        let manager = ProcessManager::infer();
        let signal = get_signal()?;
        let signal_handler = SignalHandler::new(signal);
        if let Some(subscriber) = signal_handler.subscribe() {
            let manager = manager.clone();
            tokio::spawn(async move {
                let _guard = subscriber.listen().await;
                manager.stop().await;
            });
        }

        Ok(ProjectRunner {
            tasks,
            target_tasks,
            task_graph,
            signal_handler,
            manager,
            concurrency: root_project.concurrency,
        })
    }

    pub async fn run(&mut self, event_rx: TaskEventSender) -> anyhow::Result<()> {
        // Run visitor
        let (mut task_receiver, visitor_future) = self.task_graph.visit(self.concurrency)?;

        let mut task_futures = Vec::new();

        while let Some((task, callback)) = task_receiver.recv().await {
            let this = self.clone();
            let event_sender = event_rx.clone_with_name(&task.name);
            task_futures.push(tokio::spawn(async move {
                let mut args = Vec::new();
                args.extend(task.shell_args.clone());
                args.push(task.command.clone());
                info!("Starting task {:?}:\nshell: {:?} {:?}\ncommand: {:?}\nenvs: {:?}", task.name, task.shell, &task.shell_args, task.command, task.envs);
                
                let cmd = Command::new(task.shell)
                    .args(args)
                    .envs(task.envs)
                    .current_dir(task.working_dir)
                    .to_owned();
                let mut process = match this.manager.spawn(cmd, Duration::from_millis(500)) {
                    Some(Ok(child)) => child,
                    Some(Err(e)) => {
                        return Err(anyhow!("unable to spawn task {:?}: {:?}", task.name, e));
                    }
                    _ => {
                        return Ok(())
                    }
                };
                event_sender.start(&task.name)?;
                let exit_status = match process.wait_with_piped_outputs(event_sender.clone()).await {
                    Ok(Some(exit_status)) => exit_status,
                    Err(e) => {
                        return Err(anyhow!(e.to_string()))
                    }
                    Ok(None) => {
                        return Err(anyhow!("unable to determine why child exited"))
                    }
                };
                // info!("Task {:?} finished. reason: {:?}", task.name, exit_status);
                event_sender.finish(&task.name, exit_status)?;
                
                callback.send(());
                Ok(())
            }));
        }

        let task_future = join_all(task_futures);

        join(visitor_future, task_future).await;
        Ok(())
    }
}


#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub tasks: HashMap<String, Task>,
    pub dir: PathBuf,
    pub concurrency: usize,
}

impl Project {
    pub fn new(name: &String, config: &ProjectConfig) -> Project {
        let config = config.clone();
        Project {
            name: name.to_owned(),
            tasks: Task::from_project_config(name, &config),
            dir: config.dir,
            concurrency: config.concurrency.expect("should be set"),
        }
    }

    pub fn task(&self, name: &String) -> Option<Task> {
        self.tasks.get(&Task::qualified_name(&self.name, name)).cloned()
    }
}

#[derive(Clone, Debug)]
pub struct Task {
    pub name: String,
    pub command: String,
    pub envs: HashMap<String, String>,
    pub depends_on: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub working_dir: PathBuf,
    pub is_service: bool,

    pub shell: String,
    pub shell_args: Vec<String>,
}


impl Task {
    pub fn from_project_config(project_name: &String, config: &ProjectConfig) -> HashMap<String, Task> {
        let mut ret = HashMap::new();
        for (task_name, task_config) in config.tasks.iter() {
            let task_config = task_config.clone();
            let task_name = Task::qualified_name(project_name, task_name);
            ret.insert(task_name.clone(), Task {
                name: task_name.clone(),
                command: task_config.command,
                envs: config.envs.clone().into_iter().chain(task_config.envs).collect(),
                depends_on: task_config.depends_on.iter()
                    .map(|s| Task::qualified_name(&project_name, s))
                    .collect(),
                inputs: task_config.inputs,
                outputs: task_config.outputs,
                is_service: task_config.service.is_some(),
                working_dir: task_config.working_dir.map(|t| {
                    if t.is_absolute() {
                        t
                    } else {
                        config.dir.join(t)
                    }
                }).unwrap_or(config.dir.clone()),
                shell: task_config.shell.clone().unwrap_or(config.shell.clone().expect("should be set")).command,
                shell_args: task_config.shell.clone().unwrap_or(config.shell.clone().expect("should be set")).args,
            });
        }
        ret
    }
    pub fn qualified_name(project_name: &str, task_name: &str) -> String {
        if task_name.contains("#") {
            task_name.to_string()
        } else {
            format!("{}#{}", project_name, task_name)
        }
    }
}
