use crate::event::{TaskEvent, TaskEventReceiver, TaskEventSender, TaskResult};
use crate::process::ChildExit;
use anyhow::{anyhow, Context};
use log::{debug, info, trace};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Layout},
    widgets::TableState,
    Frame, Terminal,
};
use std::collections::HashMap;
use std::io::Read;
use std::{
    collections::BTreeMap,
    io::{self, Stdout, Write},
    mem,
    time::Duration,
};
use indexmap::IndexMap;
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

pub const FRAME_RATE: Duration = Duration::from_millis(3);
const RESIZE_DEBOUNCE_DELAY: Duration = Duration::from_millis(10);

use super::{
    event::{Direction, PaneSize},
    input,
    Debouncer, Event, InputOptions, SizeInfo, TaskTable, TerminalPane, TuiReceiver,
};
use crate::tui::{
    term_output::TerminalOutput,
};
use crate::tui::task::{TaskPlan, TaskStatus};

#[derive(Debug, Clone)]
pub enum LayoutSections {
    Pane,
    TaskList,
    // Search {
    //     previous_selection: String,
    //     results: SearchResults,
    // },
}

pub struct TuiApp<W> {
    size: SizeInfo,
    task_outputs: IndexMap<String, TerminalOutput<W>>,
    task_statuses: IndexMap<String, TaskStatus>,
    focus: LayoutSections,
    scroll: TableState,
    selected_task_index: usize,
    has_user_scrolled: bool,
    has_sidebar: bool,
    done: bool,
}

impl<W> TuiApp<W> {
    pub fn new(rows: u16, cols: u16, mut target_tasks: Vec<String>, mut dep_tasks: Vec<String>) -> Self {
        let size = SizeInfo::new(rows, cols, target_tasks.iter().chain(dep_tasks.iter()).map(|s| s.as_str()));

        target_tasks.sort_unstable();
        dep_tasks.sort_unstable();

        let has_user_interacted = false;
        let selected_task_index: usize = 0;

        let pane_rows = size.pane_rows();
        let pane_cols = size.pane_cols();

        let task_outputs = target_tasks.iter().chain(dep_tasks.iter())
            .map(|t| (t.clone(), TerminalOutput::new(t, pane_rows, pane_cols, None)))
            .collect::<IndexMap<_, _>>();

        let task_statuses = target_tasks.iter()
            .map(|t| (t.clone(), TaskStatus::Planned(TaskPlan::new(true))))
            .chain(dep_tasks.iter()
                    .map(|t| (t.clone(), TaskStatus::Planned(TaskPlan::new(false)))))
            .collect::<IndexMap<_, _>>();
        
        Self {
            size,
            task_outputs,
            task_statuses,
            done: false,
            focus: LayoutSections::TaskList,
            scroll: TableState::default().with_selected(selected_task_index),
            selected_task_index,
            has_sidebar: true,
            has_user_scrolled: has_user_interacted,
        }
    }

    pub fn active_task(&self) -> anyhow::Result<&TerminalOutput<W>> {
        self.nth_task(self.selected_task_index)
    }

    pub fn active_task_mut(&mut self) -> anyhow::Result<&mut TerminalOutput<W>> {
        self.nth_task_mut(self.selected_task_index)
    }

    pub fn task(&self, name: &str) -> anyhow::Result<&TerminalOutput<W>> {
        self.task_outputs.get(name).with_context(|| format!("task {} not found", name))
    }

    pub fn task_mut(&mut self, name: &str) -> anyhow::Result<&mut TerminalOutput<W>> {
        self.task_outputs.get_mut(name).with_context(|| format!("task {} not found", name))
    }

    fn input_options(&self) -> anyhow::Result<InputOptions> {
        let has_selection = self.active_task()?.has_selection();
        Ok(InputOptions {
            focus: &self.focus,
            has_selection,
        })
    }

