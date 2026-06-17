//! Self-hosted integration test.
//!
//! Runs trybuild against its own `tests/ui/*.rs` fixtures. The fixtures
//! deliberately pair files with mismatched expectations (for example a passing
//! file registered as `compile_fail`), so a successful run of this harness is
//! one where [`TestCases::run`](trybuild::TestCases::run) reports an error.

#[cfg(test)]
mod tests {
    #[test]
    fn test() -> Result<(), strict_test_support::TestFailure> {
        let mut cases = trybuild::TestCases::new();
        cases.pass("tests/ui/run-pass-0.rs");
        cases.pass("tests/ui/print-stdout.rs");
        cases.pass("tests/ui/run-pass-1.rs");
        cases.pass("tests/ui/print-stderr.rs");
        cases.pass("tests/ui/run-pass-2.rs");
        cases.pass("tests/ui/print-both.rs");
        cases.pass("tests/ui/run-pass-4.rs");
        cases.compile_fail("tests/ui/run-pass-3.rs");
        cases.pass("tests/ui/run-pass-5.rs");
        cases.pass("tests/ui/compile-fail-0.rs");
        cases.pass("tests/ui/run-pass-6.rs");
        cases.pass("tests/ui/run-pass-7.rs");
        cases.pass("tests/ui/run-pass-8.rs");
        cases.compile_fail("tests/ui/compile-fail-1.rs");
        cases.pass("tests/ui/run-fail.rs");
        cases.pass("tests/ui/run-pass-9.rs");
        cases.compile_fail("tests/ui/compile-fail-2.rs");
        cases.compile_fail("tests/ui/compile-fail-3.rs");

        // This self-test deliberately registers files with mismatched expectations
        // (e.g. a passing file as `compile_fail`), so the run is expected to fail.
        strict_test_support::ensure(
            cases.run().is_err(),
            "trybuild's self-test fixtures intentionally include failing cases",
        )
    }
}
