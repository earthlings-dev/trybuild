//! Build script for trybuild: declares custom `cfg` flags and rerun triggers.

use std::io;

/// Complete ordered instruction stream consumed by Cargo.
const CARGO_DIRECTIVES: &[u8] = b"cargo:rerun-if-changed=src/tests\n\
cargo:rustc-cfg=check_cfg\n\
cargo:rustc-check-cfg=cfg(check_cfg)\n\
cargo:rustc-check-cfg=cfg(trybuild_no_target)\n";

/// Write the complete Cargo instruction stream to `output`.
#[allow(
  clippy::single_call_fn,
  reason = "the build script and its explicit test target compile this shared owner under disjoint cfg modes"
)]
fn write_cargo_directives(mut output: impl io::Write) -> io::Result<()> {
  output.write_all(CARGO_DIRECTIVES)
}

#[cfg(not(test))]
fn main() -> io::Result<()> {
  // Warning: build.rs is not published to crates.io.

  write_cargo_directives(io::stdout().lock())
}

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_ok_source;
  use strict_test_support::ensure_some;

  use super::*;

  #[test]
  fn writes_the_complete_ordered_cargo_directive_stream() -> Result<(), TestFailure> {
    let mut output = Vec::new();

    ensure_ok_source(write_cargo_directives(&mut output), "the Cargo directive stream can be written")?;

    ensure(
      output == CARGO_DIRECTIVES,
      "the build script preserves every Cargo directive and its order",
    )
  }

  #[test]
  fn propagates_cargo_directive_write_failures() -> Result<(), TestFailure> {
    let mut no_capacity = [];
    let error = ensure_some(
      write_cargo_directives(no_capacity.as_mut_slice()).err(),
      "a rejected directive write must return its I/O error",
    )?;

    ensure(
      error.kind() == io::ErrorKind::WriteZero,
      "the build script preserves the directive writer's error kind",
    )
  }
}
