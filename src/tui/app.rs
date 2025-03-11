use crate::event::{Direction, PaneSize, ScrollSize, TaskRun, TaskStatus};
use crate::event::{Event, TaskResult};
use crate::event::{EventReceiver, EventSender};
use crate::tui::clipboard::copy_to_clipboard;
use crate::tui::input::{InputHandler, InputOptions};
use crate::tui::pane::{TerminalPane, TerminalScroll};
use crate::tui::search::{Match, SearchResults};
use crate::tui::size::SizeInfo;
use crate::tui::table::TaskTable;
use crate::tui::task::Task;
use crate::tui::term_output::TerminalOutput;
use anyhow::Context;
use indexmap::IndexMap;
use ratatui::widgets::ScrollbarState;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    widgets::TableState,
    Frame, Terminal,
};
use std::collections::HashMap;
use std::{
    io::{self, Stdout, Write},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};
use tracing::{debug, error, info};

pub const FRAME_RATE: Duration = Duration::from_millis(3);

#[derive(Debug, Clone)]
pub enum LayoutSections {
    Pane,
    TaskList(Option<SearchResults>),
    Search { query: String },
}

pub struct TuiApp {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    crossterm_rx: mpsc::Receiver<crossterm::event::Event>,
    sender: EventSender,
    state: TuiAppState,
    receiver: EventReceiver,
    input_handler: InputHandler,
}

pub struct TuiAppState {
    size: SizeInfo,
    tasks: IndexMap<String, Task>,
    focus: LayoutSections,
    table: TableState,
    scrollbar: ScrollbarState,
    selected_task_index: usize,
    has_sidebar: bool,
    done: bool,
    runner_done: bool,
}

