//! TUI rendering tests.
//!
//! These tests drive `TuiAppState` with the same `AppCommand`s the real
//! application sends it, but render onto `ratatui::backend::TestBackend` so
//! the resulting screen buffer can be asserted cell by cell. They guard
//! against display regressions coming from the terminal emulation layer
//! (`vt100`, `tui-term`, `ratatui`) as well as from firepit's own widgets.
//!
//! Only the TUI layer is covered: task output is fed in directly rather than
//! produced by a real process, so there is no `firepit.yml` fixture and no
//! runner involved. Feeding the bytes in keeps the exact-screen assertions
//! deterministic, which is what makes them useful for catching regressions.
//! `tests/runner.rs` covers the config-to-runner path.

use chrono::{Local, TimeZone};
use firepit::app::command::{AppCommand, ScrollSize, TaskResult};
use firepit::app::tui::TuiAppState;
use firepit::log::LogRecord;
use firepit::runner::command::RunnerCommandChannel;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use std::collections::HashMap;
use tracing::Level;

const COLS: u16 = 70;
const ROWS: u16 = 10;
/// First screen row of terminal output (row 0 is the pane title, row 1 is padding).
const PANE_Y: u16 = 2;
/// Number of output rows visible in the pane.
const PANE_ROWS: u16 = 6;

struct Tui {
    state: TuiAppState,
    terminal: Terminal<TestBackend>,
    runner_tx: RunnerCommandChannel,
}

impl Tui {
    fn new(tasks: &[&str]) -> Self {
        let tasks: Vec<String> = tasks.iter().map(|s| s.to_string()).collect();
        let state = TuiAppState::new(ROWS, COLS, &tasks, &[], &[], &HashMap::new());
        let terminal = Terminal::new(TestBackend::new(COLS, ROWS)).unwrap();
        let (runner_tx, _rx) = RunnerCommandChannel::new(16);
        Self {
            state,
            terminal,
            runner_tx,
        }
    }

    fn send(&mut self, cmd: AppCommand) {
        self.state.update(cmd, &self.runner_tx).unwrap();
    }

    fn output(&mut self, bytes: &[u8]) {
        self.send(AppCommand::TaskOutput {
            task: "build".to_string(),
            output: bytes.to_vec(),
        });
    }

    fn log(&mut self, task: Option<&str>, level: Level, message: &str) {
        self.send(AppCommand::Log(LogRecord {
            task: task.map(String::from),
            level,
            message: message.to_string(),
        }));
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.terminal.backend_mut().resize(cols, rows);
        self.send(AppCommand::Resize { rows, cols });
    }

    fn draw(&mut self) -> &Buffer {
        self.terminal.draw(|f| self.state.view(f)).unwrap();
        self.terminal.backend().buffer()
    }

    /// Renders and returns each screen row as a plain string.
    fn lines(&mut self) -> Vec<String> {
        let buf = self.draw();
        let area = *buf.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn assert_lines(&mut self, expected: &[&str]) {
        let actual = self.lines();
        assert_eq!(actual, expected.to_vec(), "\nactual:\n{}", actual.join("\n"));
    }

    /// Screen column of the first output cell (right of the sidebar border).
    fn pane_x(&mut self) -> u16 {
        let buf = self.draw();
        (0..buf.area().width)
            .find(|&x| buf[(x, 1)].symbol() == "│")
            .map(|x| x + 1)
            .unwrap_or(0)
    }

    /// Cell at pane-relative coordinates (row 0 = first output row).
    fn cell(&mut self, row: u16, col: u16) -> Cell {
        let x = self.pane_x() + col;
        self.draw()[(x, PANE_Y + row)].clone()
    }

    /// Text of one pane output row (trailing spaces trimmed, scrollbar excluded).
    fn pane_row(&mut self, row: u16) -> String {
        let x0 = self.pane_x();
        let buf = self.draw();
        let width = buf.area().width;
        (x0..width - 1)
            .map(|x| buf[(x, PANE_Y + row)].symbol())
            .collect::<String>()
            .trim_end()
            .trim_end_matches('█') // cursor
            .to_string()
    }

    fn pane_rows(&mut self) -> Vec<String> {
        (0..PANE_ROWS).map(|r| self.pane_row(r)).collect()
    }

    fn footer(&mut self) -> String {
        self.lines()[ROWS as usize - 2..].join("\n")
    }

    fn scrollbar(&mut self) -> String {
        let buf = self.draw();
        let x = buf.area().width - 1;
        (0..ROWS).map(|y| buf[(x, y)].symbol()).collect()
    }

    fn start_task(&mut self, task: &str, pid: u32, restart: u64, max_restart: Option<u64>) {
        let start = Local.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        self.send(AppCommand::StartTask {
            task: task.into(),
            pid,
            restart,
            max_restart,
            rerun: 0,
            datetime: start,
        });
    }

    fn finish_task(&mut self, task: &str, result: TaskResult) {
        let end = Local.with_ymd_and_hms(2026, 1, 1, 0, 0, 3).unwrap();
        self.send(AppCommand::FinishTask {
            task: task.into(),
            result,
            datetime: Some(end),
        });
    }
}

fn lines(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("line{:02}", i)).collect()
}

