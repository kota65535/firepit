mod clipboard;
mod dialog;
mod hyperlink;
mod input;
mod lib;
mod pane;
mod search;
mod size;
mod table;
mod task;
mod term_output;

use crate::app::command::AppCommandChannel;
use crate::app::command::{AppCommand, TaskResult};
use crate::app::command::{Direction, PaneSize, ScrollSize, TaskRun, TaskStatus};
use crate::app::signal::SignalHandler;
use crate::app::tui::clipboard::copy_to_clipboard;
use crate::app::tui::dialog::{
    help_dialog_size, render_help_dialog, render_toast, COPIED_TXT, FORCE_QUIT_TXT, QUIT_TXT,
};
use crate::app::tui::input::{InputHandler, InputOptions};
use crate::app::tui::pane::{TerminalPane, TerminalScroll};
use crate::app::tui::search::{Match, SearchResults};
use crate::app::tui::size::SizeInfo;
use crate::app::tui::table::TaskTable;
use crate::app::tui::task::Task;
use crate::app::tui::term_output::TerminalOutput;
use crate::app::{DOUBLE_CLICK_DURATION, FRAME_RATE};
use crate::runner::command::RunnerCommandChannel;
use crate::tokio_spawn;
use anyhow::Context;
use chrono::{DateTime, Local};
use indexmap::IndexMap;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    widgets::{ScrollbarState, TableState},
    Frame, Terminal,
};
use std::collections::HashMap;
use std::io::{self, Stdout, Write};
use tokio::sync::broadcast::error::RecvError;
use tokio::{sync::mpsc, time::Instant};
use tracing::{debug, error, info};
use unicode_width::UnicodeWidthStr;

/// How long a transient toast (e.g. "Copied to clipboard") stays visible.
const TOAST_DURATION: std::time::Duration = std::time::Duration::from_millis(1500);

/// A short message shown in a box at the bottom of the screen.
/// `expires_at: None` keeps the toast visible until it is replaced.
#[derive(Debug, Clone)]
pub struct Toast {
    message: String,
    expires_at: Option<Instant>,
    /// Clear the log selection together when this toast disappears.
    clear_selection_on_expire: bool,
}

impl Toast {
    fn copied() -> Self {
        Self {
            message: COPIED_TXT.to_string(),
            expires_at: Some(Instant::now() + TOAST_DURATION),
            clear_selection_on_expire: true,
        }
    }

