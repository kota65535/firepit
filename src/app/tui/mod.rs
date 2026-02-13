mod clipboard;
mod dialog;
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
use crate::app::tui::dialog::{help_dialog_size, render_help_dialog, render_quit_dialog};
use crate::app::tui::input::{InputHandler, InputOptions};
use crate::app::tui::pane::{TerminalPane, TerminalScroll};
use crate::app::tui::search::{Match, SearchResults};
use crate::app::tui::size::SizeInfo;
use crate::app::tui::table::TaskTable;
use crate::app::tui::task::Task;
use crate::app::tui::term_output::TerminalOutput;
use crate::app::FRAME_RATE;
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
use tokio::{sync::mpsc, time::Instant};
use tracing::{debug, error, info};
use unicode_width::UnicodeWidthStr;

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
}

impl TuiApp {
    pub fn new(
        target_tasks: &Vec<String>,
        dep_tasks: &Vec<String>,
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
                (
                    t.clone(),
                    Task::new(
                        t,
                        b,
                        TerminalOutput::new(output_raws, output_cols, None),
                        labels.get(t).map(|t| t.as_str()),
                    ),
                )
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
        let signal_handler = self.signal_handler.clone();
        let command_tx = self.command_tx.clone();
        tokio_spawn!("app-canceller", async move {
            let subscriber = signal_handler.subscribe();
            if let Some(subscriber) = subscriber {
                let _guard = subscriber.listen().await;
                command_tx.quit().await;
            }
        });

        let ret = self.run_inner(runner_tx).await;
        self.cleanup()?;

        if let Err(err) = ret {
            error!("Error: {}", err);
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
            if !matches!(event, AppCommand::Tick) {
                needs_rerender = true;
            }
            if matches!(event, AppCommand::Resize { .. }) {
                self.terminal.autoresize()?;
            }
            self.state.update(event, runner_tx)?;
            if self.state.done {
                break;
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
    async fn poll<'a>(&mut self) -> anyhow::Result<Option<AppCommand>> {
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
    const SEARCH_MATCH_BG: vt100::Color = vt100::Color::Idx(3); // Yellow
    const CURRENT_SEARCH_MATCH_BG: vt100::Color = vt100::Color::Idx(0);

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

    fn input_options(&self) -> anyhow::Result<InputOptions> {
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
        self.set_status(task, TaskStatus::Finished(result, datetime))
    }

    pub fn has_stdin(&self) -> anyhow::Result<bool> {
        let task = self.active_task()?;
        Ok(task.output.stdin().is_some())
    }

    pub fn interact(&mut self) -> anyhow::Result<()> {
        if matches!(self.focus, LayoutSections::Pane) {
            self.focus = LayoutSections::TaskList(None)
        } else if self.has_stdin()? {
            self.focus = LayoutSections::Pane;
        }
        Ok(())
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

        let active_task = match self.active_task() {
            Ok(task) => task,
            Err(e) => {
                error!("Error on rendering: {}", e);
                return;
            }
        };
        let content_length = active_task.output.screen().current_scrollback_len();
        let scrollback = active_task.output.screen().scrollback();

        // Render pane
        let pane_to_render = TerminalPane::new(&active_task, &self.focus, self.has_sidebar);
        f.render_widget(&pane_to_render, pane);

        // Render pane scrollbar
        self.scrollbar = self.scrollbar.content_length(content_length);
        self.scrollbar = self.scrollbar.position(content_length.saturating_sub(scrollback));
        let scrollbar_to_render = TerminalScroll::new(&self.focus);
        f.render_stateful_widget(scrollbar_to_render, pane, &mut self.scrollbar);

        // Render task list
        let table_to_render = TaskTable::new(&self.tasks, &self.focus);
        f.render_stateful_widget(&table_to_render, table, &mut self.table);

        // Render quitting dialog
        if self.quitting {
            render_quit_dialog(f, self.force_quitting);
        }

        // Render help dialog
        if let LayoutSections::Help { scroll, max_scroll } = self.focus {
            render_help_dialog(f, scroll);
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

    pub fn copy_selection(&self) -> anyhow::Result<()> {
        let task = self.active_task()?;
        let Some(text) = task.output.copy_selection() else {
            return Ok(());
        };
        copy_to_clipboard(&text);
        Ok(())
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

    pub fn line_selection(&mut self, rows: u16) -> anyhow::Result<()> {
        let task = self.active_task_mut()?;
        task.output.line_selection(rows);
        Ok(())
    }

    pub fn enter_search(&mut self) -> anyhow::Result<()> {
        self.clear_search_highlights()?;
        self.focus = LayoutSections::Search { query: "".to_string() };
        Ok(())
    }

    pub fn clear_search_highlights(&mut self) -> anyhow::Result<()> {
        let LayoutSections::TaskList(Some(results)) = &mut self.focus else {
            return Ok(());
        };
        let results = results.clone();
        self.highlight_search_matches(&results, vt100::Color::Default)?;
        Ok(())
    }

    fn highlight_search_matches(&mut self, results: &SearchResults, color: vt100::Color) -> anyhow::Result<()> {
        let active_task_name = self.active_task()?.name.clone();
        if active_task_name != results.task {
            return Ok(());
        }
        let query_len = results.query.len() as u16;
        self.highlight_cell(&results.matches, query_len, color)?;
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
                    let mut row_idx = row_idx - previous_row_widths.len();
                    for width in previous_row_widths.iter().chain(std::iter::once(&current_row_width)) {
                        if col_idx < *width {
                            // The match exists in this line
                            matches.push(Match(row_idx as u16, col_idx as u16));
                            break;
                        }
                        // The match may be in the next line
                        col_idx -= *width;
                        row_idx += 1;
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

        self.highlight_search_matches(&search_results, Self::SEARCH_MATCH_BG)?;
        if let Some(m) = search_results.current() {
            self.highlight_cell(&vec![m.clone()], query_len as u16, Self::CURRENT_SEARCH_MATCH_BG)?;
            self.scroll_to_row(m.0)?;
        }

        self.focus = LayoutSections::TaskList(Some(search_results));
        Ok(())
    }

    fn highlight_cell(&mut self, matches: &Vec<Match>, length: u16, color: vt100::Color) -> anyhow::Result<()> {
        let task = self.active_task_mut()?;
        let screen = task.output.screen_mut();
        let mut matches = matches.clone();
        matches.sort_by_key(|m| (m.0, m.1));
        // Pending matches are sorted by row index
        let mut pending = matches.into_iter().peekable();
        // Matches for the row
        let mut active: Vec<(u16, u16)> = Vec::new(); // (start_col, remaining)

        for (row_idx, row) in screen.grid_mut().all_rows_mut().enumerate() {
            // If there's no active match and no pending match, we're done
            if active.is_empty() && pending.peek().is_none() {
                break;
            }
            let row_idx = row_idx as u16;
            // Find active matches for this row
            while let Some(m) = pending.peek() {
                if m.0 != row_idx {
                    break;
                }
                active.push((m.1, length));
                pending.next();
            }

            // Highlight active matches
            for (start_col, rest) in active.iter_mut() {
                let mut col = *start_col;
                while *rest > 0 {
                    // If no column left, go to next line
                    let Some(c) = row.get_mut(col) else { break };

                    c.attrs_mut().bgcolor = color;
                    *rest -= 1;
                    col += 1;
                }
                *start_col = 0;
            }

            // Remove completed matches from the active list
            active.retain(|(_, rest)| *rest > 0);
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

        if let Some(m) = results.next() {
            self.highlight_cell(m, query_len as u16, Self::CURRENT_SEARCH_MATCH_BG)?;
            self.scroll_to_row(m.0)?;
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

        if let Some(m) = results.previous() {
            self.highlight_cell(m, query_len as u16, true)?;
            self.scroll_to_row(m.0)?;
        }

        self.focus = LayoutSections::TaskList(Some(results));

        Ok(())
    }

    pub fn exit_search(&mut self) -> anyhow::Result<()> {
        if let LayoutSections::TaskList(results) = &mut self.focus {
            let Some(mut results) = results.clone() else {
                return Ok(());
            };
            self.clear_search_highlights()?;
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
                runner_tx.quit();
            }
            AppCommand::OpenHelp => {
                let (rect, content_width, content_height) = help_dialog_size(self.size.cols(), self.size.rows());
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
                self.interact()?;
            }
            AppCommand::ExitInteractive => {
                self.interact()?;
            }
            AppCommand::Input { bytes } => {
                self.forward_input(&bytes)?;
            }
            AppCommand::ClearSelection => {
                self.clear_selection()?;
            }
            AppCommand::LineSelection { rows } => {
                self.line_selection(rows)?;
            }
            AppCommand::UpdateSelection { rows, cols, edge } => {
                self.update_selection(rows, cols, edge)?;
            }
            AppCommand::CopySelection => {
                self.copy_selection()?;
                self.clear_selection()?;
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
