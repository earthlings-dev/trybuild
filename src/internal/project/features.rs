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
use std::path::Path;
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

/// Recovers the active feature set from the running test binary's fingerprint,
/// trying Cargo's legacy layout before its new build-directory layout.
#[allow(
  clippy::single_call_fn,
  reason = "fingerprint decoding is the fallible Cargo-layout interpreter beneath the best-effort feature-discovery boundary"
)]
fn find_from(test_binary: &OsStr) -> Result<Vec<String>, Ignored> {
  find_from_legacy_layout(test_binary).or_else(|_legacy_error| find_from_new_layout(test_binary))
}

/// Reads features from Cargo's legacy `target/<profile>/.fingerprint` layout.
#[allow(
  clippy::single_call_fn,
  reason = "the named legacy-layout reader preserves Cargo layout identity and the orchestrator's legacy-first fallback order"
)]
fn find_from_legacy_layout(test_binary: &OsStr) -> Result<Vec<String>, Ignored> {
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

  let binary_path = PathBuf::from(test_binary);

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

  read_features(hash_matches.first().ok_or(Ignored)?)
}

/// Reads features from Cargo's new
/// `target/<profile>/build/<crate>/<hash>/fingerprint` layout.
#[allow(
  clippy::single_call_fn,
  reason = "the named new-layout reader preserves Cargo layout identity and the orchestrator's explicit fallback boundary"
)]
fn find_from_new_layout(test_binary: &OsStr) -> Result<Vec<String>, Ignored> {
  // The binary resembles:
  //   target/debug/build/$CRATE/$HASH/out/test_name-HASH
  let binary_path = PathBuf::from(test_binary);
  let out_dir = binary_path.parent().ok_or(Ignored)?;
  if out_dir.file_name() != Some(OsStr::new("out")) {
    return Err(Ignored);
  }
  let build_dir = out_dir.parent().ok_or(Ignored)?;
  read_features(&build_dir.join("fingerprint"))
}

/// Reads the single regular JSON file in `fingerprint_dir` and decodes its
/// embedded feature list.
fn read_features(fingerprint_dir: &Path) -> Result<Vec<String>, Ignored> {
  let mut json_matches = Vec::new();
  for child in fingerprint_dir.read_dir()? {
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
  let build_json_path = json_matches.first().ok_or(Ignored)?;
  let build_json = fs::read_to_string(build_json_path)?;
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

  fn new_build_dir(root: &Path) -> PathBuf {
    root.join("target/debug/build/trybuild/unit-hash")
  }

  fn new_binary_path(root: &Path) -> PathBuf {
    new_build_dir(root).join("out").join(format!("test{HASH}"))
  }

  fn write_fingerprint(fingerprint: &Path, json_name: &str, body: &str) -> StdResult<(), TestFailure> {
    ensure_ok_source(fs::create_dir_all(fingerprint), "fingerprint directory can be created")?;
    ensure_ok_source(fs::write(fingerprint.join(json_name), body), "fingerprint JSON can be written")
  }

  fn write_features(root: &Path, package: &str, json_name: &str, body: &str) -> StdResult<(), TestFailure> {
    let fingerprint = root.join("target/debug/.fingerprint").join(format!("{package}{HASH}"));
    write_fingerprint(&fingerprint, json_name, body)
  }

  fn write_new_features(root: &Path, json_name: &str, body: &str) -> StdResult<(), TestFailure> {
    write_fingerprint(&new_build_dir(root).join("fingerprint"), json_name, body)
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
  fn find_from_falls_back_to_the_new_build_directory_layout() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("features-new-layout")?;
    write_new_features(fixture.path(), "trybuild.json", r#"{"features":"[\"new-layout\",\"serde\"]"}"#)?;

    let found = ensure_some(
      find_from(new_binary_path(fixture.path()).as_os_str()).ok(),
      "feature detection falls back to the new Cargo layout",
    )?;

    ensure(
      found == ["new-layout", "serde"],
      "the new-layout fingerprint feature list is decoded",
    )
  }

  #[test]
  fn find_from_prefers_legacy_fingerprint_data_when_both_layouts_are_present() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("features-legacy-first")?;
    let legacy_fingerprint = new_build_dir(fixture.path())
      .join(".fingerprint")
      .join(format!("trybuild{HASH}"));
    write_fingerprint(&legacy_fingerprint, "legacy.json", r#"{"features":"[\"legacy\"]"}"#)?;
    write_new_features(fixture.path(), "new.json", r#"{"features":"[\"new\"]"}"#)?;

    let found = ensure_some(
      find_from(new_binary_path(fixture.path()).as_os_str()).ok(),
      "feature detection reads one of the available layouts",
    )?;

    ensure(found == ["legacy"], "legacy fingerprint data wins while both layouts are available")
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
  fn new_layout_rejects_missing_ambiguous_and_malformed_json() -> StdResult<(), TestFailure> {
    let no_json = TempDir::new("features-new-no-json")?;
    let no_json_fingerprint = new_build_dir(no_json.path()).join("fingerprint");
    ensure_ok_source(
      fs::create_dir_all(no_json_fingerprint.join("directory.json")),
      "json-named directory can be created",
    )?;
    ensure_ok_source(
      fs::write(no_json_fingerprint.join("notes.txt"), "not fingerprint data"),
      "non-json fingerprint noise can be written",
    )?;

    let ambiguous = TempDir::new("features-new-ambiguous")?;
    write_new_features(ambiguous.path(), "first.json", r#"{"features":"[]"}"#)?;
    write_new_features(ambiguous.path(), "second.json", r#"{"features":"[]"}"#)?;

    let malformed = TempDir::new("features-new-malformed")?;
    write_new_features(malformed.path(), "malformed.json", "{not json")?;

    ensure_all(&[
      (
        find_from(new_binary_path(no_json.path()).as_os_str()).is_err(),
        "new-layout feature detection rejects directories without a regular JSON file",
      ),
      (
        find_from(new_binary_path(ambiguous.path()).as_os_str()).is_err(),
        "new-layout feature detection rejects multiple JSON files",
      ),
      (
        find_from(new_binary_path(malformed.path()).as_os_str()).is_err(),
        "new-layout feature detection rejects malformed JSON",
      ),
    ])
  }

  #[test]
  fn new_layout_ignores_unrelated_entries_around_one_fingerprint() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("features-new-noise")?;
    let fingerprint = new_build_dir(fixture.path()).join("fingerprint");
    write_new_features(fixture.path(), "trybuild.json", r#"{"features":"[\"diff\"]"}"#)?;
    ensure_ok_source(fs::write(fingerprint.join("notes.txt"), "noise"), "non-json file can be written")?;
    ensure_ok_source(
      fs::create_dir_all(fingerprint.join("directory.json")),
      "json-named directory can be created",
    )?;

    let found = ensure_some(
      find_from(new_binary_path(fixture.path()).as_os_str()).ok(),
      "new-layout feature detection ignores unrelated entries",
    )?;

    ensure(found == ["diff"], "the one regular JSON fingerprint remains authoritative")
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
