use crate::app::event::{Event, ScrollSize};
use crate::app::tui::lib::RingBuffer;
use crate::app::tui::LayoutSections;
use crate::tokio_spawn;
use crossterm::event::{EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use itertools::Itertools;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct InputHandler {
    click_times: RingBuffer<Instant>,
}

pub struct InputOptions<'a> {
    pub focus: &'a LayoutSections,
    pub has_selection: bool,
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
            if a.duration_since(*b) > Duration::from_millis(300) {
                break;
            }
            count += 1;
        }
        count
    }

    pub fn handle(&mut self, event: crossterm::event::Event, options: InputOptions) -> Option<Event> {
        match event {
            crossterm::event::Event::Key(k) => translate_key_event(options, k),
            crossterm::event::Event::Mouse(m) => match m.kind {
                crossterm::event::MouseEventKind::ScrollDown => Some(Event::ScrollDown(ScrollSize::One)),
                crossterm::event::MouseEventKind::ScrollUp => Some(Event::ScrollUp(ScrollSize::One)),
                crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                    self.click_times.push(Instant::now());
                    let num_clicks = self.num_of_multiple_clicks();
                    if num_clicks == 1 {
                        Some(Event::Mouse(m))
                    } else {
                        debug!("Clicked {} times", num_clicks);
                        Some(Event::MouseMultiClick(m, num_clicks))
                    }
                }
                crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left) => Some(Event::Mouse(m)),
                _ => None,
            },
            crossterm::event::Event::Resize(cols, rows) => Some(Event::Resize { rows, cols }),
            _ => None,
        }
    }
}

/// Converts a crossterm key event into a TUI interaction event
fn translate_key_event(options: InputOptions, key_event: KeyEvent) -> Option<Event> {
    if key_event.kind == KeyEventKind::Release {
        return None;
    }
    match key_event.code {
        // On task list
        KeyCode::Char('/') if options.on_task_list() => Some(Event::EnterSearch),
        KeyCode::Char('h') if options.on_task_list() => Some(Event::ToggleSidebar),
        KeyCode::Char('e') if options.on_task_list() => Some(Event::ScrollDown(ScrollSize::One)),
        KeyCode::Char('y') if options.on_task_list() => Some(Event::ScrollUp(ScrollSize::One)),
        KeyCode::Char('d') if options.on_task_list() => Some(Event::ScrollDown(ScrollSize::Half)),
        KeyCode::Char('u') if options.on_task_list() => Some(Event::ScrollUp(ScrollSize::Half)),
        KeyCode::Char('f') if options.on_task_list() => Some(Event::ScrollDown(ScrollSize::Full)),
        KeyCode::Char('b') if options.on_task_list() => Some(Event::ScrollUp(ScrollSize::Full)),
        KeyCode::Char('G') if options.on_task_list() => Some(Event::ScrollDown(ScrollSize::Edge)),
        KeyCode::Char('g') if options.on_task_list() => Some(Event::ScrollUp(ScrollSize::Edge)),
        KeyCode::Char('j') if options.on_task_list() => Some(Event::Down),
        KeyCode::Char('k') if options.on_task_list() => Some(Event::Up),
        KeyCode::Char('n') if options.on_task_list() => Some(Event::SearchNext),
        KeyCode::Char('N') if options.on_task_list() => Some(Event::SearchPrevious),
        KeyCode::Up if options.on_task_list() => Some(Event::Up),
        KeyCode::Down if options.on_task_list() => Some(Event::Down),
        KeyCode::Char('c') if options.has_selection => Some(Event::CopySelection),
        KeyCode::Enter if options.on_task_list() => Some(Event::EnterInteractive),

        // On pane (interactive mode)
        KeyCode::Char('z') if options.on_pane() && key_event.modifiers == KeyModifiers::CONTROL => {
            Some(Event::ExitInteractive)
        }
        // If we're in interactive mode, convert the key event to bytes to send to stdin
        _ if options.on_pane() => Some(Event::Input {
            bytes: encode_key(key_event),
        }),

        // On search
        KeyCode::Char(c) if options.on_search() => Some(Event::SearchInputChar(c)),
        KeyCode::Backspace if options.on_search() => Some(Event::SearchBackspace),
        KeyCode::Esc if options.on_search() || options.on_task_list() => Some(Event::ExitSearch),
        KeyCode::Enter if options.on_search() => Some(Event::SearchRun),

        // Global
        KeyCode::Char('c') if key_event.modifiers == KeyModifiers::CONTROL => {
            ctrl_c();
            Some(Event::Stop)
        }
        _ => None,
    }
}

