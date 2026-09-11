use crate::log::OutputCollector;
use crate::process::{Child, ChildExit, Command, ProcessManager};
use crate::project::Env;
use crate::PROBE_STOP_TIMEOUT;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub enum Probe {
    LogLine(LogLineProbe),
    Exec(ExecProbe),
    None,
}

#[derive(Debug, Clone)]
pub struct LogLineProbe {
    regex: Regex,
    timeout: u64,
}

impl LogLineProbe {
    pub fn new(regex: Regex, timeout: u64) -> Self {
        Self { regex, timeout }
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
                        // Dropping the whole line on one bad byte would keep the
                        // pattern from ever matching, with nothing to say why
                        let line = String::from_utf8_lossy(&event);
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
    // Public constructor mirroring the exec health-check config fields; a builder
    // refactor would change the public API without real benefit.
    #[allow(clippy::too_many_arguments)]
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

    /// Runs the health check until it succeeds, fails for good, or is cancelled.
    ///
    /// Every try is logged with the output of the command, so that a service
    /// that never becomes ready can be diagnosed from the log. The try that
    /// gives up is logged at `WARN`, the level at which the log is read by
    /// default; the ones before it are only of interest once something is wrong.
    pub async fn run(&self, mut cancel_rx: watch::Receiver<()>) -> anyhow::Result<bool> {
        let env = self.env.load()?;
        info!("Probe started. command: {}", self.command);
        let start = Instant::now();

        // Wait `interval` seconds before the first health check
        tokio::time::sleep(Duration::from_secs(self.interval)).await;

        let mut retries = 0;
        let mut tries = 0;
        loop {
            tries += 1;
            debug!("Probe try ({}/{})", retries, self.retries);

            let mut process = match self.exec(&env).await {
                Ok(p) => p,
                Err(e) => {
                    error!("Probe cannot run its command: {:?}", e);
                    return Ok(false);
                }
            };

            let collector = OutputCollector::new();
            // How this try ended, and what the command wrote while it ran. A try
            // that succeeds returns right away, so what is left is a failed one.
            let (outcome, output) = tokio::select! {
                // Cancelling branch, kill the process and quits immediately
                _ = cancel_rx.changed() => {
                    info!("Probe cancelled{}", Self::logged_output(&collector));
                    if let Some(pid) = process.pid() { self.manager.stop_by_pid(pid).await; }
                    return Ok(false);
                },
                // Timeout branch, stop the process before the next retry
                _ = tokio::time::sleep(Duration::from_secs(self.timeout)) => {
                    if let Some(pid) = process.pid() {
                        let exit = self.manager.stop_by_pid(pid).await;
                        debug!("Probe process stopped by timeout. exit: {:?}", exit);
                    }
                    (format!("timed out after {}s", self.timeout), Self::logged_output(&collector))
                },
                // Normal branch, success if finished with code 0
                exit = process.wait_with_piped_outputs(collector.clone(), collector.clone()) => {
                    match exit {
                        Ok(Some(exit_status)) => {
                            let output = Self::logged_output(&collector);
                            match exit_status {
                                ChildExit::Finished(Some(0)) => {
                                    info!("Probe finished with exit code 0{}", output);
                                    return Ok(true);
                                }
                                ChildExit::Finished(Some(code)) => (format!("finished with exit code {code}"), output),
                                other => (format!("ended: {other:?}"), output),
                            }
                        },
                        Ok(None) => anyhow::bail!("cannot determine why the probe exited"),
                        Err(e) => anyhow::bail!("error while waiting probe: {:?}", e),
                    }
                }
            };

            // Retry up to `self.retries` times when timeout or finished with non-zero code
            if retries >= self.retries {
                // The service will not become ready, so say why at a level that is
                // read by default, together with the output that explains it.
                warn!("Probe {outcome}, giving up after {tries} tries{output}");
                return Ok(false);
            }
            info!("Probe {outcome}{output}");

            // Retry count does not increase until `start_period` seconds elapsed
            if start.elapsed().as_secs() >= self.start_period {
                retries += 1;
            }

            debug!(
                "Probe next retry {}/{} after {} sec",
                retries, self.retries, self.interval
            );
            tokio::time::sleep(Duration::from_secs(self.interval)).await;
        }
    }

    /// What the probe command wrote, for the end of a log message: on lines of
    /// its own so that it stays readable, and nothing at all when it wrote nothing.
    fn logged_output(collector: &OutputCollector) -> String {
        let output = collector.take_output();
        let output = output.trim_end();
        if output.is_empty() {
            String::new()
        } else {
            format!("\n{output}")
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
    use std::io::Write;
    use std::sync::Once;

    static INIT: Once = Once::new();

    pub fn setup() {
        INIT.call_once(|| {
            tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).init();
        });
    }

    /// The output of a probe command goes into the log on lines of its own, so
    /// that it stays readable, and a command that wrote nothing adds nothing.
    #[test]
    fn test_logged_output() {
        let collector = OutputCollector::new();
        assert_eq!(ExecProbe::logged_output(&collector), "");

        collector.clone().write_all(b"connection refused\n").unwrap();
        assert_eq!(ExecProbe::logged_output(&collector), "\nconnection refused");

        // Reading the output empties it, so the next try starts clean
        assert_eq!(ExecProbe::logged_output(&collector), "");

        collector.clone().write_all(b"first\nsecond\n\n").unwrap();
        assert_eq!(ExecProbe::logged_output(&collector), "\nfirst\nsecond");
    }

    #[tokio::test]
    async fn test_log_line_probe_succeeds() {
        setup();
        let probe = LogLineProbe::new(Regex::new("test").unwrap(), 1);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(());

        log_tx.send(String::from("aaaa").into_bytes()).ok();
        log_tx.send(String::from("testtest").into_bytes()).ok();

        // log line matches and succeed
        let result = probe.run(log_rx, cancel_rx).await;

        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_log_line_probe_timeout() {
        setup();
        let probe = LogLineProbe::new(Regex::new("test").unwrap(), 1);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(());

        log_tx.send(String::from("aaaa").into_bytes()).ok();

        let result = probe.run(log_rx, cancel_rx).await;

        assert!(!(result.unwrap()));
    }

    #[tokio::test]
    async fn test_log_line_probe_canceled() {
        setup();
        let probe = LogLineProbe::new(Regex::new("test").unwrap(), 1);
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(());

        log_tx.send(String::from("aaaa").into_bytes()).ok();
        cancel_tx.send(());

        let result = probe.run(log_rx, cancel_rx).await;

        assert!(!(result.unwrap()));
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
            Env::default(),
            1,
            1,
            1,
            0,
        );
        let (cancel_tx, cancel_rx) = watch::channel(());

        let result = probe.run(cancel_rx).await;

        assert!(result.unwrap());
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
            Env::default(),
            1,
            1,
            1,
            0,
        );
        let (cancel_tx, cancel_rx) = watch::channel(());

        let result = probe.run(cancel_rx).await;

        assert!(!(result.unwrap()));
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
            Env::default(),
            1,
            1,
            1,
            0,
        );
        let (cancel_tx, cancel_rx) = watch::channel(());

        let result = probe.run(cancel_rx).await;

        assert!(!(result.unwrap()));
    }

