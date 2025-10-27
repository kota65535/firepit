use std::{borrow::Cow, env, f64::consts::PI};

use console::{Style, StyledObject};
use once_cell::sync::Lazy;

#[derive(Debug, Clone, Copy)]
pub struct ColorConfig {
    pub should_strip_ansi: bool,
}

impl ColorConfig {
    pub fn new(should_strip_ansi: bool) -> Self {
        Self { should_strip_ansi }
    }

    /// Infer the color choice from environment variables and checking if stdout
    /// is a tty
    pub fn infer() -> Self {
        let env_setting = std::env::var("FORCE_COLOR")
            .ok()
            .and_then(|force_color| match force_color.as_str() {
                "false" | "0" => Some(true),
                "true" | "1" | "2" | "3" => Some(false),
                _ => None,
            });
        let should_strip_ansi = env_setting.unwrap_or_else(|| !atty::is(atty::Stream::Stdout));
        Self { should_strip_ansi }
    }

    /// Apply the UI color mode to the given styled object
    ///
    /// This is required to match the Go turborepo coloring logic which differs
    /// from console's coloring detection.
    pub fn apply<D>(&self, obj: StyledObject<D>) -> StyledObject<D> {
        // Setting this to false will skip emitting any ansi codes associated
        // with the style when the object is displayed.
        obj.force_styling(!self.should_strip_ansi)
    }

    // Ported from Go code. Converts an index to a color along the rainbow
    fn rainbow_rgb(i: usize) -> (u8, u8, u8) {
        let f = 0.275;
        let r = (f * i as f64 + 4.0 * PI / 3.0).sin() * 127.0 + 128.0;
        let g = 45.0;
        let b = (f * i as f64).sin() * 127.0 + 128.0;

        (r as u8, g as u8, b as u8)
    }

    pub fn rainbow<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.should_strip_ansi {
            return Cow::Borrowed(text);
        }

        // On the macOS Terminal, the rainbow colors don't show up correctly.
        // Instead, we print in bold magenta
        if matches!(env::var("TERM_PROGRAM"), Ok(terminal_program) if terminal_program == "Apple_Terminal") {
            return BOLD.apply_to(MAGENTA.apply_to(text)).to_string().into();
        }

        let mut out = Vec::new();
        for (i, c) in text.char_indices() {
            let (r, g, b) = Self::rainbow_rgb(i);
            out.push(format!("\x1b[1m\x1b[38;2;{};{};{}m{}\x1b[0m\x1b[0;1m", r, g, b, c));
        }
        out.push(RESET.to_string());

        Cow::Owned(out.join(""))
    }
}

pub static GREY: Lazy<Style> = Lazy::new(|| Style::new().dim());
pub static RED: Lazy<Style> = Lazy::new(|| Style::new().red());
pub static CYAN: Lazy<Style> = Lazy::new(|| Style::new().cyan());
pub static BOLD: Lazy<Style> = Lazy::new(|| Style::new().bold());
pub static MAGENTA: Lazy<Style> = Lazy::new(|| Style::new().magenta());
pub static YELLOW: Lazy<Style> = Lazy::new(|| Style::new().yellow());
pub static BOLD_YELLOW_REVERSE: Lazy<Style> = Lazy::new(|| Style::new().yellow().bold().reverse());
pub static UNDERLINE: Lazy<Style> = Lazy::new(|| Style::new().underlined());
pub static BOLD_CYAN: Lazy<Style> = Lazy::new(|| Style::new().cyan().bold());
pub static BOLD_GREY: Lazy<Style> = Lazy::new(|| Style::new().dim().bold());
pub static BOLD_YELLOW: Lazy<Style> = Lazy::new(|| Style::new().yellow().bold());
pub static BOLD_GREEN: Lazy<Style> = Lazy::new(|| Style::new().green().bold());
pub static BOLD_RED: Lazy<Style> = Lazy::new(|| Style::new().red().bold());

pub const RESET: &str = "\x1b[0m";

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_color_config_strips_ansi() {
        let color_config = ColorConfig::new(true);
        let grey_str = GREY.apply_to("gray");
        assert_eq!(format!("{}", color_config.apply(grey_str)), "gray");
    }

    #[test]
    fn test_color_config_resets_term() {
        let color_config = ColorConfig::new(false);
        let grey_str = GREY.apply_to("gray");
        assert_eq!(format!("{}", color_config.apply(grey_str)), "\u{1b}[2mgray\u{1b}[0m");
    }
}
