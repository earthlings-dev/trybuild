//! Integration coverage for overwrite-mode and the all-`compile_fail` batched
//! runner path.

#[cfg(test)]
mod tests {
  use std::fs;
  use std::path::Path;

  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;
  use strict_test_support::ensure_some;

  fn write_compile_fail(path: &Path, body: &str) -> Result<(), TestFailure> {
    ensure_ok_source(fs::write(path, body), "compile-fail fixture can be written")
  }

  fn outcome_of<'a>(report: &'a trybuild::Report, path: &Path) -> Option<&'a Result<trybuild::Outcome, trybuild::TryBuildError>> {
    report.cases.iter().find(|case| case.path == path).map(|case| &case.outcome)
  }

  #[test]
  fn typed_runs_cover_empty_and_batched_overwrite_paths() -> Result<(), TestFailure> {
    let empty_cases = trybuild::TestCases::new();
    let empty_report = ensure_ok_source(
      empty_cases.try_run(trybuild::Update::Verify),
      "empty try_run completes without generated-project setup",
    )?;
    ensure(empty_report.cases.is_empty(), "empty try_run reports no cases")?;

    let fixture = TempDir::new("overwrite-batched")?;
    let stale = fixture.child("stale.rs");
    let missing = fixture.child("missing.rs");
    let bad_glob = fixture.child("[*.rs");
    write_compile_fail(&stale, "fn main() { let _: u32 = \"text\"; }\n")?;
    write_compile_fail(&missing, "fn main() { let _: bool = 0u8; }\n")?;
    ensure_ok_source(
      fs::write(stale.with_extension("stderr"), "stale snapshot\n"),
      "stale snapshot can be written",
    )?;

    let mut cases = trybuild::TestCases::new();
    cases.compile_fail(&stale);
    cases.compile_fail(&missing);
    cases.compile_fail(&bad_glob);

    let report = ensure_ok_source(
      cases.try_run(trybuild::Update::Overwrite),
      "overwrite try_run over absolute fixtures completes",
    )?;
    let stale_outcome = ensure_some(outcome_of(&report, &stale), "stale fixture is reported")?;
    let missing_outcome = ensure_some(outcome_of(&report, &missing), "missing-snapshot fixture is reported")?;
    let glob_outcome = ensure_some(outcome_of(&report, &bad_glob), "bad glob fixture is reported")?;
    let stale_snapshot = ensure_ok_source(fs::read_to_string(stale.with_extension("stderr")), "stale snapshot can be read")?;
    let missing_snapshot = ensure_ok_source(
      fs::read_to_string(missing.with_extension("stderr")),
      "missing snapshot can be read after overwrite",
    )?;

    ensure_all(&[
      (report.cases.len() == 3, "every registered absolute fixture is reported"),
      (
        matches!(stale_outcome, Ok(trybuild::Outcome::Overwrote(_))),
        "stale snapshots are overwritten in place",
      ),
      (
        matches!(missing_outcome, Ok(trybuild::Outcome::Overwrote(_))),
        "missing snapshots are created in place under overwrite",
      ),
      (glob_outcome.is_err(), "bad glob patterns remain per-case errors"),
      (
        stale_snapshot.contains("expected `u32`, found `&str`"),
        "the stale snapshot is replaced with real diagnostics",
      ),
      (
        missing_snapshot.contains("expected `bool`, found `u8`"),
        "the missing snapshot is populated with real diagnostics",
      ),
    ])
  }
}
