use crate::event::{Event, EventReceiver, EventSender, TaskResult};
use crate::process::{ChildExit, Command, ProcessManager};
use anyhow::Context;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

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
}

impl LogLineProber {
    pub fn new(name: &str, regex: Regex) -> Self {
        Self {
            name: name.to_string(),
            regex,
        }
    }

    pub async fn probe(&mut self, tx: EventSender, mut rx: EventReceiver) {
        let this = self.clone();
        let tx = tx.clone();
        let mut matched = false;
        while let Some(event) = rx.recv().await {
            match event {
                Event::StartTask { task} => {
                    tx.start_task(task)
                }
                Event::TaskOutput { task, output } => {
                    tx.output(task.clone(), output.clone()).ok();
                    let line = String::from_utf8(output).unwrap_or_default();
                    if !matched && this.regex.is_match(&line) {
                        tx.ready_task(task.clone());
                        matched = true
                    }
                }
                Event::ReadyTask { task} => {
                    tx.ready_task(task)
                }
                Event::EndTask { task, result} => {
                    tx.end_task(task, result);
                    break
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
    initial_delay_seconds: u64,
    period_seconds: u64,
    timeout_seconds: u64,
    success_threshold: u64,
    failure_threshold: u64,
    manager: ProcessManager,
}

impl ExecProber {
    pub fn new(
        name: &str,
        command: &str,
        working_dir: PathBuf,
        shell: String,
        shell_args: Vec<String>,
        envs: HashMap<String, String>,
        initial_delay_seconds: u64,
        period_seconds: u64,
        timeout_seconds: u64,
        success_threshold: u64,
        failure_threshold: u64,
    ) -> Self {
        Self {
            name: name.to_string(),
            command: command.to_string(),
            working_dir,
            shell,
            shell_args,
            envs,
            initial_delay_seconds,
            period_seconds,
            timeout_seconds,
            success_threshold,
            failure_threshold,
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

        let timeout_fut = tokio::time::timeout(Duration::from_secs(self.timeout_seconds), async {});
        let process_fut = process.wait();

        tokio::select! {
            _ = timeout_fut => {
                Ok(false)
            }
            result = process_fut => {
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

    pub async fn probe(&mut self, tx: EventSender) {
        let mut success = 0;
        let mut failure = 0;
        loop {
            match self.exec().await {
                Ok(result) => {
                    if result {
                        success += 1;
                        failure = 0;
                    } else {
                        success = 0;
                        failure += 1
                    }
                }
                _ => {
                    failure += 1;
                }
            }
            if success >= self.success_threshold {
                tx.ready_task(self.name.clone());
                break;
            }
            if failure >= self.failure_threshold {
                tx.end_task(self.name.clone(), TaskResult::Failure(1));
                break;
            }
        }
    }
}
