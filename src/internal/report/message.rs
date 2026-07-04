//! Rendering a typed [`CaseReport`] (and setup failures) to the terminal through
//! a [`Reporter`].
//!
//! This is the human-facing *view* over the runner's typed result: the runner
//! computes outcomes as data and never prints; [`run`](crate::TestCases::run)
//! drives these functions per case to reproduce trybuild's progress output,
//! while [`try_run`](crate::TestCases::try_run) skips them entirely.

use std::env;
use std::ffi::OsStr;

use termcolor::Color;
use termcolor::Color::Blue;
use termcolor::Color::Green;
use termcolor::Color::Red;
use termcolor::Color::Yellow;
use termcolor::WriteColor;

use crate::TryBuildError;
use crate::internal::build::BuildError;
use crate::internal::diagnostics::DiagnosticsError;
use crate::internal::diagnostics::UnexpectedSuccess;
use crate::internal::model::Expected;
use crate::internal::outcome::CaseReport;
use crate::internal::outcome::Outcome;
use crate::internal::outcome::OverwriteDetail;
use crate::internal::outcome::PassDetail;
use crate::internal::outcome::WipDetail;
use crate::internal::report::diff::Diff;
#[cfg(all(feature = "diff", not(windows)))]
use crate::internal::report::diff::Render;
use crate::internal::report::reporter::Reporter;
use crate::internal::runner::RunOutput;
use crate::internal::runner::RunnerError;

/// Renders one resolved case: the `test <name> ...` prefix, then its outcome.
#[allow(
  clippy::single_call_fn,
  reason = "the per-case render entry point invoked from run's streaming view closure"
)]
pub(in crate::internal) fn render_case<W>(reporter: &mut Reporter<W>, case: &CaseReport, show_expected: bool)
where
  W: WriteColor,
{
  let display_name = case.path.as_os_str().to_string_lossy();

  reporter.emit(format_args!("test "));
  reporter.bold();
  reporter.emit(format_args!("{display_name}"));
  reporter.reset();

  if show_expected {
    match case.expected {
      Expected::Pass => reporter.emit(format_args!(" [should pass]")),
      Expected::CompileFail => reporter.emit(format_args!(" [should fail to compile]")),
    }
  }

  reporter.emit(format_args!(" ... "));

  match case.outcome {
    Ok(Outcome::Passed(ref detail)) => render_pass(reporter, detail),
    Ok(Outcome::CreatedWip(ref detail)) => render_wip(reporter, detail),
    Ok(Outcome::Overwrote(ref detail)) => render_overwrite(reporter, detail),
    Err(ref error) => render_error(reporter, error),
  }
}

/// Reports a setup failure that aborts the whole run.
pub(in crate::internal) fn render_setup_fail<W>(reporter: &mut Reporter<W>, error: &TryBuildError)
where
  W: WriteColor,
{
  reporter.bold_color(Red);
  reporter.emit(format_args!("ERROR"));
  reporter.reset();
  reporter.emitln(format_args!(": {error}"));
  reporter.emitln(format_args!(""));
}

/// Reports that no trybuild tests were enabled.
#[allow(
  clippy::single_call_fn,
  reason = "the no-tests-enabled render, kept beside the other case renders rather than inlined into run"
)]
pub(in crate::internal) fn render_no_tests<W>(reporter: &mut Reporter<W>)
where
  W: WriteColor,
{
  reporter.color(Yellow);
  reporter.emitln(format_args!("There are no trybuild tests enabled yet."));
  reporter.reset();
}

/// Renders a passing case: `ok`, plus any captured run output of a pass-test.
#[allow(
  clippy::single_call_fn,
  reason = "a named outcome render dispatched from render_case, paired with render_wip/render_overwrite"
)]
fn render_pass<W>(reporter: &mut Reporter<W>, detail: &PassDetail)
where
  W: WriteColor,
{
  let has_output = !detail.stdout.is_empty() || !detail.stderr.is_empty();

  reporter.color(Green);
  reporter.emitln(format_args!("ok"));
  reporter.reset();
  if has_output || !detail.warnings.is_empty() {
    reporter.emitln(format_args!(""));
  }

  warnings(reporter, &detail.warnings);

  output_streams(reporter, Yellow, &detail.stdout, &detail.stderr);
}

