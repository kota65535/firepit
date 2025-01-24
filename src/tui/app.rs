use crate::event::{Direction, PaneSize, TaskStatus};
use crate::event::{Event, TaskResult};
use crate::event::{EventReceiver, EventSender};
use crate::tui::input;
use crate::tui::input::InputOptions;
use crate::tui::pane::TerminalPane;
use crate::tui::size::SizeInfo;
use crate::tui::table::TaskTable;
use crate::tui::task::TaskDetail;
use crate::tui::term_output::TerminalOutput;
use anyhow::Context;
use indexmap::IndexMap;
use log::{debug, info};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    widgets::TableState,
    Frame, Terminal,
};
use std::{
    io::{self, Stdout, Write},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

pub const FRAME_RATE: Duration = Duration::from_millis(3);

#[derive(Debug, Clone)]
pub enum LayoutSections {
    Pane,
    TaskList,
}

pub struct TuiApp {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    crossterm_rx: mpsc::Receiver<crossterm::event::Event>,
    sender: EventSender,
    state: TuiAppState,
    receiver: EventReceiver,
}

pub struct TuiAppState {
    size: SizeInfo,
    task_outputs: IndexMap<String, TerminalOutput>,
    task_details: IndexMap<String, TaskDetail>,
    focus: LayoutSections,
    scroll: TableState,
    selected_task_index: usize,
    has_user_scrolled: bool,
    has_sidebar: bool,
    done: bool,
}

impl TuiApp {
    pub fn new(target_tasks: Vec<String>, dep_tasks: Vec<String>) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();

        let terminal = Self::setup_terminal()?;
        let rect = terminal.size()?;

        let (crossterm_tx, crossterm_rx) = mpsc::channel(1024);
        input::start_crossterm_stream(crossterm_tx.clone());

        let size = SizeInfo::new(
            rect.height,
            rect.width,
            target_tasks
                .iter()
                .chain(dep_tasks.iter())
                .map(|s| s.as_str()),
        );

        debug!("Terminal size: height={} width={}", rect.height, rect.width);

        let pane_rows = size.pane_rows();
        let pane_cols = size.pane_cols();

        let task_outputs = target_tasks
            .iter()
            .chain(dep_tasks.iter())
            .map(|t| {
                (
                    t.clone(),
                    TerminalOutput::new(t, pane_rows, pane_cols, None),
                )
            })
            .collect::<IndexMap<_, _>>();

        let task_details = target_tasks
            .iter()
            .map(|t| (t.clone(), TaskDetail::new(t, true)))
            .chain(
                dep_tasks
                    .iter()
                    .map(|t| (t.clone(), TaskDetail::new(t, false))),
            )
            .collect::<IndexMap<_, _>>();

        let selected_task_index = 0;

        Ok(Self {
            terminal,
            crossterm_rx,
            sender: EventSender::new(tx),
            receiver: EventReceiver::new(rx),
            state: TuiAppState {
                size,
                task_outputs,
                task_details,
                focus: LayoutSections::TaskList,
                scroll: TableState::default().with_selected(selected_task_index),
                selected_task_index,
                has_sidebar: true,
                has_user_scrolled: false,
                done: false,
            },
        })
    }

    fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Ensure all pending writes are flushed before we switch to alternative screen
        stdout.flush()?;
        crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
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
        self.cleanup()
    }

    pub async fn run_inner(&mut self) -> anyhow::Result<Option<oneshot::Sender<()>>> {
        self.terminal.draw(|f| self.state.view(f))?;

        let mut last_render = Instant::now();
        let mut callback = None;
        let mut needs_rerender = true;

        while let Some(event) = self.poll().await {
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
    async fn poll<'a>(&mut self) -> Option<Event> {
        let input_closed = self.crossterm_rx.is_closed();

        if input_closed {
            self.receiver.recv().await
        } else {
            let mut event;
            loop {
                tokio::select! {
                    e = self.crossterm_rx.recv() => {
                        event = e.and_then(|e| self.state.input_options().unwrap().handle_crossterm_event(e));
                    }
                    e = self.receiver.recv() => {
                        event = e;
                    }
                }
                if event.is_some() {
                    break;
                }
            }
            event
        }
    }

    fn cleanup(&mut self) -> anyhow::Result<()> {
        self.terminal.clear()?;
        crossterm::execute!(
            self.terminal.backend_mut(),
            crossterm::terminal::LeaveAlternateScreen,
        )?;
        self.state.persist_tasks()?;
        crossterm::terminal::disable_raw_mode()?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl TuiAppState {
    pub fn active_task(&self) -> anyhow::Result<&TerminalOutput> {
        self.nth_task(self.selected_task_index)
    }

    pub fn active_task_mut(&mut self) -> anyhow::Result<&mut TerminalOutput> {
        self.nth_task_mut(self.selected_task_index)
    }

    pub fn task(&self, name: &str) -> anyhow::Result<&TerminalOutput> {
        self.task_outputs
            .get(name)
            .with_context(|| format!("task {} not found", name))
    }

    pub fn task_mut(&mut self, name: &str) -> anyhow::Result<&mut TerminalOutput> {
        self.task_outputs
            .get_mut(name)
            .with_context(|| format!("task {} not found", name))
    }

    fn input_options(&self) -> anyhow::Result<InputOptions> {
        Ok(InputOptions { focus: &self.focus })
    }

    pub fn nth_task(&self, num: usize) -> anyhow::Result<&TerminalOutput> {
        self.task_outputs
            .iter()
            .nth(num)
            .map(|e| e.1)
            .with_context(|| anyhow::anyhow!("{}th task not found", num))
    }

    pub fn nth_task_mut(&mut self, num: usize) -> anyhow::Result<&mut TerminalOutput> {
        self.task_outputs
            .iter_mut()
            .nth(num)
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

    fn set_status(&mut self, task: &str, status: TaskStatus) {
        if let Some(task) = self.task_details.get_mut(task) {
            task.status = status;
        }
        if let Some(output) = self.task_outputs.get_mut(task) {
            output.status = status
        }
    }

    pub fn start_task(&mut self, task: &str) {
        self.set_status(task, TaskStatus::Running);
    }

    pub fn ready_task(&mut self, task: &str) {
        self.set_status(task, TaskStatus::Ready);
    }

    pub fn finish_task(&mut self, task: &str, result: TaskResult) {
        self.set_status(task, TaskStatus::Finished(result));
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

    pub fn persist_tasks(&mut self) -> anyhow::Result<()> {
        for (_, o) in self
            .task_details
            .values()
            .zip(self.task_outputs.values())
            .filter(|(s, _)| matches!(s.status, TaskStatus::Running | TaskStatus::Finished(_)))
        {
            o.persist_screen()?
        }
        Ok(())
    }

    fn select_task(&mut self, task_name: &str) -> anyhow::Result<()> {
        if !self.has_user_scrolled {
            return Ok(());
        }

        let new_index_to_highlight = self
            .task_outputs
            .iter()
            .position(|task| task.0 == task_name)
            .with_context(|| format!("{} not found", task_name))?;

        self.selected_task_index = new_index_to_highlight;
        self.scroll.select(Some(new_index_to_highlight));

        Ok(())
    }

    pub fn reset_scroll(&mut self) {
        self.has_user_scrolled = false;
        self.scroll.select(Some(0));
        self.selected_task_index = 0;
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        debug!("Terminal size: height={} width={}", rows, cols);
        self.size.resize(rows, cols);
        let pane_rows = self.size.pane_rows();
        let pane_cols = self.size.pane_cols();
        self.task_outputs.values_mut().for_each(|task| {
            task.resize(pane_rows, pane_cols);
        })
    }

    pub fn view(&mut self, f: &mut Frame) {
        let cols = self.size.pane_cols();
        let horizontal = if self.has_sidebar {
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(cols)])
        } else {
            Layout::horizontal([Constraint::Max(0), Constraint::Length(cols)])
        };
        let [table, pane] = horizontal.areas(f.size());

        let active_task = self.active_task().unwrap();
        let pane_to_render = TerminalPane::new(
            active_task,
            &active_task.name,
            &self.focus,
            self.has_sidebar,
        );
        let table_to_render = TaskTable::new(&self.task_details);

        f.render_widget(&pane_to_render, pane);
        f.render_stateful_widget(&table_to_render, table, &mut self.scroll);
    }

    /// Insert a stdin to be associated with a task
    pub fn insert_stdin(
        &mut self,
        task: &str,
        stdin: Option<Box<dyn Write + Send>>,
    ) -> anyhow::Result<()> {
        let task = self
            .task_outputs
            .get_mut(task)
            .with_context(|| format!("{} not found", task))?;
        task.stdin = stdin;
        Ok(())
    }

    pub fn forward_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if matches!(self.focus, LayoutSections::Pane) {
            let task = self.active_task_mut()?;
            if let Some(stdin) = &mut task.stdin {
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
        task.process(output);
        Ok(())
    }

    fn update(&mut self, event: Event) -> anyhow::Result<Option<oneshot::Sender<()>>> {
        match event {
            Event::StartTask { task } => {
                self.start_task(&task);
            }
            Event::TaskOutput { task, output } => {
                self.process_output(&task, &output)?;
            }
            Event::ReadyTask { task } => {
                self.ready_task(&task);
            }
            Event::InternalStop => {
                debug!("shutting down due to internal failure");
                self.done = true;
            }
            Event::Stop(callback) => {
                debug!("shutting down due to message");
                self.done = true;
                return Ok(Some(callback));
            }
            Event::Tick => {
                // self.table.tick();
            }
            Event::EndTask { task, result } => {
                self.finish_task(&task, result);
                self.insert_stdin(&task, None)?;
            }
            Event::Up => {
                self.previous();
            }
            Event::Down => {
                self.next();
            }
            Event::ScrollUp => {
                self.has_user_scrolled = true;
                self.scroll_terminal_output(Direction::Up)?;
            }
            Event::ScrollDown => {
                self.has_user_scrolled = true;
                self.scroll_terminal_output(Direction::Down)?;
            }
            Event::EnterInteractive => {
                self.has_user_scrolled = true;
                self.interact()?;
            }
            Event::ExitInteractive => {
                self.has_user_scrolled = true;
                self.interact()?;
            }
            Event::ToggleSidebar => {
                self.has_sidebar = !self.has_sidebar;
            }
            Event::Input { bytes } => {
                self.forward_input(&bytes)?;
            }
            Event::SetStdin { task, stdin } => {
                self.insert_stdin(&task, Some(stdin))?;
            }
            Event::Resize { rows, cols } => {
                self.resize(rows, cols);
            }
            Event::PaneSizeQuery(callback) => {
                // If caller has already hung up do nothing
                callback
                    .send(PaneSize {
                        rows: self.size.pane_rows(),
                        cols: self.size.pane_cols(),
                    })
                    .ok();
            }
            _ => {}
        }
        Ok(None)
    }
}
