use crate::event::{Event, EventReceiver, EventSender, TaskResult};
use crate::graph::{CallbackMessage, NodeStatus};
use crate::process::{ChildExit, Command, ProcessManager};
use anyhow::Context;
use log::info;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum Prober {
    LogLine(LogLineProber),
    Exec(ExecProber),
    None(NullProber),
}

#[derive(Debug, Clone)]
pub struct LogLineProber {
    name: String,
    regex: Regex,
    timeout: u64,
    start_period: u64,
}

impl LogLineProber {
    pub fn new(name: &str, regex: Regex, timeout: u64, start_period: u64) -> Self {
        Self {
            name: name.to_string(),
            regex,
            timeout,
            start_period,
        }
    }

    pub async fn probe(
        &mut self,
        tx: EventSender,
        mut rx: EventReceiver,
        callback: mpsc::Sender<CallbackMessage>,
    ) -> anyhow::Result<()> {
        tokio::time::sleep(Duration::from_secs(self.start_period)).await;
        let this = self.clone();
        let tx = tx.clone();
        let mut matched = false;
        while let Some(event) = rx.recv().await {
            match event {
                Event::StartTask { task, pid, restart } => tx.start_task(task, pid, restart),
                Event::TaskOutput { task, output } => {
                    tx.output(task.clone(), output.clone());
                    let line = String::from_utf8(output).unwrap_or_default();
                    if !matched && this.regex.is_match(&line) {
                        tx.ready_task(task.clone());
                        callback.send(CallbackMessage(NodeStatus::Ready)).await?;
                        matched = true
                    }
                }
                Event::SetStdin { task, stdin } => tx.set_stdin(task, stdin),
                Event::ReadyTask { task } => tx.ready_task(task),
                Event::FinishTask { task, result } => {
                    tx.finish_task(task, result);
                    break;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ExecProber {
    name: String,
    command: String,
    working_dir: PathBuf,
    shell: String,
    shell_args: Vec<String>,
    envs: HashMap<String, String>,
    interval: u64,
    timeout: u64,
    retries: u64,
    start_period: u64,
    manager: ProcessManager,
}

impl ExecProber {
    pub fn new(
        name: &str,
        command: &str,
        shell: String,
        shell_args: Vec<String>,
        working_dir: PathBuf,
        envs: HashMap<String, String>,
        interval: u64,
        timeout: u64,
        retries: u64,
        start_period: u64,
    ) -> Self {
        Self {
            name: name.to_string(),
            command: command.to_string(),
            working_dir,
            shell,
            shell_args,
            envs,
            interval,
            timeout,
            retries,
            start_period,
            manager: ProcessManager::new(false),
        }
    }

    async fn exec(&self) -> anyhow::Result<bool> {
        let mut args = Vec::new();
        args.extend(self.shell_args.clone());
        args.push(self.command.clone());
        let cmd = Command::new(self.shell.clone())
            .args(args)
            .envs(self.envs.clone())
            .current_dir(self.working_dir.clone())
            .to_owned();

        let mut process = match self.manager.spawn(cmd, Duration::from_millis(500)) {
            Some(Ok(child)) => Ok(child),
            Some(Err(e)) => Err(e).with_context(|| "unable to spawn task"),
            _ => Err(anyhow::anyhow!("unable to spawn task")),
        }?;

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(self.timeout)) => {
                Ok(false)
            }
            result = process.wait() => {
                match result {
                    Some(exit_status) => match exit_status {
                        ChildExit::Finished(Some(code)) if code == 0 => Ok(true),
                        _ => Ok(false)
                    },
                    None => Err(anyhow::anyhow!("unable to determine why child exited")),
                }
            }
        }
    }

    pub async fn probe(
        &mut self,
        tx: EventSender,
        mut rx: EventReceiver,
        callback: mpsc::Sender<CallbackMessage>,
    ) -> anyhow::Result<()> {
        tokio::time::sleep(Duration::from_secs(self.start_period)).await;

        let tx1 = tx.clone();
        let interceptor_fut = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    Event::StartTask { task, pid, restart } => tx1.start_task(task, pid, restart),
                    Event::TaskOutput { task, output } => tx1.output(task.clone(), output.clone()),
                    Event::SetStdin { task, stdin } => tx1.set_stdin(task, stdin),
                    Event::ReadyTask { task } => tx1.ready_task(task),
                    Event::FinishTask { task, result } => {
                        tx1.finish_task(task, result);
                        break;
                    }
                    _ => {}
                }
            }
        });

        let this = self.clone();
        let tx2 = tx.clone();
        let probe_fut = tokio::spawn(async move {
            let mut retries = 0;
            loop {
                let success = match this.exec().await {
                    Ok(result) => result,
                    _ => false,
                };
                if success {
                    tx2.ready_task(this.name.clone());
                    callback.send(CallbackMessage(NodeStatus::Ready)).await.ok();
                    break;
                }
                if retries >= this.retries {
                    tx2.finish_task(this.name.clone(), TaskResult::Failure(1));
                    break;
                }

                tokio::time::sleep(Duration::from_secs(this.interval)).await;
                retries += 1;
            }
        });

        tokio::select! {
            _ = interceptor_fut => {}
            _ = probe_fut => {}
        }

        info!("Task {:?} prober finished", self.name);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct NullProber {
    name: String,
}

impl NullProber {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string() }
    }

    pub async fn probe(&self, tx: EventSender) -> anyhow::Result<()> {
        tx.ready_task(self.name.clone());
        Ok(())
    }
}