#[test]
fn basic_layout() {
    let mut tui = Tui::new(&["build", "serve"]);
    tui.output(b"hello\r\nworld\r\n");

    tui.assert_lines(&[
        "🏕  Tasks  Running │% build (Waiting)                                  ",
        "──────────────────│                                                   ",
        "build          🪵  │hello                                              ",
        "serve          🪵  │world                                              ",
        "                  │█                                                  ",
        "                  │                                                   ",
        "                  │                                                   ",
        "──────────────────│                                                   ",
        "[↑↓] Navigate     │                                        [q]  Quit  ",
        "[h]  Hide         │   [/･?] Search  [r] Re-run  [s] Stop   [F1] Help  ",
    ]);
}

#[test]
fn task_status_title_and_icons() {
    let mut tui = Tui::new(&["build", "serve"]);
    tui.start_task("build", 42, 0, Some(3));
    tui.finish_task("build", TaskResult::Success);
    tui.start_task("serve", 43, 1, None);
    tui.finish_task("serve", TaskResult::Failure(1));

    let lines = tui.lines();
    assert_eq!(
        lines[0],
        "🏕  Tasks  Failure │% build (Finished - Success, Restart: 0/3, Re-run: "
    );
    assert!(lines[2].starts_with("build          ✅️"), "{}", lines[2]);
    assert!(lines[3].starts_with("serve          ❌️"), "{}", lines[3]);

    tui.send(AppCommand::Down);
    let lines = tui.lines();
    assert_eq!(
        lines[0],
        "🏕  Tasks  Failure │% serve (Finished - Failed with exit code 1, Restar"
    );
}

/// A task that cannot restart, e.g. any task that is not a service, has no
/// restart to count, so the title leaves it out.
#[test]
fn title_omits_the_restart_count_when_restarting_is_off() {
    let mut tui = Tui::new(&["build"]);
    tui.start_task("build", 42, 0, Some(0));
    assert!(
        tui.lines()[0].contains("% build (Running, PID: 42, Re-run: 0, Elapsed:"),
        "{}",
        tui.lines()[0]
    );
}

/// A service that restarts without a limit shows the limit as infinity.
#[test]
fn title_shows_an_unlimited_restart_count() {
    let mut tui = Tui::new(&["serve"]);
    tui.start_task("serve", 42, 2, None);
    assert!(
        tui.lines()[0].contains("% serve (Running, PID: 42, Restart: 2/\u{221e}, Re-run: 0"),
        "{}",
        tui.lines()[0]
    );
}

/// A task that could not run shows the cause in red in its pane, which has no
/// process output to mix with.
#[test]
fn error_result_shows_cause_in_pane() {
    let mut tui = Tui::new(&["build"]);
    tui.start_task("build", 42, 0, None);
    tui.finish_task("build", TaskResult::Error("boom".to_string()));

    let lines = tui.lines();
    assert!(lines[0].contains("% build (Finished - Error, Restart"), "{}", lines[0]);
    assert_eq!(tui.pane_row(0), "Task failed to run: boom");
    assert_eq!(tui.cell(0, 0).fg, Color::Indexed(1)); // red
    assert!(tui.pane_row(1).is_empty());
}