    fn persistent(message: &str) -> Self {
        Self {
            message: message.to_string(),
            expires_at: None,
            clear_selection_on_expire: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum LayoutSections {
    Pane,
    TaskList(Option<SearchResults>),
    Search { query: String },
    Help { scroll: usize, max_scroll: usize },
}

pub struct TuiApp {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    crossterm_rx: mpsc::Receiver<crossterm::event::Event>,
    command_tx: AppCommandChannel,
    command_rx: mpsc::UnboundedReceiver<AppCommand>,
    input_handler: InputHandler,
    signal_handler: SignalHandler,
    state: TuiAppState,
}

pub struct TuiAppState {
    size: SizeInfo,
    tasks: IndexMap<String, Task>,
    focus: LayoutSections,
    table: TableState,
    scrollbar: ScrollbarState,
    selected_task_index: usize,
    has_sidebar: bool,
    quitting: bool,
    force_quitting: bool,
    done: bool,
    /// Cached URL spans for the current active task's visible output.
    detected_urls: Vec<hyperlink::UrlSpan>,
    /// Index into `detected_urls` of the currently hovered URL, if any.
    hovered_url_index: Option<usize>,
    /// Message shown at the bottom of the screen, if any.
    toast: Option<Toast>,
    /// When to copy a multi-click selection. Waiting out the multi-click
    /// window keeps the double-click stage of a triple-click from copying.
    pending_copy_at: Option<Instant>,
}

impl TuiApp {
    pub fn new(
        target_tasks: &[String],
        dep_tasks: &[String],
        finalizer_tasks: &[String],
        labels: &HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        let terminal = Self::setup_terminal()?;
        let input_handler = InputHandler::new();
        let crossterm_rx = input_handler.start();
        let (command_tx, command_rx) = AppCommandChannel::new();
        let signal_handler = SignalHandler::infer()?;

        let rect = terminal.size()?;
        let size = SizeInfo::new(
            rect.height,
            rect.width,
            target_tasks
                .iter()
                .chain(dep_tasks.iter())
                .map(|s| labels.get(s).unwrap_or(s).as_str()),
        );

        debug!("Terminal size: height={} width={}", rect.height, rect.width);

        let has_sidebar = true;
        let output_raws = size.pane_rows();
        let output_cols = size.output_cols(has_sidebar);
        let tasks = target_tasks
            .iter()
            .map(|t| (t, true))
            .chain(dep_tasks.iter().map(|t| (t, false)))
            .map(|(t, b)| {
                let mut task = Task::new(
                    t,
                    b,
                    TerminalOutput::new(output_raws, output_cols, None),
                    labels.get(t).map(|t| t.as_str()),
                );
                task.is_finalizer = finalizer_tasks.contains(t);
                (t.clone(), task)
            })
            .collect::<IndexMap<_, _>>();

        let selected_task_index = 0;

        Ok(Self {
            terminal,
            crossterm_rx,
            command_tx,
            command_rx,
            input_handler,
            signal_handler,
            state: TuiAppState {
                size,
                tasks,
                focus: LayoutSections::TaskList(None),
                table: TableState::default().with_selected(selected_task_index),
                scrollbar: ScrollbarState::default(),
                selected_task_index,
                has_sidebar,
                quitting: false,
                force_quitting: false,
                done: false,
                detected_urls: Vec::new(),
                hovered_url_index: None,
                toast: None,
                pending_copy_at: None,
            },
        })
    }

    fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
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

    pub fn command_tx(&self) -> AppCommandChannel {
        self.command_tx.clone()
    }

    pub async fn run(&mut self, runner_tx: &RunnerCommandChannel) -> anyhow::Result<i32> {
        // Translate every signal into a quit command, exactly like a Ctrl-C key
        // press. The app forwards each one to the runner, which turns a repeated
        // quit into a forced kill.
        let mut signals = self.signal_handler.subscribe();
        let command_tx = self.command_tx.clone();
        tokio_spawn!("app-canceller", async move {
            // A lagged receiver still means signals arrived, so treat it the same.
            while let Ok(_) | Err(RecvError::Lagged(_)) = signals.recv().await {
                command_tx.quit().await;
            }
        });

        let ret = self.run_inner(runner_tx).await;
        self.cleanup()?;

        if let Err(err) = ret {
            error!("Error: {}", err);
            // `run_inner` has returned early without stopping the runner.
            runner_tx.quit();
            return Err(err);
        }

        info!("App is exiting");
        Ok(0)
    }

    pub async fn run_inner(&mut self, runner_tx: &RunnerCommandChannel) -> anyhow::Result<()> {
        self.terminal.draw(|f| self.state.view(f))?;

        let mut last_render = Instant::now();
        let mut needs_rerender = true;
        while let Some(event) = self.poll().await? {
            // For non-tick events, always set needs_rerender to true
            if !matches!(event, AppCommand::Tick | AppCommand::HoverPane { .. }) {
                needs_rerender = true;
            }
            if matches!(event, AppCommand::Resize { .. }) {
                self.terminal.autoresize()?;
            }
            let hover_changed = matches!(event, AppCommand::HoverPane { .. });
            self.state.update(event, runner_tx)?;
            if self.state.done {
                break;
            }
            if hover_changed {
                // Hover changes need immediate re-render for responsive feedback
                needs_rerender = true;
            }
            if self.state.flush_pending_copy()? {
                needs_rerender = true;
            }
            if self.state.expire_toast() {
                needs_rerender = true;
            }
            if FRAME_RATE <= last_render.elapsed() && needs_rerender {
                self.terminal.draw(|f| self.state.view(f))?;
                last_render = Instant::now();
                needs_rerender = false;
            }
        }

        Ok(())
    }

    /// Blocking poll for events, will only return None if app handle has been
    /// dropped
    async fn poll(&mut self) -> anyhow::Result<Option<AppCommand>> {
        let input_closed = self.crossterm_rx.is_closed();

        if input_closed {
            Ok(self.command_rx.recv().await)
        } else {
            let mut event = None;
            loop {
                tokio::select! {
                    e = self.crossterm_rx.recv() => {
                        if let Some(e) = e {
                            let options = self.state.input_options()?;
                            event = self.input_handler.handle(e, options);
                        }
                    }
                    e = self.command_rx.recv() => {
                        event = e;
                    }
                }
                if event.is_some() {
                    break;
                }
            }
            Ok(event)
        }
    }

    fn cleanup(&mut self) -> anyhow::Result<()> {
        self.terminal.clear()?;
        crossterm::execute!(
            self.terminal.backend_mut(),
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen
        )?;
        self.state.persist_tasks()?;
        crossterm::terminal::disable_raw_mode()?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl TuiAppState {
    pub fn active_task(&self) -> anyhow::Result<&Task> {
        self.nth_task(self.selected_task_index)
    }

    pub fn active_task_mut(&mut self) -> anyhow::Result<&mut Task> {
        self.nth_task_mut(self.selected_task_index)
    }

    pub fn task(&self, name: &str) -> anyhow::Result<&Task> {
        self.tasks
            .get(name)
            .with_context(|| format!("task {:?} not found", name))
    }

    pub fn task_mut(&mut self, name: &str) -> anyhow::Result<&mut Task> {
        self.tasks
            .get_mut(name)
            .with_context(|| format!("task {:?} not found", name))
    }

    fn input_options(&self) -> anyhow::Result<InputOptions<'_>> {
        let task = self.active_task()?;
        Ok(InputOptions {
            focus: &self.focus,
            has_selection: task.output.has_selection(),
            task: task.name.clone(),
            has_sidebar: self.has_sidebar,
            sidebar_width: self.size.task_list_width(),
            pane_rows: self.size.pane_rows(),
        })
    }

    pub fn nth_task(&self, num: usize) -> anyhow::Result<&Task> {
        self.tasks
            .iter()
            .nth(num)
            .map(|e| e.1)
            .with_context(|| anyhow::anyhow!("{}th task not found", num))
    }

    pub fn nth_task_mut(&mut self, num: usize) -> anyhow::Result<&mut Task> {
        self.tasks
            .iter_mut()
            .nth(num)
            .map(|e| e.1)
            .with_context(|| anyhow::anyhow!("{}th task not found", num))
    }

    pub fn select_next_task(&mut self) {
        let num_rows = self.tasks.len();
        let next_index = (self.selected_task_index + 1).clamp(0, num_rows - 1);
        self.selected_task_index = next_index;
        self.table.select(Some(next_index));
    }

    pub fn select_previous_task(&mut self) {
        let i = match self.selected_task_index {
            0 => 0,
            i => i - 1,
        };
        self.selected_task_index = i;
        self.table.select(Some(i));
    }

    pub fn select_task(&mut self, index: usize) {
        let num_rows = self.tasks.len();
        if index >= num_rows {
            return;
        }
        self.selected_task_index = index;
        self.table.select(Some(index));
    }

    pub fn scroll_terminal_output(&mut self, direction: Direction, stride: usize) -> anyhow::Result<()> {
        let (scroll_current, scroll_len) = self.active_task_mut()?.output.scroll(direction, stride)?;
        self.scrollbar = self.scrollbar.position(scroll_len.saturating_sub(scroll_current));
        Ok(())
    }

    pub fn scroll_to_row(&mut self, row: u16) -> anyhow::Result<()> {
        self.active_task_mut()?.output.scroll_to(row);
        Ok(())
    }

    pub fn task_names(&self) -> Vec<String> {
        self.tasks.iter().map(|t| t.0.clone()).collect()
    }

    fn set_status(&mut self, task: &str, status: TaskStatus) -> anyhow::Result<()> {
        self.task_mut(task)?.set_status(status);
        Ok(())
    }

    pub fn plan_task(&mut self, task: &str) -> anyhow::Result<()> {
        self.set_status(task, TaskStatus::Planned)
    }

    pub fn start_task(
        &mut self,
        task: &str,
        pid: u32,
        restart: u64,
        max_restart: Option<u64>,
        reload: u64,
        datetime: DateTime<Local>,
    ) -> anyhow::Result<()> {
        self.set_status(
            task,
            TaskStatus::Running(TaskRun {
                pid,
                restart,
                max_restart,
                reload,
                start_time: datetime,
            }),
        )
    }

    pub fn ready_task(&mut self, task: &str) -> anyhow::Result<()> {
        self.set_status(task, TaskStatus::Ready)
    }

    pub fn finish_task(
        &mut self,
        task: &str,
        result: TaskResult,
        datetime: Option<DateTime<Local>>,
    ) -> anyhow::Result<()> {
        self.set_status(task, TaskStatus::Finished(result, datetime))?;
        // A finished task has no stdin, so staying in interaction mode would leave
        // the user typing into a dead shell. Reloading tasks are exempt since they
        // are restarted right away and their stdin comes back.
        if !matches!(result, TaskResult::Reloading) && self.is_interacting_with(task)? {
            self.exit_interaction();
        }
        Ok(())
    }

    pub fn has_stdin(&self) -> anyhow::Result<bool> {
        let task = self.active_task()?;
        Ok(task.output.stdin().is_some())
    }

    pub fn is_interacting_with(&self, task: &str) -> anyhow::Result<bool> {
        Ok(matches!(self.focus, LayoutSections::Pane) && self.active_task()?.name == task)
    }

    pub fn enter_interaction(&mut self) -> anyhow::Result<()> {
        if self.has_stdin()? {
            self.focus = LayoutSections::Pane;
        }
        Ok(())
    }

    pub fn exit_interaction(&mut self) {
        if matches!(self.focus, LayoutSections::Pane) {
            self.focus = LayoutSections::TaskList(None);
        }
    }

    pub fn persist_tasks(&mut self) -> anyhow::Result<()> {
        for t in self.tasks.values().rev().filter(|t| {
            matches!(
                t.status(),
                TaskStatus::Running(_) | TaskStatus::Ready | TaskStatus::Finished(_, _)
            )
        }) {
            t.persist_screen()?
        }
        Ok(())
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        debug!("Terminal size: height={} width={}", rows, cols);
        self.size.resize(rows, cols);
        let output_rows = self.size.pane_rows();
        let output_cols = self.size.output_cols(self.has_sidebar);
        self.tasks.values_mut().for_each(|task| {
            task.output.resize(output_rows, output_cols);
        })
    }

    pub fn view(&mut self, f: &mut Frame) {
        let cols = self.size.pane_cols(self.has_sidebar);
        let horizontal = if self.has_sidebar {
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(cols)])
        } else {
            Layout::horizontal([Constraint::Max(0), Constraint::Length(cols)])
        };
        let [table, pane] = horizontal.areas(f.size());

        // Update cached URLs for hover/click detection.
        // Separate borrow scope: detect_urls returns owned data, so the
        // immutable borrow of self via active_task() ends before assignment.
        let new_urls = match self.active_task() {
            Ok(task) => hyperlink::detect_urls(task.output.screen()),
            Err(e) => {
                error!("Error on rendering: {}", e);
                return;
            }
        };
        self.detected_urls = new_urls;

        let active_task = match self.active_task() {
            Ok(task) => task,
            Err(e) => {
                error!("Error on rendering: {}", e);
                return;
            }
        };

        let content_length = active_task.output.screen().current_scrollback_len();
        let scrollback = active_task.output.screen().scrollback();

        // Get hovered URL segments for visual overlay
        let hovered_segments = self
            .hovered_url_index
            .and_then(|idx| self.detected_urls.get(idx))
            .map(|span| span.segments.as_slice());

        // Render pane
        let pane_to_render = TerminalPane::new(active_task, &self.focus, self.has_sidebar, hovered_segments);
        f.render_widget(&pane_to_render, pane);

        // Render pane scrollbar
        self.scrollbar = self.scrollbar.content_length(content_length);
        self.scrollbar = self.scrollbar.position(content_length.saturating_sub(scrollback));
        let scrollbar_to_render = TerminalScroll::new(&self.focus);
        f.render_stateful_widget(scrollbar_to_render, pane, &mut self.scrollbar);

        // Render task list
        let table_to_render = TaskTable::new(&self.tasks, &self.focus);
        f.render_stateful_widget(&table_to_render, table, &mut self.table);

        // Render help dialog
        if let LayoutSections::Help { scroll, max_scroll: _ } = self.focus {
            render_help_dialog(f, scroll);
        }

        // Render toast (copy notification, quit message, ...) above everything
        if let Some(toast) = &self.toast {
            render_toast(f, &toast.message);
        }
    }

    /// Insert a stdin to be associated with a task
    pub fn insert_stdin(&mut self, task: &str, stdin: Option<Box<dyn Write + Send>>) -> anyhow::Result<()> {
        let task = self
            .tasks
            .get_mut(task)
            .with_context(|| format!("task {:?} not found", task))?;
        task.output.set_stdin(stdin);
        Ok(())
    }

    pub fn forward_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if matches!(self.focus, LayoutSections::Pane) {
            let task = self.active_task_mut()?;
            // Jump back to the live output before forwarding the input,
            // otherwise the user types into a view they cannot see.
            task.output.scroll_to_bottom();
            if let Some(stdin) = task.output.stdin_mut() {
                stdin
                    .write_all(bytes)
                    .with_context(|| format!("task {} failed to forward input", task.name))?;
            }
            Ok(())
        } else {
            Ok(())
        }
    }

    pub fn process_output(&mut self, task: &str, output: &[u8]) -> anyhow::Result<()> {
        let task = self.task_mut(task)?;
        task.output.process(output);
        Ok(())
    }

    fn scroll_size(&self, size: ScrollSize) -> usize {
        let s = match size {
            ScrollSize::One => 1,
            ScrollSize::Half => self.size.pane_rows() / 2,
            ScrollSize::Full => self.size.pane_rows(),
            ScrollSize::Edge => 0,
        };
        usize::from(s)
    }

    pub fn copy_selection(&mut self) -> anyhow::Result<()> {
        self.pending_copy_at = None;
        let task = self.active_task()?;
        let Some(text) = task.output.copy_selection() else {
            return Ok(());
        };
        copy_to_clipboard(&text);
        self.toast = Some(Toast::copied());
        Ok(())
    }

    /// Schedule a copy of the current selection for when the multi-click
    /// window has passed, so a further click can still upgrade (and cancel)
    /// it instead of copying twice.
    pub fn defer_copy_selection(&mut self) {
        self.pending_copy_at = Some(Instant::now() + DOUBLE_CLICK_DURATION);
    }

    /// Copy the selection once its scheduled time has arrived.
    /// Returns true when the copy ran and a re-render is needed.
    pub fn flush_pending_copy(&mut self) -> anyhow::Result<bool> {
        match self.pending_copy_at {
            Some(at) if at <= Instant::now() => {
                self.copy_selection()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Drop the toast if its display time has elapsed, clearing the log
    /// selection together when the toast asks for it (copy toast).
    /// Returns true when the toast was removed and a re-render is needed.
    pub fn expire_toast(&mut self) -> bool {
        match &self.toast {
            Some(Toast {
                expires_at: Some(expires_at),
                clear_selection_on_expire,
                ..
            }) if *expires_at <= Instant::now() => {
                if *clear_selection_on_expire {
                    // Failing to resolve the active task must not keep the
                    // expired toast on screen
                    self.clear_selection().ok();
                }
                self.toast = None;
                true
            }
            _ => false,
        }
    }

    /// Drop a pending copy toast and any scheduled copy when the user starts
    /// a new selection, so neither can fire against the selection being made.
    fn cancel_copy_feedback(&mut self) {
        self.pending_copy_at = None;
        if matches!(
            self.toast,
            Some(Toast {
                clear_selection_on_expire: true,
                ..
            })
        ) {
            self.toast = None;
        }
    }

    /// Update the hovered URL index from pane-relative mouse coordinates.
    /// pane_row/pane_col from input.rs are already vt100 visible row/col
    /// (same coordinates used by line_selection/update_selection).
    fn update_hover(&mut self, pane_row: u16, pane_col: u16) {
        self.hovered_url_index = hyperlink::find_url_at(&self.detected_urls, pane_row, pane_col);
    }

    fn open_url(&self, url: &str) {
        debug!("Opening URL: {}", url);
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(url).spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        }
    }

    pub fn clear_selection(&mut self) -> anyhow::Result<()> {
        let task = self.active_task_mut()?;
        task.output.clear_selection();
        Ok(())
    }

    pub fn update_selection(&mut self, rows: u16, cols: u16, edge: Option<Direction>) -> anyhow::Result<()> {
        if let Some(direction) = edge {
            // Scroll the terminal when dragging selection beyond the visible viewport.
            self.scroll_terminal_output(direction, 1)?;
        }
        let task = self.active_task_mut()?;
        task.output.update_selection(rows, cols);
        Ok(())
    }

    pub fn word_selection(&mut self, rows: u16, cols: u16) -> anyhow::Result<()> {
        let task = self.active_task_mut()?;
        task.output.word_selection(rows, cols);
        Ok(())
    }

    pub fn line_selection(&mut self, rows: u16) -> anyhow::Result<()> {
        let task = self.active_task_mut()?;
        task.output.line_selection(rows);
        Ok(())
    }

    pub fn enter_search(&mut self) -> anyhow::Result<()> {
        self.remove_search_highlight()?;
        self.focus = LayoutSections::Search { query: "".to_string() };
        Ok(())
    }

    pub fn remove_search_highlight(&mut self) -> anyhow::Result<()> {
        let LayoutSections::TaskList(Some(results)) = &mut self.focus else {
            return Ok(());
        };
        let results = results.clone();
        let query_len = results.query.width();
        let task = self.active_task_mut()?;
        if task.name != results.task {
            return Ok(());
        }
        if let Some(Match(row, col)) = results.current() {
            self.highlight_cell(row, col, query_len as u16, false)?;
        }
        Ok(())
    }

    pub fn run_search(&mut self) -> anyhow::Result<()> {
        let LayoutSections::Search { query, .. } = &mut self.focus else {
            return Ok(());
        };
        if query.is_empty() {
            return Ok(());
        }

        let query = query.clone();
        let task = self.active_task_mut()?;
        let screen = task.output.screen_mut();
        let size = screen.size();

        let mut matches = Vec::new();
        let mut line_buf = String::new();
        let mut previous_row_widths = Vec::new();
        for (row_idx, row) in screen.grid_mut().all_rows_mut().enumerate() {
            let mut s = String::new();
            row.write_contents(&mut s, 0, size.1, true);
            let current_row_width = s.width();
            line_buf.push_str(&s);
            if row.wrapped() {
                previous_row_widths.push(current_row_width);
                continue;
            }
            for (offset, _) in line_buf.match_indices(&query) {
                // Convert byte offset to display width to handle wide chars properly
                let mut col_idx = line_buf[..offset].width();
                if previous_row_widths.is_empty() {
                    matches.push(Match(row_idx as u16, col_idx as u16));
                } else {
                    // The line is wrapped
                    // Reset the current row index to the first line
                    let first_row_idx = row_idx - previous_row_widths.len();
                    for (row_idx, width) in
                        (first_row_idx..).zip(previous_row_widths.iter().chain(std::iter::once(&current_row_width)))
                    {
                        if col_idx < *width {
                            // The match exists in this line
                            matches.push(Match(row_idx as u16, col_idx as u16));
                            break;
                        }
                        // The match may be in the next line
                        col_idx -= *width;
                    }
                }
            }
            previous_row_widths.clear();
            line_buf.clear();
        }

        let query_len = query.width();

        // Find the initial search result index
        let offset = screen.current_scrollback_len() - screen.scrollback();
        let mut index = 0;
        for (i, m) in matches.iter().enumerate() {
            index = i;
            if offset <= (m.0 as usize) {
                break;
            }
        }

        let search_results = SearchResults::new(&task.name, query, matches, index)?;

        if let Some(Match(row, col)) = search_results.current() {
            self.highlight_cell(row, col, query_len as u16, true)?;
            self.scroll_to_row(row)?;
        }

        self.focus = LayoutSections::TaskList(Some(search_results));
        Ok(())
    }

    fn highlight_cell(
        &mut self,
        mut num_row: u16,
        mut num_col: u16,
        length: u16,
        highlight: bool,
    ) -> anyhow::Result<()> {
        let task = self.active_task_mut()?;
        let screen = task.output.screen_mut();
        // Rest of chars to highlight
        let mut rest = length;
        while rest > 0 {
            // Stop if no rows left
            let Some(row) = screen.grid_mut().all_rows_mut().nth(num_row as usize) else {
                break;
            };
            for idx in num_col..num_col + length {
                if rest == 0 {
                    break;
                }
                // If no column left, go to next line
                let Some(c) = row.get_mut(idx) else { break };

                c.attrs_mut().bgcolor = if highlight {
                    vt100::Color::Idx(3) // Yellow
                } else {
                    vt100::Color::Default
                };
                rest -= 1;
            }
            num_row += 1;
            num_col = 0;
        }
        Ok(())
    }

    pub fn next_search_result(&mut self) -> anyhow::Result<()> {
        let LayoutSections::TaskList(Some(results)) = &mut self.focus else {
            return Ok(());
        };
        let mut results = results.clone();
        let query_len = results.query.width();

        self.remove_search_highlight()?;

        if let Some(Match(row, col)) = results.next() {
            self.highlight_cell(row, col, query_len as u16, true)?;
            self.scroll_to_row(row)?;
        }

        self.focus = LayoutSections::TaskList(Some(results));

        Ok(())
    }

    pub fn previous_search_result(&mut self) -> anyhow::Result<()> {
        let LayoutSections::TaskList(Some(results)) = &mut self.focus else {
            return Ok(());
        };
        let mut results = results.clone();
        let query_len = results.query.width();

        self.remove_search_highlight()?;

        if let Some(Match(row, col)) = results.previous() {
            self.highlight_cell(row, col, query_len as u16, true)?;
            self.scroll_to_row(row)?;
        }

        self.focus = LayoutSections::TaskList(Some(results));

        Ok(())
    }

    pub fn exit_search(&mut self) -> anyhow::Result<()> {
        if let LayoutSections::TaskList(results) = &mut self.focus {
            let Some(mut results) = results.clone() else {
                return Ok(());
            };
            let task = self.active_task_mut()?;
            if task.name != results.task {
                return Ok(());
            }
            self.remove_search_highlight()?;
            results.reset();
        };

        self.focus = LayoutSections::TaskList(None);

        Ok(())
    }

    pub fn scroll_help_up(&mut self) {
        if let LayoutSections::Help { scroll, max_scroll } = &mut self.focus {
            self.focus = LayoutSections::Help {
                max_scroll: *max_scroll,
                scroll: scroll.saturating_sub(1),
            }
        }
    }

    pub fn scroll_help_down(&mut self) {
        if let LayoutSections::Help { scroll, max_scroll } = &mut self.focus {
            self.focus = LayoutSections::Help {
                max_scroll: *max_scroll,
                scroll: scroll.saturating_add(1).min(*max_scroll),
            }
        }
    }

    pub fn search_input_char(&mut self, c: char) -> anyhow::Result<()> {
        let LayoutSections::Search { query, .. } = &mut self.focus else {
            debug!("Modifying search query while not searching");
            return Ok(());
        };
        query.push(c);
        Ok(())
    }

    pub fn search_remove_char(&mut self) -> anyhow::Result<()> {
        let LayoutSections::Search { query, .. } = &mut self.focus else {
            debug!("Modified search query while not searching");
            return Ok(());
        };
        if query.pop().is_none() {
            self.exit_search()?;
        }
        Ok(())
    }

    fn update(&mut self, event: AppCommand, runner_tx: &RunnerCommandChannel) -> anyhow::Result<()> {
        match event {
            AppCommand::PlanTask { task } => {
                self.plan_task(&task)?;
            }
            AppCommand::StartTask {
                task,
                pid,
                restart,
                max_restart,
                reload,
                datetime,
            } => {
                self.start_task(&task, pid, restart, max_restart, reload, datetime)?;
            }
            AppCommand::TaskOutput { task, output } => {
                self.process_output(&task, &output)?;
            }
            AppCommand::ReadyTask { task } => {
                self.ready_task(&task)?;
            }
            AppCommand::FinishTask { task, result, datetime } => {
                self.finish_task(&task, result, datetime)?;
                self.insert_stdin(&task, None)?;
            }
            AppCommand::SetStdin { task, stdin } => {
                self.insert_stdin(&task, Some(stdin))?;
            }
            AppCommand::PaneSizeQuery(callback) => {
                // If caller has already hung up do nothing
                callback
                    .send(PaneSize {
                        rows: self.size.pane_rows(),
                        cols: self.size.output_cols(self.has_sidebar),
                    })
                    .ok();
            }
            AppCommand::Done => {
                self.done = true;
                runner_tx.quit();
            }
            AppCommand::Quit => {
                if self.quitting {
                    self.force_quitting = true;
                }
                self.quitting = true;
                // Quit messages stay visible until the app exits.
                self.toast = Some(Toast::persistent(if self.force_quitting {
                    FORCE_QUIT_TXT
                } else {
                    QUIT_TXT
                }));
                runner_tx.quit();
            }
            AppCommand::OpenHelp => {
                let (rect, _content_width, content_height) = help_dialog_size(self.size.cols(), self.size.rows());
                self.focus = LayoutSections::Help {
                    scroll: 0,
                    max_scroll: content_height.saturating_sub(rect.height.saturating_sub(2) as usize),
                };
            }
            AppCommand::ExitHelp => {
                self.focus = LayoutSections::TaskList(None);
            }
            AppCommand::StopTask { task } => {
                runner_tx.stop_task(&task);
            }
            AppCommand::RestartTask { task, force } => {
                runner_tx.restart_task(&task, force);
            }
            AppCommand::Tick => {}
            AppCommand::Redraw => {}
            AppCommand::Up => {
                self.exit_search()?;
                self.select_previous_task();
            }
            AppCommand::Down => {
                self.exit_search()?;
                self.select_next_task();
            }
            AppCommand::Select { index } => {
                self.exit_search()?;
                self.select_task(index + self.table.offset());
            }
            AppCommand::ScrollUp(size) => {
                self.scroll_terminal_output(Direction::Up, self.scroll_size(size))?;
                self.scroll_help_up();
            }
            AppCommand::ScrollDown(size) => {
                self.scroll_terminal_output(Direction::Down, self.scroll_size(size))?;
                self.scroll_help_down();
            }
            AppCommand::ToggleSidebar => {
                self.has_sidebar = !self.has_sidebar;
                self.resize(self.size.rows(), self.size.cols());
            }
            AppCommand::EnterInteractive => {
                self.enter_interaction()?;
            }
            AppCommand::ExitInteractive => {
                self.exit_interaction();
            }
            AppCommand::Input { bytes } => {
                self.forward_input(&bytes)?;
            }
            AppCommand::HoverPane { row, col } => {
                self.update_hover(row, col);
            }
            AppCommand::ClickPane { row, col } => {
                self.update_hover(row, col);
                if let Some(idx) = self.hovered_url_index {
                    if let Some(url_span) = self.detected_urls.get(idx) {
                        let url = url_span.url.clone();
                        self.open_url(&url);
                    }
                }
            }
            AppCommand::OpenUrl { url } => {
                self.open_url(&url);
            }
            AppCommand::ClearSelection => {
                self.cancel_copy_feedback();
                self.clear_selection()?;
            }
            AppCommand::WordSelection { rows, cols } => {
                self.cancel_copy_feedback();
                self.word_selection(rows, cols)?;
            }
            AppCommand::LineSelection { rows } => {
                self.cancel_copy_feedback();
                self.line_selection(rows)?;
            }
            AppCommand::UpdateSelection { rows, cols, edge } => {
                self.cancel_copy_feedback();
                self.update_selection(rows, cols, edge)?;
            }
            AppCommand::CopySelection => {
                self.copy_selection()?;
            }
            AppCommand::DeferCopySelection => {
                self.defer_copy_selection();
            }
            AppCommand::Resize { rows, cols } => {
                self.resize(rows, cols);
            }
            AppCommand::EnterSearch => {
                self.enter_search()?;
            }
            AppCommand::SearchInputChar(c) => {
                self.search_input_char(c)?;
            }
            AppCommand::SearchBackspace => {
                self.search_remove_char()?;
            }
            AppCommand::SearchRun => {
                self.run_search()?;
            }
            AppCommand::SearchNext => {
                self.next_search_result()?;
            }
            AppCommand::SearchPrevious => {
                self.previous_search_result()?;
            }
            AppCommand::ExitSearch => {
                self.exit_search()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(tasks: &[&str]) -> TuiAppState {
        let size = SizeInfo::new(24, 80, tasks.iter().copied());
        let tasks = tasks
            .iter()
            .map(|t| {
                (
                    t.to_string(),
                    Task::new(t, true, TerminalOutput::new(10, 40, None), None),
                )
            })
            .collect::<IndexMap<_, _>>();
        TuiAppState {
            size,
            tasks,
            focus: LayoutSections::Pane,
            table: TableState::default().with_selected(0),
            scrollbar: ScrollbarState::default(),
            selected_task_index: 0,
            has_sidebar: true,
            quitting: false,
            force_quitting: false,
            done: false,
            detected_urls: Vec::new(),
            hovered_url_index: None,
            toast: None,
            pending_copy_at: None,
        }
    }

    #[test]
    fn exits_interaction_when_active_task_finishes() {
        let mut state = state(&["a", "b"]);

        state.finish_task("a", TaskResult::Success, None).unwrap();

        assert!(matches!(state.focus, LayoutSections::TaskList(None)));
    }

    #[test]
    fn keeps_interaction_when_another_task_finishes() {
        let mut state = state(&["a", "b"]);

        state.finish_task("b", TaskResult::Success, None).unwrap();

        assert!(matches!(state.focus, LayoutSections::Pane));
    }

    #[test]
    fn keeps_interaction_while_active_task_is_reloading() {
        let mut state = state(&["a", "b"]);

        state.finish_task("a", TaskResult::Reloading, None).unwrap();

        assert!(matches!(state.focus, LayoutSections::Pane));
    }

    #[test]
    fn keeps_task_list_focus_when_not_interacting() {
        let mut state = state(&["a", "b"]);
        state.focus = LayoutSections::TaskList(None);

        state.finish_task("a", TaskResult::Success, None).unwrap();

        assert!(matches!(state.focus, LayoutSections::TaskList(None)));
    }

    #[test]
    fn enters_interaction_only_when_task_has_stdin() {
        let mut state = state(&["a", "b"]);
        state.focus = LayoutSections::TaskList(None);

        state.enter_interaction().unwrap();
        assert!(matches!(state.focus, LayoutSections::TaskList(None)));

        state.insert_stdin("a", Some(Box::new(Vec::new()))).unwrap();
        state.enter_interaction().unwrap();
        assert!(matches!(state.focus, LayoutSections::Pane));
    }

    /// A copy toast whose display time has already passed.
    fn expired_copy_toast() -> Toast {
        Toast {
            expires_at: Some(Instant::now()),
            ..Toast::copied()
        }
    }

    #[test]
    fn expires_copy_toast_after_duration() {
        let mut state = state(&["a"]);

        // A freshly created copy toast is not expired yet
        state.toast = Some(Toast::copied());
        assert!(!state.expire_toast());
        assert!(state.toast.is_some());

        // A toast whose deadline has passed is removed
        state.toast = Some(expired_copy_toast());
        assert!(state.expire_toast());
        assert!(state.toast.is_none());
    }

    #[test]
    fn clears_selection_when_copy_toast_expires() {
        let mut state = state(&["a"]);
        let task = state.active_task_mut().unwrap();
        task.output.process(b"hello world\r\n");
        task.output.line_selection(0);
        assert!(state.active_task().unwrap().output.has_selection());

        state.toast = Some(expired_copy_toast());
        assert!(state.expire_toast());
        assert!(!state.active_task().unwrap().output.has_selection());
    }

    #[test]
    fn cancels_copy_toast_when_new_selection_starts() {
        let mut state = state(&["a"]);

        state.toast = Some(Toast::copied());
        state.cancel_copy_feedback();
        assert!(state.toast.is_none());

        // Persistent toasts (quit message) are not affected
        state.toast = Some(Toast::persistent(QUIT_TXT));
        state.cancel_copy_feedback();
        assert!(state.toast.is_some());
    }

    #[test]
    fn keeps_persistent_toast() {
        let mut state = state(&["a"]);

        state.toast = Some(Toast::persistent(QUIT_TXT));
        assert!(!state.expire_toast());
        assert!(state.toast.is_some());
    }

    #[test]
    fn deferred_copy_waits_for_multi_click_window() {
        let mut state = state(&["a"]);

        // A freshly deferred copy does not run yet
        state.defer_copy_selection();
        assert!(!state.flush_pending_copy().unwrap());
        assert!(state.pending_copy_at.is_some());

        // Once the deadline has passed, the copy runs and the schedule clears
        state.pending_copy_at = Some(Instant::now());
        assert!(state.flush_pending_copy().unwrap());
        assert!(state.pending_copy_at.is_none());
    }

    #[test]
    fn cancels_deferred_copy_when_new_selection_starts() {
        let mut state = state(&["a"]);

        state.defer_copy_selection();
        state.cancel_copy_feedback();
        assert!(state.pending_copy_at.is_none());
        assert!(!state.flush_pending_copy().unwrap());
    }
}
