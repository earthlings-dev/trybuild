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
    use super::Render;
    use dissimilar::Chunk;
    use std::cmp;
    use std::panic;

    /// A computed character-level diff between two snapshots.
    pub(in crate::internal) struct Diff<'a> {
        /// The expected snapshot text.
        expected: &'a str,
        /// The actual compiler output.
        actual: &'a str,
        /// The chunk sequence produced by `dissimilar`.
        chunks: Vec<Chunk<'a>>,
    }

    impl<'a> Diff<'a> {
        /// Diffs `expected` against `actual`, returning `None` when a diff would
        /// be untrustworthy (large or non-ASCII input) or not worth showing (too
        /// little text in common).
        #[allow(
            clippy::single_call_fn,
            reason = "the diff constructor named on Diff's API and mirrored across both feature-gated impls so message::snippet_diff calls Diff::compute either way"
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
        pub(in crate::internal) fn iter<'i>(
            &'i self,
            input: &str,
        ) -> impl Iterator<Item = Render<'a>> + 'i {
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
            reason = "the diff constructor named on Diff's API and mirrored across both feature-gated impls so message::snippet_diff calls Diff::compute either way"
        )]
        pub(in crate::internal) const fn compute(
            _expected: &'a str,
            _actual: &'a str,
        ) -> Option<Self> {
            None
        }
    }
}