/// A restarted process is announced so its output is not mistaken for the
/// previous run's.
#[test]
fn restart_is_noted_dim_in_pane() {
    let mut tui = Tui::new(&["build"]);
    tui.start_task("build", 42, 0, None);
    tui.output(b"old\r\n");
    tui.finish_task("build", TaskResult::Failure(1));
    tui.start_task("build", 43, 1, Some(3));
    tui.output(b"new\r\n");

    assert_eq!(tui.pane_rows()[..3], ["old", "Task restarted (1/3), PID: 43", "new"]);
    assert!(tui.cell(1, 0).modifier.contains(Modifier::DIM));
    assert!(!tui.cell(2, 0).modifier.contains(Modifier::DIM));
}

/// Output that did not end its line is not overwritten by the note.
#[test]
fn note_starts_on_a_fresh_line() {
    let mut tui = Tui::new(&["build"]);
    tui.start_task("build", 42, 0, None);
    tui.output(b"progress 50%");
    tui.finish_task("build", TaskResult::Failure(1));
    tui.start_task("build", 43, 1, None);

    assert_eq!(tui.pane_rows()[..2], ["progress 50%", "Task restarted (1), PID: 43"]);
}

/// An in-place progress bar leaves the cursor at column zero of a line it has
/// written to, which the note must not overwrite either.
#[test]
fn note_starts_on_a_fresh_line_after_a_carriage_return() {
    let mut tui = Tui::new(&["build"]);
    tui.start_task("build", 42, 0, None);
    tui.output(b"progress 50%\r");
    tui.finish_task("build", TaskResult::Failure(1));
    tui.start_task("build", 43, 1, None);

    assert_eq!(tui.pane_rows()[..2], ["progress 50%", "Task restarted (1), PID: 43"]);
}

/// The TUI reports failed tasks the same way the CUI does, so the exit code matches.
#[test]
fn failed_tasks_are_collected_for_exit_code() {
    let mut tui = Tui::new(&["build", "serve"]);
    assert!(tui.state.failed_tasks().is_empty());

    tui.start_task("build", 42, 0, None);
    tui.finish_task("build", TaskResult::Success);
    assert!(tui.state.failed_tasks().is_empty());

    tui.start_task("serve", 43, 0, None);
    tui.finish_task("serve", TaskResult::Failure(1));
    assert_eq!(
        tui.state.failed_tasks(),
        vec![("serve".to_string(), TaskResult::Failure(1))]
    );
}

/// The summary reflects the final state: tasks stopped by quitting are not
/// failures, a task that failed before quitting still is.
#[test]
fn failed_tasks_are_the_final_state_without_stopped() {
    let mut tui = Tui::new(&["serve", "build"]);
    tui.start_task("build", 42, 0, None);
    tui.finish_task("build", TaskResult::Failure(1));

    tui.start_task("serve", 43, 0, None);
    tui.send(AppCommand::Quit);
    tui.finish_task("serve", TaskResult::Stopped);
    assert_eq!(
        tui.state.failed_tasks(),
        vec![("build".to_string(), TaskResult::Failure(1))]
    );
}

/// A task fixed by a restart is not a failure anymore.
#[test]
fn failed_tasks_forget_a_failure_fixed_by_restart() {
    let mut tui = Tui::new(&["build"]);
    tui.start_task("build", 42, 0, None);
    tui.finish_task("build", TaskResult::Failure(1));
    assert_eq!(tui.state.failed_tasks().len(), 1);

    tui.start_task("build", 43, 1, None);
    tui.finish_task("build", TaskResult::Success);
    assert!(tui.state.failed_tasks().is_empty());
}