fn ctrl_c() -> Option<Event> {
    use nix::sys::signal;
    match signal::raise(signal::SIGINT) {
        Ok(_) => None,
        // We're unable to send the signal, stop rendering to force shutdown
        Err(_) => {
            debug!("unable to send sigint, shutting down");
            Some(Event::Stop)
        }
    }
}

// Inspired by mprocs encode_term module
// https://github.com/pvolok/mprocs/blob/08d17adebd110501106f86124ef1955fb2beb881/src/encode_term.rs
fn encode_key(key: KeyEvent) -> Vec<u8> {
    use crossterm::event::KeyCode::*;

    if key.kind == KeyEventKind::Release {
        return Vec::new();
    }

    let code = key.code;
    let mods = key.modifiers;

    let mut buf = String::new();

    let code = normalize_shift_to_upper_case(code, &mods);

    // Normalize Backspace and Delete
    let code = match code {
        Char('\x7f') => Backspace,
        Char('\x08') => Delete,
        c => c,
    };

    match code {
        Char(c) if mods.contains(KeyModifiers::CONTROL) && ctrl_mapping(c).is_some() => {
            let c = ctrl_mapping(c).unwrap();
            if mods.contains(KeyModifiers::ALT) {
                buf.push(0x1b as char);
            }
            buf.push(c);
        }

        // When alt is pressed, send escape first to indicate to the peer that
        // ALT is pressed.  We do this only for ascii alnum characters because
        // eg: on macOS generates altgr style glyphs and keeps the ALT key
        // in the modifier set.  This confuses eg: zsh which then just displays
        // <fffffffff> as the input, so we want to avoid that.
        Char(c) if (c.is_ascii_alphanumeric() || c.is_ascii_punctuation()) && mods.contains(KeyModifiers::ALT) => {
            buf.push(0x1b as char);
            buf.push(c);
        }

        Enter | Esc | Backspace => {
            let c = match code {
                Enter => '\r',
                Esc => '\x1b',
                // Backspace sends the default VERASE which is confusingly
                // the DEL ascii codepoint
                Backspace => '\x7f',
                _ => unreachable!(),
            };
            if mods.contains(KeyModifiers::ALT) {
                buf.push(0x1b as char);
            }
            buf.push(c);
        }

        Tab => {
            if mods.contains(KeyModifiers::ALT) {
                buf.push(0x1b as char);
            }
            let mods = mods & !KeyModifiers::ALT;
            if mods == KeyModifiers::CONTROL {
                buf.push_str("\x1b[9;5u");
            } else if mods == KeyModifiers::CONTROL | KeyModifiers::SHIFT {
                buf.push_str("\x1b[1;5Z");
            } else if mods == KeyModifiers::SHIFT {
                buf.push_str("\x1b[Z");
            } else {
                buf.push('\t');
            }
        }

        BackTab => {
            buf.push_str("\x1b[Z");
        }

        Char(c) => {
            buf.push(c);
        }

        Home | End | Up | Down | Right | Left => {
            let c = match code {
                Up => 'A',
                Down => 'B',
                Right => 'C',
                Left => 'D',
                Home => 'H',
                End => 'F',
                _ => unreachable!(),
            };

            if mods.contains(KeyModifiers::ALT)
                || mods.contains(KeyModifiers::SHIFT)
                || mods.contains(KeyModifiers::CONTROL)
            {
                buf.push_str("\x1b[1;");
                buf.push_str(&(1 + encode_modifiers(mods)).to_string());
                buf.push(c);
            } else {
                buf.push_str("\x1b[");
                buf.push(c);
            }
        }

        PageUp | PageDown | Insert | Delete => {
            let c = match code {
                Insert => '2',
                Delete => '3',
                PageUp => '5',
                PageDown => '6',
                _ => unreachable!(),
            };

            if mods.contains(KeyModifiers::ALT)
                || mods.contains(KeyModifiers::SHIFT)
                || mods.contains(KeyModifiers::CONTROL)
            {
                buf.push_str("\x1b[");
                buf.push(c);
                buf.push_str(&(1 + encode_modifiers(mods)).to_string());
            } else {
                buf.push_str("\x1b[");
                buf.push(c);
                buf.push('~');
            }
        }

        F(n) => {
            if mods.is_empty() && n < 5 {
                // F1-F4 are encoded using SS3 if there are no modifiers
                let s = match n {
                    1 => "\x1bOP",
                    2 => "\x1bOQ",
                    3 => "\x1bOR",
                    4 => "\x1bOS",
                    _ => unreachable!("wat?"),
                };
                buf.push_str(s);
            } else {
                // Higher numbered F-keys plus modified F-keys are encoded
                // using CSI instead of SS3.
                let intro = match n {
                    1 => "\x1b[11",
                    2 => "\x1b[12",
                    3 => "\x1b[13",
                    4 => "\x1b[14",
                    5 => "\x1b[15",
                    6 => "\x1b[17",
                    7 => "\x1b[18",
                    8 => "\x1b[19",
                    9 => "\x1b[20",
                    10 => "\x1b[21",
                    11 => "\x1b[23",
                    12 => "\x1b[24",
                    _ => panic!("unhandled fkey number {}", n),
                };
                let encoded_mods = encode_modifiers(mods);
                if encoded_mods == 0 {
                    // If no modifiers are held, don't send the modifier
                    // sequence, as the modifier encoding is a CSI-u extension.
                    buf.push_str(intro);
                    buf.push('~');
                } else {
                    buf.push_str(intro);
                    buf.push(';');
                    buf.push_str(&(1 + encoded_mods).to_string());
                    buf.push('~');
                }
            }
        }

        Null => (),
        CapsLock => (),
        ScrollLock => (),
        NumLock => (),
        PrintScreen => (),
        Pause => (),
        Menu => (),
        KeypadBegin => (),
        Media(_) => (),
        Modifier(_) => (),
    };

    buf.into_bytes()
}

