//! Expanding the registered glob patterns into concrete, uniquely named test
//! cases, deduplicating paths so an explicit registration overrides a glob
//! match.

use std::collections::BTreeMap as Map;
use std::path::PathBuf;

use crate::internal::model::Name;
use crate::internal::model::Test;
use crate::internal::runner::Result;
use crate::internal::runner::RunnerError;

/// One concrete test case after glob expansion, with its generated bin name.
#[derive(Debug)]
pub(super) struct ExpandedTest {
  /// The generated `[[bin]]` name, e.g. `trybuild007`.
  pub name:         Name,
  /// The source file and its expected outcome.
  pub test:         Test,
  /// An error captured during expansion (e.g. a bad glob), surfaced when run.
  pub error:        Option<crate::TryBuildError>,
  /// Whether this entry came from a glob match rather than an explicit path.
  pub is_from_glob: bool,
}

/// Expands each registered [`Test`], turning glob patterns into one entry per
/// matched file and assigning every entry a unique bin name.
#[allow(
  clippy::single_call_fn,
  reason = "glob expansion is the deterministic registration phase that deduplicates explicit paths, orders matches, and assigns \
            generated target names"
)]
pub(super) fn expand_globs(tests: &[Test]) -> Vec<ExpandedTest> {
  let mut set = ExpandedTestSet {
    vec:           Vec::new(),
    path_to_index: Map::new(),
  };

  for case in tests {
    match case.path.to_str() {
      Some(utf8) if utf8.contains('*') => match glob(utf8) {
        Ok(paths) => {
          let expected = case.expected;
          for path in paths {
            set.insert(
              Test {
                path,
                expected,
              },
              None,
              true,
            );
          }
        }
        Err(error) => set.insert(case.clone(), Some(error.into()), false),
      },
      _ => set.insert(case.clone(), None, false),
    }
  }

  set.vec
}

/// Accumulator that assigns bin names and deduplicates entries by source path.
struct ExpandedTestSet {
  /// The expanded cases in insertion order.
  vec:           Vec<ExpandedTest>,
  /// Maps each source path to its index in `vec`, for deduplication.
  path_to_index: Map<PathBuf, usize>,
}

impl ExpandedTestSet {
  /// Adds a case, or — when the path was already added from a glob — updates
  /// that entry's expectation rather than duplicating it.
  fn insert(&mut self, case: Test, error: Option<crate::TryBuildError>, is_from_glob: bool) {
    if let Some(&i) = self.path_to_index.get(&case.path)
      && let Some(prev) = self.vec.get_mut(i)
      && prev.is_from_glob
    {
      prev.test.expected = case.expected;
      prev.error = error;
      prev.is_from_glob = is_from_glob;
      return;
    }

    let index = self.vec.len();
    let name = Name(format!("trybuild{index:03}"));
    let _previous = self.path_to_index.insert(case.path.clone(), index);
    self.vec.push(ExpandedTest {
      name,
      test: case,
      error,
      is_from_glob,
    });
  }
}

/// Expands one glob pattern into a sorted list of matching paths.
#[allow(
  clippy::single_call_fn,
  reason = "glob expansion owns deterministic lexical ordering and converts per-entry traversal failures into runner errors"
)]
fn glob(pattern: &str) -> Result<Vec<PathBuf>> {
  let mut paths = glob::glob(pattern)?
    .map(|entry| entry.map_err(RunnerError::from))
    .collect::<Result<Vec<PathBuf>>>()?;
  paths.sort();
  Ok(paths)
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::path::Path;
  use std::path::PathBuf;
  use std::result::Result as StdResult;

  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;
  use strict_test_support::ensure_some;

  use super::*;
  use crate::internal::model::Expected;

  fn registered(path: impl Into<PathBuf>, expected: Expected) -> Test {
    Test {
      path: path.into(),
      expected,
    }
  }

  fn touch(path: &Path) -> StdResult<(), TestFailure> {
    ensure_ok_source(fs::write(path, "fn main() {}\n"), "fixture source file can be written")
  }

  fn entry(entries: &[ExpandedTest], index: usize) -> StdResult<&ExpandedTest, TestFailure> {
    ensure_some(entries.get(index), "expanded entry exists")
  }

  #[test]
  fn expand_globs_sorts_matches_and_assigns_names() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("expand-globs")?;
    let alpha = fixture.child("a.rs");
    let beta = fixture.child("b.rs");
    touch(&beta)?;
    touch(&alpha)?;

    let pattern = fixture.child("*.rs").to_string_lossy().into_owned();
    let expanded = expand_globs(&[registered(pattern, Expected::CompileFail)]);
    let first = entry(&expanded, 0)?;
    let second = entry(&expanded, 1)?;

    ensure_all(&[
      (expanded.len() == 2, "glob expansion produces one entry per match"),
      (
        first.name.0 == "trybuild000",
        "first expanded entry receives the first generated name",
      ),
      (
        second.name.0 == "trybuild001",
        "second expanded entry receives the second generated name",
      ),
      (first.test.path == alpha, "glob expansion sorts paths before insertion"),
      (second.test.path == beta, "glob expansion preserves the sorted second path"),
      (first.is_from_glob, "glob matches are marked as glob-derived"),
    ])
  }

  #[test]
  fn invalid_glob_is_reported_on_the_registered_case() -> StdResult<(), TestFailure> {
    let expanded = expand_globs(&[registered("[*.rs", Expected::CompileFail)]);
    let first = entry(&expanded, 0)?;

    ensure_all(&[
      (expanded.len() == 1, "invalid globs still produce one reportable entry"),
      (first.error.is_some(), "invalid glob errors are stored on the entry"),
      (!first.is_from_glob, "invalid glob entries are not marked as matches"),
    ])
  }

  #[test]
  fn explicit_paths_override_globs_but_explicit_duplicates_remain() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("expand-dedup")?;
    let alpha = fixture.child("a.rs");
    let beta = fixture.child("b.rs");
    touch(&alpha)?;
    touch(&beta)?;

    let pattern = fixture.child("*.rs").to_string_lossy().into_owned();
    let expanded = expand_globs(&[
      registered(pattern, Expected::CompileFail),
      registered(beta.clone(), Expected::Pass),
      registered(beta.clone(), Expected::CompileFail),
    ]);
    let first = entry(&expanded, 0)?;
    let second = entry(&expanded, 1)?;
    let third = entry(&expanded, 2)?;

    ensure_all(&[
      (expanded.len() == 3, "an explicit duplicate remains after overriding a glob match"),
      (first.test.path == alpha, "the non-overridden glob match remains first"),
      (second.test.path == beta, "the explicit path replaces the glob-derived entry"),
      (
        matches!(second.test.expected, Expected::Pass),
        "the explicit path overrides the glob expectation",
      ),
      (!second.is_from_glob, "the overridden entry is no longer considered glob-derived"),
      (third.test.path == beta, "a later explicit duplicate is kept as its own entry"),
    ])
  }
}