/// A stopped task is not a failure: the header says so instead.
#[test]
fn stopped_tasks_are_not_failures() {
    let mut tui = Tui::new(&["serve"]);
    tui.start_task("serve", 42, 0, None);
    tui.finish_task("serve", TaskResult::Stopped);

    assert!(tui.lines()[0].starts_with("🏕  Tasks  Stopped "), "{}", tui.lines()[0]);
    assert!(tui.state.failed_tasks().is_empty());
}

#[test]
fn ansi_attributes_are_rendered() {
    let mut tui = Tui::new(&["build"]);
    // red, bold, dim, italic, underline, inverse, rgb fg, indexed bg, then plain
    tui.output(b"\x1b[31mR\x1b[0m\x1b[1mB\x1b[0m\x1b[2mD\x1b[0m\x1b[3mI\x1b[0m\x1b[4mU\x1b[0m\x1b[7mV\x1b[0m\x1b[38;2;1;2;3mT\x1b[0m\x1b[44mG\x1b[0mP");
    assert_eq!(tui.pane_row(0), "RBDIUVTGP");
    assert_eq!(tui.cell(0, 0).fg, Color::Indexed(1));
    assert!(tui.cell(0, 1).modifier.contains(Modifier::BOLD));
    assert!(tui.cell(0, 2).modifier.contains(Modifier::DIM));
    assert!(tui.cell(0, 3).modifier.contains(Modifier::ITALIC));
    assert!(tui.cell(0, 4).modifier.contains(Modifier::UNDERLINED));
    assert!(tui.cell(0, 5).modifier.contains(Modifier::REVERSED));
    assert_eq!(tui.cell(0, 6).fg, Color::Rgb(1, 2, 3));
    assert_eq!(tui.cell(0, 7).bg, Color::Indexed(4));
    let plain = tui.cell(0, 8);
    assert_eq!(plain.fg, Color::Reset);
    assert_eq!(plain.bg, Color::Reset);
    assert_eq!(plain.modifier, Modifier::empty());
}

/// Bold and dim are the same SGR intensity, so the later one wins rather than
/// both applying.
#[test]
fn bold_and_dim_are_exclusive() {
    let mut tui = Tui::new(&["build"]);
    tui.output(b"\x1b[1m\x1b[2mX\x1b[0m\x1b[2m\x1b[1mY\x1b[0m");
    let x = tui.cell(0, 0);
    assert!(x.modifier.contains(Modifier::DIM) && !x.modifier.contains(Modifier::BOLD));
    let y = tui.cell(0, 1);
    assert!(y.modifier.contains(Modifier::BOLD) && !y.modifier.contains(Modifier::DIM));
}

#[test]
fn long_line_wraps() {
    let mut tui = Tui::new(&["build"]);
    let width = usize::from(tui.state.active_task().unwrap().output.size().1);
    let long: String = (0..width + 4).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    tui.output(format!("{long}\r\nnext").as_bytes());
    assert_eq!(tui.pane_row(0), long[..width]);
    assert_eq!(tui.pane_row(1), long[width..]);
    assert_eq!(tui.pane_row(2), "next");
}

#[test]
fn wide_characters() {
    let mut tui = Tui::new(&["build"]);
    tui.output("日本語 🔥ok".as_bytes());
    assert_eq!(tui.cell(0, 0).symbol(), "日");
    assert_eq!(tui.cell(0, 2).symbol(), "本");
    assert_eq!(tui.cell(0, 4).symbol(), "語");
    assert_eq!(tui.cell(0, 6).symbol(), " ");
    assert_eq!(tui.cell(0, 7).symbol(), "🔥");
    assert_eq!(tui.cell(0, 9).symbol(), "o");
    assert_eq!(tui.cell(0, 10).symbol(), "k");
}

#[test]
fn carriage_return_overwrites_and_erase_line() {
    let mut tui = Tui::new(&["build"]);
    tui.output(b"progress 50%\rprogress 100%\r\nlong line\r\x1b[Kshort\r\n");
    assert_eq!(tui.pane_row(0), "progress 100%");
    assert_eq!(tui.pane_row(1), "short");
}

#[test]
fn clear_screen_and_cursor_home() {
    let mut tui = Tui::new(&["build"]);
    tui.output(b"one\r\ntwo\r\nthree\r\n\x1b[2J\x1b[Hfresh");
    assert_eq!(tui.pane_rows(), ["fresh", "", "", "", "", ""]);
}