/// Map c to its Ctrl equivalent.
/// In theory, this mapping is simply translating alpha characters
/// to upper case and then masking them by 0x1f, but xterm inherits
/// some built-in translation from legacy X11 so that are some
/// aliased mappings and a couple that might be technically tied
/// to US keyboard layout (particularly the punctuation characters
/// produced in combination with SHIFT) that may not be 100%
/// the right thing to do here for users with non-US layouts.
fn ctrl_mapping(c: char) -> Option<char> {
    Some(match c {
        '@' | '`' | ' ' | '2' => '\x00',
        'A' | 'a' => '\x01',
        'B' | 'b' => '\x02',
        'C' | 'c' => '\x03',
        'D' | 'd' => '\x04',
        'E' | 'e' => '\x05',
        'F' | 'f' => '\x06',
        'G' | 'g' => '\x07',
        'H' | 'h' => '\x08',
        'I' | 'i' => '\x09',
        'J' | 'j' => '\x0a',
        'K' | 'k' => '\x0b',
        'L' | 'l' => '\x0c',
        'M' | 'm' => '\x0d',
        'N' | 'n' => '\x0e',
        'O' | 'o' => '\x0f',
        'P' | 'p' => '\x10',
        'Q' | 'q' => '\x11',
        'R' | 'r' => '\x12',
        'S' | 's' => '\x13',
        'T' | 't' => '\x14',
        'U' | 'u' => '\x15',
        'V' | 'v' => '\x16',
        'W' | 'w' => '\x17',
        'X' | 'x' => '\x18',
        'Y' | 'y' => '\x19',
        'Z' | 'z' => '\x1a',
        '[' | '3' | '{' => '\x1b',
        '\\' | '4' | '|' => '\x1c',
        ']' | '5' | '}' => '\x1d',
        '^' | '6' | '~' => '\x1e',
        '_' | '7' | '/' => '\x1f',
        '8' | '?' => '\x7f', // `Delete`
        _ => return None,
    })
}

/// if SHIFT is held and we have KeyCode::Char('c') we want to normalize
/// that keycode to KeyCode::Char('C'); that is what this function does.
fn normalize_shift_to_upper_case(code: KeyCode, modifiers: &KeyModifiers) -> KeyCode {
    if modifiers.contains(KeyModifiers::SHIFT) {
        match code {
            KeyCode::Char(c) if c.is_ascii_lowercase() => KeyCode::Char(c.to_ascii_uppercase()),
            _ => code,
        }
    } else {
        code
    }
}

fn encode_modifiers(mods: KeyModifiers) -> u8 {
    let mut number = 0;
    if mods.contains(KeyModifiers::SHIFT) {
        number |= 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        number |= 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        number |= 4;
    }
    number
}
