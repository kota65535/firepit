use crate::event::{Event, EventReceiver, EventSender, TaskResult, TaskStatus};
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
    None,
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
        callback: mpsc::Sender<TaskStatus>,
    ) {
        tokio::time::sleep(Duration::from_secs(self.start_period)).await;
        let this = self.clone();
        let tx = tx.clone();
        let mut matched = false;
        while let Some(event) = rx.recv().await {
            match event {
                Event::StartTask { task } => tx.start_task(task),
                Event::TaskOutput { task, output } => {
                    tx.output(task.clone(), output.clone()).ok();
                    let line = String::from_utf8(output).unwrap_or_default();
                    if !matched && this.regex.is_match(&line) {
                        tx.ready_task(task.clone());
                        callback.send(TaskStatus::Ready).await.ok();
                        matched = true
                    }
                }
                Event::SetStdin { task, stdin } => {
                    tx.set_stdin(task, stdin);
                }
                Event::ReadyTask { task } => {
                    tx.ready_task(task);
                }
                Event::EndTask { task, result } => {
                    tx.end_task(task, result);
                    break;
                }
                _ => {}
            }
        }
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

    pub async fn probe(&mut self, tx: EventSender, callback: mpsc::Sender<TaskStatus>) {
        tokio::time::sleep(Duration::from_secs(self.start_period)).await;

        info!("Task {:?} prober started", self.name);
        let mut retries = 0;
        loop {
            info!("Run prober");
            let success = match self.exec().await {
                Ok(result) => result,
                _ => false,
            };
            if success {
                tx.ready_task(self.name.clone());
                callback.send(TaskStatus::Ready).await.ok();
                break;
            }
            if retries >= self.retries {
                tx.end_task(self.name.clone(), TaskResult::Failure(1));
                break;
            }

            tokio::time::sleep(Duration::from_secs(self.interval)).await;
            retries += 1;
        }
        info!("Task {:?} prober finished", self.name);
    }
}
