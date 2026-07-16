//! Assembling the rustflags passed to the test crates: trybuild's own cfgs,
//! ignored lints, and any coverage flags forwarded from the environment.

use std::ffi::OsString;

use toml::Value;

/// Lints silenced in the test crates, where warnings would be noise.
const IGNORED_LINTS: &[&str] = &["dead_code"];

/// Builds the rustflags TOML value from an injected `RUSTFLAGS` value.
#[allow(
  clippy::single_call_fn,
  reason = "the injectable rustflags builder separates pure flag policy from the environment-reading entry point"
)]
pub(in crate::internal) fn toml_from(rustflags_env: Option<OsString>, extra_rustflags: &[&'static str]) -> Value {
  let mut rustflags = vec!["--cfg", "trybuild", "--verbose"];

  for &lint in IGNORED_LINTS {
    rustflags.push("-A");
    rustflags.push(lint);
  }

  if let Some(flags) = rustflags_env {
    // Trybuild deliberately forwards only the coverage instrumentation flag it
    // understands; unrelated host compilation policy must not leak into the
    // generated diagnostic project.
    if flags.to_string_lossy().contains("-C instrument-coverage") {
      rustflags.extend(["-C", "instrument-coverage"]);
    }
  }

  rustflags.extend(extra_rustflags);

  Value::Array(rustflags.into_iter().map(|flag| Value::String(flag.to_owned())).collect())
}

#[cfg(test)]
mod tests {
  use std::ffi::OsString;
  use std::result::Result as StdResult;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_some;

  use super::*;

  fn flags(rustflags_toml: &Value) -> StdResult<Vec<&str>, TestFailure> {
    let array = ensure_some(rustflags_toml.as_array(), "rustflags TOML is an array")?;
    Ok(array.iter().filter_map(Value::as_str).collect())
  }

  #[test]
  fn toml_from_builds_default_and_extra_flags() -> StdResult<(), TestFailure> {
    let rustflags_toml = toml_from(None, &["--diagnostic-width=140"]);
    let flags = flags(&rustflags_toml)?;

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
    let rustflags_toml = toml_from(Some(OsString::from("-C instrument-coverage")), &[]);
    let flags = flags(&rustflags_toml)?;

    ensure_all(&[
      (flags.contains(&"-C"), "coverage forwarding includes the -C option"),
      (
        flags.contains(&"instrument-coverage"),
        "coverage forwarding includes the instrument-coverage value",
      ),
    ])
  }
}
