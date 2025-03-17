use crate::project::Task;
use crate::tokio_spawn;
use anyhow::Context;
use futures::future::join_all;
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
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

#[derive(Clone)]
pub struct TaskGraph {
    graph: DiGraph<Task, bool>,
    targets: Vec<String>,
}

pub struct VisitorMessage {
    pub node: Task,
    pub count: u64,
    pub deps_ok: bool,
    pub callback: mpsc::Sender<CallbackMessage>,
}

pub struct VisitorHandle {
    pub node_rx: mpsc::Receiver<VisitorMessage>,
    pub cancel: broadcast::Sender<()>,
    pub future: FuturesUnordered<JoinHandle<anyhow::Result<()>>>,
}

/// Callback message takes following values:
/// Some(true): success
/// Some(false): failure
/// None: should restart
#[derive(Debug, Clone)]
pub struct CallbackMessage(pub Option<bool>);

impl TaskGraph {
    pub fn new(tasks: &Vec<Task>, targets: Option<&Vec<String>>) -> anyhow::Result<TaskGraph> {
        let mut graph = DiGraph::<Task, bool>::new();
        let mut nodes = HashMap::new();

        for t in tasks {
            let idx = graph.add_node(t.clone());
            nodes.insert(t.name.clone(), idx);
        }

        // It is user's responsibility to ensure that the dependency task exists.
        // If the specified dependency task does not exist, it is simply ignored
        for t in tasks {
            for d in &t.depends_on {
                let from = nodes.get(&t.name);
                let to = nodes.get(&d.task);
                match (from, to) {
                    (Some(from), Some(to)) => {
                        graph.add_edge(*from, *to, d.cascade);
                    }
                    _ => {
                        warn!("Cannot find node for task {} and dependency {}", t.name, d.task);
                    }
                }
            }
        }

        // If targets are not given, consider all tasks as target.
        let targets = targets
            .map(|t| t.clone())
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

    pub fn visit(&self, concurrency: usize, no_quit: bool) -> anyhow::Result<VisitorHandle> {
        debug!("Visitor started");
        // Each node has a broadcast channel to notify all dependent nodes when it finishes
        let mut txs = HashMap::new();
        let mut rxs = HashMap::new();
        for node_id in self.graph.node_identifiers() {
            // Each node can finish at most once so we set the capacity to 1
            let (tx, rx) = broadcast::channel::<bool>(1);
            txs.insert(node_id, tx);
            rxs.insert(node_id, rx);
        }
        // Channel to notify when all its dependency nodes have finished
        let (node_tx, node_rx) = mpsc::channel(max(concurrency, 1));
        // Channel to stop visitor
        let (cancel_tx, cancel_rx) = broadcast::channel(1);

        // Remaining target tasks
        let targets_remaining: HashSet<String> = self.targets.iter().map(|s| s.clone()).collect();
        let targets_remaining = Arc::new(Mutex::new(targets_remaining));

        // Run visitor thread for all nodes
        let nodes_fut = FuturesUnordered::new();
        for node_id in self.graph.node_identifiers() {
            let tx = txs.remove(&node_id).context("sender not found")?;
            let node_tx = node_tx.clone();
            let mut cancel_rx = cancel_rx.resubscribe();

            let task = self.graph.node_weight(node_id).context("node not found")?.clone();
            let neighbors = self.graph.neighbors_directed(node_id, Direction::Outgoing);
            let dep_tasks = neighbors
                .clone()
                .map(|n| self.graph.node_weight(n).context("node not found").cloned())
                .collect::<anyhow::Result<Vec<_>>>()?;
            let mut dep_rxs = neighbors
                .clone()
                .map(|n| rxs.get(&n).map(|rx| rx.resubscribe()).context("sender not found"))
                .collect::<anyhow::Result<Vec<_>>>()?;

            let task_name = task.name.clone();
            let targets_remaining_cloned = targets_remaining.clone();
            let cancel_tx_cloned = cancel_tx.clone();
            nodes_fut.push(tokio_spawn!("node", { name = task_name }, async move {
                if dep_tasks.is_empty() {
                    info!("No dependency")
                } else {
                    info!(
                        "Waiting for {} deps: {:?}",
                        dep_tasks.len(),
                        dep_tasks.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
                    );
                }

                let deps = dep_rxs.iter_mut().map(|rx| rx.recv()).collect::<Vec<_>>();
                let deps_fut = join_all(deps);

                let deps_ok = tokio::select! {
                    // Cancelling branch, quits immediately
                     _ = cancel_rx.recv() => {
                        info!("Visitor cancelled");
                        return Ok(())
                     }
                    // Normal branch, waiting for all dependency tasks
                    results = deps_fut => {
                        info!("Dependencies finished");
                        results.iter().map(|r| match r {
                                Ok(r) => *r,
                                Err(e) => {
                                    error!("Cannot receive the result: {:?}", e);
                                    false
                                }
                        }).all(|r| r)
                    }
                };

                let mut count = 0;
                // Loop for restarting service tasks
                'send: loop {
                    let (callback_tx, mut callback_rx) = mpsc::channel::<CallbackMessage>(1);
                    let message = VisitorMessage {
                        node: task.clone(),
                        count,
                        deps_ok,
                        callback: callback_tx.clone(),
                    };
                    match node_tx.send(message).await {
                        Ok(_) => {
                            // Loop for restarting service tasks
                            'recv: loop {
                                tokio::select! {
                                    // Cancelling branch, quits immediately
                                    _ = cancel_rx.recv() => {
                                        info!("Visitor cancelled");
                                        return Ok(())
                                    }
                                    // Normal branch, waiting for the node result
                                    result = callback_rx.recv() => {
                                        match result {
                                            Some(CallbackMessage(result)) => {
                                                match result {
                                                    Some(success) => {
                                                        // Send errors indicate that there are no receivers which
                                                        // happens when this node has no dependents
                                                        if let Err(e) = tx.send(success) {
                                                            debug!("Cannot send the result to the graph: {:?}", e);
                                                        };
                                                        // Service task should continue recv loop so that it can restart
                                                        // even after reaching ready state
                                                        if success && task.is_service {
                                                            info!("Result: {:?}, still waiting callback", result);
                                                            continue 'recv;
                                                        }
                                                        // Finish the visitor
                                                        info!("Result: {:?}", result);
                                                        break 'send;
                                                    }
                                                    None => {
                                                        // No result means we should restart the task
                                                        info!("Result is empty, resending");
                                                        count += 1;
                                                        continue 'send;
                                                    }
                                                }
                                            }
                                            _ => {
                                                // If the caller drops the callback sender without signaling
                                                // that the node processing is finished we assume that it is finished.
                                                warn!("Callback sender dropped");
                                                tx.send(false).ok();
                                                break 'send;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            // Receiving end of node channel has been closed/dropped
                            // Since there's nothing the mark the node as being done
                            // we act as if we have been canceled.
                            warn!("Cannot send to the runner: {:?}", e);
                            tx.send(false).ok();
                            break 'send;
                        }
                    };
                }

                info!("Visitor finished");
                let targets_done = {
                    let mut t = targets_remaining_cloned.lock().expect("not poisoned");
                    t.remove(&task.name);
                    t.is_empty()
                };
                if !no_quit && targets_done {
                    info!("All target node done, cancelling visitors");
                    cancel_tx_cloned.send(()).ok();
                }
                Ok(())
            }));
        }

        Ok(VisitorHandle {
            node_rx,
            cancel: cancel_tx,
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

        TaskGraph::new(&tasks, Some(names))
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