/// Renders a newly created `wip` snapshot and where to move it.
#[allow(
  clippy::single_call_fn,
  reason = "a named outcome render dispatched from render_case, paired with render_pass/render_overwrite"
)]
fn render_wip<W>(reporter: &mut Reporter<W>, detail: &WipDetail)
where
  W: WriteColor,
{
  let wip_display = detail.wip_path.to_string_lossy();
  let stderr_display = detail.stderr_path.to_string_lossy();

  reporter.bold_color(Yellow);
  reporter.emitln(format_args!("wip"));
  reporter.emitln(format_args!(""));
  reporter.emit(format_args!("NOTE"));
  reporter.reset();
  reporter.emitln(format_args!(": writing the following output to `{wip_display}`."));
  reporter.emitln(format_args!("Move this file to `{stderr_display}` to accept it as correct."));
  snippet(reporter, Yellow, &detail.stderr);
  reporter.emitln(format_args!(""));
}

/// Renders a snapshot overwritten in place.
#[allow(
  clippy::single_call_fn,
  reason = "a named outcome render dispatched from render_case, paired with render_pass/render_wip"
)]
fn render_overwrite<W>(reporter: &mut Reporter<W>, detail: &OverwriteDetail)
where
  W: WriteColor,
{
  let stderr_display = detail.stderr_path.to_string_lossy();

  reporter.bold_color(Yellow);
  reporter.emitln(format_args!("wip"));
  reporter.emitln(format_args!(""));
  reporter.emit(format_args!("NOTE"));
  reporter.reset();
  reporter.emitln(format_args!(": writing the following output to `{stderr_display}`."));
  snippet(reporter, Yellow, &detail.stderr);
  reporter.emitln(format_args!(""));
}

/// Renders a failing case by dispatching on the carried diagnostic data, falling
/// back to the error's `Display` for failures without a richer rendering.
#[allow(
  clippy::single_call_fn,
  reason = "the failing-case render dispatcher invoked from render_case; an if-let chain so it need not match the whole non_exhaustive \
            taxonomy"
)]
fn render_error<W>(reporter: &mut Reporter<W>, error: &TryBuildError)
where
  W: WriteColor,
{
  if let TryBuildError::Diagnostics(DiagnosticsError::Mismatch(ref detail)) = *error {
    mismatch(reporter, &detail.expected, &detail.actual);
  } else if let TryBuildError::Diagnostics(DiagnosticsError::ShouldNotHaveCompiled(ref detail)) = *error {
    compiled_unexpectedly(reporter, detail);
  } else if let TryBuildError::Build(BuildError::CompileFailed(ref detail)) = *error {
    failed_to_build(reporter, &detail.diagnostics);
  } else if let TryBuildError::Runner(RunnerError::RunFailed(ref detail)) = *error {
    run_failed(reporter, detail);
  } else {
    error_line(reporter, error);
  }
}

/// Renders the expected-vs-actual diff of a snapshot mismatch.
#[allow(
  clippy::single_call_fn,
  reason = "the mismatch render, the most involved failing-case rendering, kept on its own off render_error's dispatch"
)]
fn mismatch<W>(reporter: &mut Reporter<W>, expected: &str, actual: &str)
where
  W: WriteColor,
{
  reporter.bold_color(Red);
  reporter.emitln(format_args!("mismatch"));
  reporter.reset();
  reporter.emitln(format_args!(""));
  let term = env::var_os("TERM");
  let diff = compute_diff(term.as_deref(), expected, actual);
  reporter.bold_color(Blue);
  reporter.emitln(format_args!("EXPECTED:"));
  snippet_diff(reporter, Blue, expected, diff.as_ref());
  reporter.emitln(format_args!(""));
  reporter.bold_color(Red);
  reporter.emitln(format_args!("ACTUAL OUTPUT:"));
  snippet_diff(reporter, Red, actual, diff.as_ref());
  reporter.emit(format_args!("note: If the "));
  reporter.color(Red);
  reporter.emit(format_args!("actual output"));
  reporter.reset();
  reporter.emitln(format_args!(" is the correct output you can bless it by rerunning"));
  reporter.emitln(format_args!("      your test with the environment variable TRYBUILD=overwrite"));
  reporter.emitln(format_args!(""));
}

/// Computes a renderable diff only for terminals where highlighting is useful.
#[allow(
  clippy::single_call_fn,
  reason = "diff eligibility is a pure render policy seam tested without mutating TERM"
)]
fn compute_diff<'a>(term: Option<&OsStr>, expected: &'a str, actual: &'a str) -> Option<Diff<'a>> {
  if term.is_none_or(|terminal_name| terminal_name == OsStr::new("dumb")) {
    // No diff in a dumb terminal or when TERM is unset.
    None
  } else {
    Diff::compute(expected, actual)
  }
}

