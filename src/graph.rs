use crate::project::{Project, Task, TaskRun, TaskStatus};
use petgraph::algo::toposort;
use petgraph::dot::{Config, Dot};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{depth_first_search, IntoNodeIdentifiers};
use std::collections::HashMap;
use std::future::Future;
use std::ops::Index;
use futures::future::{join_all, JoinAll};
use futures::stream::FuturesUnordered;
use petgraph::data::DataMap;
use petgraph::Direction;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::sync::mpsc::error::RecvError;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use tracing::{info, trace};

#[derive(Debug)]
pub struct TaskGraph {
    pub graph: DiGraph<Task, ()>,
    // pub sender: Option<Sender<WalkMessage<NodeIndex>>>
}

pub type WalkMessage<N> = (N, oneshot::Sender<()>);

pub struct Visitor {
    
}

impl TaskGraph {
    pub fn new(root_project: &Project, child_projects: &HashMap<String, Project>) -> anyhow::Result<TaskGraph> {
        let mut graph = DiGraph::<Task, ()>::new();
        let mut nodes = HashMap::new();
        let mut nodes_rev = HashMap::new();

        for (name, task) in &root_project.tasks {
            let idx = graph.add_node(task.clone());
            nodes.insert(name.clone(), idx);
            nodes_rev.insert(idx, name.clone());
        }
        for (_, project) in child_projects {
            for (name, task) in &project.tasks {
                let idx = graph.add_node(task.clone());
                nodes.insert(name.clone(), idx);
                nodes_rev.insert(idx, name.clone());
            }
        }

        for (name, task) in &root_project.tasks {
            for d in &task.depends_on {
                graph.add_edge(*nodes.get(name).unwrap(), *nodes.get(d).unwrap(), ());
            }
        }
        for (_, project) in child_projects {
            for (name, task) in &project.tasks {
                for dep in &task.depends_on {
                    graph.add_edge(*nodes.get(name).unwrap(), *nodes.get(dep).unwrap(), ());
                }
            }
        }

        match toposort(&graph, None) {
            Ok(_) => {}
            Err(err) => {
                let task_name = nodes_rev.get(&err.node_id()).unwrap();
                return Err(anyhow::anyhow!("Cyclic dependency detected at task {}", task_name))
            }
        }

        println!("{:?}", Dot::with_config(&graph, &[Config::EdgeNoLabel]));

        Ok(TaskGraph {
            graph,
            // sender: None,
        })
    }

    pub fn visit(&mut self, tasks: &Vec<String>) -> (mpsc::Receiver<WalkMessage<NodeIndex>>, FuturesUnordered<JoinHandle<()>>) {
        let (cancel, cancel_rx) = watch::channel(false);
        let mut txs = HashMap::new();
        let mut rxs = HashMap::new();
        for node in self.graph.node_identifiers() {
            // Each node can finish at most once so we set the capacity to 1
            let (tx, rx) = broadcast::channel::<()>(1);
            txs.insert(node, tx);
            rxs.insert(node, rx);
        }
        let (node_tx, node_rx) = mpsc::channel(std::cmp::max(txs.len(), 1));
        // self.sender = Some(node_tx.clone());
        let join_handles = FuturesUnordered::new();
        for node in self.graph.node_identifiers() {
            let tx = txs.remove(&node).expect("should have sender for all nodes");
            let mut cancel_rx = cancel_rx.clone();
            let node_tx = node_tx.clone();
            let neighbors = self.graph.neighbors_directed(node, Direction::Outgoing);
            let mut deps_rx = self.graph
                .neighbors_directed(node, Direction::Outgoing)
                .map(|dep| {
                    rxs.get(&dep)
                        .expect("graph should have all nodes")
                        .resubscribe()
                })
                .collect::<Vec<_>>();

            join_handles.push(tokio::spawn(async move {
                let deps = deps_rx.iter_mut().map(|rx| rx.recv()).collect::<Vec<_>>();
                println!("Waiting for {:?} deps of task {:?} finish", deps.len(), node);
                let deps_fut = join_all(deps);

                tokio::select! {
                    // If both the cancel and dependencies are ready, we want to
                    // execute the cancel instead of sending an additional node.
                    results = deps_fut => {
                        println!("Task {:?} can start", node);
                        for res in results {
                            match res {
                                // No errors from reading dependency channels
                                Ok(()) => (),
                                // A dependency finished without sending a finish
                                // Could happen if a cancel is sent and is racing with deps
                                // so we interpret this as a cancel.
                                Err(broadcast::error::RecvError::Closed) => {
                                    return;
                                }
                                // A dependency sent a finish signal more than once
                                // which shouldn't be possible.
                                // Since the message is always the unit type we can proceed
                                // but we log as this is unexpected behavior.
                                Err(broadcast::error::RecvError::Lagged(x)) => {
                                    debug_assert!(false, "A node finished {x} more times than expected");
                                    trace!("A node finished {x} more times than expected");
                                }
                            }
                        }

                        let (callback_tx, callback_rx) = oneshot::channel::<()>();
                        // do some err handling with the send failure?
                        if node_tx.send((node, callback_tx)).await.is_err() {
                            // Receiving end of node channel has been closed/dropped
                            // Since there's nothing the mark the node as being done
                            // we act as if we have been canceled.
                            trace!("Receiver was dropped before walk finished without calling cancel");
                            return;
                        }
                        if callback_rx.await.is_err() {
                            // If the caller drops the callback sender without signaling
                            // that the node processing is finished we assume that it is finished.
                            trace!("Callback sender was dropped without sending a finish signal")
                        }
                        // Send errors indicate that there are no receivers which
                        // happens when this node has no dependents
                        tx.send(()).ok();
                    }
                }
            }));
        }

        (node_rx, join_handles)
    }

    pub fn print(&self) {
        println!("{:?}", Dot::with_config(&self.graph, &[Config::EdgeNoLabel]));
    }

    pub fn finish_task(&mut self, name: &String) {
        if let Some((_, idx)) = self.find_node(name) {
            self.graph.remove_node(idx);
        }
    }

    fn transitive_closure(&self, names: &Vec<String>) -> Vec<NodeIndex> {
        let mut visited = Vec::new();
        let visitor = |idx| {
            if let petgraph::visit::DfsEvent::Discover(n, _) = idx {
                visited.push(n)
            }
        };

        let indices = names.iter()
            .filter_map(|n| self.find_node(n))
            .map(|n| n.1)
            .collect::<Vec<_>>();
        depth_first_search(&self.graph, indices, visitor);
        visited
    }

    fn find_node(&self, name: &String) -> Option<(&Task, NodeIndex)> {
        for (i, n) in  self.graph.node_weights().enumerate() {
            if n.name == *name {
                return Some((n, NodeIndex::new(i)))
            }
        }
        None
    }
    
    fn find_node_mut(&mut self, name: &String) -> Option<(&mut Task, NodeIndex)> {
        for (i, n) in  self.graph.node_weights_mut().enumerate() {
            if n.name == *name {
                return Some((n, NodeIndex::new(i)))
            }
        }
        None
    }
}
