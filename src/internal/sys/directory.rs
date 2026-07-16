//! A thin wrapper around [`PathBuf`] for directory paths.
//!
//! [`Directory::new`] appends a trailing separator so the value reads
//! unambiguously as a directory, and the type carries serde impls so it can sit
//! directly inside the manifest structs.

use std::borrow::Cow;
use std::env;
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use serde::de::Deserialize;
use serde::de::Deserializer;
use serde_derive::Serialize;

/// A filesystem directory path.
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub(in crate::internal) struct Directory {
  /// The path, always carrying a trailing separator (see [`Directory::new`]).
  path: PathBuf,
}

impl Directory {
  /// Wraps `path`, appending a trailing separator so it reads as a directory.
  pub(in crate::internal) fn new<P: Into<PathBuf>>(input: P) -> Self {
    let mut path = input.into();
    path.push("");
    Self {
      path,
    }
  }

  /// The current working directory.
  #[allow(
    clippy::single_call_fn,
    reason = "current-directory observation converts the operating-system path directly into the directory wrapper's trailing-separator invariant"
  )]
  pub(in crate::internal) fn current() -> io::Result<Self> {
    env::current_dir().map(Self::new)
  }

  /// The path as a possibly-lossy UTF-8 string.
  pub(in crate::internal) fn to_string_lossy(&self) -> Cow<'_, str> {
    self.path.to_string_lossy()
  }

  /// Joins `tail` onto this directory, yielding a [`PathBuf`].
  pub(in crate::internal) fn join<P: AsRef<Path>>(&self, tail: P) -> PathBuf {
    self.path.join(tail)
  }

  /// This directory's parent, if any.
  pub(in crate::internal) fn parent(&self) -> Option<Self> {
    self.path.parent().map(Self::new)
  }

  /// The canonicalized form of this directory.
  pub(in crate::internal) fn canonicalize(&self) -> io::Result<Self> {
    self.path.canonicalize().map(Self::new)
  }
}

impl From<OsString> for Directory {
  fn from(os_string: OsString) -> Self {
    Self::new(PathBuf::from(os_string))
  }
}

impl AsRef<Path> for Directory {
  fn as_ref(&self) -> &Path {
    &self.path
  }
}

impl<'de> Deserialize<'de> for Directory {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: Deserializer<'de>,
  {
    PathBuf::deserialize(deserializer).map(Self::new)
  }
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::path::Path;

  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;

  use super::*;

  #[test]
  fn directory_wraps_paths_with_join_parent_and_lossy_views() -> Result<(), TestFailure> {
    let dir = Directory::new(PathBuf::from("target/tests"));
    let maybe_parent = dir.parent();
    let parent = strict_test_support::ensure_some(maybe_parent, "directory parent exists")?;

    ensure_all(&[
      (
        dir.as_ref().ends_with(Path::new("target/tests")),
        "directory stores the requested path",
      ),
      (
        dir.join("case.rs").ends_with(Path::new("target/tests/case.rs")),
        "directory joins child paths",
      ),
      (
        parent.as_ref().ends_with(Path::new("target")),
        "directory parent strips the final component",
      ),
      (dir.to_string_lossy().contains("target"), "directory exposes a lossy display string"),
    ])
  }

  #[test]
  fn current_and_canonicalize_report_filesystem_directories() -> Result<(), TestFailure> {
    let fixture = TempDir::new("directory-canonical")?;
    let child = fixture.child("child");
    ensure_ok_source(fs::create_dir_all(&child), "child directory can be created")?;
    let current = ensure_ok_source(Directory::current(), "current directory can be read")?;
    let canonical = ensure_ok_source(Directory::new(child).canonicalize(), "directory can be canonicalized")?;

    ensure_all(&[
      (current.as_ref().is_absolute(), "current directory is absolute"),
      (canonical.as_ref().is_absolute(), "canonicalized directory is absolute"),
      (
        Directory::new(fixture.child("missing")).canonicalize().is_err(),
        "canonicalizing a missing directory fails",
      ),
    ])
  }

  #[test]
  fn deserialize_wraps_path_values() -> Result<(), TestFailure> {
    #[derive(serde_derive::Deserialize)]
    struct Fixture {
      dir: Directory,
    }

    let fixture = ensure_ok_source(
      toml::from_str::<Fixture>(r#"dir = "target/tests""#),
      "directory deserializes from TOML",
    )?;

    ensure(
      fixture.dir.as_ref().ends_with(Path::new("target/tests")),
      "deserialized directories preserve the input path",
    )
  }
}
