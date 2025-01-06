use crate::process::ProcessManager;
use crate::run::ProjectRunner;
use crate::signal::SignalHandler;
use std::future::Future;
use tokio::signal::unix;
use tracing::error;

pub fn get_signal() -> anyhow::Result<impl Future<Output=Option<()>>> {
    let mut sigint = unix::signal(unix::SignalKind::interrupt())?;
    let mut sigterm = unix::signal(unix::SignalKind::terminate())?;

    // Wait SIGINT (Ctrl+C) & SIGTERM (Ctrl+\)
    Ok(async move {
        tokio::select! {
            res = sigint.recv() => {
                res
            }
            res = sigterm.recv() => {
                res
            }
        }
    })
}


pub async fn run(task_names: Vec<String>) -> anyhow::Result<i32> {
    let signal = get_signal()?;
    let handler = SignalHandler::new(signal);

    let processes = ProcessManager::new(atty::is(atty::Stream::Stdout));
    
    let run = ProjectRunner::new(processes, handler.clone());
    
    let run_fut = async {
        let (sender, handle) = run.start_tui()?.unzip();

        let result = run.run(sender.clone(), false).await;
        
        if let Some(handle) = handle {
            if let Err(e) = handle.await.expect("render thread panicked") {
                error!("error encountered rendering tui: {e}");
            }
        }

        // if let Some(sender) = sender {
        //     sender.stop().await;
        // }
        
        result
    };

    let handler_fut = handler.done();
    
    tokio::select! {
        biased;
        // If we get a handler exit at the same time as a run finishes we choose that
        // future to display that we're respecting user input
        _ = handler_fut => {
            // We caught a signal, which already notified the subscribers
            Ok(1)
        }
        result = run_fut => {
            // Run finished so close the signal handler
            handler.close().await;
            result
        },
    }
}