/// Renders a `compile_fail` case that unexpectedly compiled, with its output.
#[allow(
  clippy::single_call_fn,
  reason = "a named failing-case render dispatched from render_error"
)]
fn compiled_unexpectedly<W>(reporter: &mut Reporter<W>, detail: &UnexpectedSuccess)
where
  W: WriteColor,
{
  reporter.bold_color(Red);
  reporter.emitln(format_args!("error"));
  reporter.color(Red);
  reporter.emitln(format_args!("Expected test case to fail to compile, but it succeeded."));
  reporter.reset();
  reporter.emitln(format_args!(""));

  output_streams(reporter, Red, &detail.stdout, "");

  warnings(reporter, &detail.warnings);
}

/// Renders a pass-test that failed to build.
#[allow(
  clippy::single_call_fn,
  reason = "a named failing-case render dispatched from render_error"
)]
fn failed_to_build<W>(reporter: &mut Reporter<W>, stderr: &str)
where
  W: WriteColor,
{
  reporter.bold_color(Red);
  reporter.emitln(format_args!("error"));
  snippet(reporter, Red, stderr);
  reporter.emitln(format_args!(""));
}

/// Renders a pass-test that compiled but failed at runtime, with its output.
#[allow(
  clippy::single_call_fn,
  reason = "a named failing-case render dispatched from render_error"
)]
fn run_failed<W>(reporter: &mut Reporter<W>, detail: &RunOutput)
where
  W: WriteColor,
{
  let has_output = !detail.stdout.is_empty() || !detail.stderr.is_empty();

  reporter.bold_color(Red);
  reporter.emitln(format_args!("error"));
  reporter.color(Red);
  if has_output {
    reporter.emitln(format_args!("Test case failed at runtime."));
  } else {
    reporter.emitln(format_args!("Execution of the test case was unsuccessful but there was no output."));
  }
  reporter.reset();
  reporter.emitln(format_args!(""));

  warnings(reporter, &detail.warnings);

  output_streams(reporter, Red, &detail.stdout, &detail.stderr);
}

/// Renders the labelled `STDOUT:`/`STDERR:` sections of a captured run in the
/// given color, skipping streams with no content.
fn output_streams<W>(reporter: &mut Reporter<W>, color: Color, stdout: &str, stderr: &str)
where
  W: WriteColor,
{
  for (label, content) in [("STDOUT", stdout), ("STDERR", stderr)] {
    if !content.is_empty() {
      reporter.bold_color(color);
      reporter.emitln(format_args!("{label}:"));
      snippet(reporter, color, content);
      reporter.emitln(format_args!(""));
    }
  }
}

/// Renders a failing case with no richer data than its `Display` message.
#[allow(
  clippy::single_call_fn,
  reason = "the generic failing-case render, the fallback arm of render_error's dispatch"
)]
fn error_line<W>(reporter: &mut Reporter<W>, error: &TryBuildError)
where
  W: WriteColor,
{
  reporter.bold_color(Red);
  reporter.emitln(format_args!("error"));
  reporter.color(Red);
  reporter.emitln(format_args!("{error}"));
  reporter.reset();
  reporter.emitln(format_args!(""));
}

/// Renders captured build warnings, if any.
fn warnings<W>(reporter: &mut Reporter<W>, warnings: &str)
where
  W: WriteColor,
{
  if warnings.is_empty() {
    return;
  }

  reporter.bold_color(Yellow);
  reporter.emitln(format_args!("WARNINGS:"));
  snippet(reporter, Yellow, warnings);
  reporter.emitln(format_args!(""));
}

/// Renders a dotted-bordered snippet in the given color.
fn snippet<W>(reporter: &mut Reporter<W>, color: Color, content: &str)
where
  W: WriteColor,
{
  snippet_diff(reporter, color, content, None);
}