    /// A probe process that outlives the probe `timeout` must be stopped, not
    /// left running until the whole probe finishes.
    #[tokio::test]
    async fn test_exec_probe_timeout_stops_process() {
        setup();
        let pid_file = std::env::temp_dir().join("firepit-test-probe-timeout.pid");
        std::fs::remove_file(&pid_file).ok();
        let probe = ExecProbe::new(
            "test",
            &format!("echo $$ >> {}; sleep 10", pid_file.to_string_lossy()),
            "bash",
            vec![String::from("-c")],
            PathBuf::from("./"),
            Env::default(),
            1,
            1,
            1,
            0,
        );
        let (_cancel_tx, cancel_rx) = watch::channel(());

        assert!(!probe.run(cancel_rx).await.unwrap());

        let pids = std::fs::read_to_string(&pid_file)
            .unwrap()
            .lines()
            .map(|l| l.trim().parse::<i32>().unwrap())
            .collect::<Vec<_>>();
        assert!(!pids.is_empty(), "no probe process was recorded");
        for pid in pids {
            // Signal 0 sends nothing and only checks that the process exists
            let alive = unsafe { libc::kill(pid, 0) == 0 };
            if alive {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            assert!(!alive, "probe process {} was left running", pid);
        }
        std::fs::remove_file(&pid_file).ok();
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
            Env::default(),
            1,
            1,
            1,
            0,
        );
        let (cancel_tx, cancel_rx) = watch::channel(());

        cancel_tx.send(());
        let result = probe.run(cancel_rx).await;

        assert!(!(result.unwrap()));
    }
}
