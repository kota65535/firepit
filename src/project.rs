use crate::config::ProjectConfig;
use crate::graph::TaskGraph;
use crate::process::{Command, ProcessManager};
use crate::ui::output::{OutputClient, OutputSink};
use anyhow::{anyhow, Context};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::env;
use futures::task::SpawnExt;
use itertools::Itertools;
use tokio::sync::{mpsc, oneshot};
use crate::run::output::StdWriter;

#[derive(Debug, Clone)]
pub struct ProjectRunner {
    pub root_project: Project,
    pub child_projects: HashMap<String, Project>,
    pub target_tasks: Vec<Task>,
    pub tasks: HashMap<String, Task>,
    pub task_graph: Arc<Mutex<TaskGraph>>,
    pub manager: ProcessManager,
    pub cwd: PathBuf,
    pub shell: String,
    pub num_workers: usize,
}

pub struct Message<T, U> {
    pub info: T,
    pub callback: oneshot::Sender<U>,
}
impl<T, U> Message<T, U> {
    pub fn new(info: T) -> (Self, oneshot::Receiver<U>) {
        let (callback, receiver) = oneshot::channel();
        (Self { info, callback }, receiver)
    }
}

impl ProjectRunner {
    
    pub fn new(root: &ProjectConfig, children: &HashMap<String, ProjectConfig>, task_names: &Vec<String>) -> anyhow::Result<ProjectRunner> {
        let root_project_name = "".to_string();
        let root_project = Project::new(&root_project_name, root);
        let child_projects = children.iter()
            .filter(|(k, _)| **k != root_project_name)
            .map(|(k, v)| (k.clone(), Project::new(k, v)))
            .collect::<HashMap<_, _>>();

        let mut tasks = root_project.tasks.clone();
        for (_, proj) in &child_projects {
            tasks.extend(proj.tasks.clone());
        }

        let mut task_graph = TaskGraph::new(&tasks.values().cloned().collect::<Vec<Task>>())?;
        if tasks.len() > 0 {
            task_graph = task_graph.transitive_closure(&task_names)?;
            tasks = task_graph.graph.node_weights().cloned().map(|w| (w.name.clone(), w)).collect();
        }
        
        let target_tasks = task_names
            .iter()
            .map(|t| tasks.get(t).cloned().with_context(|| format!("Task '{}' not found", t)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        
        let shell = root_project.shell.clone();
        
        Ok(ProjectRunner {
            root_project,
            child_projects,
            tasks,
            target_tasks,
            task_graph: Arc::new(Mutex::new(task_graph)),
            manager: ProcessManager::infer(),
            cwd: env::current_dir()?,
            shell,
            num_workers: 2,
        })
    }

    pub async fn visit(&mut self, visitor: mpsc::Sender<Message<String, ()>>, task_names: &Vec<String>) -> anyhow::Result<()> {
        let mut tasks: FuturesUnordered<tokio::task::JoinHandle<anyhow::Result<()>>> = FuturesUnordered::new();

        let (mut nodes, handle) =  self.task_graph
            .lock().expect("lock poisoned")
            .visit();
        loop {
            if let Some((node_id, callback)) = nodes.recv().await {
                let visitor = visitor.clone();
                let graph = self.task_graph.lock().expect("lock poisoned");
                let t = graph.graph.node_weight(node_id)
                    .expect("node id should be present");

                let (message, result) = Message::new(t.name.clone());
                tasks.push(tokio::spawn(async move {
                    visitor.send(message).await?;
                    result.await?;
                    callback.send(()).unwrap();
                    Ok(())
                }));
            } else {
                println!("No more messages!");
                break
            }
        }

        while let Some(res) = tasks.next().await {
            res.expect("unable to join task")?;
        }

        Ok(())

    }


    pub async fn run(&mut self, task_names: &Vec<String>) -> anyhow::Result<()> {
        let (node_sender, mut node_stream) = mpsc::channel(self.root_project.concurrency);
        
        let task_names = task_names.clone();

        let mut this = self.clone();
        let engine_handle = tokio::spawn(async move {
            this.visit(node_sender, &task_names).await
        });

        let mut tasks: FuturesUnordered<tokio::task::JoinHandle<Result<(), anyhow::Error>>> = FuturesUnordered::new();

        while let Some(message) = node_stream.recv().await {
            let manager = self.manager.clone();
            let this = self.clone();
            tasks.push(tokio::spawn(async move {
                let task = this.tasks.get(&message.info).expect("must be there");
                let cmd = Command::new("bash".to_string())
                    .args(["-c", task.command.as_str()])
                    .to_owned();
                let mut process = match manager.spawn(cmd, Duration::from_millis(500)) {
                    Some(Ok(child)) => child,
                    Some(Err(e)) => {
                        return Ok(())
                    },
                    _ => {
                        return Ok(())
                    }
                };
                let sink = sink();
                let mut logger = sink.logger(crate::ui::output::OutputClientBehavior::Passthrough);
                // let out = PrefixedWriter::new(
                //     ColorConfig::new(false),
                //     Style::new().bold().apply_to("output ".to_string()),
                //     logger
                // );
                let exit_status = match process.wait_with_piped_outputs(logger.stdout()).await {
                    Ok(Some(exit_status)) => exit_status,
                    Err(e) => {
                        return Err(anyhow!(e.to_string()))
                    }
                    Ok(None) => {
                        return Err(anyhow!("unable to determine why child exited"))
                    }
                };
                
                message.callback.send(());
                Ok(())
            }));
        }

        engine_handle.await.expect("engine execution panicked")?;
        let mut internal_errors = Vec::new();
        while let Some(result) = tasks.next().await {
            if let Err(e) = result.unwrap_or_else(|e| panic!("task executor panicked: {e}")) {
                internal_errors.push(anyhow!("{}", e));
            }
        }

        Ok(())
    }
}


#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub tasks: HashMap<String, Task>,
    pub dir: PathBuf,
    pub concurrency: usize,
    pub shell: String
}

impl Project {
    pub fn new(name: &String, config: &ProjectConfig) -> Project {
        let config = config.clone();
        Project {
            name: name.to_owned(),
            tasks: Task::from_project_config(name, &config),
            dir: config.dir,
            concurrency: config.concurrency.unwrap_or(1),
            shell: config.interpreter.unwrap(),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum TaskStatus {
    UNKNOWN,
    WAITING,
    RUNNING,
    FINISHED,
}

// #[derive(Clone, Debug)]
// pub enum ServiceStatus {
//     UNKNOWN,
//     WAITING,
//     STARTING,
//     READY,
//     STOPPING,
//     STOPPED,
// }

#[derive(Clone, Debug)]
pub struct TaskRun {
    pub task: Task,
    pub status: TaskStatus,
}

#[derive(Clone, Debug)]
pub struct Task {
    pub name: String,
    pub command: String,
    pub depends_on: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub working_dir: PathBuf,
    pub is_service: bool
}

fn sink() -> OutputSink<StdWriter> {
    let (out, err) = (std::io::stdout().into(), std::io::stdout().into());
    OutputSink::new(out, err)
}


impl Task {

    pub fn from_project_config(project_name: &String, config: &ProjectConfig) -> HashMap<String, Task> {
        let mut ret = HashMap::new();
        if let Some(tasks) = &config.tasks {
            for (task_name, task_config) in tasks.iter() {
                let task_config = task_config.clone();
                let task_name = Task::qualified_name(project_name, task_name);
                ret.insert(task_name.clone(), Task {
                    name: task_name.clone(),
                    command: task_config.command,
                    depends_on: task_config.depends_on.unwrap_or_default().iter()
                        .map(|s| Task::qualified_name(&project_name, s))
                        .collect(),
                    inputs: task_config.inputs.unwrap_or_default(),
                    outputs: task_config.outputs.unwrap_or_default(),
                    is_service: task_config.service.is_some(),
                    working_dir: task_config.working_dir.unwrap_or(config.clone().dir)
                });
            }
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
