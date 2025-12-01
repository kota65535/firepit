use crate::project::Task;
use crate::tokio_spawn;
use anyhow::Context;
use futures::stream::FuturesUnordered;
use petgraph::algo::toposort;
use petgraph::dot::{Config, Dot};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{depth_first_search, Control, IntoNodeIdentifiers, Reversed};
use petgraph::Direction;
use std::cmp::max;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct TaskGraph {
    graph: DiGraph<Task, bool>,
    targets: Vec<String>,
}

pub struct VisitorMessage {
    pub node: Task,
    pub num_runs: u64,
    pub num_restart: u64,
    pub deps_ok: bool,
    pub callback: mpsc::Sender<CallbackMessage>,
}

#[derive(Debug, Clone)]
pub enum VisitorCommand {
    Stop,
    Restart { task: String, force: bool },
}

pub struct VisitorHandle {
    pub node_rx: mpsc::Receiver<VisitorMessage>,
    pub visitor_tx: broadcast::Sender<VisitorCommand>,
    pub future: FuturesUnordered<JoinHandle<anyhow::Result<()>>>,
}

#[derive(Debug, Clone)]
pub struct CallbackMessage(pub NodeResult);

#[derive(Debug, Clone)]
pub enum NodeResult {
    None,
    Success,
    Failure,
}

impl NodeResult {
    pub fn success(&self) -> bool {
        match self {
            NodeResult::Success => true,
            _ => false,
        }
    }
    pub fn present(&self) -> bool {
        match self {
            NodeResult::None => false,
            _ => true,
        }
    }
}

impl TaskGraph {
    pub fn new(tasks: &Vec<Task>, targets: Option<&Vec<String>>, force: bool) -> anyhow::Result<TaskGraph> {
        let mut graph = DiGraph::<Task, bool>::new();
        let mut nodes = HashMap::new();

        // If `force` is true, we only add the target tasks as nodes to the graph
        if force {
            let target_set = targets
                .map(|t| t.iter().cloned().collect::<HashSet<_>>())
                .unwrap_or_default();
            for t in tasks {
                if target_set.contains(&t.name) {
                    let idx = graph.add_node(t.clone());
                    nodes.insert(t.name.clone(), idx);
                }
            }
        } else {
            for t in tasks {
                let idx = graph.add_node(t.clone());
                nodes.insert(t.name.clone(), idx);
            }
        }

        // Add edges to the graph based on task dependencies
        for t in tasks {
            for d in &t.depends_on {
                let from = nodes.get(&t.name);
                let to = nodes.get(&d.task);
                match (from, to) {
                    (Some(from), Some(to)) => {
                        graph.add_edge(*from, *to, d.cascade);
                    }
                    // Ignore if the dependent task does not exist.
                    // This can occur when creating a subgraph by `transitive_closure`.
                    _ => {
                        warn!("Cannot find node for task {} and dependency {}", t.name, d.task);
                    }
                }
            }
        }

        // If targets are not given, consider all tasks as target
        let targets = targets
            .cloned()
            .unwrap_or_else(|| tasks.iter().map(|t| t.name.clone()).collect());

        let ret = TaskGraph { graph, targets };

        ret.sort()?;

        Ok(ret)
    }

    pub fn sort(&self) -> anyhow::Result<Vec<Task>> {
        match toposort(&self.graph, None) {
            Ok(ids) => Ok(ids
                .iter()
                .map(|&i| self.graph.node_weight(i).expect("should exist").clone())
                .collect::<Vec<_>>()),
            Err(err) => {
                let task = self.graph.node_weight(err.node_id()).expect("should exist");
                anyhow::bail!("cyclic dependency detected at task {:?}", task.name.clone())
            }
        }
    }

