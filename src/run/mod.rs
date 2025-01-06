mod exec;
pub(crate) mod output;

use crate::process::{Command, ProcessManager};
use crate::signal::SignalHandler;
use crate::tui;
use crate::tui::TuiSender;
use crate::ui::sender::UISender;
use std::io::Write;
use std::time::{Duration};
use futures::future;
use tokio::task::JoinHandle;
use crate::graph::TaskGraph;
use crate::run::exec::ExecContext;
use crate::run::output::{StdWriter, TaskOutput};
use crate::ui::lib::ColorConfig;
use crate::ui::output::OutputSink;

#[derive(Clone)]
pub struct ProjectRunner {
    pub process_manager: ProcessManager,
    pub signal_handler: SignalHandler,
    pub tasks: Vec<String>,
    pub ui_mode: UIMode,
    pub concurrency: u32,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum UIMode {
    Tui,
    Stream,
}

impl Default for UIMode {
    fn default() -> Self {
        Self::Tui
    }
}

enum ExecOutcome {
    // All operations during execution succeeded
    Success(),
    // An error with the task execution
    Task {
        exit_code: Option<i32>,
        message: String,
    },
    // Task didn't execute normally due to a shutdown being initiated by another task
    Shutdown,
}


impl ProjectRunner {
    pub fn new(process_manager: ProcessManager, signal_handler: SignalHandler) -> Self {
        Self {
            process_manager,
            signal_handler,
            tasks: vec!["#foo".to_string(), "#bar".to_string()],
            ui_mode: UIMode::Tui,
            concurrency: 2
        }
    }
    
    pub fn start_tui(&self) -> anyhow::Result<Option<(TuiSender, JoinHandle<anyhow::Result<()>>)>> {
        if self.ui_mode != UIMode::Tui {
            return Ok(None);
        }
        let (sender, receiver) = TuiSender::new();
        let handle = tokio::task::spawn(async move {
            Ok(tui::run_app(vec!["#foo".to_string(), "#bar".to_string()], receiver).await?)
        });

        Ok(Some((sender, handle)))
    }

    fn sink() -> OutputSink<StdWriter> {
        let (out, err) = (std::io::stdout().into(), std::io::stdout().into());
        OutputSink::new(out, err)
    }


    pub async fn run(&self, ui_sender: Option<TuiSender>, is_watch: bool) -> anyhow::Result<i32> {
        if let Some(app) = &ui_sender {
            if let Some(pane_size) = app.pane_size().await {
                self.process_manager.set_pty_size(pane_size.rows, pane_size.cols);
            }
        }

        let mut cmd1 = Command::new("bash");
        cmd1.args(["count.sh"]);
        
        let mut cmd2 = Command::new("bash");
        cmd2.args(["count.sh", "10"]);

        let mut c = ExecContext::new(
            ColorConfig::new(false),
            self.ui_mode,
            "#foo".to_string(),
            self.process_manager.clone(),
            cmd1
        );
        
        let mut c2 = ExecContext::new(
            ColorConfig::new(false),
            self.ui_mode,
            "#bar".to_string(),
            self.process_manager.clone(),
            cmd2
        );
        
        let output_client = if let Some(handle) = &ui_sender {
            TaskOutput::UI(handle.task("#foo".to_string()))
        } else {
            let sink = Self::sink();
            let mut logger = sink.logger(crate::ui::output::OutputClientBehavior::Passthrough);    
            TaskOutput::Direct(logger)
        };
        
        let output_client2 = if let Some(handle) = &ui_sender {
            TaskOutput::UI(handle.task("#bar".to_string()))
        } else {
            let sink = Self::sink();
            let mut logger = sink.logger(crate::ui::output::OutputClientBehavior::Passthrough);
            TaskOutput::Direct(logger)
        };
        
        let h1 = c.execute(output_client);
        let h2 = c2.execute(output_client2);
        future::join(h1, h2).await;
        
        Ok(1)
    }