    pub fn nth_task(&self, num: usize) -> anyhow::Result<&TerminalOutput<W>> {
        self.task_outputs.iter().nth(num)
            .map(|e| e.1)
            .with_context(|| anyhow::anyhow!("{}th task not found", num))
    }

    pub fn nth_task_mut(&mut self, num: usize) -> anyhow::Result<&mut TerminalOutput<W>> {
        self.task_outputs.iter_mut().nth(num)
            .map(|e| e.1)
            .with_context(|| anyhow::anyhow!("{}th task not found", num))
    }

    pub fn next(&mut self) {
        let num_rows = self.task_outputs.len();
        let next_index = (self.selected_task_index + 1).clamp(0, num_rows - 1);
        self.selected_task_index = next_index;
        self.scroll.select(Some(next_index));
        self.has_user_scrolled = true;
    }

    pub fn previous(&mut self) {
        let i = match self.selected_task_index {
            0 => 0,
            i => i - 1,
        };
        self.selected_task_index = i;
        self.scroll.select(Some(i));
        self.has_user_scrolled = true;
    }

    pub fn scroll_terminal_output(&mut self, direction: Direction) -> anyhow::Result<()> {
        self.active_task_mut()?.scroll(direction)?;
        Ok(())
    }

    pub fn task_names(&self) -> Vec<String> {
        self.task_outputs.iter().map(|t| t.0.clone()).collect()
    }

    pub fn enter_search(&mut self) -> anyhow::Result<()> {
        // self.focus = LayoutSections::Search {
        //     previous_selection: self.active_task()?.to_string(),
        //     results: SearchResults::new(self.task_names()),
        // };
        // // We set scroll as we want to keep the current selection
        // self.has_user_scrolled = true;
        Ok(())
    }

    pub fn exit_search(&mut self, restore_scroll: bool) {
        // let mut prev_focus = LayoutSections::TaskList;
        // mem::swap(&mut self.focus, &mut prev_focus);
        // if let LayoutSections::Search {
        //     previous_selection, ..
        // } = prev_focus
        // {
        //     if restore_scroll && self.select_task(&previous_selection).is_err() {
        //         // If the task that was selected is no longer in the task list we reset
        //         // scrolling.
        //         self.reset_scroll();
        //     }
        // }
    }

    pub fn search_scroll(&mut self, direction: Direction) -> anyhow::Result<()> {
        // let LayoutSections::Search { results, .. } = &self.focus else {
        //     debug!("scrolling search while not searching");
        //     return Ok(());
        // };
        // let new_selection: Option<String> = match direction {
        //     Direction::Up => results.first_match(
        //         self.task_names().iter()
        //             .rev()
        //             // We skip all of the tasks that are at or after the current selection
        //             .skip(self.tasks.len() - self.selected_task_index))
        //     Direction::Down => results.first_match(
        //         self.task_names().iter()
        //             .skip(self.selected_task_index + 1),
        //     ),
        // };
        // if let Some(new_selection) = new_selection {
        //     self.select_task(&new_selection)?;
        // }
        Ok(())
    }

    pub fn search_enter_char(&mut self, c: char) -> anyhow::Result<()> {
        // let LayoutSections::Search { results, .. } = &mut self.focus else {
        //     debug!("modifying search query while not searching");
        //     return Ok(());
        // };
        // results.modify_query(|s| s.push(c));
        // self.update_search_results();
        Ok(())
    }

    pub fn search_remove_char(&mut self) -> anyhow::Result<()> {
        // let LayoutSections::Search { results, .. } = &mut self.focus else {
        //     debug!("modified search query while not searching");
        //     return Ok(());
        // };
        // let mut query_was_empty = false;
        // results.modify_query(|s| {
        //     query_was_empty = s.pop().is_none();
        // });
        // if query_was_empty {
        //     self.exit_search(true);
        // } else {
        //     // self.update_search_results();
        // }
        Ok(())
    }

