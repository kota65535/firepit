use crate::process::{Child, ChildExit, Command, ProcessManager};
use anyhow::Context;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub enum Probe {
    LogLine(LogLineProbe),
    Exec(ExecProbe),
    None,
}

#[derive(Debug, Clone)]
pub struct LogLineProbe {
    name: String,
    regex: Regex,
    timeout: u64,
    start_period: u64,
}

impl LogLineProbe {
    pub fn new(name: &str, regex: Regex, timeout: u64, start_period: u64) -> Self {
        Self {
            name: name.to_string(),
            regex,
            timeout,
            start_period,
        }
    }

    pub async fn run(
        &self,
        mut log_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        mut cancel: watch::Receiver<()>,
    ) -> anyhow::Result<bool> {
        debug!("Prober started (LogLineProber)");
        tokio::time::sleep(Duration::from_secs(self.start_period)).await;

        loop {
            tokio::select! {
                _ = cancel.changed() => {
                    debug!("Prober cancelled");
                    return Ok(false);
                },
                _ = tokio::time::sleep(Duration::from_secs(self.timeout)) => {
                    debug!("Prober timeout");
                    return Ok(false);
                },
                event = log_rx.recv() => {
                    if let Some(event) = event {
                        let line = String::from_utf8(event).unwrap_or_default();
                        if self.regex.is_match(&line) {
                            debug!("Prober succeeded");
                            return Ok(true);
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecProbe {
    name: String,
    command: String,
    working_dir: PathBuf,
    shell: String,
    shell_args: Vec<String>,
    env: HashMap<String, String>,
    interval: u64,
    timeout: u64,
    retries: u64,
    start_period: u64,
    manager: ProcessManager,
}

impl ExecProbe {
    pub fn new(
        name: &str,
        command: &str,
        shell: &str,
        shell_args: Vec<String>,
        working_dir: PathBuf,
        env: HashMap<String, String>,
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
            env,
            interval,
            timeout,
            retries,
            start_period,
            manager: ProcessManager::new(false),
        }
    }

    pub async fn run(&self, mut cancel: watch::Receiver<()>) -> anyhow::Result<bool> {
        debug!("Prober started (ExecProber)");
        tokio::time::sleep(Duration::from_secs(self.start_period)).await;

        let mut retries = 0;
        loop {
            debug!("Prober try ({}/{})", retries, self.retries);

            let mut process = match self.exec() {
                Ok(p) => p,
                Err(e) => {
                    warn!("Prober failed to exec: {:?}", e);
                    return Ok(false);
                }
            };

            tokio::select! {
                _ = cancel.changed() => {
                    debug!("Prober cancelled");
                    return Ok(false);
                },
                _ = tokio::time::sleep(Duration::from_secs(self.timeout)) => {
                    debug!("Prober timeout");
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
                        debug!("Prober succeeded");
                        return Ok(true);
                    }
                }
            }

            if retries >= self.retries {
                debug!("Prober failed");
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
            .with_args(args)
            .with_envs(self.env.clone())
            .with_current_dir(self.working_dir.clone())
            .with_label(&format!("{} probe", self.name))
            .to_owned();

        match self.manager.spawn(cmd, Duration::from_millis(500)) {
            Some(Ok(child)) => Ok(child),
            Some(Err(e)) => Err(e).with_context(|| "unable to spawn task"),
            _ => anyhow::bail!("unable to spawn task"),
        }
    }
}

#[cfg(test)]
#[allow(unused)]
mod test {
    use super::*;
    use std::sync::Once;

    static INIT: Once = Once::new();

    pub fn setup() {
        INIT.call_once(|| {
            tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).init();
        });
    }

    #[tokio::test]
    async fn test_log_line_probe_succeeds() {
        setup();
        let probe = LogLineProbe::new("test", Regex::new("test").unwrap(), 1, 0);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(());

        log_tx.send(String::from("aaaa").into_bytes()).ok();
        log_tx.send(String::from("testtest").into_bytes()).ok();

        // log line matches and succeed
        let result = probe.run(log_rx, cancel_rx).await;

        assert_eq!(result.unwrap(), true);
    }

    #[tokio::test]
    async fn test_log_line_probe_timeout() {
        setup();
        let probe = LogLineProbe::new("test", Regex::new("test").unwrap(), 1, 0);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(());

        log_tx.send(String::from("aaaa").into_bytes()).ok();

        let result = probe.run(log_rx, cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_log_line_probe_canceled() {
        setup();
        let probe = LogLineProbe::new("test", Regex::new("test").unwrap(), 1, 0);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(());

        log_tx.send(String::from("aaaa").into_bytes()).ok();
        cancel_tx.send(());

        let result = probe.run(log_rx, cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_exec_probe_succeeds() {
        setup();
        let probe = ExecProbe::new(
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

        let result = probe.run(cancel_rx).await;

        assert_eq!(result.unwrap(), true);
    }

    #[tokio::test]
    async fn test_exec_probe_fails() {
        setup();
        let probe = ExecProbe::new(
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

        let result = probe.run(cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_exec_probe_timeout() {
        setup();
        let probe = ExecProbe::new(
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

        let result = probe.run(cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_exec_probe_canceled() {
        setup();
        let probe = ExecProbe::new(
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
        let result = probe.run(cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }
}
