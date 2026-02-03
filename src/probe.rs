use crate::log::OutputCollector;
use crate::process::{Child, ChildExit, Command, ProcessManager};
use crate::project::Env;
use crate::PROBE_STOP_TIMEOUT;
use anyhow::Context;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

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
}

impl LogLineProbe {
    pub fn new(name: &str, regex: Regex, timeout: u64) -> Self {
        Self {
            name: name.to_string(),
            regex,
            timeout,
        }
    }

    pub async fn run(
        &self,
        mut log_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        mut cancel_rx: watch::Receiver<()>,
    ) -> anyhow::Result<bool> {
        debug!(
            "Probe started (LogLineProbe).\nregex: {:?}\ntimeout: {:?}",
            self.regex, self.timeout
        );

        loop {
            tokio::select! {
                // Cancelling branch, quits immediately
                _ = cancel_rx.changed() => {
                    debug!("Probe cancelled");
                    return Ok(false);
                },
                // Timeout branch
                _ = tokio::time::sleep(Duration::from_secs(self.timeout)) => {
                    debug!("Probe timed-out");
                    return Ok(false);
                },
                // Normal branch, tries to match the pattern with the log event
                event = log_rx.recv() => {
                    if let Some(event) = event {
                        let line = String::from_utf8(event).unwrap_or_default();
                        if self.regex.is_match(&line) {
                            debug!("Probe succeeded");
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
    env: Env,
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
        env: Env,
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

    pub async fn run(&self, mut cancel_rx: watch::Receiver<()>) -> anyhow::Result<bool> {
        let env = self.env.load()?;
        info!("Probe started (ExecProbe). command: {:?}", self.command);
        let start = Instant::now();

        // Wait `interval` seconds before the first health check
        tokio::time::sleep(Duration::from_secs(self.interval)).await;

        let mut retries = 0;
        loop {
            info!("Probe try ({}/{})", retries, self.retries);

            let mut process = match self.exec(&env).await {
                Ok(p) => p,
                Err(e) => {
                    warn!("Probe failed to exec: {:?}", e);
                    return Ok(false);
                }
            };

            let output_collector = OutputCollector::new();
            tokio::select! {
                // Cancelling branch, kill the process and quits immediately
                _ = cancel_rx.changed() => {
                    info!("Probe cancelled. output: {:?}", output_collector.take_output());
                    if let Some(pid) = process.pid() { self.manager.stop_by_pid(pid).await; }
                    return Ok(false);
                },
                // Timeout branch
                _ = tokio::time::sleep(Duration::from_secs(self.timeout)) => {
                    info!("Probe timed-out. output: {:?}", output_collector.take_output());
                },
                // Normal branch, success if finished with code 0
                exit = process.wait_with_piped_outputs(output_collector.clone()) => {
                    let success = match exit {
                        Ok(Some(exit_status)) => {
                            info!("Probe finished with exit code {:?}.\noutput: {:?}", exit_status, output_collector.take_output());
                            match exit_status {
                                ChildExit::Finished(Some(code)) if code == 0 => true,
                                _ => false
                            }
                        },
                        Ok(None) => anyhow::bail!("unable to determine why probe exited"),
                        Err(e) => anyhow::bail!("error while waiting probe: {:?}", e),
                    };
                    if success {
                        info!("Probe succeeded");
                        return Ok(true);
                    }
                }
            }

            // Retry up to `self.retries` times when timeout or finished with non-zero code
            if retries >= self.retries {
                info!("Probe failed");
                return Ok(false);
            }

            // Retry count does not increase until `start_period` seconds elapsed
            if start.elapsed().as_secs() >= self.start_period {
                retries += 1;
            }

            info!(
                "Probe next retry {}/{} after {} sec",
                retries, self.retries, self.interval
            );
            tokio::time::sleep(Duration::from_secs(self.interval)).await;
        }
    }

    async fn exec(&self, env: &HashMap<String, String>) -> anyhow::Result<Child> {
        let mut args = Vec::new();
        args.extend(self.shell_args.clone());
        args.push(self.command.clone());
        let cmd = Command::new(self.shell.clone())
            .with_args(args)
            .with_envs(env.clone())
            .with_current_dir(self.working_dir.clone())
            .with_label(&format!("{} probe", self.name))
            .to_owned();

        match self.manager.spawn(cmd, PROBE_STOP_TIMEOUT).await {
            Some(Ok(child)) => Ok(child),
            Some(Err(e)) => anyhow::bail!("failed to spawn probe process: {:?}", e),
            _ => anyhow::bail!("failed to spawn probe process"),
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
        let probe = LogLineProbe::new("test", Regex::new("test").unwrap(), 1);
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
        let probe = LogLineProbe::new("test", Regex::new("test").unwrap(), 1);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(());

        log_tx.send(String::from("aaaa").into_bytes()).ok();

        let result = probe.run(log_rx, cancel_rx).await;

        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_log_line_probe_canceled() {
        setup();
        let probe = LogLineProbe::new("test", Regex::new("test").unwrap(), 1);
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
            Env::new(),
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
            Env::new(),
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
            Env::new(),
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
            Env::new(),
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
