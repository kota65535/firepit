use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use console::{Style, StyledObject};
use tokio::sync::oneshot;
use tracing::{error, Instrument};
use crate::process::{ChildExit, Command, ProcessManager};
use crate::run::output::{TaskCacheOutput, TaskOutput};
use crate::run::UIMode;
use crate::tui::event::OutputLogs;
use crate::ui::color_selector::ColorSelector;
use crate::ui::lib::ColorConfig;
use crate::ui::output::OutputWriter;
use crate::ui::prefixed::PrefixedUI;

pub struct ExecContext {
    color_config: ColorConfig,
    task_id: String,
    ui_mode: UIMode,
    pretty_prefix: StyledObject<String>,
    manager: ProcessManager,
    takes_input: bool,
    cmd: Command,
}

enum ExecOutcome {
    // All operations during execution succeeded
    Success(SuccessOutcome),
    // An error with the task execution
    Task {
        exit_code: Option<i32>,
        message: String,
    },
    // Task didn't execute normally due to a shutdown being initiated by another task
    Shutdown,
}

enum SuccessOutcome {
    CacheHit,
    Run,
}

impl ExecContext {
    
    
    pub fn new(
        color_config: ColorConfig,
        ui_mode: UIMode,
        task_id: String,
        manager: ProcessManager,
        cmd: Command, 
        
    ) -> ExecContext {
        let selector = ColorSelector::default();
        
        Self {
            color_config: color_config,
            ui_mode: ui_mode,
            pretty_prefix: selector.prefix_with_color(&task_id, &task_id),
            task_id,
            manager: manager,
            takes_input: true,
            cmd,
        }
    }
    pub async fn execute(
        &mut self,
        output_client: TaskOutput<impl Write>
    ) -> Result<(), InternalError> {
        let span = tracing::debug_span!("execute_task", task = %self.task_id);
        let mut result = self
            .execute_inner(&output_client)
            .instrument(span)
            .await;

        // If the task resulted in an error, do not group in order to better highlight
        // the error.
        let is_error = matches!(result, Ok(ExecOutcome::Task { .. }));
        let is_cache_hit = matches!(result, Ok(ExecOutcome::Success(SuccessOutcome::CacheHit)));
        let logs = match output_client.finish(is_error, is_cache_hit) {
            Ok(logs) => logs,
            Err(e) => {
                error!("unable to flush output client: {e}");
                result = Err(InternalError::Io(e));
                None
            }
        };

        match result {
            Ok(ExecOutcome::Success(outcome)) => {
            }
            Ok(ExecOutcome::Shutdown) => {
                // Probably overkill here, but we should make sure the process manager is
                // stopped if we think we're shutting down.
                self.manager.stop().await;
            }
            Ok(ExecOutcome::Task { exit_code, message }) => {}
            Err(e) => {
                self.manager.stop().await;
                return Err(e);
            }
        }

        Ok(())
    }

    fn my_prefixed_ui<W: Write>(
        &self,
        color_config: ColorConfig,
        is_github_actions: bool,
        stdout: W,
        stderr: W,
        prefix: StyledObject<String>,
    ) -> PrefixedUI<W> {
        let mut prefixed_ui = PrefixedUI::new(color_config, stdout, stderr)
            .with_output_prefix(prefix.clone())
            // TODO: we can probably come up with a more ergonomic way to achieve this
            .with_error_prefix(
                Style::new().apply_to(format!("{}ERROR: ", color_config.apply(prefix.clone()))),
            )
            .with_warn_prefix(prefix);
        if is_github_actions {
            prefixed_ui = prefixed_ui
                .with_error_prefix(Style::new().apply_to("[ERROR] ".to_string()))
                .with_warn_prefix(Style::new().apply_to("[WARN] ".to_string()));
        }
        prefixed_ui
    }