#[test]
fn cursor_positioning() {
    let mut tui = Tui::new(&["build"]);
    // CUP to row 3 col 5, then CUU/CUF relative moves
    tui.output(b"\x1b[3;5HX\x1b[2A\x1b[3CY");
    assert_eq!(tui.pane_rows(), ["        Y", "", "    X", "", "", ""]);
}

#[test]
fn alternate_screen_switches_and_restores() {
    let mut tui = Tui::new(&["build"]);
    tui.output(b"main\r\n\x1b[?1049h\x1b[Halt");
    assert_eq!(tui.pane_rows(), ["alt", "", "", "", "", ""]);

    tui.output(b"\x1b[?1049l");
    assert_eq!(tui.pane_rows()[0], "main");
}

#[test]
fn scrollback_scrolling() {
    let mut tui = Tui::new(&["build"]);
    for l in lines(20) {
        tui.output(format!("{l}\r\n").as_bytes());
    }
    // Bottom of the scrollback: last output line is followed by the cursor row
    assert_eq!(tui.pane_rows(), ["line16", "line17", "line18", "line19", "line20", ""]);
    assert_eq!(tui.scrollbar(), "↑║║║║║███↓");

    tui.send(AppCommand::ScrollUp(ScrollSize::One));
    assert_eq!(tui.pane_row(0), "line15");

    tui.send(AppCommand::ScrollUp(ScrollSize::Half));
    assert_eq!(tui.pane_row(0), "line12");

    tui.send(AppCommand::ScrollUp(ScrollSize::Edge));
    assert_eq!(
        tui.pane_rows(),
        ["line01", "line02", "line03", "line04", "line05", "line06"]
    );
    assert_eq!(tui.scrollbar(), "↑███║║║║║↓");

    tui.send(AppCommand::ScrollDown(ScrollSize::Full));
    assert_eq!(tui.pane_row(0), "line07");

    tui.send(AppCommand::ScrollDown(ScrollSize::Edge));
    assert_eq!(tui.pane_row(0), "line16");
}

#[test]
fn resize_reflows_output() {
    let mut tui = Tui::new(&["build"]);
    let width = usize::from(tui.state.active_task().unwrap().output.size().1);
    let long: String = (0..width + 4).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    tui.output(format!("{long}\r\n").as_bytes());
    assert_eq!(tui.pane_row(1), long[width..]);

    // Wider terminal: the line fits on a single row after re-parsing
    tui.resize(ROWS, COLS + 20);
    assert_eq!(tui.pane_row(0), long);
    assert_eq!(tui.pane_row(1), "");

    // Back to the original size: wraps again
    tui.resize(ROWS, COLS);
    assert_eq!(tui.pane_row(0), long[..width]);
    assert_eq!(tui.pane_row(1), long[width..]);
}

#[test]
fn toggle_sidebar_uses_full_width() {
    let mut tui = Tui::new(&["build"]);
    tui.output(b"hello\r\n");
    tui.send(AppCommand::ToggleSidebar);
    let lines = tui.lines();
    assert!(lines[0].starts_with("% build (Waiting)"), "{}", lines[0]);
    assert!(lines[2].starts_with("hello"), "{}", lines[2]);
    assert!(lines[9].contains("[h] Show Tasks"), "{}", lines[9]);

    tui.send(AppCommand::ToggleSidebar);
    let lines = tui.lines();
    assert!(lines[0].starts_with("🏕  Tasks"), "{}", lines[0]);
    assert_eq!(tui.pane_row(0), "hello");
}

