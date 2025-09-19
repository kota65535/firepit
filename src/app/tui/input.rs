use crate::app::command::{AppCommand, ScrollSize};
use crate::app::input::encode_key;
use crate::app::tui::lib::RingBuffer;
use crate::app::tui::LayoutSections;
use crate::app::DOUBLE_CLICK_DURATION;
use crate::tokio_spawn;
use crossterm::event::{EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use itertools::Itertools;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct InputHandler {
    click_times: RingBuffer<Instant>,
}

pub struct InputOptions<'a> {
    pub focus: &'a LayoutSections,
    pub has_selection: bool,
    pub task: String,
}

impl InputOptions<'_> {
    pub fn on_task_list(&self) -> bool {
        matches!(self.focus, LayoutSections::TaskList { .. })
    }

    pub fn on_search(&self) -> bool {
        matches!(self.focus, LayoutSections::Search { .. })
    }

    pub fn on_pane(&self) -> bool {
        matches!(self.focus, LayoutSections::Pane { .. })
    }
}

impl InputHandler {
    pub fn new() -> Self {
        Self {
            click_times: RingBuffer::new(3),
        }
    }

    pub fn start(&self) -> mpsc::Receiver<crossterm::event::Event> {
        let (tx, rx) = mpsc::channel(1024);

        if atty::is(atty::Stream::Stdin) {
            let mut events = EventStream::new();
            tokio_spawn!("input-handler", async move {
                while let Some(Ok(event)) = events.next().await {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            });
        }

        rx
    }

    pub fn num_of_multiple_clicks(&self) -> usize {
        let mut count = 1;
        for (a, b) in self.click_times.iter().rev().tuple_windows() {
            if a.duration_since(*b) > DOUBLE_CLICK_DURATION {
                break;
            }
            count += 1;
        }
        count
    }

    pub fn handle(&mut self, event: crossterm::event::Event, options: InputOptions) -> Option<AppCommand> {
        match event {
            crossterm::event::Event::Key(k) => translate_key_event(options, k),
            crossterm::event::Event::Mouse(m) => match m.kind {
                crossterm::event::MouseEventKind::ScrollDown => Some(AppCommand::ScrollDown(ScrollSize::One)),
                crossterm::event::MouseEventKind::ScrollUp => Some(AppCommand::ScrollUp(ScrollSize::One)),
                crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                    self.click_times.push(Instant::now());
                    let num_clicks = self.num_of_multiple_clicks();
                    if num_clicks == 1 {
                        Some(AppCommand::Mouse(m))
                    } else {
                        debug!("Clicked {} times", num_clicks);
                        Some(AppCommand::MouseMultiClick(m, num_clicks))
                    }
                }
                crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                    Some(AppCommand::Mouse(m))
                }
                _ => None,
            },
            crossterm::event::Event::Resize(cols, rows) => Some(AppCommand::Resize { rows, cols }),
            _ => None,
        }
    }
}

/// Converts a crossterm key event into a TUI interaction event
fn translate_key_event(options: InputOptions, key_event: KeyEvent) -> Option<AppCommand> {
    if key_event.kind == KeyEventKind::Release {
        return None;
    }
    match key_event.code {
        // On task list
        KeyCode::Char('/') if options.on_task_list() => Some(AppCommand::EnterSearch),
        KeyCode::Char('h') if options.on_task_list() => Some(AppCommand::ToggleSidebar),
        KeyCode::Char('e') if options.on_task_list() => Some(AppCommand::ScrollDown(ScrollSize::One)),
        KeyCode::Char('y') if options.on_task_list() => Some(AppCommand::ScrollUp(ScrollSize::One)),
        KeyCode::Char('d') if options.on_task_list() => Some(AppCommand::ScrollDown(ScrollSize::Half)),
        KeyCode::Char('u') if options.on_task_list() => Some(AppCommand::ScrollUp(ScrollSize::Half)),
        KeyCode::Char('f') if options.on_task_list() => Some(AppCommand::ScrollDown(ScrollSize::Full)),
        KeyCode::Char('b') if options.on_task_list() => Some(AppCommand::ScrollUp(ScrollSize::Full)),
        KeyCode::Char('G') if options.on_task_list() => Some(AppCommand::ScrollDown(ScrollSize::Edge)),
        KeyCode::Char('g') if options.on_task_list() => Some(AppCommand::ScrollUp(ScrollSize::Edge)),
        KeyCode::Char('j') if options.on_task_list() => Some(AppCommand::Down),
        KeyCode::Char('k') if options.on_task_list() => Some(AppCommand::Up),
        KeyCode::Char('n') if options.on_task_list() => Some(AppCommand::SearchNext),
        KeyCode::Char('N') if options.on_task_list() => Some(AppCommand::SearchPrevious),
        KeyCode::Up if options.on_task_list() => Some(AppCommand::Up),
        KeyCode::Down if options.on_task_list() => Some(AppCommand::Down),
        KeyCode::Char('c') if options.has_selection => Some(AppCommand::CopySelection),
        KeyCode::Enter if options.on_task_list() => Some(AppCommand::EnterInteractive),
        KeyCode::Char('q') if options.on_task_list() => Some(AppCommand::Quit),
        KeyCode::Char('s') if options.on_task_list() => Some(AppCommand::StopTask { task: options.task }),
        KeyCode::Char('r') if options.on_task_list() => Some(AppCommand::RestartTask {
            task: options.task,
            force: true,
        }),
        KeyCode::Char('R') if options.on_task_list() => Some(AppCommand::RestartTask {
            task: options.task,
            force: false,
        }),

        // On pane (interactive mode)
        KeyCode::Char('z') if options.on_pane() && key_event.modifiers == KeyModifiers::CONTROL => {
            Some(AppCommand::ExitInteractive)
        }
        // If we're in interactive mode, convert the key event to bytes to send to stdin
        _ if options.on_pane() => Some(AppCommand::Input {
            bytes: encode_key(key_event),
        }),

        // On search
        KeyCode::Char(c) if options.on_search() => Some(AppCommand::SearchInputChar(c)),
        KeyCode::Backspace if options.on_search() => Some(AppCommand::SearchBackspace),
        KeyCode::Esc if options.on_search() || options.on_task_list() => Some(AppCommand::ExitSearch),
        KeyCode::Enter if options.on_search() => Some(AppCommand::SearchRun),

        // Global
        KeyCode::Char('c') if key_event.modifiers == KeyModifiers::CONTROL => Some(AppCommand::Quit),
        _ => None,
    }
}
