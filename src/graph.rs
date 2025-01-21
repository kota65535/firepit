use crate::event::TaskResult;
use crate::runner::Task;
use anyhow::Context;
use futures::future::{join_all, JoinAll};
use log::{error, info, warn};
use petgraph::algo::toposort;
use petgraph::dot::{Config, Dot};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{depth_first_search, IntoNodeIdentifiers};
use petgraph::Direction;
use std::cmp::max;
use std::collections::HashMap;
use std::fmt;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct TaskGraph {
    graph: DiGraph<Task, ()>,
}

pub type TaskVisitorMessage = (Task, bool, oneshot::Sender<TaskResult>);

impl TaskGraph {
    pub fn new(tasks: &Vec<Task>) -> anyhow::Result<TaskGraph> {
        let mut graph = DiGraph::<Task, ()>::new();
        let mut nodes = HashMap::new();

        for t in tasks {
            let idx = graph.add_node(t.clone());
            nodes.insert(t.name.clone(), idx);
        }

        for t in tasks {
            for d in &t.depends_on {
                let from = nodes
                    .get(&t.name)
                    .with_context(|| format!("node {} should be found", &t.name))?;
                let to = nodes
                    .get(d)
                    .with_context(|| format!("node {} should be found", d))?;
                graph.add_edge(*from, *to, ());
            }
        }

        let ret = TaskGraph { graph };

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
                Err(anyhow::anyhow!(
                    "cyclic dependency detected at task {}",
                    task.name.clone()
                ))
            }
        }
    }

    pub fn visit(
        &self,
        concurrency: usize,
    ) -> anyhow::Result<(
        mpsc::Receiver<TaskVisitorMessage>,
        watch::Sender<bool>,
        JoinAll<JoinHandle<()>>,
    )> {
        // Channel for each node to notify all dependent nodes when it finishes
        let mut txs = HashMap::new();
        let mut rxs = HashMap::new();
        for node in self.graph.node_identifiers() {
            // Each node can finish at most once so we set the capacity to 1
            let (tx, rx) = broadcast::channel::<TaskResult>(1);
            txs.insert(node, tx);
            rxs.insert(node, rx);
        }
        // Channel to notify when all dependency nodes have finished
        let (node_tx, node_rx) = mpsc::channel(max(concurrency, 1));
        // Channel to stop visiting
        let (cancel_tx, cancel_rx) = watch::channel(false);

        let mut node_futures = Vec::new();
        for node_id in self.graph.node_identifiers() {
            let tx = txs.remove(&node_id).with_context(|| "sender not found")?;
            let node_tx = node_tx.clone();
            let mut cancel_rx = cancel_rx.clone();

            let task = self
                .graph
                .node_weight(node_id)
                .with_context(|| "node not found")?
                .clone();

            let neighbors = self.graph.neighbors_directed(node_id, Direction::Outgoing);

            let dep_tasks = neighbors
                .clone()
                .map(|n| {
                    self.graph
                        .node_weight(n)
                        .with_context(|| "node not found")
                        .cloned()
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let mut dep_rxs = neighbors
                .clone()
                .map(|n| {
                    txs.get(&n)
                        .map(|tx| tx.subscribe())
                        .with_context(|| "sender not found")
                })
                .collect::<anyhow::Result<Vec<_>>>()?;

            node_futures.push(tokio::spawn(async move {
                if !dep_tasks.is_empty() {
                    info!(
                        "Task \"{}\" is waiting for {} deps: {:?}",
                        task.name,
                        dep_tasks.len(),
                        dep_tasks
                            .iter()
                            .map(|t| t.name.clone())
                            .collect::<Vec<String>>()
                    );
                }

                let deps = dep_rxs.iter_mut().map(|rx| rx.recv()).collect::<Vec<_>>();
                let deps_fut = join_all(deps);

                let mut deps_ok = true;
                tokio::select! {
                     _ = cancel_rx.changed() => {
                        info!("Visitor is cancelled");
                        deps_ok = false
                     }
                    // If both the cancel and dependencies are ready, we want to
                    // execute the cancel instead of sending an additional node.
                    results = deps_fut => {
                        info!("Deps of task {:?} are resolved", task.name);
                        // info!("Task {:?} is starting. cwd={:?}", task.name, task.working_dir);
                        for res in results {
                            match res {
                                // No errors from reading dependency channels
                                Ok(result) => {
                                    match result {
                                        TaskResult::Success => {}
                                        _ => { deps_ok = false }
                                    }
                                }
                                Err(e) => {
                                    error!("Cannot receive task result: {:?}", e);
                                    deps_ok = false
                                }
                            }
                        }
                    }
                }

                let (callback_tx, callback_rx) = oneshot::channel::<TaskResult>();
                let result = match node_tx.send((task.clone(), deps_ok, callback_tx)).await {
                    Ok(_) => {
                        match callback_rx.await {
                            Ok(result) => result,
                            _ => {
                                // If the caller drops the callback sender without signaling
                                // that the node processing is finished we assume that it is finished.
                                info!(
                                    "Callback sender was dropped without sending a finish signal"
                                );
                                TaskResult::Skipped
                            }
                        }
                    }
                    Err(e) => {
                        // Receiving end of node channel has been closed/dropped
                        // Since there's nothing the mark the node as being done
                        // we act as if we have been canceled.
                        warn!("Cannot send task to the runner: {:?}", e);
                        TaskResult::Unknown
                    }
                };
                info!("Task {:?} finished. result: {:?}", task.name, result);
                // Send errors indicate that there are no receivers which
                // happens when this node has no dependents
                tx.send(result).ok();
            }));
        }

        let join_all = join_all(node_futures);

        Ok((node_rx, cancel_tx, join_all))
    }

    pub fn transitive_closure(&self, names: &Vec<String>) -> anyhow::Result<TaskGraph> {
        let mut visited = Vec::<NodeIndex>::new();
        let visitor = |idx| {
            if let petgraph::visit::DfsEvent::Discover(n, _) = idx {
                visited.push(n)
            }
        };

        let indices = names
            .iter()
            .filter_map(|n| self.node_by_task(n))
            .map(|n| n.1)
            .collect::<Vec<_>>();
        depth_first_search(&self.graph, indices, visitor);

        // TODO: nicer error handling
        let tasks = visited
            .iter()
            .map(|&i| self.graph.node_weight(i).unwrap().clone())
            .collect::<Vec<_>>();

        TaskGraph::new(&tasks)
    }

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
                &|e, r| String::new(),
                &|n, r| format!("label = \"{}\" ", r.1.name.clone())
            )
        )
    }
}