    // fn update_search_results(&mut self) {
    //     let LayoutSections::Search { results, .. } = &self.focus else {
    //         return;
    //     };
    //
    //     // if currently selected task is in results stay on it
    //     // if not we go forward looking for a task in results
    //     if let Some(result) = results
    //         .first_match(
    //             self.tasks_by_status
    //                 .task_names_in_displayed_order()
    //                 .skip(self.selected_task_index),
    //         )
    //         .or_else(|| results.first_match(self.tasks_by_status.task_names_in_displayed_order()))
    //     {
    //         let new_selection = result.to_owned();
    //         self.has_user_scrolled = true;
    //         self.select_task(&new_selection).expect("todo");
    //     }
    // }

    pub fn start_task(&mut self, task: &str) -> anyhow::Result<()> {
        self.task_statuses.insert(task.to_string(), TaskStatus::Running);
        Ok(())
    }

    pub fn finish_task(&mut self, task: &str, result: TaskResult) -> anyhow::Result<()> {
        self.task_statuses.insert(task.to_string(), TaskStatus::Finished(result));
        Ok(())
    }

    pub fn has_stdin(&self) -> anyhow::Result<bool> {
        let task = self.active_task()?;
        Ok(task.stdin.is_some())
    }

    pub fn interact(&mut self) -> anyhow::Result<()> {
        if matches!(self.focus, LayoutSections::Pane) {
            self.focus = LayoutSections::TaskList
        } else if self.has_stdin()? {
            self.focus = LayoutSections::Pane;
        }
        Ok(())
    }

    pub fn update_tasks(&mut self, tasks: Vec<String>) -> anyhow::Result<()> {
        if tasks.is_empty() {
            debug!("got request to update task list to empty list, ignoring request");
            return Ok(());
        }
        debug!("updating task list: {tasks:?}");
        let highlighted_task = self.active_task()?.name.clone();
        if self.select_task(&highlighted_task).is_err() {
            self.reset_scroll();
        }

        // if let LayoutSections::Search { results, .. } = &mut self.focus {
        //     results.update_tasks(&self.tasks_by_status);
        // }
        // self.update_search_results();

        Ok(())
    }

    pub fn restart_tasks(&mut self, tasks: Vec<String>) -> anyhow::Result<()> {
        debug!("tasks to reset: {tasks:?}");
        // let highlighted_task = self.active_task()?.to_owned();
        // // Make sure all tasks have a terminal output
        // for task in &tasks {
        //     self.tasks.entry(task.clone()).or_insert_with(|| {
        //         TerminalOutput::new(self.size.pane_rows(), self.size.pane_cols(), None)
        //     });
        // }
        //
        // self.tasks_by_status
        //     .restart_tasks(tasks.iter().map(|s| s.as_str()));
        //
        // if let LayoutSections::Search { results, .. } = &mut self.focus {
        //     results.update_tasks(&self.tasks_by_status);
        // }
        //
        // if self.select_task(&highlighted_task).is_err() {
        //     debug!("was unable to find {highlighted_task} after restart");
        //     self.reset_scroll();
        // }

        Ok(())
    }

    /// Persist all task output to the after closing the TUI
    pub fn persist_tasks(&mut self) -> anyhow::Result<()> {
        for (_, o) in self.task_statuses.values().zip(self.task_outputs.values())
            .filter(|(s, o)| matches!(s, TaskStatus::Running | TaskStatus::Finished(_))) {
            o.persist_screen()?
        }
        Ok(())
    }

    // pub fn set_status(
    //     &mut self,
    //     task: String,
    //     status: String,
    // ) -> Result<(), Error> {
    //     let task = self
    //         .tasks
    //         .get_mut(&task)
    //         .ok_or_else(|| Error::TaskNotFound {
    //             name: task.to_owned(),
    //         })?;
    //     task.status = Some(status);
    //     Ok(())
    // }

