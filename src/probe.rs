use crate::event::EventSender;
use crate::process::{Child, ChildExit, Command, ProcessManager};
use anyhow::Context;
use log::{debug, warn};
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

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
        &self,
        mut log_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        mut cancel: watch::Receiver<()>,
    ) -> anyhow::Result<bool> {
        debug!("Task {:?} prober started (LogLineProber)", self.name);
        tokio::time::sleep(Duration::from_secs(self.start_period)).await;

        loop {
            tokio::select! {
                _ = cancel.changed() => {
                    debug!("Task {:?} prober cancelled", self.name);
                    return Ok(false);
                },
                _ = tokio::time::sleep(Duration::from_secs(self.timeout)) => {
                    debug!("Task {:?} prober timeout", self.name);
                    return Ok(false);
                },
                event = log_rx.recv() => {
                    if let Some(event) = event {
                        let line = String::from_utf8(event).unwrap_or_default();
                        if self.regex.is_match(&line) {
                            debug!("Task {:?} prober succeeded", self.name);
                            return Ok(true);
                        }
                    }
                }
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
        shell: &str,
        shell_args: Vec<String>,
        working_dir: PathBuf,
        envs: HashMap<String, String>,
        interval: u64,
        timeout: u64,
        retries: u64,
        start_period: u64,
    ) -> Self {
        Self {
            name: String::from(name),
            command: String::from(command),
            working_dir,
            shell: String::from(shell),
            shell_args,
            envs,
            interval,
            timeout,
            retries,
            start_period,
            manager: ProcessManager::new(false),
        }
    }

    pub async fn probe(&self, mut cancel: watch::Receiver<()>) -> anyhow::Result<bool> {
        debug!("Task {:?} prober started (ExecProber)", self.name);
        tokio::time::sleep(Duration::from_secs(self.start_period)).await;

        let mut retries = 0;
        loop {
            debug!("Task {:?} prober try ({}/{})", self.name, retries, self.retries);

            let mut process = match self.exec() {
                Ok(p) => p,
                Err(e) => {
                    warn!("Task {:?} prober failed to exec: {:?}", self.name, e);
                    return Ok(false);
                }
            };

            tokio::select! {
                _ = cancel.changed() => {
                    debug!("Task {:?} prober cancelled", self.name);
                    return Ok(false);
                },
                _ = tokio::time::sleep(Duration::from_secs(self.timeout)) => {
                    debug!("Task {:?} prober timeout", self.name);
                },
                exit = process.wait() => {
                    let success = match exit {
                        Some(exit_status) => match exit_status {
                            ChildExit::Finished(Some(code)) if code == 0 => true,
                            _ => false
                        },
                        None => false
                    };
                    if success {
                        debug!("Task {:?} prober succeeded", self.name);
                        return Ok(true);
                    }
                }
            }

            if retries >= self.retries {
                debug!("Task {:?} prober failed", self.name);
                return Ok(false);
            }
            retries += 1;

            tokio::time::sleep(Duration::from_secs(self.interval)).await;
        }
    }

    fn exec(&self) -> anyhow::Result<Child> {
        let mut args = Vec::new();
        args.extend(self.shell_args.clone());
        args.push(self.command.clone());
        let cmd = Command::new(self.shell.clone())
            .args(args)
            .envs(self.envs.clone())
            .current_dir(self.working_dir.clone())
            .to_owned();

        match self.manager.spawn(cmd, Duration::from_millis(500)) {
            Some(Ok(child)) => Ok(child),
            Some(Err(e)) => Err(e).with_context(|| "unable to spawn task"),
            _ => Err(anyhow::anyhow!("unable to spawn task")),
        }
    }
}

#[cfg(test)]
#[allow(unused)]
mod test {
    use super::*;
    use log::LevelFilter;
    use std::sync::Once;

    static INIT: Once = Once::new();

    pub fn setup() {
        INIT.call_once(|| {
            env_logger::builder()
                .filter_level(LevelFilter::Debug)
                .is_test(true)
                .init()
        });
    }

    #[tokio::test]
    async fn test_log_line_prober_succeeds() {
        setup();
        let prober = LogLineProber::new("test", Regex::new("test").unwrap(), 1, 0);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(());

        log_tx.send(String::from("aaaa").into_bytes()).ok();
        log_tx.send(String::from("testtest").into_bytes()).ok();

        // log line matches and succeed
        let result = prober.probe(log_rx, cancel_rx).await;

        assert_eq!(result.unwrap(), true);
    }

    #[tokio::test]
    async fn test_log_line_prober_timeout() {
        setup();
        let prober = LogLineProber::new("test", Regex::new("test").unwrap(), 1, 0);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(());

        log_tx.send(String::from("aaaa").into_bytes()).ok();

        let result = prober.probe(log_rx, cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_log_line_prober_canceled() {
        setup();
        let prober = LogLineProber::new("test", Regex::new("test").unwrap(), 1, 0);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(());

        log_tx.send(String::from("aaaa").into_bytes()).ok();
        cancel_tx.send(());

        let result = prober.probe(log_rx, cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_exec_prober_succeeds() {
        setup();
        let prober = ExecProber::new(
            "test",
            "pwd",
            "bash",
            vec![String::from("-c")],
            PathBuf::from("./"),
            HashMap::new(),
            1,
            1,
            1,
            0,
        );
        let (cancel_tx, cancel_rx) = watch::channel(());

        let result = prober.probe(cancel_rx).await;

        assert_eq!(result.unwrap(), true);
    }

    #[tokio::test]
    async fn test_exec_prober_fails() {
        setup();
        let mut builder = env_logger::Builder::new();
        builder.filter_level(LevelFilter::Debug).init();
        let prober = ExecProber::new(
            "test",
            "exit 1",
            "bash",
            vec![String::from("-c")],
            PathBuf::from("./"),
            HashMap::new(),
            1,
            1,
            1,
            0,
        );
        let (cancel_tx, cancel_rx) = watch::channel(());

        let result = prober.probe(cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_exec_prober_timeout() {
        setup();
        let prober = ExecProber::new(
            "test",
            "sleep 10",
            "bash",
            vec![String::from("-c")],
            PathBuf::from("./"),
            HashMap::new(),
            1,
            1,
            1,
            0,
        );
        let (cancel_tx, cancel_rx) = watch::channel(());

        let result = prober.probe(cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_exec_prober_canceled() {
        setup();
        let prober = ExecProber::new(
            "test",
            "pwd",
            "bash",
            vec![String::from("-c")],
            PathBuf::from("./"),
            HashMap::new(),
            1,
            1,
            1,
            0,
        );
        let (cancel_tx, cancel_rx) = watch::channel(());

        cancel_tx.send(());
        let result = prober.probe(cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }
}
