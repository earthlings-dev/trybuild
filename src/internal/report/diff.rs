//! The optional expected-vs-actual diff. With the `diff` feature enabled (and
//! not on Windows), `Diff::compute` runs `dissimilar` over the two snapshots so
//! the mismatch report can highlight the differing runs; otherwise it is an
//! inert no-op that always declines to diff.

pub(in crate::internal) use self::r#impl::Diff;

/// A run of diff output: text common to both snapshots, or unique to one.
#[cfg(all(feature = "diff", not(windows)))]
pub(in crate::internal) enum Render<'a> {
  /// Text present in both the expected and actual output.
  Common(&'a str),
  /// Text present in only one side, to be highlighted.
  Unique(&'a str),
}

/// The active diff implementation, used when the `diff` feature is enabled on a
/// non-Windows target.
#[cfg(all(feature = "diff", not(windows)))]
mod r#impl {
  use std::cmp;
  use std::panic;

  use dissimilar::Chunk;

  use super::Render;

  /// A computed character-level diff between two snapshots.
  pub(in crate::internal) struct Diff<'a> {
    /// The expected snapshot text.
    expected: &'a str,
    /// The actual compiler output.
    actual:   &'a str,
    /// The chunk sequence produced by `dissimilar`.
    chunks:   Vec<Chunk<'a>>,
  }

  impl<'a> Diff<'a> {
    /// Diffs `expected` against `actual`, returning `None` when a diff would
    /// be untrustworthy (large or non-ASCII input) or not worth showing (too
    /// little text in common).
    #[allow(
      clippy::single_call_fn,
      reason = "the feature-stable diff contract validates size, character domain, and commonality before exposing highlighted chunks"
    )]
    pub(in crate::internal) fn compute(expected: &'a str, actual: &'a str) -> Option<Self> {
      if expected.len().saturating_add(actual.len()) > 2048 {
        // We don't yet trust the dissimilar crate to work well on large
        // inputs.
        return None;
      }

      // Nor on non-ascii inputs.
      let chunks = panic::catch_unwind(|| dissimilar::diff(expected, actual)).ok()?;

      let common_len: usize = chunks
        .iter()
        .filter_map(|chunk| match *chunk {
          Chunk::Equal(common) => Some(common.len()),
          Chunk::Delete(_) | Chunk::Insert(_) => None,
        })
        .sum();

      let bigger_len = cmp::max(expected.len(), actual.len());
      let worth_printing = common_len.saturating_mul(5) >= bigger_len.saturating_mul(4);
      if !worth_printing {
        return None;
      }

      Some(Self {
        expected,
        actual,
        chunks,
      })
    }

    /// Iterates the diff as [`Render`] runs for one side; `input` must be
    /// either the `expected` or the `actual` string.
    pub(in crate::internal) fn iter<'i>(&'i self, input: &str) -> impl Iterator<Item = Render<'a>> + 'i {
      let expected = input == self.expected;
      let actual = input == self.actual;
      self.chunks.iter().filter_map(move |chunk| match *chunk {
        Chunk::Equal(common) => Some(Render::Common(common)),
        Chunk::Delete(unique) if expected => Some(Render::Unique(unique)),
        Chunk::Insert(unique) if actual => Some(Render::Unique(unique)),
        Chunk::Delete(_) | Chunk::Insert(_) => None,
      })
    }
  }
}

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  #[cfg(all(feature = "diff", not(windows)))]
  use strict_test_support::ensure_all;
  #[cfg(all(feature = "diff", not(windows)))]
  use strict_test_support::ensure_some;

  use super::*;

  #[cfg(all(feature = "diff", not(windows)))]
  fn rendered_chunks(diff: &Diff<'_>, input: &str) -> Vec<String> {
    diff
      .iter(input)
      .map(|chunk| match chunk {
        Render::Common(text) => format!("common:{text}"),
        Render::Unique(text) => format!("unique:{text}"),
      })
      .collect()
  }

  #[cfg(all(feature = "diff", not(windows)))]
  #[test]
  fn compute_declines_oversized_or_low_similarity_input() -> Result<(), TestFailure> {
    let oversized = "x".repeat(2049);

    ensure(Diff::compute(&oversized, "").is_none(), "large inputs skip diff computation")?;
    ensure(
      Diff::compute("aaaa\nbbbb\n", "xxxx\nyyyy\n").is_none(),
      "low-similarity inputs skip diff computation",
    )
  }

  #[cfg(all(feature = "diff", not(windows)))]
  #[test]
  fn iter_marks_unique_runs_for_each_side() -> Result<(), TestFailure> {
    let expected = "prefix same X suffix same";
    let actual = "prefix same Y suffix same";
    let diff = ensure_some(Diff::compute(expected, actual), "similar snapshots produce a diff")?;
    let expected_chunks = rendered_chunks(&diff, expected);
    let actual_chunks = rendered_chunks(&diff, actual);

    ensure_all(&[
      (
        expected_chunks.iter().any(|chunk| chunk == "unique:X"),
        "expected-side iteration highlights deleted text",
      ),
      (
        actual_chunks.iter().any(|chunk| chunk == "unique:Y"),
        "actual-side iteration highlights inserted text",
      ),
      (
        expected_chunks.iter().any(|chunk| chunk.starts_with("common:prefix same")),
        "expected-side iteration keeps common prefix text",
      ),
      (
        actual_chunks.iter().any(|chunk| chunk.ends_with("suffix same")),
        "actual-side iteration keeps common suffix text",
      ),
    ])
  }

  #[cfg(any(not(feature = "diff"), windows))]
  #[test]
  fn inert_diff_declines_computation() -> Result<(), TestFailure> {
    ensure(
      Diff::compute("expected", "actual").is_none(),
      "diff computation is disabled without the diff implementation",
    )
  }
}

/// The inert diff implementation used when the `diff` feature is disabled or on
/// Windows; [`Diff`] is a placeholder and never constructed.
#[cfg(any(not(feature = "diff"), windows))]
mod r#impl {
  use std::marker::PhantomData;

  /// A placeholder `Diff` that is never constructed in this build.
  pub(in crate::internal) struct Diff<'a> {
    /// Carries the snapshot lifetime so this type matches the active impl.
    _lifetime: PhantomData<&'a str>,
  }

  impl<'a> Diff<'a> {
    /// Always `None`: there is no diff to compute when the feature is off.
    #[allow(
      clippy::single_call_fn,
      reason = "the feature-stable diff contract remains inert when highlighting support is unavailable"
    )]
    pub(in crate::internal) const fn compute(_expected: &'a str, _actual: &'a str) -> Option<Self> {
      None
    }
  }
}