    // async fn execute_inner(&self, output_client: &TaskOutput<impl Write>, ) -> anyhow::Result<ExecOutcome> {
    //     let task_start = Instant::now();
    //     let mut prefixed_ui = self.prefixed_ui(output_client);
    // 
    //     if self.ui_mode.has_sender() {
    //         if let TaskOutput::UI(task) = output_client {
    //             let output_logs = self.task_cache.output_logs().into();
    //             task.start(output_logs);
    //         }
    //     }
    // 
    //     let cmd = self.cmd.clone();
    // 
    //     let mut process = match self.process_manager.spawn(cmd, Duration::from_millis(500)) {
    //         Some(Ok(child)) => child,
    //         // Turbo was unable to spawn a process
    //         Some(Err(e)) => {
    //             // Note: we actually failed to spawn, but this matches the Go output
    //             prefixed_ui.error(&format!("command finished with error: {e}"));
    //             let error_string = e.to_string();
    //             self.errors
    //                 .lock()
    //                 .expect("lock poisoned")
    //                 .push(TaskError::from_spawn(self.task_id_for_display.clone(), e));
    //             return Ok(ExecOutcome::Task {
    //                 exit_code: None,
    //                 message: error_string,
    //             });
    //         }
    //         // Turbo is shutting down
    //         None => {
    //             return Ok(ExecOutcome::Shutdown);
    //         }
    //     };
    // 
    //     if self.ui_mode.has_sender() && self.takes_input {
    //         if let TaskOutput::UI(task) = output_client {
    //             if let Some(stdin) = process.stdin() {
    //                 task.set_stdin(stdin);
    //             }
    //         }
    //     }
    // 
    //     // Even if user does not have the TUI and cannot interact with a task, we keep
    //     // stdin open for persistent tasks as some programs will shut down if stdin is
    //     // closed.
    //     if !self.takes_input && !self.process_manager.closing_stdin_ends_process() {
    //         process.stdin();
    //     }
    // 
    //     let mut stdout_writer = self
    //         .task_cache
    //         .output_writer(prefixed_ui.task_writer())
    //         .inspect_err(|_| {
    //             telemetry.track_error(TrackedErrors::FailedToCaptureOutputs);
    //         })?;
    // 
    //     let exit_status = match process.wait_with_piped_outputs(&mut stdout_writer).await {
    //         Ok(Some(exit_status)) => exit_status,
    //         Err(e) => {
    //             telemetry.track_error(TrackedErrors::FailedToPipeOutputs);
    //             return Err(e.into());
    //         }
    //         Ok(None) => {
    //             // TODO: how can this happen? we only update the
    //             // exit status with Some and it is only initialized with
    //             // None. Is it still running?
    //             telemetry.track_error(TrackedErrors::UnknownChildExit);
    //             error!("unable to determine why child exited");
    //             return Err(InternalError::UnknownChildExit);
    //         }
    //     };
    //     let task_duration = task_start.elapsed();
    // 
    //     match exit_status {
    //         ChildExit::Finished(Some(0)) => {
    //             // Attempt to flush stdout_writer and log any errors encountered
    //             if let Err(e) = stdout_writer.flush() {
    //                 error!("{e}");
    //             } else if self
    //                 .task_access
    //                 .can_cache(&self.task_hash, &self.task_id_for_display)
    //                 .unwrap_or(true)
    //             {
    //                 if let Err(e) = self.task_cache.save_outputs(task_duration, telemetry).await {
    //                     error!("error caching output: {e}");
    //                     return Err(e.into());
    //                 } else {
    //                     // If no errors, update hash tracker with expanded outputs
    //                     self.hash_tracker.insert_expanded_outputs(
    //                         self.task_id.clone(),
    //                         self.task_cache.expanded_outputs().to_vec(),
    //                     );
    //                 }
    //             }
    // 
    //             // Return success outcome
    //             Ok(ExecOutcome::Success(SuccessOutcome::Run))
    //         }
    //         ChildExit::Finished(Some(code)) => {
    //             // If there was an error, flush the buffered output
    //             if let Err(e) = stdout_writer.flush() {
    //                 error!("error flushing logs: {e}");
    //             }
    //             if let Err(e) = self.task_cache.on_error(&mut prefixed_ui) {
    //                 error!("error reading logs: {e}");
    //             }
    //             let error = TaskErrorCause::from_execution(process.label().to_string(), code);
    //             let message = error.to_string();
    //             if self.continue_on_error {
    //                 prefixed_ui.warn("command finished with error, but continuing...");
    //             } else {
    //                 prefixed_ui.error(&format!("command finished with error: {error}"));
    //             }
    //             self.errors
    //                 .lock()
    //                 .expect("lock poisoned")
    //                 .push(TaskError::new(self.task_id_for_display.clone(), error));
    //             Ok(ExecOutcome::Task {
    //                 exit_code: Some(code),
    //                 message,
    //             })
    //         }
    //         // The child exited in a way where we can't figure out how it finished so we assume it
    //         // failed.
    //         ChildExit::Finished(None) | ChildExit::Failed => Err(InternalError::UnknownChildExit),
    //         // Something else killed the child
    //         ChildExit::KilledExternal => Err(InternalError::ExternalKill),
    //         // The child was killed by turbo indicating a shutdown
    //         ChildExit::Killed => Ok(ExecOutcome::Shutdown),
    //     }
    // }
}