impl TuiApp {
    pub fn new(
        target_tasks: &Vec<String>,
        dep_tasks: &Vec<String>,
        labels: &HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();

        let terminal = Self::setup_terminal()?;
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
        let input_handler = InputHandler::new();
        let crossterm_rx = input_handler.start();

        Ok(Self {
            terminal,
            crossterm_rx,
            sender: EventSender::new(tx),
            receiver: EventReceiver::new(rx),
            input_handler,
            state: TuiAppState {
                size,
                tasks,
                focus: LayoutSections::TaskList(None),
                table: TableState::default().with_selected(selected_task_index),
                scrollbar: ScrollbarState::default(),
                selected_task_index,
                has_sidebar,
                done: false,
                runner_done: false,
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

    pub fn sender(&self) -> EventSender {
        self.sender.clone()
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        let (result, callback) = match self.run_inner().await {
            Ok(callback) => (Ok(()), callback),
            Err(err) => (Err(err).with_context(|| "failed to run tui app"), None),
        };
        self.cleanup()?;
        info!("App is exiting");
        Ok(())
    }

    pub async fn run_inner(&mut self) -> anyhow::Result<Option<oneshot::Sender<()>>> {
        self.terminal.draw(|f| self.state.view(f))?;

        let mut last_render = Instant::now();
        let mut callback = None;
        let mut needs_rerender = true;
        while let Some(event) = self.poll().await? {
            // If we only receive ticks, then there's been no state change so no update needed
            if !matches!(event, Event::Tick) {
                needs_rerender = true;
            }
            if matches!(event, Event::Resize { .. }) {
                self.terminal.autoresize()?;
            }
            callback = self.state.update(event)?;
            if self.state.done {
                break;
            }
            if FRAME_RATE <= last_render.elapsed() && needs_rerender {
                self.terminal.draw(|f| self.state.view(f))?;
                last_render = Instant::now();
                needs_rerender = false;
            }
        }

        Ok(callback)
    }

    /// Blocking poll for events, will only return None if app handle has been
    /// dropped
    async fn poll<'a>(&mut self) -> anyhow::Result<Option<Event>> {
        let input_closed = self.crossterm_rx.is_closed();

        if input_closed {
            Ok(self.receiver.recv().await)
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
                    e = self.receiver.recv() => {
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
        self.tasks.get(name).with_context(|| format!("task {} not found", name))
    }

    pub fn task_mut(&mut self, name: &str) -> anyhow::Result<&mut Task> {
        self.tasks
            .get_mut(name)
            .with_context(|| format!("task {} not found", name))
    }

    fn input_options(&self) -> anyhow::Result<InputOptions> {
        let has_selection = self.active_task()?.output.has_selection();
        Ok(InputOptions {
            focus: &self.focus,
            has_selection,
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
    ) -> anyhow::Result<()> {
        self.set_status(
            task,
            TaskStatus::Running(TaskRun {
                pid,
                restart,
                max_restart,
                reload,
            }),
        )
    }

    pub fn ready_task(&mut self, task: &str) -> anyhow::Result<()> {
        self.set_status(task, TaskStatus::Ready)
    }

    pub fn finish_task(&mut self, task: &str, result: TaskResult) -> anyhow::Result<()> {
        self.set_status(task, TaskStatus::Finished(result))
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
        for (d, o) in self.tasks.values().zip(self.tasks.values()).filter(|(s, _)| {
            matches!(
                s.status(),
                TaskStatus::Running(_) | TaskStatus::Ready | TaskStatus::Finished(_)
            )
        }) {
            o.persist_screen()?
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
        let pane_to_render = TerminalPane::new(&active_task, &self.focus, self.has_sidebar, self.runner_done);
        f.render_widget(&pane_to_render, pane);

        // Render pane scrollbar
        self.scrollbar = self.scrollbar.content_length(content_length);
        self.scrollbar = self.scrollbar.position(content_length.saturating_sub(scrollback));
        let scrollbar_to_render = TerminalScroll::new(&self.focus);
        f.render_stateful_widget(scrollbar_to_render, pane, &mut self.scrollbar);

        // Render table
        let table_to_render = TaskTable::new(&self.tasks);
        f.render_stateful_widget(&table_to_render, table, &mut self.table);
    }

    /// Insert a stdin to be associated with a task
    pub fn insert_stdin(&mut self, task: &str, stdin: Option<Box<dyn Write + Send>>) -> anyhow::Result<()> {
        let task = self
            .tasks
            .get_mut(task)
            .with_context(|| format!("{} not found", task))?;
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

    pub fn handle_mouse(&mut self, mut event: crossterm::event::MouseEvent, clicks: usize) -> anyhow::Result<()> {
        let table_width = self.size.task_list_width();
        debug!("Original mouse event: {event:?}, table_width: {table_width}");

        // Do nothing in table & pane header
        if event.row < 2 {
            return Ok(());
        }

        // Subtract header height
        event.row -= 2;

        if self.has_sidebar {
            if event.column < table_width - 1 {
                // Task table clicked
                self.select_task(event.row as usize);
                // Set mouse event column to 0 for ease of selection
                event.column = 0;
            } else {
                // Terminal pane clicked
                // So subtract the width of the table if we have sidebar
                if self.has_sidebar && event.column >= table_width {
                    event.column -= table_width;
                }
                debug!("Translated mouse event: {event:?}");
            }
        }

        let task = self.active_task_mut()?;
        task.output.handle_mouse(event, clicks)?;

        Ok(())
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
        let task = self.active_task_mut()?;
        if task.name != results.task {
            return Ok(());
        }
        if let Some(Match(row, col)) = results.current() {
            self.highlight_cell(row, col, false)?;
        }
        Ok(())
    }

    pub fn run_search(&mut self) -> anyhow::Result<()> {
        let LayoutSections::Search { query, .. } = &mut self.focus else {
            return Ok(());
        };
        let query = query.clone();
        let task = self.active_task_mut()?;
        let screen = task.output.screen_mut();
        let size = screen.size();

        let mut matches = Vec::new();
        let mut buf = String::new();
        let mut previous_rows = Vec::new();
        for (row_idx, row) in screen.grid_mut().all_rows_mut().enumerate() {
            let mut s = String::new();
            row.write_contents(&mut s, 0, size.1, true);
            buf.push_str(&s);
            if row.wrapped() {
                previous_rows.push((row, s.len()));
                continue;
            }
            for (mut col_idx, _) in buf.match_indices(&query) {
                if previous_rows.is_empty() {
                    matches.push(Match(row_idx as u16, col_idx as u16));
                } else {
                    let mut row_idx = row_idx - previous_rows.len();
                    for (pr, l) in previous_rows.iter() {
                        if col_idx <= size.1 as usize {
                            matches.push(Match(row_idx as u16, col_idx as u16));
                        } else {
                            col_idx -= l;
                        }
                        row_idx += 1;
                    }
                }
            }
            previous_rows.clear();
            buf.clear();
        }

        let search_results = SearchResults::new(&task.name, query, matches)?;

        if let Some(Match(row, col)) = search_results.current() {
            self.highlight_cell(row, col, true)?;
            self.scroll_to_row(row)?;
        }

        self.focus = LayoutSections::TaskList(Some(search_results));
        Ok(())
    }

    fn highlight_cell(&mut self, row: u16, col: u16, highlight: bool) -> anyhow::Result<()> {
        let task = self.active_task_mut()?;
        let screen = task.output.screen_mut();
        let cell = screen
            .grid_mut()
            .all_rows_mut()
            .nth(row as usize)
            .map(|r| r.get_mut(col))
            .flatten();
        if let Some(cell) = cell {
            if highlight {
                cell.attrs.bgcolor = vt100::Color::Rgb(150, 30, 30)
            } else {
                cell.attrs.bgcolor = vt100::Color::Default;
            }
        }
        Ok(())
    }

    pub fn next_search_result(&mut self) -> anyhow::Result<()> {
        let LayoutSections::TaskList(Some(results)) = &mut self.focus else {
            return Ok(());
        };
        let mut results = results.clone();

        self.remove_search_highlight()?;

        if let Some(Match(row, col)) = results.next() {
            self.highlight_cell(row, col, true)?;
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

        self.remove_search_highlight()?;

        if let Some(Match(row, col)) = results.previous() {
            self.highlight_cell(row, col, true)?;
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

    fn update(&mut self, event: Event) -> anyhow::Result<Option<oneshot::Sender<()>>> {
        match event {
            Event::PlanTask { task } => {
                self.plan_task(&task);
            }
            Event::StartTask {
                task,
                pid,
                restart,
                max_restart,
                reload,
            } => {
                self.start_task(&task, pid, restart, max_restart, reload);
            }
            Event::TaskOutput { task, output } => {
                self.process_output(&task, &output)?;
            }
            Event::ReadyTask { task } => {
                self.ready_task(&task);
            }
            Event::FinishTask { task, result } => {
                self.finish_task(&task, result);
                self.insert_stdin(&task, None)?;
            }
            Event::SetStdin { task, stdin } => {
                self.insert_stdin(&task, Some(stdin))?;
            }
            Event::PaneSizeQuery(callback) => {
                // If caller has already hung up do nothing
                callback
                    .send(PaneSize {
                        rows: self.size.pane_rows(),
                        cols: self.size.output_cols(self.has_sidebar),
                    })
                    .ok();
            }
            Event::Stop(callback) => {
                debug!("Shutting down initiated by runner");
                self.runner_done = true;
                callback.send(()).ok();
            }
            Event::InternalStop => {
                debug!("Shutting down initiated by TUI");
                self.done = true;
            }
            Event::Tick => {
                // self.table.tick();
            }
            Event::Up => {
                self.exit_search()?;
                self.select_previous_task();
            }
            Event::Down => {
                self.exit_search()?;
                self.select_next_task();
            }
            Event::ScrollUp(size) => {
                self.scroll_terminal_output(Direction::Up, self.scroll_size(size))?;
            }
            Event::ScrollDown(size) => {
                self.scroll_terminal_output(Direction::Down, self.scroll_size(size))?;
            }
            Event::ToggleSidebar => {
                self.has_sidebar = !self.has_sidebar;
                self.resize(self.size.rows(), self.size.cols());
            }
            Event::EnterInteractive => {
                self.interact()?;
            }
            Event::ExitInteractive => {
                self.interact()?;
            }
            Event::Input { bytes } => {
                self.forward_input(&bytes)?;
            }
            Event::Mouse(m) => {
                self.handle_mouse(m, 1)?;
            }
            Event::MouseMultiClick(m, n) => {
                self.handle_mouse(m, n)?;
            }
            Event::CopySelection => {
                self.copy_selection()?;
                self.clear_selection()?;
            }
            Event::Resize { rows, cols } => {
                self.resize(rows, cols);
            }
            Event::EnterSearch => {
                self.enter_search()?;
            }
            Event::SearchInputChar(c) => {
                self.search_input_char(c)?;
            }
            Event::SearchBackspace => {
                self.search_remove_char()?;
            }
            Event::SearchRun => {
                self.run_search()?;
            }
            Event::SearchNext => {
                self.next_search_result()?;
            }
            Event::SearchPrevious => {
                self.previous_search_result()?;
            }
            Event::ExitSearch => {
                self.exit_search()?;
            }
        }
        Ok(None)
    }
}