#[test]
fn search_highlights_matches() {
    let mut tui = Tui::new(&["build"]);
    tui.output(b"foo bar\r\nbaz foo\r\n");
    tui.send(AppCommand::EnterSearch { backward: false });
    for c in "foo".chars() {
        tui.send(AppCommand::SearchInputChar(c));
    }
    let footer = tui.footer();
    assert!(footer.contains("/foo█"), "{footer}");
    assert!(footer.contains("[Esc] Exit Search"), "{footer}");

    tui.send(AppCommand::SearchRun);
    // First match highlighted, second not
    assert_eq!(tui.cell(0, 0).bg, Color::Indexed(3));
    assert_eq!(tui.cell(0, 2).bg, Color::Indexed(3));
    assert_eq!(tui.cell(0, 3).bg, Color::Reset);
    assert_eq!(tui.cell(1, 4).bg, Color::Reset);
    assert!(tui.footer().contains("Next/Prev Match"));

    tui.send(AppCommand::SearchNext);
    assert_eq!(tui.cell(0, 0).bg, Color::Reset);
    assert_eq!(tui.cell(1, 4).bg, Color::Indexed(3));
    assert_eq!(tui.cell(1, 6).bg, Color::Indexed(3));

    tui.send(AppCommand::ExitSearch);
    assert_eq!(tui.cell(1, 4).bg, Color::Reset);
    assert!(!tui.footer().contains("Next/Prev Match"));
}

#[test]
fn backward_search_starts_above_the_view_and_reverses_n() {
    let mut tui = Tui::new(&["build"]);
    for l in lines(20) {
        tui.output(format!("{l}\r\n").as_bytes());
    }
    // The view sits at the bottom, so a backward search for the common prefix
    // starts at the last match above it rather than wrapping to the top.
    tui.send(AppCommand::EnterSearch { backward: true });
    for c in "line0".chars() {
        tui.send(AppCommand::SearchInputChar(c));
    }
    let footer = tui.footer();
    assert!(footer.contains("?line0█"), "{footer}");

    tui.send(AppCommand::SearchRun);
    assert_eq!(tui.pane_row(0), "line09");

    // n keeps walking up the log, N turns back down.
    tui.send(AppCommand::SearchNext);
    assert_eq!(tui.pane_row(0), "line08");
    tui.send(AppCommand::SearchPrevious);
    assert_eq!(tui.pane_row(0), "line09");
}

#[test]
fn search_scrolls_to_match_in_scrollback() {
    let mut tui = Tui::new(&["build"]);
    for l in lines(20) {
        tui.output(format!("{l}\r\n").as_bytes());
    }
    tui.send(AppCommand::EnterSearch { backward: false });
    for c in "line03".chars() {
        tui.send(AppCommand::SearchInputChar(c));
    }
    tui.send(AppCommand::SearchRun);
    assert_eq!(tui.pane_row(0), "line03");
    for col in 0..6 {
        assert_eq!(tui.cell(0, col).bg, Color::Indexed(3), "col {col}");
    }
}

#[test]
fn mouse_selection_is_inverted_and_copyable() {
    let mut tui = Tui::new(&["build"]);
    tui.output(b"hello world\r\nsecond\r\n");
    // Drag from col 2 to col 6 on the first output row (pane-relative)
    tui.send(AppCommand::UpdateSelection {
        rows: 0,
        cols: 2,
        edge: None,
    });
    tui.send(AppCommand::UpdateSelection {
        rows: 0,
        cols: 6,
        edge: None,
    });
    assert!(!tui.cell(0, 1).modifier.contains(Modifier::REVERSED));
    for col in 2..=6 {
        assert!(tui.cell(0, col).modifier.contains(Modifier::REVERSED), "col {col}");
    }
    assert!(!tui.cell(0, 7).modifier.contains(Modifier::REVERSED));
    assert_eq!(
        tui.state.active_task().unwrap().output.copy_selection().as_deref(),
        Some("llo w")
    );

    // Multi-line selection
    tui.send(AppCommand::UpdateSelection {
        rows: 1,
        cols: 2,
        edge: None,
    });
    assert_eq!(
        tui.state.active_task().unwrap().output.copy_selection().as_deref(),
        Some("llo world\nsec")
    );

    tui.send(AppCommand::ClearSelection);
    assert!(!tui.cell(0, 3).modifier.contains(Modifier::REVERSED));
    assert_eq!(tui.state.active_task().unwrap().output.copy_selection(), None);
}

