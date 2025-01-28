use crate::runner::Task;
use anyhow::Context;
use futures::future::join_all;
use futures::stream::FuturesUnordered;
use log::{error, info, warn};
use petgraph::algo::toposort;
use petgraph::dot::{Config, Dot};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{depth_first_search, IntoNodeIdentifiers};
use petgraph::Direction;
use std::cmp::max;
use std::collections::HashMap;
use std::fmt;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct TaskGraph {
    graph: DiGraph<Task, ()>,
}

pub struct VisitorMessage {
    pub node: Task,
    pub deps_ok: bool,
    pub callback: mpsc::Sender<CallbackMessage>,
}

pub struct Visitor {
    pub node_rx: mpsc::Receiver<VisitorMessage>,
    pub cancel: watch::Sender<bool>,
    pub future: FuturesUnordered<JoinHandle<anyhow::Result<()>>>,
}

#[derive(Debug, Clone)]
pub struct CallbackMessage(pub bool);

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
                let to = nodes.get(d).with_context(|| format!("node {} should be found", d))?;
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

    pub fn visit(&self, concurrency: usize) -> anyhow::Result<Visitor> {
        // Channel for each node to notify all dependent nodes when it finishes
        let mut txs = HashMap::new();
        let mut rxs = HashMap::new();
        for node_id in self.graph.node_identifiers() {
            // Each node can finish at most once so we set the capacity to 1
            let (tx, rx) = broadcast::channel::<bool>(1);
            txs.insert(node_id, tx);
            rxs.insert(node_id, rx);
        }
        // Channel to notify when all dependency nodes have finished
        let (node_tx, node_rx) = mpsc::channel(max(concurrency, 1));
        // Channel to stop visitor
        let (cancel_tx, cancel_rx) = watch::channel(false);

        let nodes_fut = FuturesUnordered::new();
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
                .map(|n| self.graph.node_weight(n).with_context(|| "node not found").cloned())
                .collect::<anyhow::Result<Vec<_>>>()?;
            let mut dep_rxs = neighbors
                .clone()
                .map(|n| {
                    rxs.get(&n)
                        .map(|rx| rx.resubscribe())
                        .with_context(|| "sender not found")
                })
                .collect::<anyhow::Result<Vec<_>>>()?;

            nodes_fut.push(tokio::spawn(async move {
                if dep_tasks.is_empty() {
                    info!("Node {:?} has no dependency", task.name)
                } else {
                    info!(
                        "Node {:?} is waiting for {} deps: {:?}",
                        task.name,
                        dep_tasks.len(),
                        dep_tasks.iter().map(|t| t.name.clone()).collect::<Vec<String>>()
                    );
                }

                let deps = dep_rxs.iter_mut().map(|rx| rx.recv()).collect::<Vec<_>>();
                let deps_fut = join_all(deps);

                let deps_ok = tokio::select! {
                     _ = cancel_rx.changed() => {
                        info!("Visitor is cancelled");
                        false
                     }
                    results = deps_fut => {
                        info!("Node {:?} dependencies have finished", task.name);
                        results.iter().map(|r| match r {
                                Ok(r) => *r,
                                Err(e) => {
                                    error!("Node {:?} cannot receive the result: {:?}", task.name, e);
                                    false
                                }
                        }).all(|r| r)
                    }
                };

                let (callback_tx, mut callback_rx) = mpsc::channel::<CallbackMessage>(1);
                let result = match node_tx
                    .send(VisitorMessage {
                        node: task.clone(),
                        deps_ok,
                        callback: callback_tx.clone(),
                    })
                    .await
                {
                    Ok(_) => {
                        match callback_rx.recv().await {
                            Some(result) => result,
                            _ => {
                                // If the caller drops the callback sender without signaling
                                // that the node processing is finished we assume that it is finished.
                                warn!(
                                    "Node {:?} callback sender was dropped without sending a finish signal",
                                    task.name
                                );
                                CallbackMessage(false)
                            }
                        }
                    }
                    Err(e) => {
                        // Receiving end of node channel has been closed/dropped
                        // Since there's nothing the mark the node as being done
                        // we act as if we have been canceled.
                        warn!("Node {:?} cannot send to the runner: {:?}", task.name, e);
                        CallbackMessage(false)
                    }
                };
                info!("Node {:?} finished. result: {:?}", task.name, result);
                // Send errors indicate that there are no receivers which
                // happens when this node has no dependents
                tx.send(result.0).ok();
                Ok(())
            }));
        }
        Ok(Visitor {
            node_rx,
            cancel: cancel_tx,
            future: nodes_fut,
        })
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
