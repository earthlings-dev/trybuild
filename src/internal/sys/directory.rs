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
    reason = "a constructor on Directory's cohesive API, keeping current-dir interop with the std boundary inside the type alongside \
              `new`/`parent`/`canonicalize`"
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
