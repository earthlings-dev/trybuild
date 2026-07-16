//! The owned [`Reporter`]: the single owner of trybuild's terminal output,
//! replacing the former process-global `Term` plus `print!`/`println!` macros.

use std::fmt;
use std::io;
use std::io::Write;

use termcolor::Color;
use termcolor::ColorChoice;
use termcolor::ColorSpec;
use termcolor::StandardStream;
use termcolor::WriteColor;

/// Buffered, colorized writer; the single owner of terminal output.
pub(in crate::internal) struct Reporter<W> {
  /// The underlying colorized stream.
  stream:        W,
  /// The color spec to apply at the start of each line.
  spec:          ColorSpec,
  /// Whether the next byte written begins a fresh line.
  start_of_line: bool,
}

impl Reporter<StandardStream> {
  /// Creates a reporter writing to stderr with automatic color detection.
  #[allow(
    clippy::single_call_fn,
    reason = "production reporter construction binds automatic color detection to the stderr output boundary"
  )]
  pub(in crate::internal) fn new() -> Self {
    Self {
      stream:        StandardStream::stderr(ColorChoice::Auto),
      spec:          ColorSpec::new(),
      start_of_line: true,
    }
  }
}

impl<W> Reporter<W>
where
  W: WriteColor,
{
  /// Creates a reporter over an injected color writer.
  #[cfg(test)]
  pub(in crate::internal) fn from_stream(stream: W) -> Self {
    Self {
      stream,
      spec: ColorSpec::new(),
      start_of_line: true,
    }
  }

  /// Returns the underlying stream, for inspecting captured output in tests.
  #[cfg(test)]
  pub(in crate::internal) fn into_stream(self) -> W {
    self.stream
  }

  /// Writes formatted output. Terminal-write failures are unrecoverable and
  /// are dropped here rather than propagated.
  pub(in crate::internal) fn emit(&mut self, args: fmt::Arguments<'_>) {
    self.write_fmt(args).unwrap_or_default();
  }

  /// [`emit`](Self::emit)s the arguments followed by a newline.
  pub(in crate::internal) fn emitln(&mut self, args: fmt::Arguments<'_>) {
    self.emit(args);
    self.emit(format_args!("\n"));
  }

  /// Sets subsequent output to bold.
  pub(in crate::internal) fn bold(&mut self) {
    self.set(ColorSpec::new().set_bold(true));
  }

  /// Sets subsequent output to the given foreground color.
  pub(in crate::internal) fn color(&mut self, color: Color) {
    self.set(ColorSpec::new().set_fg(Some(color)));
  }

  /// Sets subsequent output to bold in the given foreground color.
  pub(in crate::internal) fn bold_color(&mut self, color: Color) {
    self.set(ColorSpec::new().set_bold(true).set_fg(Some(color)));
  }

  /// Clears all styling for subsequent output.
  pub(in crate::internal) fn reset(&mut self) {
    self.spec = ColorSpec::new();
    self.stream.reset().unwrap_or_default();
  }

  /// Records a new color spec, to be applied at the next start-of-line.
  fn set(&mut self, spec: &ColorSpec) {
    if self.spec != *spec {
      self.spec = spec.clone();
      self.start_of_line = true;
    }
  }
}

impl<W> Write for Reporter<W>
where
  W: WriteColor,
{
  // Color one line at a time because Travis does not preserve color setting
  // across output lines.
  fn write(&mut self, mut buf: &[u8]) -> io::Result<usize> {
    if self.spec.is_none() {
      return self.stream.write(buf);
    }

    let len = buf.len();
    while !buf.is_empty() {
      if self.start_of_line {
        self.stream.set_color(&self.spec).unwrap_or_default();
      }
      if let Some(line_len) = buf.iter().position(|byte| *byte == b'\n') {
        self.stream.write_all(buf.get(..=line_len).unwrap_or(buf))?;
        self.start_of_line = true;
        buf = buf.get(line_len.saturating_add(1)..).unwrap_or(&[]);
      } else {
        self.stream.write_all(buf)?;
        self.start_of_line = false;
        break;
      }
    }
    Ok(len)
  }

  fn flush(&mut self) -> io::Result<()> {
    self.stream.flush()
  }
}

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_ok_source;
  use termcolor::NoColor;

  use super::*;

  #[test]
  fn injected_stream_captures_rendered_output() -> Result<(), TestFailure> {
    let mut reporter = Reporter::from_stream(NoColor::new(Vec::new()));

    reporter.bold_color(Color::Red);
    reporter.emitln(format_args!("hello"));

    let stream = reporter.into_stream();
    let output = ensure_ok_source(
      String::from_utf8(stream.into_inner()),
      "captured reporter output remains valid UTF-8",
    )?;
    ensure_eq(&output.as_str(), &"hello\n", "reporter writes through the injected stream")
  }
}
