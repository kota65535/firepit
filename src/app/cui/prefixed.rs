use crate::app::cui::lib::ColorConfig;
use crate::app::cui::line::LineWriter;
use console::StyledObject;
use std::{fmt::Display, io::Write};

pub struct PrefixedWriter<W> {
    prefix: String,
    writer: LineWriter<W>,
}

impl<W: Write> PrefixedWriter<W> {
    pub fn new(color_config: ColorConfig, prefix: StyledObject<impl Display>, writer: W) -> Self {
        let prefix = color_config.apply(prefix).to_string();
        Self {
            prefix,
            writer: LineWriter::new(writer),
        }
    }
}

impl<W: Write> Write for PrefixedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut is_first = true;
        for chunk in buf.split_inclusive(|c| *c == b'\r') {
            // Before we write the chunk we write the prefix as either:
            // - this is the first iteration and we haven't written the prefix
            // - the previous chunk ended with a \r and the cursor is currently as the start
            //   of the line so we want to rewrite the prefix over the existing prefix in
            //   the line
            // or if the last chunk is just a newline we can skip rewriting the prefix
            if is_first || chunk != b"\n" {
                self.writer.write_all(self.prefix.as_bytes())?;
            }
            self.writer.write_all(chunk)?;
            is_first = false;
        }

        // We do end up writing more bytes than this to the underlying writer, but we
        // cannot report this to the callers as the amount of bytes we report
        // written must be less than or equal to the number of bytes in the buffer.
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}
