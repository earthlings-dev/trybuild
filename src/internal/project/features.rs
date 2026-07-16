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

use serde::de;
use serde::de::Deserialize as _;
use serde::de::DeserializeOwned;
use serde::de::Deserializer;
use serde_derive::Deserialize;

/// The features the test binary was compiled with, or `None` if they could not
/// be determined.
#[allow(
  clippy::single_call_fn,
  reason = "feature discovery is the best-effort host boundary that intentionally collapses fingerprint-layout failures to absence"
)]
pub(in crate::internal) fn find() -> Option<Vec<String>> {
  env::args_os().next().and_then(|test_binary| find_from(&test_binary).ok())
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
  reason = "fingerprint decoding is the fallible Cargo-layout interpreter beneath the best-effort feature-discovery boundary"
)]
fn find_from(test_binary: &OsStr) -> Result<Vec<String>, Ignored> {
  // This will look something like:
  //   /path/to/crate_name/target/debug/deps/test_name-HASH
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
  reason = "the lowercase-hex predicate is the function-item grammar used to validate Cargo fingerprint hashes"
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

#[cfg(test)]
mod tests {
  use std::fs;
  use std::path::Path;
  use std::result::Result as StdResult;

  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;
  use strict_test_support::ensure_some;

  use super::*;

  const HASH: &str = "-0123456789abcdef";

  fn binary_path(root: &Path, suffix: &str) -> PathBuf {
    root.join("target/debug/deps").join(format!("test{suffix}"))
  }

  fn write_features(root: &Path, package: &str, json_name: &str, body: &str) -> StdResult<(), TestFailure> {
    let fingerprint = root.join("target/debug/.fingerprint").join(format!("{package}{HASH}"));
    ensure_ok_source(fs::create_dir_all(&fingerprint), "fingerprint directory can be created")?;
    ensure_ok_source(fs::write(fingerprint.join(json_name), body), "fingerprint JSON can be written")
  }

  #[test]
  fn find_from_reads_features_from_the_matching_fingerprint() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("features-success")?;
    write_features(
      fixture.path(),
      "trybuild",
      "trybuild-0123456789abcdef.json",
      r#"{"features":"[\"diff\",\"serde\"]"}"#,
    )?;

    let found = ensure_some(
      find_from(binary_path(fixture.path(), HASH).as_os_str()).ok(),
      "feature detection reads the matching fingerprint",
    )?;

    ensure_all(&[
      (found == ["diff", "serde"], "feature detection parses the embedded feature list"),
      (
        binary_path(fixture.path(), HASH).ends_with(format!("test{HASH}")),
        "the test fixture uses a cargo-style binary hash suffix",
      ),
    ])
  }

  #[test]
  fn find_from_rejects_non_cargo_hash_suffixes() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("features-bad-hash")?;

    ensure_all(&[
      (
        find_from(binary_path(fixture.path(), "-not-a-valid-hash").as_os_str()).is_err(),
        "feature detection rejects non-hex binary suffixes",
      ),
      (
        find_from(binary_path(fixture.path(), "0123456789abcdef").as_os_str()).is_err(),
        "feature detection rejects suffixes without cargo's dash separator",
      ),
    ])
  }

  #[test]
  fn find_from_rejects_missing_or_ambiguous_fingerprint_layouts() -> StdResult<(), TestFailure> {
    let missing = TempDir::new("features-missing")?;
    let ambiguous = TempDir::new("features-ambiguous")?;
    write_features(ambiguous.path(), "first", "first.json", r#"{"features":"[]"}"#)?;
    write_features(ambiguous.path(), "second", "second.json", r#"{"features":"[]"}"#)?;

    ensure_all(&[
      (
        find_from(binary_path(missing.path(), HASH).as_os_str()).is_err(),
        "feature detection rejects a missing .fingerprint directory",
      ),
      (
        find_from(binary_path(ambiguous.path(), HASH).as_os_str()).is_err(),
        "feature detection rejects multiple matching fingerprint directories",
      ),
    ])
  }

  #[test]
  fn find_from_rejects_malformed_fingerprint_json() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("features-malformed")?;
    write_features(fixture.path(), "trybuild", "trybuild.json", "{not json")?;

    ensure(
      find_from(binary_path(fixture.path(), HASH).as_os_str()).is_err(),
      "feature detection rejects malformed fingerprint JSON",
    )
  }

  #[test]
  fn find_from_ignores_nonmatching_fingerprint_noise() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("features-noise")?;
    let fingerprint = fixture.path().join("target/debug/.fingerprint");
    let matching = fingerprint.join(format!("trybuild{HASH}"));
    ensure_ok_source(fs::create_dir_all(&matching), "matching fingerprint directory can be created")?;
    ensure_ok_source(
      fs::create_dir_all(fingerprint.join("trybuild-1111111111111111")),
      "nonmatching fingerprint directory can be created",
    )?;
    ensure_ok_source(
      fs::write(fingerprint.join(format!("file{HASH}")), ""),
      "matching fingerprint files are ignored",
    )?;
    ensure_ok_source(fs::create_dir_all(matching.join("not-json.json")), "json directories are ignored")?;
    ensure_ok_source(fs::write(matching.join("notes.txt"), ""), "non-json files are ignored")?;
    ensure_ok_source(
      fs::write(matching.join("trybuild.json"), r#"{"features":"[\"diff\"]"}"#),
      "matching fingerprint JSON can be written",
    )?;

    let found = ensure_some(
      find_from(binary_path(fixture.path(), HASH).as_os_str()).ok(),
      "feature detection ignores unrelated fingerprint entries",
    )?;

    ensure(found == ["diff"], "feature detection keeps the single matching fingerprint JSON")
  }
}