    pub fn handle_mouse(&mut self, mut event: crossterm::event::MouseEvent) -> anyhow::Result<()> {
        let table_width = self.size.task_list_width();
        debug!("original mouse event: {event:?}, table_width: {table_width}");
        // Only handle mouse event if it happens inside of pane
        // We give a 1 cell buffer to make it easier to select the first column of a row
        if event.row > 0 && event.column >= table_width {
            // Subtract 1 from the y axis due to the title of the pane
            event.row -= 1;
            // Subtract the width of the table
            event.column -= table_width;
            debug!("translated mouse event: {event:?}");

            let task = self.active_task_mut()?;
            task.handle_mouse(event)?;
        }

        Ok(())
    }

    pub fn copy_selection(&self) -> anyhow::Result<()> {
        let task = self.active_task()?;
        let Some(text) = task.copy_selection() else {
            return Ok(());
        };
        super::copy_to_clipboard(&text);
        Ok(())
    }

    fn select_task(&mut self, task_name: &str) -> anyhow::Result<()> {
        if !self.has_user_scrolled {
            return Ok(());
        }

        let new_index_to_highlight = self.task_outputs.iter()
            .position(|task| task.0 == task_name)
            .with_context(|| format!("{} not found", task_name))?;

        self.selected_task_index = new_index_to_highlight;
        self.scroll.select(Some(new_index_to_highlight));

        Ok(())
    }

    /// Resets scroll state
    pub fn reset_scroll(&mut self) {
        self.has_user_scrolled = false;
        self.scroll.select(Some(0));
        self.selected_task_index = 0;
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.size.resize(rows, cols);
        let pane_rows = self.size.pane_rows();
        let pane_cols = self.size.pane_cols();
        self.task_outputs.values_mut().for_each(|task| {
            task.resize(pane_rows, pane_cols);
        })
    }
}

impl<W: Write> TuiApp<W> {
    /// Insert a stdin to be associated with a task
    pub fn insert_stdin(&mut self, task: &str, stdin: Option<W>) -> anyhow::Result<()> {
        let task = self.task_outputs.get_mut(task).with_context(|| format!("{} not found", task))?;
        task.stdin = stdin;
        Ok(())
    }

    pub fn forward_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if matches!(self.focus, LayoutSections::Pane) {
            let task = self.active_task_mut()?;
            if let Some(stdin) = &mut task.stdin {
                stdin.write_all(bytes).with_context(|| format!("task {} failed to forward input", task.name))?;
            }
            Ok(())
        } else {
            Ok(())
        }
    }

    pub fn process_output(&mut self, task: &str, output: &[u8]) -> anyhow::Result<()> {
        let task = self.task_mut(task)?;
        task.process(output);
        Ok(())
    }
}

/// Handle the rendering of the `App` widget based on events received by
/// `receiver`
pub async fn run_app(target_tasks: Vec<String>, dep_tasks: Vec<String>, task_receiver: TaskEventReceiver, receiver: TuiReceiver) -> anyhow::Result<()> {
    let mut terminal = startup()?;
    let size = terminal.size()?;

    let mut app: TuiApp<Box<dyn Write + Send>> = TuiApp::new(size.height, size.width, target_tasks, dep_tasks);

    let (crossterm_tx, crossterm_rx) = mpsc::channel(1024);
    input::start_crossterm_stream(crossterm_tx);

    let (result, callback) =
        match run_app_inner(&mut terminal, &mut app, task_receiver, receiver, crossterm_rx).await {
            Ok(callback) => (Ok(()), callback),
            Err(err) => {
                (Err(anyhow!("Tui shutting down: {}" , err)), None)
            }
        };

    cleanup(terminal, app, callback)?;

    result
}

