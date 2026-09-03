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
    graph: DiGraph<Task, Edge>,
    targets: Vec<String>,
}

/// Edge from a task to a task it waits for.
#[derive(Debug, Clone, Copy)]
struct Edge {
    /// Whether the dependent task re-runs when this dependency re-runs, in watch mode.
    cascade: bool,

    /// Whether this edge only orders the two tasks, without making one depend on the other.
    /// Such an edge comes from `wait_for`: it neither pulls the task into the run nor
    /// blocks the dependent task when it fails.
    ordering_only: bool,
}

impl Edge {
    fn depends_on(cascade: bool) -> Self {
        Edge {
            cascade,
            ordering_only: false,
        }
    }

    fn wait_for() -> Self {
        Edge {
            cascade: false,
            ordering_only: true,
        }
    }
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
        matches!(self, NodeResult::Success)
    }
    pub fn present(&self) -> bool {
        !matches!(self, NodeResult::None)
    }
}

impl TaskGraph {
    pub fn new(tasks: &Vec<Task>, targets: Option<&Vec<String>>, force: bool) -> anyhow::Result<TaskGraph> {
        let mut graph = DiGraph::<Task, Edge>::new();
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
                        graph.add_edge(*from, *to, Edge::depends_on(d.cascade));
                    }
                    // Ignore if the dependent task does not exist.
                    // This can occur when creating a subgraph by `transitive_closure`.
                    _ => {
                        warn!("Cannot find node for task {} and dependency {}", t.name, d.task);
                    }
                }
            }
        }

        // Index the nodes by the name their task was given in the config, which every variant a
        // parameterized dependency split it into shares. A `wait_for` entry names a task rather
        // than a node, so this is what it looks up.
        let mut nodes_by_orig_name = HashMap::<&str, Vec<(&Task, NodeIndex)>>::new();
        for t in tasks {
            if let Some(idx) = nodes.get(&t.name) {
                nodes_by_orig_name.entry(&t.orig_name).or_default().push((t, *idx));
            }
        }

        // Add ordering-only edges from `wait_for`. Unlike `depends_on`, a task that is not a node
        // is not an error here: `wait_for` only orders against tasks that are already in the run.
        // An entry orders against every variant of the task it names, narrowed down by the vars
        // the entry gives.
        for t in tasks {
            let Some(from) = nodes.get(&t.name) else {
                continue;
            };
            for w in &t.wait_for {
                let candidates = nodes_by_orig_name.get(w.task.as_str()).into_iter().flatten();
                for (_, to) in candidates.filter(|(t, _)| w.matches(t)) {
                    // Exclude the task itself, which it matches when it is one of the variants
                    // named by its own `wait_for`
                    if to == from {
                        continue;
                    }
                    // A dependency edge already orders the two tasks and is stricter, so keep it.
                    // This also keeps the graph free of parallel edges, so an edge can be looked
                    // up by its endpoints alone.
                    if graph.find_edge(*from, *to).is_some() {
                        continue;
                    }
                    graph.add_edge(*from, *to, Edge::wait_for());
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
        let targets_remaining: HashSet<String> = self.targets.iter().cloned().collect();
        let targets_remaining = Arc::new(Mutex::new(targets_remaining));

        // Run visitor thread for all nodes
        let nodes_fut = FuturesUnordered::new();
        for node_id in self.graph.node_identifiers() {
            let tx = txs.remove(&node_id).context("sender not found")?;
            let node_tx = node_tx.clone();
            let mut visitor_rx = visitor_rx.resubscribe();

            let task = self.graph.node_weight(node_id).context("node not found")?.clone();
            // Tasks this node waits for, and the watch channels to receive their results.
            // Ordering-only tasks, from `wait_for`, are only awaited: unlike a dependency task,
            // their failure does not stop this node from running.
            let mut dep_tasks = Vec::new();
            let mut dep_rxs = Vec::new();
            let mut order_rxs = Vec::new();
            for n in self.graph.neighbors_directed(node_id, Direction::Outgoing) {
                let edge = self
                    .graph
                    .find_edge(node_id, n)
                    .and_then(|e| self.graph.edge_weight(e))
                    .context("edge not found")?;
                dep_tasks.push(self.graph.node_weight(n).cloned().context("node not found")?);
                let rx = rxs.get(&n).cloned().context("sender not found")?;
                if edge.ordering_only {
                    order_rxs.push(rx);
                } else {
                    dep_rxs.push(rx);
                }
            }

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
                                Ok(deps_ok) = Self::wait_all_watches(dep_rxs.clone(), order_rxs.clone()) => {
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
                depth_first_search(&self.graph, indices, |event| {
                    // An ordering-only edge does not pull its task into the run, so do not
                    // follow it. The task still gets visited if a dependency edge reaches it,
                    // or if it is a target itself.
                    if let petgraph::visit::DfsEvent::TreeEdge(u, v) = event {
                        if self.edge(u, v).map(|e| e.ordering_only).unwrap_or(false) {
                            return Control::Prune;
                        }
                        return Control::Continue;
                    }
                    visitor(event)
                });
            }
            Direction::Incoming => {
                depth_first_search(Reversed(&self.graph), indices, |event| {
                    if let petgraph::visit::DfsEvent::TreeEdge(u, v) = event {
                        // The graph is reversed here, so the edge to look up runs from v to u.
                        // Re-running a task does not re-run the ones merely ordered after it,
                        // just as it does not re-run those that opted out of cascading.
                        if let Some(edge) = self.edge(v, u) {
                            if edge.ordering_only || !edge.cascade {
                                return Control::Prune;
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

    fn edge(&self, from: NodeIndex, to: NodeIndex) -> Option<Edge> {
        self.graph
            .find_edge(from, to)
            .and_then(|e| self.graph.edge_weight(e))
            .copied()
    }

    fn node_by_task(&self, name: &str) -> Option<(&Task, NodeIndex)> {
        for (i, n) in self.graph.node_weights().enumerate() {
            if n.name == *name {
                return Some((n, NodeIndex::new(i)));
            }
        }
        None
    }

    /// Waits until every dependency and ordering-only task has finished.
    ///
    /// Returns whether all dependency tasks succeeded. An ordering-only task, from `wait_for`,
    /// is awaited just the same, but its result is ignored: it orders the tasks without making
    /// this one depend on it.
    async fn wait_all_watches(
        dep_receivers: Vec<watch::Receiver<NodeResult>>,
        order_receivers: Vec<watch::Receiver<NodeResult>>,
    ) -> anyhow::Result<bool> {
        for (rx, required) in dep_receivers
            .into_iter()
            .map(|rx| (rx, true))
            .chain(order_receivers.into_iter().map(|rx| (rx, false)))
        {
            let mut rx = rx;
            if !(*rx.borrow()).present() {
                loop {
                    if rx.changed().await.is_err() {
                        anyhow::bail!("watch channel closed");
                    }
                    if (*rx.borrow()).present() {
                        break;
                    }
                }
            }
            // A failed dependency makes this node skip its run, so there is nothing left to
            // order against and no reason to wait for the remaining tasks
            if required && !(*rx.borrow()).success() {
                return Ok(false);
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
