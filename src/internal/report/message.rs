//! High-level, semantic test-progress messages, rendered through a
//! [`Reporter`]. These are an extension trait rather than inherent methods so
//! the primitive [`Reporter`] impl can live in its own module.

use crate::TryBuildError;
use crate::internal::diagnostics::normalize;
use crate::internal::model::{Expected, Test};
use crate::internal::report::diff::{Diff, Render};
use crate::internal::report::reporter::Reporter;
use std::env;
use std::path::Path;
use std::process::Output;
use termcolor::Color::{self, Blue, Green, Red, Yellow};

/// Whether [`Messages::fail_output`] renders a failure or a warning.
pub(in crate::internal) enum Level {
    /// Render the output as a hard failure.
    Fail,
    /// Render the output as a non-fatal warning.
    Warn,
}

pub(in crate::internal) use self::Level::*;

/// Semantic test-progress messages layered over a [`Reporter`].
pub(in crate::internal) trait Messages {
    /// Reports a setup failure that aborts the whole run.
    fn prepare_fail(&mut self, err: &TryBuildError);
    /// Reports the failure of a single test case.
    fn test_fail(&mut self, err: &TryBuildError);
    /// Reports that no tests were enabled.
    fn no_tests_enabled(&mut self);
    /// Reports that a test case passed.
    fn ok(&mut self);
    /// Prints the `test <name> ...` prefix for a case about to run.
    fn begin_test(&mut self, case: &Test, show_expected: bool);
    /// Reports that a `compile_fail` case failed to build as a hard error.
    fn failed_to_build(&mut self, stderr: &str);
    /// Reports that a `compile_fail` case unexpectedly compiled.
    fn should_not_have_compiled(&mut self);
    /// Reports that a new `wip` snapshot was written.
    fn write_stderr_wip(&mut self, wip_path: &Path, stderr_path: &Path, stderr: &str);
    /// Reports that a snapshot was overwritten in place.
    fn overwrite_stderr(&mut self, stderr_path: &Path, stderr: &str);
    /// Reports a mismatch between expected and actual compiler output.
    fn mismatch(&mut self, expected: &str, actual: &str);
    /// Reports the runtime output of a pass-test.
    fn output(&mut self, warnings: &str, output: &Output);
    /// Renders captured stdout for a failing or warning case.
    fn fail_output(&mut self, level: Level, stdout: &str);
    /// Renders captured warnings, if any.
    fn warnings(&mut self, warnings: &str);
}

impl Messages for Reporter {
    fn prepare_fail(&mut self, err: &TryBuildError) {
        if err.already_printed() {
            return;
        }

        self.bold_color(Red);
        self.emit(format_args!("ERROR"));
        self.reset();
        self.emitln(format_args!(": {err}"));
        self.emitln(format_args!(""));
    }

    fn test_fail(&mut self, err: &TryBuildError) {
        if err.already_printed() {
            return;
        }

        self.bold_color(Red);
        self.emitln(format_args!("error"));
        self.color(Red);
        self.emitln(format_args!("{err}"));
        self.reset();
        self.emitln(format_args!(""));
    }

    fn no_tests_enabled(&mut self) {
        self.color(Yellow);
        self.emitln(format_args!("There are no trybuild tests enabled yet."));
        self.reset();
    }

    fn ok(&mut self) {
        self.color(Green);
        self.emitln(format_args!("ok"));
        self.reset();
    }

    fn begin_test(&mut self, case: &Test, show_expected: bool) {
        let display_name = case.path.as_os_str().to_string_lossy();

        self.emit(format_args!("test "));
        self.bold();
        self.emit(format_args!("{display_name}"));
        self.reset();

        if show_expected {
            match case.expected {
                Expected::Pass => self.emit(format_args!(" [should pass]")),
                Expected::CompileFail => self.emit(format_args!(" [should fail to compile]")),
            }
        }

        self.emit(format_args!(" ... "));
    }

    fn failed_to_build(&mut self, stderr: &str) {
        self.bold_color(Red);
        self.emitln(format_args!("error"));
        snippet(self, Red, stderr);
        self.emitln(format_args!(""));
    }

    fn should_not_have_compiled(&mut self) {
        self.bold_color(Red);
        self.emitln(format_args!("error"));
        self.color(Red);
        self.emitln(format_args!(
            "Expected test case to fail to compile, but it succeeded."
        ));
        self.reset();
        self.emitln(format_args!(""));
    }

