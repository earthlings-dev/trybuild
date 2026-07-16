//! Path construction helpers: the `path!` macro for assembling a `PathBuf` (or
//! a `Directory`) out of `/`-separated component expressions, and the
//! [`CanonicalPath`] newtype used as a stable key when grouping diagnostics by
//! source file.

use std::path::Path;
use std::path::PathBuf;

/// Builds a path from `/`-separated component expressions.
///
/// Each segment between the `/` separators is an arbitrary expression pushed
/// onto a [`PathBuf`], e.g. `path!(project.dir / "tests" / name)`. A trailing
/// slash — `path!(project.target_dir / "trybuild" /)` — instead yields a
/// `Directory` (the engine's owned directory type), which is how a call site
/// signals at construction time that the path names a directory.
macro_rules! path {
    ($($tt:tt)+) => {
        tokenize_path!([] [] $($tt)+)
    };
}

/// Internal accumulator for the `path!` macro: folds the `/`-separated token
/// stream into a list of component expressions, then either builds a [`PathBuf`]
/// or wraps it in `Directory::new` when the input ended with a trailing slash.
macro_rules! tokenize_path {
    ([$(($($component:tt)+))*] [$($cur:tt)+] /) => {
        crate::internal::sys::directory::Directory::new(tokenize_path!([$(($($component)+))*] [$($cur)+]))
    };

    ([$(($($component:tt)+))*] [$($cur:tt)+] / $($rest:tt)+) => {
        tokenize_path!([$(($($component)+))* ($($cur)+)] [] $($rest)+)
    };

    ([$(($($component:tt)+))*] [$($cur:tt)*] $first:tt $($rest:tt)*) => {
        tokenize_path!([$(($($component)+))*] [$($cur)* $first] $($rest)*)
    };

    ([$(($($component:tt)+))*] [$($cur:tt)+]) => {
        tokenize_path!([$(($($component)+))* ($($cur)+)])
    };

    ([$(($($component:tt)+))*]) => {{
        let mut path = PathBuf::new();
        $(
            path.push(&($($component)+));
        )*
        path
    }};
}

/// A filesystem path canonicalized for use as a map key.
///
/// Diagnostics are grouped by the source file they refer to; canonicalizing the
/// path first makes those keys stable regardless of how rustc happened to spell
/// the path. If canonicalization fails (for instance the file no longer exists)
/// the original path is retained so lookups still work.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone)]
pub(in crate::internal) struct CanonicalPath(PathBuf);

impl CanonicalPath {
  /// Canonicalizes `path`, falling back to the path as given if that fails.
  pub(in crate::internal) fn new(path: &Path) -> Self {
    path.canonicalize().map_or_else(|_| Self(path.to_owned()), Self)
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;

  use super::*;

  #[test]
  fn test_path_macro() -> Result<(), TestFailure> {
    struct Project {
      dir: PathBuf,
    }

    let project = Project {
      dir: PathBuf::from("../target/tests"),
    };

    let cargo_dir = path!(project.dir / ".cargo" / "config.toml");
    ensure(
      cargo_dir.as_path() == Path::new("../target/tests/.cargo/config.toml"),
      "path! builds the expected cargo config path",
    )?;
    Ok(())
  }

  #[test]
  fn canonical_path_resolves_existing_paths_and_falls_back_for_missing() -> Result<(), TestFailure> {
    let fixture = TempDir::new("canonical-path")?;
    let source = fixture.child("source.rs");
    ensure_ok_source(fs::write(&source, "fn main() {}\n"), "source fixture can be written")?;
    let existing = CanonicalPath::new(&source);
    let missing_path = fixture.child("missing.rs");
    let missing = CanonicalPath::new(&missing_path);

    ensure_all(&[
      (existing.0.is_absolute(), "existing paths are canonicalized to an absolute path"),
      (missing.0 == missing_path, "missing paths fall back to the original spelling"),
    ])
  }
}
