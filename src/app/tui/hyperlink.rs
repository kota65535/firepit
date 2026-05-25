use crossterm::cursor::{MoveTo, RestorePosition, SavePosition};
use crossterm::style::{
    Attribute, Color as CColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::{queue, style::Print};
use std::io::Write;
use unicode_width::UnicodeWidthStr;

/// A URL and its screen coordinate segments
#[derive(Debug, Clone)]
pub struct UrlSpan {
    pub url: String,
    pub segments: Vec<UrlSegment>,
}

/// A single-row portion of a URL on screen
#[derive(Debug, Clone)]
pub struct UrlSegment {
    pub row: u16,
    pub start_col: u16,
    pub end_col: u16,
}

/// Detect URLs in the visible rows of a vt100 screen.
///
/// Joins wrapped rows into logical lines (same pattern as the search logic),
/// finds `https://` and `http://` URLs, then maps them back to (row, col) segments.
pub fn detect_urls(screen: &vt100::Screen) -> Vec<UrlSpan> {
    let size = screen.size();
    let cols = size.1;

    let mut urls = Vec::new();
    let mut line_buf = String::new();
    let mut row_widths: Vec<(u16, usize)> = Vec::new();

    for (row_idx, row) in screen.grid().visible_rows().enumerate() {
        let mut s = String::new();
        row.write_contents(&mut s, 0, cols, true);
        let current_row_width = s.width();
        row_widths.push((row_idx as u16, current_row_width));
        line_buf.push_str(&s);

        if row.wrapped() {
            continue;
        }

        find_urls_in_line(&line_buf, &row_widths, &mut urls);
        line_buf.clear();
        row_widths.clear();
    }

    // Handle trailing wrapped rows
    if !line_buf.is_empty() {
        find_urls_in_line(&line_buf, &row_widths, &mut urls);
    }

    urls
}

/// Find URLs in a logical line and map them to row/col segments
fn find_urls_in_line(line: &str, row_widths: &[(u16, usize)], urls: &mut Vec<UrlSpan>) {
    for prefix in &["https://", "http://"] {
        let mut search_start = 0;
        while let Some(rel_start) = line[search_start..].find(prefix) {
            let abs_start = search_start + rel_start;

            // Find URL end: stop at whitespace or common URL delimiters
            let url_end = line[abs_start..]
                .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '>' | '<' | ')' | ']' | '|'))
                .map(|i| abs_start + i)
                .unwrap_or(line.len());

            let url_text = &line[abs_start..url_end];
            if url_text.len() <= prefix.len() {
                search_start = url_end;
                continue;
            }

            let display_start = line[..abs_start].width();
            let url_display_width = url_text.width();

            let segments = map_to_segments(display_start, url_display_width, row_widths);
            if !segments.is_empty() {
                urls.push(UrlSpan {
                    url: url_text.to_string(),
                    segments,
                });
            }

            search_start = url_end;
        }
    }
}

/// Map a display-width range to (row, start_col, end_col) segments
fn map_to_segments(
    display_start: usize,
    display_width: usize,
    row_widths: &[(u16, usize)],
) -> Vec<UrlSegment> {
    let display_end = display_start + display_width;
    let mut segments = Vec::new();
    let mut cumulative = 0usize;

    for &(row_idx, row_width) in row_widths {
        let row_start = cumulative;
        let row_end = cumulative + row_width;

        if display_start < row_end && display_end > row_start {
            let seg_start = display_start.saturating_sub(row_start) as u16;
            let seg_end = if display_end < row_end {
                (display_end - row_start) as u16
            } else {
                row_width as u16
            };

            segments.push(UrlSegment {
                row: row_idx,
                start_col: seg_start,
                end_col: seg_end,
            });
        }

        cumulative = row_end;
    }

    segments
}

/// Convert a vt100::Color to a crossterm Color
fn vt100_to_crossterm_color(color: vt100::Color) -> CColor {
    match color {
        vt100::Color::Default => CColor::Reset,
        vt100::Color::Idx(i) => CColor::AnsiValue(i),
        vt100::Color::Rgb(r, g, b) => CColor::Rgb { r, g, b },
    }
}

