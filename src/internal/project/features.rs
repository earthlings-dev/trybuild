//! Discovering which Cargo features the test runner itself was built with, so
//! the generated project can be built with the same set.
//!
//! The active feature set isn't handed to the test binary directly, so it is
//! recovered by locating the binary's fingerprint JSON under the `target`
//! directory.

use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;

use serde::de::Deserialize as _;
use serde::de::DeserializeOwned;
use serde::de::Deserializer;
use serde::de::{
  self,
};
use serde_derive::Deserialize;

/// The features the test binary was compiled with, or `None` if they could not
/// be determined.
#[allow(
  clippy::single_call_fn,
  reason = "the infallible feature-detection entry point on this module's surface, wrapping the best-effort try_find and collapsing its \
            error to None"
)]
pub(in crate::internal) fn find() -> Option<Vec<String>> {
  try_find().ok()
}

/// A unit error meaning "feature detection failed; fall back to no features".
///
/// Any underlying error converts into this, since detection is entirely
/// best-effort.
struct Ignored;

impl<E: Error> From<E> for Ignored {
  fn from(_error: E) -> Self {
    Self
  }
}

/// The subset of cargo's fingerprint JSON that records the enabled features.
#[derive(Deserialize)]
struct Build {
  /// The enabled feature names, stored by cargo as an embedded JSON string.
  #[serde(deserialize_with = "from_json")]
  features: Vec<String>,
}

/// Recovers the active feature set from the running test binary's fingerprint.
///
/// Derives the binary's hash from its own path, finds the matching
/// `target/.../.fingerprint/*-HASH/*.json`, and reads the feature list from it.
/// Returns [`Ignored`] at the first sign the layout does not match expectations.
#[allow(
  clippy::single_call_fn,
  reason = "the fallible detection routine, named and split from find so the `?`-on-Ignored body stays separate from the lossy None \
            fallback"
)]
fn try_find() -> Result<Vec<String>, Ignored> {
  // This will look something like:
  //   /path/to/crate_name/target/debug/deps/test_name-HASH
  let test_binary = env::args_os().next().ok_or(Ignored)?;

  // The hash at the end is ascii so not lossy, rest of conversion doesn't
  // matter.
  let test_binary_lossy = test_binary.to_string_lossy();
  let hash_range = if cfg!(windows) {
    // Trim ".exe" from the binary name for windows.
    test_binary_lossy.len().saturating_sub(21)..test_binary_lossy.len().saturating_sub(4)
  } else {
    test_binary_lossy.len().saturating_sub(17)..test_binary_lossy.len()
  };
  let hash = test_binary_lossy.get(hash_range).ok_or(Ignored)?;
  if !hash.starts_with('-') || !hash.get(1..).unwrap_or("").bytes().all(is_lower_hex_digit) {
    return Err(Ignored);
  }

  let binary_path = PathBuf::from(&test_binary);

  // Feature selection is saved in:
  //   /path/to/crate_name/target/debug/.fingerprint/*-HASH/*-HASH.json
  let up = binary_path.parent().ok_or(Ignored)?.parent().ok_or(Ignored)?;
  let fingerprint_dir = up.join(".fingerprint");
  if !fingerprint_dir.is_dir() {
    return Err(Ignored);
  }

  let mut hash_matches = Vec::new();
  for child in fingerprint_dir.read_dir()? {
    let entry = child?;
    let is_dir = entry.file_type()?.is_dir();
    let matching_hash = entry.file_name().to_string_lossy().ends_with(hash);
    if is_dir && matching_hash {
      hash_matches.push(entry.path());
    }
  }

  if hash_matches.len() != 1 {
    return Err(Ignored);
  }

  let mut json_matches = Vec::new();
  for child in hash_matches.first().ok_or(Ignored)?.read_dir()? {
    let entry = child?;
    let is_file = entry.file_type()?.is_file();
    let is_json = entry.path().extension() == Some(OsStr::new("json"));
    if is_file && is_json {
      json_matches.push(entry.path());
    }
  }

  if json_matches.len() != 1 {
    return Err(Ignored);
  }

  let build_json = fs::read_to_string(json_matches.first().ok_or(Ignored)?)?;
  let build: Build = serde_json::from_str(&build_json)?;
  Ok(build.features)
}

/// Whether `byte` is an ASCII lowercase hexadecimal digit.
#[allow(
  clippy::single_call_fn,
  reason = "a named predicate passed by reference to Iterator::all, clearer at the call site than an inline closure"
)]
const fn is_lower_hex_digit(byte: u8) -> bool {
  matches!(byte, b'0'..=b'9' | b'a'..=b'f')
}

/// serde `deserialize_with` adapter that parses a field cargo stored as an
/// embedded JSON string.
fn from_json<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
  T: DeserializeOwned,
  D: Deserializer<'de>,
{
  let json = String::deserialize(deserializer)?;
  serde_json::from_str(&json).map_err(de::Error::custom)
}