// Break out inner loop so we can use `?` without worrying about cleaning up the
// terminal.
async fn run_app_inner<B: Backend + Write>(
    terminal: &mut Terminal<B>,
    app: &mut TuiApp<Box<dyn Write + Send>>,
    mut task_receiver: TaskEventReceiver,
    mut receiver: TuiReceiver,
    mut crossterm_rx: mpsc::Receiver<crossterm::event::Event>,
) -> anyhow::Result<Option<oneshot::Sender<()>>> {
    // Render initial state to paint the screen
    terminal.draw(|f| view(app, f))?;
    let mut last_render = Instant::now();
    let mut resize_debouncer = Debouncer::new(RESIZE_DEBOUNCE_DELAY);
    let mut callback = None;
    let mut needs_rerender = true;
    while let Some(event) = poll(app.input_options()?, &mut task_receiver, &mut receiver, &mut crossterm_rx).await {
        // If we only receive ticks, then there's been no state change so no update
        // needed
        if !matches!(event, Event::Tick) {
            needs_rerender = true;
        }
        let mut event = Some(event);
        let mut resize_event = None;
        if matches!(event, Some(Event::Resize { .. })) {
            resize_event = resize_debouncer.update(
                event
                    .take()
                    .expect("we just matched against a present value"),
            );
        }
        if let Some(resize) = resize_event.take().or_else(|| resize_debouncer.query()) {
            // If we got a resize event, make sure to update ratatui backend.
            terminal.autoresize()?;
            update(app, resize)?;
        }
        if let Some(event) = event {
            callback = update(app, event)?;
            if app.done {
                break;
            }
            if FRAME_RATE <= last_render.elapsed() && needs_rerender {
                terminal.draw(|f| view(app, f))?;
                last_render = Instant::now();
                needs_rerender = false;
            }
        }
    }

    Ok(callback)
}

/// Blocking poll for events, will only return None if app handle has been
/// dropped
async fn poll<'a>(
    input_options: InputOptions<'a>,
    task_receiver: &mut TaskEventReceiver,
    receiver: &mut TuiReceiver,
    crossterm_rx: &mut mpsc::Receiver<crossterm::event::Event>,
) -> Option<Event> {
    let input_closed = crossterm_rx.is_closed();

    if input_closed {
        receiver.recv().await
    } else {
        // tokio::select is messing with variable read detection
        #[allow(unused_assignments)]
        let mut event = None;
        loop {
            tokio::select! {
                e = crossterm_rx.recv() => {
                    event = e.and_then(|e| input_options.handle_crossterm_event(e));
                }
                e = receiver.recv() => {
                    event = e;
                }
                e = task_receiver.recv() => {
                    event = e.and_then(|e| {
                        match e {
                            TaskEvent::Start { task} => {
                                Some(Event::StartTask { task })
                            }
                            TaskEvent::Output { task, output } => {
                                Some(Event::TaskOutput { task, output })
                            }
                            TaskEvent::SetStdin { task, stdin} => {
                                Some(Event::SetStdin { task, stdin })
                            }
                            TaskEvent::Finish { task, result } => {
                                Some(Event::EndTask { task, result })
                            }
                            _ => None
                        }
                    })
                }
            }
            if event.is_some() {
                break;
            }
        }
        event
    }
}

fn startup() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Ensure all pending writes are flushed before we switch to alternative screen
    stdout.flush()?;
    crossterm::execute!(
        stdout,
        crossterm::event::EnableMouseCapture,
        crossterm::terminal::EnterAlternateScreen
    )?;
    let backend = CrosstermBackend::new(stdout);

    let mut terminal = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Fullscreen,
        },
    )?;
    terminal.hide_cursor()?;

    Ok(terminal)
}

/// Restores terminal to expected state
#[tracing::instrument(skip_all)]
fn cleanup<B: Backend + Write>(
    mut terminal: Terminal<B>,
    mut app: TuiApp<Box<dyn Write + Send>>,
    callback: Option<oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    terminal.clear()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen,
    )?;
    app.persist_tasks()?;
    crossterm::terminal::disable_raw_mode()?;
    terminal.show_cursor()?;
    // We can close the channel now that terminal is back restored to a normal state
    drop(callback);
    Ok(())
}

