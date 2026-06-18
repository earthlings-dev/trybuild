//! Rendering a typed [`CaseReport`] (and setup failures) to the terminal through
//! a [`Reporter`].
//!
//! This is the human-facing *view* over the runner's typed result: the runner
//! computes outcomes as data and never prints; [`run`](crate::TestCases::run)
//! drives these functions per case to reproduce trybuild's progress output,
//! while [`try_run`](crate::TestCases::try_run) skips them entirely.

use crate::TryBuildError;
use crate::internal::build::BuildError;
use crate::internal::diagnostics::{DiagnosticsError, UnexpectedSuccess};
use crate::internal::model::Expected;
use crate::internal::outcome::{CaseReport, Outcome, OverwriteDetail, PassDetail, WipDetail};
use crate::internal::report::diff::Diff;
#[cfg(all(feature = "diff", not(windows)))]
use crate::internal::report::diff::Render;
use crate::internal::report::reporter::Reporter;
use crate::internal::runner::{RunOutput, RunnerError};
use std::env;
use termcolor::Color::{self, Blue, Green, Red, Yellow};

/// Renders one resolved case: the `test <name> ...` prefix, then its outcome.
#[allow(
    clippy::single_call_fn,
    reason = "the per-case render entry point invoked from run's streaming view closure"
)]
pub(in crate::internal) fn render_case(
    reporter: &mut Reporter,
    case: &CaseReport,
    show_expected: bool,
) {
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
pub(in crate::internal) fn render_setup_fail(reporter: &mut Reporter, error: &TryBuildError) {
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
pub(in crate::internal) fn render_no_tests(reporter: &mut Reporter) {
    reporter.color(Yellow);
    reporter.emitln(format_args!("There are no trybuild tests enabled yet."));
    reporter.reset();
}

/// Renders a passing case: `ok`, plus any captured run output of a pass-test.
#[allow(
    clippy::single_call_fn,
    reason = "a named outcome render dispatched from render_case, paired with render_wip/render_overwrite"
)]
fn render_pass(reporter: &mut Reporter, detail: &PassDetail) {
    let has_output = !detail.stdout.is_empty() || !detail.stderr.is_empty();

    reporter.color(Green);
    reporter.emitln(format_args!("ok"));
    reporter.reset();
    if has_output || !detail.warnings.is_empty() {
        reporter.emitln(format_args!(""));
    }

    warnings(reporter, &detail.warnings);

    for (name, content) in [("STDOUT", &detail.stdout), ("STDERR", &detail.stderr)] {
        if !content.is_empty() {
            reporter.bold_color(Yellow);
            reporter.emitln(format_args!("{name}:"));
            snippet(reporter, Yellow, content);
            reporter.emitln(format_args!(""));
        }
    }
}

/// Renders a newly created `wip` snapshot and where to move it.
#[allow(
    clippy::single_call_fn,
    reason = "a named outcome render dispatched from render_case, paired with render_pass/render_overwrite"
)]
fn render_wip(reporter: &mut Reporter, detail: &WipDetail) {
    let wip_display = detail.wip_path.to_string_lossy();
    let stderr_display = detail.stderr_path.to_string_lossy();

    reporter.bold_color(Yellow);
    reporter.emitln(format_args!("wip"));
    reporter.emitln(format_args!(""));
    reporter.emit(format_args!("NOTE"));
    reporter.reset();
    reporter.emitln(format_args!(
        ": writing the following output to `{wip_display}`."
    ));
    reporter.emitln(format_args!(
        "Move this file to `{stderr_display}` to accept it as correct."
    ));
    snippet(reporter, Yellow, &detail.stderr);
    reporter.emitln(format_args!(""));
}

/// Renders a snapshot overwritten in place.
#[allow(
    clippy::single_call_fn,
    reason = "a named outcome render dispatched from render_case, paired with render_pass/render_wip"
)]
fn render_overwrite(reporter: &mut Reporter, detail: &OverwriteDetail) {
    let stderr_display = detail.stderr_path.to_string_lossy();

    reporter.bold_color(Yellow);
    reporter.emitln(format_args!("wip"));
    reporter.emitln(format_args!(""));
    reporter.emit(format_args!("NOTE"));
    reporter.reset();
    reporter.emitln(format_args!(
        ": writing the following output to `{stderr_display}`."
    ));
    snippet(reporter, Yellow, &detail.stderr);
    reporter.emitln(format_args!(""));
}