/// Write OSC 8 hyperlink sequences to stdout for detected URLs.
///
/// For each URL segment, re-renders the text with the original vt100 cell styles
/// wrapped in OSC 8 start/end sequences so the host terminal creates a proper hyperlink.
pub fn write_hyperlinks<W: Write>(
    writer: &mut W,
    urls: &[UrlSpan],
    screen: &vt100::Screen,
    offset_x: u16,
    offset_y: u16,
) -> std::io::Result<()> {
    if urls.is_empty() {
        return Ok(());
    }

    // Save cursor position so ratatui's next draw starts from the right place
    queue!(writer, SavePosition)?;

    for (link_idx, url_span) in urls.iter().enumerate() {
        // Use id parameter to group multi-segment links (OSC 8 spec recommendation)
        let osc8_start = format!("\x1b]8;id=firepit-{};{}\x1b\\", link_idx, url_span.url);
        let osc8_end = "\x1b]8;;\x1b\\";

        for segment in &url_span.segments {
            let screen_y = offset_y + segment.row;
            let screen_x_start = offset_x + segment.start_col;

            queue!(writer, MoveTo(screen_x_start, screen_y))?;
            writer.write_all(osc8_start.as_bytes())?;

            // Re-render each cell with its original style
            let mut last_fg: Option<CColor> = None;
            let mut last_bg: Option<CColor> = None;
            for col in segment.start_col..segment.end_col {
                if let Some(cell) = screen.cell(segment.row, col) {
                    if cell.is_wide_continuation() {
                        continue;
                    }

                    let fg = vt100_to_crossterm_color(cell.fgcolor());
                    let bg = vt100_to_crossterm_color(cell.bgcolor());

                    if last_fg.as_ref() != Some(&fg) {
                        queue!(writer, SetForegroundColor(fg))?;
                        last_fg = Some(fg);
                    }
                    if last_bg.as_ref() != Some(&bg) {
                        queue!(writer, SetBackgroundColor(bg))?;
                        last_bg = Some(bg);
                    }

                    // Apply text modifiers
                    if cell.bold() {
                        queue!(writer, SetAttribute(Attribute::Bold))?;
                    }
                    if cell.underline() {
                        queue!(writer, SetAttribute(Attribute::Underlined))?;
                    }

                    let contents = cell.contents();
                    if !contents.is_empty() {
                        queue!(writer, Print(&contents))?;
                    }

                    // Reset modifiers for next cell
                    if cell.bold() || cell.underline() {
                        queue!(writer, SetAttribute(Attribute::Reset))?;
                        // Force re-apply colors after reset
                        last_fg = None;
                        last_bg = None;
                    }
                }
            }

            writer.write_all(osc8_end.as_bytes())?;
        }
    }

    // Reset styles and restore cursor position
    queue!(
        writer,
        SetForegroundColor(CColor::Reset),
        SetBackgroundColor(CColor::Reset),
        SetAttribute(Attribute::Reset),
        RestorePosition,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_to_segments_single_row() {
        // URL at cols 5..15 in a single row of width 80
        let row_widths = vec![(0, 80)];
        let segments = map_to_segments(5, 10, &row_widths);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].row, 0);
        assert_eq!(segments[0].start_col, 5);
        assert_eq!(segments[0].end_col, 15);
    }

    #[test]
    fn test_map_to_segments_wrapped() {
        // Two rows of width 40 each. URL starts at col 35 of row 0, 15 chars wide.
        // Row 0: cols 35..40 (5 chars)
        // Row 1: cols 0..10 (10 chars)
        let row_widths = vec![(0, 40), (1, 40)];
        let segments = map_to_segments(35, 15, &row_widths);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].row, 0);
        assert_eq!(segments[0].start_col, 35);
        assert_eq!(segments[0].end_col, 40);
        assert_eq!(segments[1].row, 1);
        assert_eq!(segments[1].start_col, 0);
        assert_eq!(segments[1].end_col, 10);
    }

    #[test]
    fn test_map_to_segments_entirely_in_second_row() {
        let row_widths = vec![(0, 40), (1, 40)];
        let segments = map_to_segments(45, 10, &row_widths);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].row, 1);
        assert_eq!(segments[0].start_col, 5);
        assert_eq!(segments[0].end_col, 15);
    }

    #[test]
    fn test_find_urls_in_line_basic() {
        let line = "Visit https://example.com for more info";
        let row_widths = vec![(0, line.width())];
        let mut urls = Vec::new();
        find_urls_in_line(line, &row_widths, &mut urls);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "https://example.com");
    }

    #[test]
    fn test_find_urls_in_line_multiple() {
        let line = "Go to https://a.com and http://b.com now";
        let row_widths = vec![(0, line.width())];
        let mut urls = Vec::new();
        find_urls_in_line(line, &row_widths, &mut urls);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url, "https://a.com");
        assert_eq!(urls[1].url, "http://b.com");
    }

    #[test]
    fn test_find_urls_in_line_no_url() {
        let line = "No URLs here";
        let row_widths = vec![(0, line.width())];
        let mut urls = Vec::new();
        find_urls_in_line(line, &row_widths, &mut urls);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_find_urls_skips_bare_prefix() {
        let line = "Invalid https:// alone";
        let row_widths = vec![(0, line.width())];
        let mut urls = Vec::new();
        find_urls_in_line(line, &row_widths, &mut urls);
        assert!(urls.is_empty());
    }
}
