//! Assembling the rustflags passed to the test crates: trybuild's own cfgs,
//! ignored lints, and any coverage flags forwarded from the environment.

use std::env;
use std::ffi::OsString;

/// Lints silenced in the test crates, where warnings would be noise.
const IGNORED_LINTS: &[&str] = &["dead_code"];

/// Builds the rustflags array for the generated project as a TOML value.
///
/// Always passes `--cfg trybuild --verbose`, allows [`IGNORED_LINTS`], forwards
/// `-C instrument-coverage` from `RUSTFLAGS` when present, and appends
/// `extra_rustflags`.
#[allow(
  clippy::single_call_fn,
  reason = "the rustflags assembly is a named, documented construction step kept separate from the cargo-command builders in \
            `build::cargo` that consume it"
)]
pub(in crate::internal) fn toml(extra_rustflags: &[&'static str]) -> toml::Value {
  toml_from(env::var_os("RUSTFLAGS"), extra_rustflags)
}

/// Builds the rustflags TOML value from an injected `RUSTFLAGS` value.
#[allow(
  clippy::single_call_fn,
  reason = "the injectable rustflags builder separates pure flag policy from the environment-reading entry point"
)]
pub(in crate::internal) fn toml_from(rustflags_env: Option<OsString>, extra_rustflags: &[&'static str]) -> toml::Value {
  let mut rustflags = vec!["--cfg", "trybuild", "--verbose"];

  for &lint in IGNORED_LINTS {
    rustflags.push("-A");
    rustflags.push(lint);
  }

  if let Some(flags) = rustflags_env {
    // TODO: could parse this properly and allowlist or blocklist certain
    // flags. This is good enough to at least support cargo-llvm-cov.
    if flags.to_string_lossy().contains("-C instrument-coverage") {
      rustflags.extend(["-C", "instrument-coverage"]);
    }
  }

  rustflags.extend(extra_rustflags);

  toml::Value::Array(rustflags.into_iter().map(|flag| toml::Value::String(flag.to_owned())).collect())
}

#[cfg(test)]
mod tests {
  use std::ffi::OsString;
  use std::result::Result as StdResult;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_some;

  use super::*;

  fn flags(value: &toml::Value) -> StdResult<Vec<&str>, TestFailure> {
    let array = ensure_some(value.as_array(), "rustflags TOML is an array")?;
    Ok(array.iter().filter_map(toml::Value::as_str).collect())
  }

  #[test]
  fn toml_from_builds_default_and_extra_flags() -> StdResult<(), TestFailure> {
    let value = toml_from(None, &["--diagnostic-width=140"]);
    let flags = flags(&value)?;

    ensure_all(&[
      (flags.contains(&"--cfg"), "default rustflags include the cfg flag"),
      (flags.contains(&"trybuild"), "default rustflags include the trybuild cfg value"),
      (flags.contains(&"-A"), "default rustflags allow selected lints"),
      (flags.contains(&"dead_code"), "default rustflags allow dead_code in fixtures"),
      (flags.contains(&"--diagnostic-width=140"), "extra rustflags are appended"),
      (
        !flags.contains(&"instrument-coverage"),
        "coverage instrumentation is not forwarded when absent",
      ),
    ])
  }

  #[test]
  fn toml_from_forwards_coverage_instrumentation() -> StdResult<(), TestFailure> {
    let value = toml_from(Some(OsString::from("-C instrument-coverage")), &[]);
    let flags = flags(&value)?;

    ensure_all(&[
      (flags.contains(&"-C"), "coverage forwarding includes the -C option"),
      (
        flags.contains(&"instrument-coverage"),
        "coverage forwarding includes the instrument-coverage value",
      ),
    ])
  }
}
