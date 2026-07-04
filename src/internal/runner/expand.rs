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
  reason = "the glob-expansion entry point the runner calls by name; the module's reason for existing"
)]
pub(super) fn expand_globs(tests: &[Test]) -> Vec<ExpandedTest> {
  let mut set = ExpandedTestSet::new();

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
  /// Creates an empty set.
  #[allow(
    clippy::single_call_fn,
    reason = "the accumulator constructor paired with `insert` on the same private type, keeping ExpandedTestSet's invariants \
              self-contained"
  )]
  const fn new() -> Self {
    Self {
      vec:           Vec::new(),
      path_to_index: Map::new(),
    }
  }

  /// Adds a case, or — when the path was already added from a glob — updates
  /// that entry's expectation rather than duplicating it.
  fn insert(&mut self, case: Test, error: Option<crate::TryBuildError>, is_from_glob: bool) {
    if let Some(&i) = self.path_to_index.get(&case.path)
      && let Some(prev) = self.vec.get_mut(i)
      && prev.is_from_glob
    {
      prev.test.expected = case.expected;
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
  reason = "a named helper wrapping glob iteration with sorting and error mapping, lifted out so expand_globs reads as one match"
)]
fn glob(pattern: &str) -> Result<Vec<PathBuf>> {
  let mut paths = glob::glob(pattern)?
    .map(|entry| entry.map_err(RunnerError::from))
    .collect::<Result<Vec<PathBuf>>>()?;
  paths.sort();
  Ok(paths)
}