/// Renders a dotted-bordered snippet, highlighting diff-unique runs if a diff
/// is supplied.
#[cfg(all(feature = "diff", not(windows)))]
fn snippet_diff<W>(reporter: &mut Reporter<W>, color: Color, content: &str, maybe_diff: Option<&Diff<'_>>)
where
  W: WriteColor,
{
  reporter.color(color);
  reporter.emitln(format_args!("{}", "-".repeat(60)));

  match maybe_diff {
    Some(diff) => {
      for chunk in diff.iter(content) {
        match chunk {
          Render::Common(text) => {
            reporter.color(color);
            reporter.emit(format_args!("{text}"));
          }
          Render::Unique(text) => {
            reporter.bold_color(color);
            reporter.emit(format_args!("\x1B[7m{text}"));
          }
        }
      }
    }
    None => reporter.emit(format_args!("{content}")),
  }

  reporter.color(color);
  reporter.emitln(format_args!("{}", "-".repeat(60)));
  reporter.reset();
}

/// Renders a dotted-bordered snippet when diff highlighting is unavailable.
#[cfg(any(not(feature = "diff"), windows))]
fn snippet_diff<W>(reporter: &mut Reporter<W>, color: Color, content: &str, _maybe_diff: Option<&Diff<'_>>)
where
  W: WriteColor,
{
  reporter.color(color);
  reporter.emitln(format_args!("{}", "-".repeat(60)));
  reporter.emit(format_args!("{content}"));
  reporter.color(color);
  reporter.emitln(format_args!("{}", "-".repeat(60)));
  reporter.reset();
}

#[cfg(test)]
mod tests {
  use std::ffi::OsString;
  use std::path::PathBuf;

  use strict_test_support::Expect;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_expectations;
  use strict_test_support::ensure_ok_source;
  use termcolor::NoColor;

  use super::*;
  use crate::internal::build::CompileFailure;
  use crate::internal::sys::SysError;

  const PASS_RENDER: &str = "ok|WARNINGS:|warning payload|STDOUT:|stdout payload|STDERR:|stderr payload";
  const ERROR_RENDER: &str = "Expected test case to fail to compile, but it succeeded.|build stdout|compiler diagnostics|Test case failed \
                              at runtime.|run stdout|run stderr|run warning";
  const MISMATCH_RENDER: &str = "mismatch|EXPECTED:|expected text|ACTUAL OUTPUT:|actual text|TRYBUILD=overwrite";
  const SNAPSHOT_RENDER: &str = "wip|NOTE|snapshot output|wip/case.stderr|tests/ui/case.stderr";
  const RUNNER_RENDER: &str = "There are no trybuild tests enabled yet.|ERROR|unrecognized value of TRYBUILD";

  fn rendered(write: impl FnOnce(&mut Reporter<NoColor<Vec<u8>>>)) -> Result<String, TestFailure> {
    let mut reporter = Reporter::from_stream(NoColor::new(Vec::new()));
    write(&mut reporter);
    let stream = reporter.into_stream();
    ensure_ok_source(String::from_utf8(stream.into_inner()), "rendered report output remains valid UTF-8")
  }

  fn ensure_rendered(text: &str, needles: &'static str, context: &'static str) -> Result<(), TestFailure> {
    let expectations = needles
      .split('|')
      .map(|needle| Expect::Present(needle, context))
      .collect::<Vec<_>>();
    ensure_expectations(text, &expectations)
  }

  fn empty_pass() -> Outcome {
    Outcome::Passed(Box::new(PassDetail {
      stdout:   String::new(),
      stderr:   String::new(),
      warnings: String::new(),
    }))
  }

  #[test]
  fn render_case_includes_expected_labels_for_mixed_suites() -> Result<(), TestFailure> {
    let pass = CaseReport {
      path:     PathBuf::from("tests/ui/pass.rs"),
      expected: Expected::Pass,
      outcome:  Ok(empty_pass()),
    };
    let fail = CaseReport {
      path:     PathBuf::from("tests/ui/fail.rs"),
      expected: Expected::CompileFail,
      outcome:  Ok(empty_pass()),
    };
    let text = rendered(|reporter| {
      render_case(reporter, &pass, true);
      render_case(reporter, &fail, true);
      render_case(reporter, &fail, false);
    })?;

    ensure_rendered(
      text.as_str(),
      "tests/ui/pass.rs|[should pass]|tests/ui/fail.rs|[should fail to compile]|ok",
      "case render includes source names, expected labels, and pass status",
    )
  }

  #[test]
  fn render_snapshot_write_outcomes_include_paths_and_contents() -> Result<(), TestFailure> {
    let wip_path = PathBuf::from("wip/case.stderr");
    let stderr_path = PathBuf::from("tests/ui/case.stderr");
    let wip = WipDetail {
      wip_path,
      stderr_path: stderr_path.clone(),
      stderr: "snapshot output\n".to_owned(),
    };
    let overwrite = OverwriteDetail {
      stderr_path,
      stderr: "snapshot output\n".to_owned(),
    };
    let text = rendered(|reporter| {
      render_wip(reporter, &wip);
      render_overwrite(reporter, &overwrite);
    })?;

    ensure_rendered(
      text.as_str(),
      SNAPSHOT_RENDER,
      "snapshot write renders include destination paths and contents",
    )
  }

  #[test]
  fn render_setup_and_no_test_messages() -> Result<(), TestFailure> {
    let error = SysError::UpdateVar(OsString::from("later")).into();
    let text = rendered(|reporter| {
      render_no_tests(reporter);
      render_setup_fail(reporter, &error);
    })?;

    ensure_rendered(
      text.as_str(),
      RUNNER_RENDER,
      "setup and no-test renders include their user-facing messages",
    )
  }

  #[test]
  fn render_pass_includes_warnings_and_captured_streams() -> Result<(), TestFailure> {
    let detail = PassDetail {
      stdout:   "stdout payload\n".to_owned(),
      stderr:   "stderr payload\n".to_owned(),
      warnings: "warning payload\n".to_owned(),
    };
    let text = rendered(|reporter| render_pass(reporter, &detail))?;

    ensure_rendered(text.as_str(), PASS_RENDER, "passing cases include status, warnings, and streams")
  }

  #[test]
  fn render_error_dispatches_rich_failure_details() -> Result<(), TestFailure> {
    let text = rendered(|reporter| {
      let unexpected = DiagnosticsError::ShouldNotHaveCompiled(Box::new(UnexpectedSuccess {
        stdout:   "build stdout\n".to_owned(),
        warnings: "unexpected warning\n".to_owned(),
      }))
      .into();
      render_error(reporter, &unexpected);

      let compile_failed = BuildError::CompileFailed(Box::new(CompileFailure {
        diagnostics: "compiler diagnostics\n".to_owned(),
      }))
      .into();
      render_error(reporter, &compile_failed);

      let run_failed = RunnerError::RunFailed(Box::new(RunOutput {
        stdout:   "run stdout\n".to_owned(),
        stderr:   "run stderr\n".to_owned(),
        warnings: "run warning\n".to_owned(),
      }))
      .into();
      render_error(reporter, &run_failed);
    })?;

    ensure_rendered(
      text.as_str(),
      ERROR_RENDER,
      "rich error render includes all selected failure details",
    )
  }

  #[test]
  fn render_error_handles_fallback_and_runtime_without_output() -> Result<(), TestFailure> {
    let text = rendered(|reporter| {
      let fallback = SysError::UpdateVar(OsString::from("later")).into();
      render_error(reporter, &fallback);

      let quiet_runtime = RunnerError::RunFailed(Box::new(RunOutput {
        stdout:   String::new(),
        stderr:   String::new(),
        warnings: String::new(),
      }))
      .into();
      render_error(reporter, &quiet_runtime);
    })?;

    ensure_rendered(
      text.as_str(),
      "unrecognized value of TRYBUILD|Execution of the test case was unsuccessful but there was no output.",
      "fallback and quiet runtime errors render their messages",
    )
  }

  #[test]
  fn mismatch_render_includes_both_sides_and_blessing_advice() -> Result<(), TestFailure> {
    let text = rendered(|reporter| mismatch(reporter, "expected text\n", "actual text\n"))?;

    ensure_rendered(text.as_str(), MISMATCH_RENDER, "mismatch render includes sides and blessing advice")
  }

  #[test]
  fn compute_diff_skips_unhelpful_terminal_modes() -> Result<(), TestFailure> {
    ensure(
      compute_diff(None, "expected\n", "actual\n").is_none(),
      "unset TERM skips diff computation",
    )?;
    ensure(
      compute_diff(Some(OsStr::new("dumb")), "expected\n", "actual\n").is_none(),
      "dumb TERM skips diff computation",
    )?;
    let helpful = compute_diff(
      Some(OsStr::new("xterm-256color")),
      "prefix same X suffix same\n",
      "prefix same Y suffix same\n",
    );
    #[cfg(all(feature = "diff", not(windows)))]
    ensure(helpful.is_some(), "useful terminals enable diff computation")?;
    #[cfg(any(not(feature = "diff"), windows))]
    ensure(helpful.is_none(), "the inert diff backend does not compute diffs")?;
    Ok(())
  }
}