    fn prefixed_ui<'a, W: Write>(
        &self,
        output_client: &'a TaskOutput<W>,
    ) -> TaskCacheOutput<OutputWriter<'a, W>> {
        match output_client {
            TaskOutput::Direct(client) => TaskCacheOutput::Direct(self.my_prefixed_ui(
                self.color_config,
                false,
                client.stdout(),
                client.stderr(),
                self.pretty_prefix.clone(),
            )),
            TaskOutput::UI(task) => TaskCacheOutput::UI(task.clone()),
        }
    }

    async fn execute_inner(
        &mut self,
        output_client: &TaskOutput<impl Write>,
    ) -> Result<ExecOutcome, InternalError> {
        let task_start = Instant::now();
        let mut prefixed_ui = self.prefixed_ui(output_client);

        if self.ui_mode == UIMode::Tui {
            if let TaskOutput::UI(task) = output_client {
                task.start(OutputLogs::Full);
            }
        }


        let cmd = self.cmd.clone();

        let mut process = match self.manager.spawn(cmd, Duration::from_millis(500)) {
            Some(Ok(child)) => child,
            // Turbo was unable to spawn a process
            Some(Err(e)) => {
                // Note: we actually failed to spawn, but this matches the Go output
                // prefixed_ui.error(&format!("command finished with error: {e}"));
                let error_string = e.to_string();
                return Ok(ExecOutcome::Task {
                    exit_code: None,
                    message: error_string,
                });
            }
            // Turbo is shutting down
            None => {
                return Ok(ExecOutcome::Shutdown);
            }
        };

        if self.ui_mode == UIMode::Tui {
            if let TaskOutput::UI(task) = output_client {
                if let Some(stdin) = process.stdin() {
                    task.set_stdin(stdin);
                }
            }
        }

        // Even if user does not have the TUI and cannot interact with a task, we keep
        // stdin open for persistent tasks as some programs will shut down if stdin is
        // closed.
        if !self.takes_input && !self.manager.closing_stdin_ends_process() {
            process.stdin();
        }

        let mut stdout_writer = prefixed_ui.task_writer();

        let exit_status = match process.wait_with_piped_outputs(&mut stdout_writer).await {
            Ok(Some(exit_status)) => exit_status,
            Err(e) => {
                return Err(e.into());
            }
            Ok(None) => {
                // TODO: how can this happen? we only update the
                // exit status with Some and it is only initialized with
                // None. Is it still running?
                error!("unable to determine why child exited");
                return Err(InternalError::UnknownChildExit);
            }
        };
        let task_duration = task_start.elapsed();

        match exit_status {
            ChildExit::Finished(Some(0)) => {
                // Attempt to flush stdout_writer and log any errors encountered
                if let Err(e) = stdout_writer.flush() {
                    error!("{e}");
                } 
                // Return success outcome
                Ok(ExecOutcome::Success(SuccessOutcome::Run))
            }
            ChildExit::Finished(Some(code)) => {
                // If there was an error, flush the buffered output
                if let Err(e) = stdout_writer.flush() {
                    error!("error flushing logs: {e}");
                }
                // let error = TaskErrorCause::from_execution(process.label().to_string(), code);
                let message = format!("Code: {}", code);
                // prefixed_ui.error(&format!("command finished with error: {error}"));
                
                    // .push(TaskError::new(self.task_id_for_display.clone(), error));
                Ok(ExecOutcome::Task {
                    exit_code: Some(code),
                    message,
                })
            }
            // The child exited in a way where we can't figure out how it finished so we assume it
            // failed.
            ChildExit::Finished(None) | ChildExit::Failed => Err(InternalError::UnknownChildExit),
            // Something else killed the child
            ChildExit::KilledExternal => Err(InternalError::ExternalKill),
            // The child was killed by turbo indicating a shutdown
            ChildExit::Killed => Ok(ExecOutcome::Shutdown),
        }
    }
}


#[derive(Debug, thiserror::Error)]
pub enum InternalError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("unable to determine why task exited")]
    UnknownChildExit,
    #[error("unable to find package manager binary: {0}")]
    Which(#[from] which::Error),
    #[error("external process killed a task")]
    ExternalKill,
}
