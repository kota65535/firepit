use crate::project::{Project, Task, TaskRun, TaskStatus};
use anyhow::Context;
use futures::future::{join_all, JoinAll};
use futures::stream::FuturesUnordered;
use log::{debug, info, warn};
use petgraph::algo::toposort;
use petgraph::data::DataMap;
use petgraph::dot::{Config, Dot};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{depth_first_search, IntoNodeIdentifiers};
use petgraph::Direction;
use std::collections::HashMap;
use std::future::Future;
use std::ops::Index;
use tokio::sync::mpsc::error::RecvError;
use tokio::sync::mpsc::Sender;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub struct TaskGraph {
    pub graph: DiGraph<Task, ()>,
}

pub type WalkMessage<N> = (N, oneshot::Sender<()>);


impl TaskGraph {
    pub fn new(tasks: &Vec<Task>) -> anyhow::Result<TaskGraph> {
        let mut graph = DiGraph::<Task, ()>::new();
        let mut nodes = HashMap::new();
        let mut nodes_reversed = HashMap::new();

        // Add nodes
        for t in tasks {
            let idx = graph.add_node(t.clone());
            nodes.insert(t.name.clone(), idx);
            nodes_reversed.insert(idx, t.name.clone());
        }
        
        // Add edges
        for t in tasks {
            for d in &t.depends_on {
                let from = nodes.get(&t.name).with_context(|| format!("node {} should be found", &t.name))?;
                let to = nodes.get(d).with_context(|| format!("node {} should be found", d))?;
                graph.add_edge(*from, *to, ());
            }
        }

        // Ensure no cyclic dependency
        match toposort(&graph, None) {
            Ok(_) => {}
            Err(err) => {
                let task_name = nodes_reversed.get(&err.node_id()).unwrap();
                return Err(anyhow::anyhow!("Cyclic dependency detected at task {}", task_name))
            }
        }

        Ok(TaskGraph {
            graph,
        })
    }

    pub fn visit(&self) -> (mpsc::Receiver<WalkMessage<NodeIndex>>, FuturesUnordered<JoinHandle<()>>) {
        let mut txs = HashMap::new();
        let mut rxs = HashMap::new();
        for node in self.graph.node_identifiers() {
            // Each node can finish at most once so we set the capacity to 1
            let (tx, rx) = broadcast::channel::<()>(1);
            txs.insert(node, tx);
            rxs.insert(node, rx);
        }
        let (node_tx, node_rx) = mpsc::channel(std::cmp::max(txs.len(), 1));
        let join_handles = FuturesUnordered::new();
        for node in self.graph.node_identifiers() {
            let tx = txs.remove(&node).expect("should have sender for all nodes");
            let node_tx = node_tx.clone();
            let neighbors = self.graph.neighbors_directed(node, Direction::Outgoing);
            let task_name = self.graph.node_weight(node).unwrap().name.clone();
            let dep_names = neighbors.clone().map(|n| self.graph.node_weight(n).cloned().unwrap().name).collect::<Vec<_>>();
            let mut deps_rx = neighbors
                .map(|dep| {
                    rxs.get(&dep)
                        .expect("graph should have all nodes")
                        .resubscribe()
                })
                .collect::<Vec<_>>();

            join_handles.push(tokio::spawn(async move {
                let deps = deps_rx.iter_mut().map(|rx| rx.recv()).collect::<Vec<_>>();
                info!("Task {} is waiting for {} deps: {}", task_name, dep_names.len(), dep_names.join(", "));
                let deps_fut = join_all(deps);

                tokio::select! {
                    // If both the cancel and dependencies are ready, we want to
                    // execute the cancel instead of sending an additional node.
                    results = deps_fut => {
                        info!("Task {} is starting", task_name);
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
                                    info!("A node finished {x} more times than expected");
                                }
                            }
                        }

                        let (callback_tx, callback_rx) = oneshot::channel::<()>();
                        // do some err handling with the send failure?
                        if node_tx.send((node, callback_tx)).await.is_err() {
                            // Receiving end of node channel has been closed/dropped
                            // Since there's nothing the mark the node as being done
                            // we act as if we have been canceled.
                            info!("Receiver was dropped before walk finished without calling cancel");
                            return;
                        }
                        if callback_rx.await.is_err() {
                            // If the caller drops the callback sender without signaling
                            // that the node processing is finished we assume that it is finished.
                            info!("Callback sender was dropped without sending a finish signal")
                        }
                        info!("Task {} finished", task_name);
                        // Send errors indicate that there are no receivers which
                        // happens when this node has no dependents
                        tx.send(()).ok();
                    }
                }
            }));
        }

        (node_rx, join_handles)
    }

    pub fn dot_notation(&self) {
        format!("{:?}", Dot::with_config(&self.graph, &[Config::EdgeNoLabel]));
    }
    
    pub fn transitive_closure(&self, names: &Vec<String>) -> anyhow::Result<TaskGraph> {
        let mut visited = Vec::<NodeIndex>::new();
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

        // TODO: nicer error handling
        let tasks = visited
            .iter()
            .map(|&i| self.graph.node_weight(i).unwrap().clone())
            .collect::<Vec<_>>();
        
        TaskGraph::new(&tasks)
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