/// Renders a failing case by dispatching on the carried diagnostic data, falling
/// back to the error's `Display` for failures without a richer rendering.
#[allow(
    clippy::single_call_fn,
    reason = "the failing-case render dispatcher invoked from render_case; an if-let chain so it need not match the whole non_exhaustive taxonomy"
)]
fn render_error(reporter: &mut Reporter, error: &TryBuildError) {
    if let TryBuildError::Diagnostics(DiagnosticsError::Mismatch(ref detail)) = *error {
        mismatch(reporter, &detail.expected, &detail.actual);
    } else if let TryBuildError::Diagnostics(DiagnosticsError::ShouldNotHaveCompiled(ref detail)) =
        *error
    {
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
fn mismatch(reporter: &mut Reporter, expected: &str, actual: &str) {
    reporter.bold_color(Red);
    reporter.emitln(format_args!("mismatch"));
    reporter.reset();
    reporter.emitln(format_args!(""));
    let diff = if env::var_os("TERM").is_none_or(|term| term == "dumb") {
        // No diff in a dumb terminal or when TERM is unset.
        None
    } else {
        Diff::compute(expected, actual)
    };
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
    reporter.emitln(format_args!(
        " is the correct output you can bless it by rerunning"
    ));
    reporter.emitln(format_args!(
        "      your test with the environment variable TRYBUILD=overwrite"
    ));
    reporter.emitln(format_args!(""));
}

/// Renders a `compile_fail` case that unexpectedly compiled, with its output.
#[allow(
    clippy::single_call_fn,
    reason = "a named failing-case render dispatched from render_error"
)]
fn compiled_unexpectedly(reporter: &mut Reporter, detail: &UnexpectedSuccess) {
    reporter.bold_color(Red);
    reporter.emitln(format_args!("error"));
    reporter.color(Red);
    reporter.emitln(format_args!(
        "Expected test case to fail to compile, but it succeeded."
    ));
    reporter.reset();
    reporter.emitln(format_args!(""));

    if !detail.stdout.is_empty() {
        reporter.bold_color(Red);
        reporter.emitln(format_args!("STDOUT:"));
        snippet(reporter, Red, &detail.stdout);
        reporter.emitln(format_args!(""));
    }

    warnings(reporter, &detail.warnings);
}

/// Renders a pass-test that failed to build.
#[allow(
    clippy::single_call_fn,
    reason = "a named failing-case render dispatched from render_error"
)]
fn failed_to_build(reporter: &mut Reporter, stderr: &str) {
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
fn run_failed(reporter: &mut Reporter, detail: &RunOutput) {
    let has_output = !detail.stdout.is_empty() || !detail.stderr.is_empty();

    reporter.bold_color(Red);
    reporter.emitln(format_args!("error"));
    reporter.color(Red);
    if has_output {
        reporter.emitln(format_args!("Test case failed at runtime."));
    } else {
        reporter.emitln(format_args!(
            "Execution of the test case was unsuccessful but there was no output."
        ));
    }
    reporter.reset();
    reporter.emitln(format_args!(""));

    warnings(reporter, &detail.warnings);

    for (name, content) in [("STDOUT", &detail.stdout), ("STDERR", &detail.stderr)] {
        if !content.is_empty() {
            reporter.bold_color(Red);
            reporter.emitln(format_args!("{name}:"));
            snippet(reporter, Red, content);
            reporter.emitln(format_args!(""));
        }
    }
}

/// Renders a failing case with no richer data than its `Display` message.
#[allow(
    clippy::single_call_fn,
    reason = "the generic failing-case render, the fallback arm of render_error's dispatch"
)]
fn error_line(reporter: &mut Reporter, error: &TryBuildError) {
    reporter.bold_color(Red);
    reporter.emitln(format_args!("error"));
    reporter.color(Red);
    reporter.emitln(format_args!("{error}"));
    reporter.reset();
    reporter.emitln(format_args!(""));
}

/// Renders captured build warnings, if any.
fn warnings(reporter: &mut Reporter, warnings: &str) {
    if warnings.is_empty() {
        return;
    }

    reporter.bold_color(Yellow);
    reporter.emitln(format_args!("WARNINGS:"));
    snippet(reporter, Yellow, warnings);
    reporter.emitln(format_args!(""));
}

/// Renders a dotted-bordered snippet in the given color.
fn snippet(reporter: &mut Reporter, color: Color, content: &str) {
    snippet_diff(reporter, color, content, None);
}

/// Renders a dotted-bordered snippet, highlighting diff-unique runs if a diff
/// is supplied.
#[cfg(all(feature = "diff", not(windows)))]
fn snippet_diff(
    reporter: &mut Reporter,
    color: Color,
    content: &str,
    maybe_diff: Option<&Diff<'_>>,
) {
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
fn snippet_diff(
    reporter: &mut Reporter,
    color: Color,
    content: &str,
    _maybe_diff: Option<&Diff<'_>>,
) {
    reporter.color(color);
    reporter.emitln(format_args!("{}", "-".repeat(60)));
    reporter.emit(format_args!("{content}"));
    reporter.color(color);
    reporter.emitln(format_args!("{}", "-".repeat(60)));
    reporter.reset();
}
