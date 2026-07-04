//! Build script for trybuild: declares custom `cfg` flags and rerun triggers.

use std::io::Write as _;
use std::io::{
  self,
};

fn main() -> io::Result<()> {
  // Warning: build.rs is not published to crates.io.

  let mut stdout = io::stdout().lock();
  writeln!(stdout, "cargo:rerun-if-changed=src/tests")?;
  writeln!(stdout, "cargo:rustc-cfg=check_cfg")?;
  writeln!(stdout, "cargo:rustc-check-cfg=cfg(check_cfg)")?;
  writeln!(stdout, "cargo:rustc-check-cfg=cfg(trybuild_no_target)")?;
  Ok(())
}
