use indexmap::IndexMap;
use itertools::Itertools;
use std::path::PathBuf;
use std::{
    ffi::{OsStr, OsString},
    process::Stdio,
};

/// A command builder that can be used to build both regular
/// child processes and ones spawned hooked up to a PTY
#[derive(Debug, Clone)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: IndexMap<OsString, OsString>,
    open_stdin: bool,
    env_clear: bool,
    label: Option<String>,
}

impl Command {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        let program = program.as_ref().to_os_string();
        Self {
            program,
            args: Vec::new(),
            cwd: None,
            env: IndexMap::new(),
            open_stdin: true,
            env_clear: false,
            label: None,
        }
    }

    pub fn with_args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args = args.into_iter().map(|arg| arg.as_ref().to_os_string()).collect();
        self
    }

    pub fn with_current_dir(&mut self, dir: PathBuf) -> &mut Self {
        self.cwd = Some(dir);
        self
    }

    pub fn with_envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (ref key, ref val) in vars {
            self.env
                .insert(key.as_ref().to_os_string(), val.as_ref().to_os_string());
        }
        self
    }

    pub fn with_env<K, V>(&mut self, key: K, val: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.env
            .insert(key.as_ref().to_os_string(), val.as_ref().to_os_string());
        self
    }

    /// Configure the child process to spawn with a piped stdin
    pub fn open_stdin(&mut self) -> &mut Self {
        self.open_stdin = true;
        self
    }

    /// Clears the environment variables for the child process
    pub fn env_clear(&mut self) -> &mut Self {
        self.env_clear = true;
        self.env.clear();
        self
    }

    /// If stdin is expected to be opened
    pub fn will_open_stdin(&self) -> bool {
        self.open_stdin
    }

    pub fn label(&self) -> String {
        match self.label.clone() {
            Some(label) => label,
            None => self.default_label(),
        }
    }

    pub fn with_label(&mut self, label: &str) -> &mut Self {
        self.label = Some(label.to_string());
        self
    }

    pub fn default_label(&self) -> String {
        format!(
            "({}) {} {}",
            self.cwd.as_deref().map(|dir| dir.to_str().unwrap()).unwrap_or_default(),
            self.program.to_string_lossy(),
            self.args.iter().map(|s| s.to_string_lossy()).join(" ")
        )
    }
}

impl From<Command> for tokio::process::Command {
    fn from(value: Command) -> Self {
        let Command {
            program,
            args,
            cwd,
            env,
            open_stdin,
            env_clear,
            ..
        } = value;

        let mut cmd = tokio::process::Command::new(program);
        if env_clear {
            cmd.env_clear();
        }
        cmd.args(args)
            .envs(env)
            // We always pipe stdout/stderr to allow us to capture task output
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Only open stdin if configured to do so
            .stdin(if open_stdin { Stdio::piped() } else { Stdio::null() });
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd.as_path());
        }
        cmd
    }
}

impl From<Command> for portable_pty::CommandBuilder {
    fn from(value: Command) -> Self {
        let Command {
            program,
            args,
            cwd,
            env,
            env_clear,
            ..
        } = value;
        let mut cmd = portable_pty::CommandBuilder::new(program);
        if env_clear {
            cmd.env_clear();
        }
        cmd.args(args);
        if let Some(cwd) = cwd {
            cmd.cwd(cwd.as_path());
        } else if let Ok(cwd) = std::env::current_dir() {
            // portably_pty defaults to a users home directory instead of cwd if one isn't
            // configured on the command builder.
            // We explicitly set the cwd if one exists to avoid this behavior
            cmd.cwd(&cwd);
        }
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd
    }
}
