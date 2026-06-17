//! Integration tests for the typed, terminal-free [`TestCases::try_run`] core.
//!
//! [`try_run`](trybuild::TestCases::try_run) returns every fixture's outcome as
//! data (never printing), so these tests assert the typed [`Report`] directly —
//! both polarities of each outcome — and that the run writes nothing to the
//! terminal (driven through `strict_test_support::capture_ignored_test`).

#[cfg(test)]
mod tests {
    use strict_test_support::TestFailure;
    use strict_test_support::capture_ignored_test;
    use strict_test_support::ensure;
    use strict_test_support::ensure_all;
    use strict_test_support::ensure_ok_source;
    use strict_test_support::ensure_some;

    /// The recorded outcome of the case whose path ends with `file`, if present.
    fn outcome_of<'a>(
        report: &'a trybuild::Report,
        file: &str,
    ) -> Option<&'a Result<trybuild::Outcome, trybuild::TryBuildError>> {
        report
            .cases
            .iter()
            .find(|case| case.path.ends_with(file))
            .map(|case| &case.outcome)
    }

    /// Asserts each fixture in the mixed suite resolved to its expected typed
    /// outcome variant — the both-polarity coverage of every outcome.
    #[allow(
        clippy::single_call_fn,
        reason = "extracted from try_run_reports_typed_outcomes to keep that test within the line budget; one in-crate caller"
    )]
    fn verify_outcome_polarities(report: &trybuild::Report) -> Result<(), TestFailure> {
        ensure_all(&[
            (
                matches!(
                    outcome_of(report, "compile-fail-2.rs"),
                    Some(&Ok(trybuild::Outcome::Passed(_)))
                ),
                "a compile_fail matching its snapshot passes",
            ),
            (
                matches!(
                    outcome_of(report, "try-run-mismatch.rs"),
                    Some(&Err(trybuild::TryBuildError::Diagnostics(
                        trybuild::DiagnosticsError::Mismatch(_)
                    ))),
                ),
                "a compile_fail whose output drifts from its snapshot is a mismatch",
            ),
            (
                matches!(
                    outcome_of(report, "compile-fail-0.rs"),
                    Some(&Err(trybuild::TryBuildError::Diagnostics(
                        trybuild::DiagnosticsError::SnapshotMissing { .. }
                    ))),
                ),
                "a missing snapshot under Verify is reported, not created",
            ),
            (
                matches!(
                    outcome_of(report, "run-pass-0.rs"),
                    Some(&Err(trybuild::TryBuildError::Diagnostics(
                        trybuild::DiagnosticsError::ShouldNotHaveCompiled(_)
                    ))),
                ),
                "a compile_fail that unexpectedly compiles is rejected",
            ),
            (
                matches!(
                    outcome_of(report, "run-pass-1.rs"),
                    Some(&Ok(trybuild::Outcome::Passed(_)))
                ),
                "a pass-test that compiles and runs cleanly passes",
            ),
            (
                matches!(
                    outcome_of(report, "compile-fail-1.rs"),
                    Some(&Err(trybuild::TryBuildError::Build(
                        trybuild::BuildError::CompileFailed(_)
                    ))),
                ),
                "a pass-test that fails to build is reported",
            ),
            (
                matches!(
                    outcome_of(report, "run-fail.rs"),
                    Some(&Err(trybuild::TryBuildError::Runner(
                        trybuild::RunnerError::RunFailed(_)
                    ))),
                ),
                "a pass-test that runs but exits unsuccessfully is reported",
            ),
        ])
    }

    /// Asserts the mismatch carries both sides of the diff as data, not printed.
    #[allow(
        clippy::single_call_fn,
        reason = "extracted from try_run_reports_typed_outcomes to keep that test within the line budget; one in-crate caller"
    )]
    fn verify_mismatch_diff(report: &trybuild::Report) -> Result<(), TestFailure> {
        let mismatch_detail = report
            .cases
            .iter()
            .find(|case| case.path.ends_with("try-run-mismatch.rs"))
            .and_then(|case| case.outcome.as_ref().err())
            .and_then(|error| {
                if let trybuild::TryBuildError::Diagnostics(trybuild::DiagnosticsError::Mismatch(
                    ref detail,
                )) = *error
                {
                    Some(detail)
                } else {
                    None
                }
            });
        let detail = ensure_some(
            mismatch_detail,
            "the mismatch carries its diagnostic detail",
        )?;
        ensure_all(&[
            (
                !detail.expected.is_empty(),
                "the mismatch carries the expected snapshot",
            ),
            (
                detail.actual.contains("the real message"),
                "the mismatch carries the actual compiler output",
            ),
        ])
    }

    #[test]
    fn try_run_reports_typed_outcomes() -> Result<(), TestFailure> {
        // A mixed pass + compile_fail suite, exercising the per-test path and
        // every outcome variant in one run.
        let mut cases = trybuild::TestCases::new();
        cases.compile_fail("tests/ui/compile-fail-2.rs"); // matching snapshot -> Passed
        cases.compile_fail("tests/ui/try-run-mismatch.rs"); // stale snapshot -> Mismatch
        cases.compile_fail("tests/ui/compile-fail-0.rs"); // no snapshot, Verify -> SnapshotMissing
        cases.compile_fail("tests/ui/run-pass-0.rs"); // compiles -> ShouldNotHaveCompiled
        cases.pass("tests/ui/run-pass-1.rs"); // compiles and runs -> Passed
        cases.pass("tests/ui/compile-fail-1.rs"); // fails to build -> CompileFailed
        cases.pass("tests/ui/run-fail.rs"); // runs but panics -> RunFailed

        let report = ensure_ok_source(
            cases.try_run(trybuild::Update::Verify),
            "try_run sets up the throwaway project",
        )?;

        verify_outcome_polarities(&report)?;
        verify_mismatch_diff(&report)
    }

    /// Driven only by [`try_run_writes_nothing_to_the_terminal`] as a captured
    /// re-exec; a compile_fail-only suite, which also exercises the batched path.
    #[test]
    #[ignore = "driven by try_run_writes_nothing_to_the_terminal via capture_ignored_test; a direct run only proves try_run returns"]
    fn silent_try_run_child() -> Result<(), TestFailure> {
        let mut cases = trybuild::TestCases::new();
        cases.compile_fail("tests/ui/compile-fail-2.rs");
        let report = ensure_ok_source(
            cases.try_run(trybuild::Update::Verify),
            "try_run sets up the throwaway project",
        )?;
        ensure(
            matches!(
                outcome_of(&report, "compile-fail-2.rs"),
                Some(&Ok(trybuild::Outcome::Passed(_)))
            ),
            "the matching compile_fail snapshot passes under the batched path",
        )
    }

    #[test]
    fn try_run_writes_nothing_to_the_terminal() -> Result<(), TestFailure> {
        let captured = capture_ignored_test("tests::silent_try_run_child")?;
        ensure_all(&[
            (
                captured.status.success(),
                "the silent child completed successfully",
            ),
            (
                captured.stderr.is_empty(),
                "try_run wrote nothing to stderr",
            ),
        ])
    }
}