fn update(
    app: &mut TuiApp<Box<dyn Write + Send>>,
    event: Event,
) -> anyhow::Result<Option<oneshot::Sender<()>>> {
    match event {
        Event::StartTask { task } => {
            app.start_task(&task)?;
        }
        Event::TaskOutput { task, output } => {
            app.process_output(&task, &output)?;
        }
        // Event::Status {
        //     task,
        //     status,
        // } => {
        //     app.set_status(task, status)?;
        // }
        Event::InternalStop => {
            debug!("shutting down due to internal failure");
            app.done = true;
        }
        Event::Stop(callback) => {
            debug!("shutting down due to message");
            app.done = true;
            return Ok(Some(callback));
        }
        Event::Tick => {
            // app.table.tick();
        }
        Event::EndTask { task, result } => {
            app.finish_task(&task, result)?;
            app.insert_stdin(&task, None)?;
        }
        Event::Up => {
            app.previous();
        }
        Event::Down => {
            app.next();
        }
        Event::ScrollUp => {
            app.has_user_scrolled = true;
            app.scroll_terminal_output(Direction::Up)?;
        }
        Event::ScrollDown => {
            app.has_user_scrolled = true;
            app.scroll_terminal_output(Direction::Down)?;
        }
        Event::EnterInteractive => {
            app.has_user_scrolled = true;
            app.interact()?;
        }
        Event::ExitInteractive => {
            app.has_user_scrolled = true;
            app.interact()?;
        }
        Event::ToggleSidebar => {
            app.has_sidebar = !app.has_sidebar;
        }
        Event::Input { bytes } => {
            app.forward_input(&bytes)?;
        }
        Event::SetStdin { task, stdin } => {
            app.insert_stdin(&task, Some(stdin))?;
        }
        Event::UpdateTasks { tasks } => {
            app.update_tasks(tasks)?;
        }
        Event::Mouse(m) => {
            app.handle_mouse(m)?;
        }
        Event::CopySelection => {
            app.copy_selection()?;
        }
        Event::RestartTasks { tasks } => {
            app.restart_tasks(tasks)?;
        }
        Event::Resize { rows, cols } => {
            app.resize(rows, cols);
        }
        // Event::SearchEnter => {
        //     app.enter_search()?;
        // }
        // Event::SearchExit { restore_scroll } => {
        //     app.exit_search(restore_scroll);
        // }
        // Event::SearchScroll { direction } => {
        //     app.search_scroll(direction)?;
        // }
        // Event::SearchEnterChar(c) => {
        //     app.search_enter_char(c)?;
        // }
        // Event::SearchBackspace => {
        //     app.search_remove_char()?;
        // }
        Event::PaneSizeQuery(callback) => {
            // If caller has already hung up do nothing
            callback
                .send(PaneSize {
                    rows: app.size.pane_rows(),
                    cols: app.size.pane_cols(),
                })
                .ok();
        }
        _ => {
            return Err(anyhow::anyhow!("error"))
        }
    }
    Ok(None)
}

fn view<W>(app: &mut TuiApp<W>, f: &mut Frame) {
    let cols = app.size.pane_cols();
    let horizontal = if app.has_sidebar {
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(cols)])
    } else {
        Layout::horizontal([Constraint::Max(0), Constraint::Length(cols)])
    };
    let [table, pane] = horizontal.areas(f.size());

    let active_task = app.active_task().unwrap();
    let pane_to_render: TerminalPane<W> = TerminalPane::new(&active_task, &active_task.name, &app.focus, app.has_sidebar);
    let table_to_render = TaskTable::new(&app.task_statuses);

    f.render_widget(&pane_to_render, pane);
    f.render_stateful_widget(&table_to_render, table, &mut app.scroll);
}