#[test]
fn line_selection_selects_whole_row() {
    let mut tui = Tui::new(&["build"]);
    tui.output(b"hello world\r\nsecond\r\n");
    tui.send(AppCommand::LineSelection { rows: 1 });
    assert_eq!(
        tui.state.active_task().unwrap().output.copy_selection().as_deref(),
        Some("second")
    );
    assert!(tui.cell(1, 0).modifier.contains(Modifier::REVERSED));
    assert!(!tui.cell(0, 0).modifier.contains(Modifier::REVERSED));
}

#[test]
fn entire_screen_includes_scrollback() {
    let mut tui = Tui::new(&["build"]);
    for (i, l) in lines(20).into_iter().enumerate() {
        tui.output(format!("\x1b[3{}m{l}\x1b[0m\r\n", (i + 1) % 8).as_bytes());
    }
    let expected = lines(20).join("\n");
    let task = tui.state.active_task().unwrap();
    let screen = task.output.entire_screen();
    assert_eq!(screen.contents(), expected);

    // Rows re-rendered with escape codes (as done by `persist_screen`) must
    // reproduce the same text and colors when fed to a fresh terminal.
    let (rows, cols) = screen.size();
    assert_eq!(rows, 20);
    let mut parser = vt100::Parser::new(rows as u16 + 1, cols, 0);
    for row in screen.rows_formatted(0, cols) {
        parser.process(&row);
        parser.process(b"\r\n");
    }
    assert_eq!(parser.screen().contents(), expected);
    assert_eq!(parser.screen().cell(0, 0).unwrap().fgcolor(), vt100::Color::Idx(1));
    assert_eq!(parser.screen().cell(1, 0).unwrap().fgcolor(), vt100::Color::Idx(2));
}

#[test]
fn quitting_closes_the_help_dialog_and_shows_the_quit_message() {
    let mut tui = Tui::new(&["build"]);
    tui.output(b"hello\r\n");

    tui.send(AppCommand::OpenHelp);
    assert!(tui.lines().iter().any(|l| l.contains("Basic")), "help dialog not shown");

    // The help dialog covers the whole frame, so quitting must close it for
    // the quit message and the task output to stay visible.
    tui.send(AppCommand::Quit);
    assert!(
        !tui.lines().iter().any(|l| l.contains("Basic")),
        "help dialog still shown"
    );
    assert!(tui.footer().contains("Quitting..."), "{}", tui.footer());
    assert_eq!(tui.pane_row(0), "hello");
}

/// A record about a task goes into that task's pane, next to the output it is
/// about, so the two read in the order they happened.
#[test]
fn task_log_goes_into_its_pane() {
    let mut tui = Tui::new(&["build", "serve"]);
    tui.output(b"building\r\n");
    tui.log(Some("build"), Level::WARN, "Task is restarting (1/3)");
    tui.output(b"building again\r\n");

    assert_eq!(
        tui.pane_rows()[..3],
        ["building", "Task is restarting (1/3)", "building again"]
    );
    assert_eq!(tui.cell(1, 0).fg, Color::Indexed(3)); // yellow, for a warning

    // The record is about `build`, so the pane of `serve` does not have it
    tui.send(AppCommand::Down);
    assert!(tui.pane_row(0).is_empty());
}

/// A record about no task has no pane to go in, so the ones worth acting on are
/// shown at the foot of the screen instead.
#[test]
fn non_task_log_is_shown_at_the_foot() {
    let mut tui = Tui::new(&["build"]);
    tui.log(None, Level::ERROR, "Failed to copy to the clipboard");

    // The pane of the task is left alone
    assert!(tui.pane_row(0).is_empty());
    assert!(tui
        .lines()
        .iter()
        .any(|l| l.contains("Failed to copy to the clipboard")));
}

/// A record below `warn` is not worth interrupting for; it is left to the log file.
#[test]
fn non_task_log_below_warn_is_not_shown() {
    let mut tui = Tui::new(&["build"]);
    tui.log(None, Level::INFO, "Start watching files");

    assert!(!tui.lines().iter().any(|l| l.contains("Start watching")));
}