    fn write_stderr_wip(&mut self, wip_path: &Path, stderr_path: &Path, stderr: &str) {
        let wip_display = wip_path.to_string_lossy();
        let stderr_display = stderr_path.to_string_lossy();

        self.bold_color(Yellow);
        self.emitln(format_args!("wip"));
        self.emitln(format_args!(""));
        self.emit(format_args!("NOTE"));
        self.reset();
        self.emitln(format_args!(
            ": writing the following output to `{wip_display}`."
        ));
        self.emitln(format_args!(
            "Move this file to `{stderr_display}` to accept it as correct."
        ));
        snippet(self, Yellow, stderr);
        self.emitln(format_args!(""));
    }

    fn overwrite_stderr(&mut self, stderr_path: &Path, stderr: &str) {
        let stderr_display = stderr_path.to_string_lossy();

        self.bold_color(Yellow);
        self.emitln(format_args!("wip"));
        self.emitln(format_args!(""));
        self.emit(format_args!("NOTE"));
        self.reset();
        self.emitln(format_args!(
            ": writing the following output to `{stderr_display}`."
        ));
        snippet(self, Yellow, stderr);
        self.emitln(format_args!(""));
    }

    fn mismatch(&mut self, expected: &str, actual: &str) {
        self.bold_color(Red);
        self.emitln(format_args!("mismatch"));
        self.reset();
        self.emitln(format_args!(""));
        let diff = if env::var_os("TERM").is_none_or(|term| term == "dumb") {
            // No diff in dumb terminal or when TERM is unset.
            None
        } else {
            Diff::compute(expected, actual)
        };
        self.bold_color(Blue);
        self.emitln(format_args!("EXPECTED:"));
        snippet_diff(self, Blue, expected, diff.as_ref());
        self.emitln(format_args!(""));
        self.bold_color(Red);
        self.emitln(format_args!("ACTUAL OUTPUT:"));
        snippet_diff(self, Red, actual, diff.as_ref());
        self.emit(format_args!("note: If the "));
        self.color(Red);
        self.emit(format_args!("actual output"));
        self.reset();
        self.emitln(format_args!(
            " is the correct output you can bless it by rerunning"
        ));
        self.emitln(format_args!(
            "      your test with the environment variable TRYBUILD=overwrite"
        ));
        self.emitln(format_args!(""));
    }

    fn output(&mut self, warnings: &str, output: &Output) {
        let success = output.status.success();
        let stdout = normalize::trim(&output.stdout);
        let stderr = normalize::trim(&output.stderr);
        let has_output = !stdout.is_empty() || !stderr.is_empty();

        if success {
            self.ok();
            if has_output || !warnings.is_empty() {
                self.emitln(format_args!(""));
            }
        } else {
            self.bold_color(Red);
            self.emitln(format_args!("error"));
            self.color(Red);
            if has_output {
                self.emitln(format_args!("Test case failed at runtime."));
            } else {
                self.emitln(format_args!(
                    "Execution of the test case was unsuccessful but there was no output."
                ));
            }
            self.reset();
            self.emitln(format_args!(""));
        }

        self.warnings(warnings);

        let color = if success { Yellow } else { Red };

        for (name, content) in [("STDOUT", &stdout), ("STDERR", &stderr)] {
            if !content.is_empty() {
                self.bold_color(color);
                self.emitln(format_args!("{name}:"));
                snippet(self, color, &normalize::trim(content));
                self.emitln(format_args!(""));
            }
        }
    }

    fn fail_output(&mut self, level: Level, stdout: &str) {
        let color = match level {
            Fail => Red,
            Warn => Yellow,
        };

        if !stdout.is_empty() {
            self.bold_color(color);
            self.emitln(format_args!("STDOUT:"));
            snippet(self, color, &normalize::trim(stdout));
            self.emitln(format_args!(""));
        }
    }

    fn warnings(&mut self, warnings: &str) {
        if warnings.is_empty() {
            return;
        }

        self.bold_color(Yellow);
        self.emitln(format_args!("WARNINGS:"));
        snippet(self, Yellow, warnings);
        self.emitln(format_args!(""));
    }
}

/// Renders a dotted-bordered snippet in the given color.
fn snippet(reporter: &mut Reporter, color: Color, content: &str) {
    snippet_diff(reporter, color, content, None);
}

/// Renders a dotted-bordered snippet, highlighting diff-unique runs if a diff
/// is supplied.
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