    pub fn visit(&self, concurrency: usize, quit_on_done: bool) -> anyhow::Result<VisitorHandle> {
        // Each node has a watch channel to send the result for all dependent nodes
        let mut txs = HashMap::new();
        let mut rxs = HashMap::new();
        for node_id in self.graph.node_identifiers() {
            let (tx, rx) = watch::channel::<NodeResult>(NodeResult::None);
            txs.insert(node_id, tx);
            rxs.insert(node_id, rx);
        }
        // Channel to notify nodes
        let (node_tx, node_rx) = mpsc::channel(max(concurrency, 1));
        // Channel to stop or restart visitor
        let (visitor_tx, visitor_rx) = broadcast::channel(1024);

        // Remaining target tasks
        let targets_remaining: HashSet<String> = self.targets.iter().map(|s| s.clone()).collect();
        let targets_remaining = Arc::new(Mutex::new(targets_remaining));

        // Run visitor thread for all nodes
        let nodes_fut = FuturesUnordered::new();
        for node_id in self.graph.node_identifiers() {
            let tx = txs.remove(&node_id).context("sender not found")?;
            let node_tx = node_tx.clone();
            let mut visitor_rx = visitor_rx.resubscribe();

            let task = self.graph.node_weight(node_id).context("node not found")?.clone();
            let neighbors = self.graph.neighbors_directed(node_id, Direction::Outgoing);
            // Dependency tasks
            let dep_tasks = neighbors
                .clone()
                .map(|n| self.graph.node_weight(n).cloned().context("node not found"))
                .collect::<anyhow::Result<Vec<_>>>()?;
            // Watch channels to receive the result of dependent tasks
            let dep_rxs = neighbors
                .clone()
                .map(|n| rxs.get(&n).cloned().context("sender not found"))
                .collect::<anyhow::Result<Vec<_>>>()?;

            let task_name = task.name.clone();
            let targets_remaining_cloned = targets_remaining.clone();
            let visitor_tx_cloned = visitor_tx.clone();
            nodes_fut.push(tokio_spawn!("node", { name = task_name }, async move {
                let mut ignore_deps = false;
                let mut num_runs = 0;
                let mut num_restart = 0;
                'start: loop {
                    if dep_tasks.is_empty() {
                        info!("No dependency")
                    } else {
                        info!(
                            "Waiting for {} deps: {:?}",
                            dep_tasks.len(),
                            dep_tasks.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
                        );
                    }

                    let deps_ok = if ignore_deps {
                        true
                    } else {
                        loop {
                            tokio::select! {
                                // Visitor command branch
                                Ok(command) = visitor_rx.recv() => {
                                    match command {
                                        VisitorCommand::Stop => {
                                            debug!("Visitor stopped");
                                            return Ok(())
                                        }
                                        VisitorCommand::Restart { task: task_name, force } => {
                                            debug!("Visitor restarted");
                                            if task.name == task_name {
                                                ignore_deps = force;
                                                num_runs += 1;
                                                tx.send(NodeResult::None).ok();
                                                continue 'start;
                                            }
                                            continue
                                        }
                                    };
                                }
                                // Normal branch, waiting for all dependency tasks
                                Ok(deps_ok) = Self::wait_all_watches(dep_rxs.clone()) => {
                                    break deps_ok;
                                }
                            }
                        }
                    };

                    info!("Dependencies finished. ok: {:?}", deps_ok);

                    // Loop for restarting service tasks
                    'send: loop {
                        let (callback_tx, mut callback_rx) = mpsc::channel::<CallbackMessage>(1);
                        let message = VisitorMessage {
                            node: task.clone(),
                            num_runs,
                            num_restart,
                            deps_ok,
                            callback: callback_tx.clone(),
                        };
                        match node_tx.send(message).await {
                            Ok(_) => {
                                // Loop for restarting service tasks
                                'recv: loop {
                                    tokio::select! {
                                        // Visitor command branch
                                        Ok(command) = visitor_rx.recv() => {
                                            match command {
                                                VisitorCommand::Stop => {
                                                    debug!("Visitor stopped");
                                                    return Ok(())
                                                }
                                                VisitorCommand::Restart { task: task_name, force } => {
                                                    debug!("Visitor restarted");
                                                    if task.name == task_name {
                                                        ignore_deps = force;
                                                        num_runs += 1;
                                                        tx.send(NodeResult::None).ok();
                                                        continue 'start;
                                                    }
                                                    continue 'recv
                                                }
                                            };
                                        }
                                        // Normal branch, waiting for the node result
                                        result = callback_rx.recv() => {
                                            match result {
                                                Some(CallbackMessage(result)) => {
                                                    match result {
                                                        NodeResult::Success | NodeResult::Failure => {
                                                            // Send errors indicate that there are no receivers that
                                                            // happen when this node has no dependents
                                                            tx.send(result.clone()).ok();

                                                            // Service task should continue recv loop so that it can restart
                                                            // even after reaching the READY state
                                                            if result.success() && task.is_service {
                                                                debug!("Result: {:?}, still waiting for callback", result);
                                                                continue 'recv;
                                                            }
                                                            // Finish the visitor
                                                            debug!("Result: {:?}", result);
                                                            break 'send;
                                                        }
                                                        NodeResult::None => {
                                                            // No result means we should restart the task
                                                            debug!("Result is empty, restarting");
                                                            num_restart += 1;
                                                            continue 'send;
                                                        }
                                                    }
                                                }
                                                _ => {
                                                    // If the caller drops the callback sender without signaling
                                                    // that the node processing is finished, we assume that it is finished.
                                                    warn!("Callback sender dropped");
                                                    tx.send(NodeResult::Failure).ok();
                                                    break 'send;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                // The receiving end of the node channel has been closed/dropped.
                                // We act as if we have been canceled.
                                warn!("Cannot send to the runner: {:?}", e);
                                tx.send(NodeResult::Failure).ok();
                                break 'send;
                            }
                        };
                    }

                    debug!("Visitor finished");
                    let targets_done = {
                        let mut t = targets_remaining_cloned.lock().expect("not poisoned");
                        t.remove(&task.name);
                        t.is_empty()
                    };
                    if quit_on_done && targets_done {
                        debug!("All target node done, stopping visitors");
                        visitor_tx_cloned.send(VisitorCommand::Stop).ok();
                    }

                    loop {
                        match visitor_rx.recv().await {
                            Ok(command) => match command {
                                VisitorCommand::Stop => {
                                    debug!("Visitor stopped");
                                    return Ok(());
                                }
                                VisitorCommand::Restart { task: task_name, force } => {
                                    if task.name == task_name {
                                        debug!("Visitor restarted");
                                        num_runs += 1;
                                        ignore_deps = force;
                                        tx.send(NodeResult::None).ok();
                                        continue 'start;
                                    }
                                }
                            },
                            Err(broadcast::error::RecvError::Closed) => {
                                debug!("Visitor command channel closed");
                                return Ok(());
                            }
                            Err(err) => {
                                warn!("Visitor command channel error: {:?}", err);
                            }
                        };
                    }
                }
            }));
        }

        Ok(VisitorHandle {
            node_rx,
            visitor_tx,
            future: nodes_fut,
        })
    }

    pub fn transitive_closure(&self, names: &Vec<String>, direction: Direction) -> anyhow::Result<TaskGraph> {
        let mut visited = Vec::<NodeIndex>::new();
        let mut visitor = |idx| {
            if let petgraph::visit::DfsEvent::Discover(n, _) = idx {
                visited.push(n);
            }
            Control::<()>::Continue
        };

        let indices = names
            .iter()
            .filter_map(|n| self.node_by_task(n))
            .map(|n| n.1)
            .collect::<Vec<_>>();

        match direction {
            Direction::Outgoing => {
                depth_first_search(&self.graph, indices, visitor);
            }
            Direction::Incoming => {
                depth_first_search(Reversed(&self.graph), indices, |event| {
                    if let petgraph::visit::DfsEvent::TreeEdge(u, v) = event {
                        if let Ok(edge_idx) = self.graph.find_edge(v, u).context("edge not found") {
                            if let Some(cascade) = self.graph.edge_weight(edge_idx) {
                                if !cascade {
                                    return Control::Prune;
                                }
                            }
                        }
                        return Control::Continue;
                    }
                    visitor(event)
                });
            }
        };

        let tasks = visited
            .iter()
            .map(|&i| self.graph.node_weight(i).unwrap().clone())
            .collect::<Vec<_>>();

        TaskGraph::new(&tasks, Some(names), false)
    }

    #[allow(dead_code)]
    pub fn tasks(&self) -> Vec<Task> {
        self.graph.node_weights().cloned().collect()
    }

    fn node_by_task(&self, name: &str) -> Option<(&Task, NodeIndex)> {
        for (i, n) in self.graph.node_weights().enumerate() {
            if n.name == *name {
                return Some((n, NodeIndex::new(i)));
            }
        }
        None
    }

    async fn wait_all_watches(mut receivers: Vec<watch::Receiver<NodeResult>>) -> anyhow::Result<bool> {
        for rx in receivers.iter_mut() {
            let v = rx.borrow().clone();
            if (*rx.borrow()).present() {
                if !(*rx.borrow()).success() {
                    return Ok(false);
                }
                continue;
            }
            loop {
                if rx.changed().await.is_err() {
                    anyhow::bail!("watch channel closed");
                }
                if (*rx.borrow()).present() {
                    if !(*rx.borrow()).success() {
                        return Ok(false);
                    }
                    break;
                }
            }
        }
        Ok(true)
    }
}

impl fmt::Debug for TaskGraph {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{:?}",
            Dot::with_attr_getters(
                &self.graph,
                &[Config::EdgeNoLabel, Config::NodeNoLabel],
                &|_, _| String::new(),
                &|_, r| format!("label = \"{}\" ", r.1.name.clone())
            )
        )
    }
}
