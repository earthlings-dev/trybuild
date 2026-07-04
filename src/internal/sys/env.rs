//! The `TRYBUILD` environment variable controlling how missing or mismatched
//! snapshots are reconciled.

use std::env;
use std::ffi::OsStr;

use crate::internal::sys::Result;
use crate::internal::sys::SysError;

/// How the runner reconciles a `compile_fail` test with its `.stderr` snapshot.
///
/// [`run`](crate::TestCases::run) reads this from the `TRYBUILD` environment
/// variable; [`try_run`](crate::TestCases::try_run) takes it as an explicit
/// argument so a programmatic caller controls it without touching the
/// environment.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Update {
  /// Fail when a snapshot is missing or mismatched, writing nothing.
  Verify,
  /// Default: write a new snapshot under `wip` (or report a mismatch) without
  /// touching the saved file.
  #[default]
  Wip,
  /// Write the snapshot in place (`TRYBUILD=overwrite`).
  Overwrite,
}

impl Update {
  /// Reads the update mode from the `TRYBUILD` environment variable,
  /// defaulting to [`Wip`](Self::Wip) when the variable is unset.
  ///
  /// # Errors
  ///
  /// Returns [`SysError::UpdateVar`] if the variable holds an unrecognized value.
  #[allow(
    clippy::single_call_fn,
    reason = "the constructor reading the update mode from the TRYBUILD variable, kept on Update's own type so the env-var contract lives \
              with the enum it produces"
  )]
  pub(in crate::internal) fn env() -> Result<Self> {
    let var = env::var_os("TRYBUILD");
    Self::parse(var.as_deref())
  }

  /// Parses an optional raw `TRYBUILD` value.
  #[allow(
    clippy::single_call_fn,
    reason = "TRYBUILD parsing is intentionally separated from environment access so tests do not mutate process env"
  )]
  pub(in crate::internal) fn parse(raw: Option<&OsStr>) -> Result<Self> {
    let Some(var) = raw else {
      return Ok(Self::default());
    };

    match var.to_str() {
      Some("verify") => Ok(Self::Verify),
      Some("wip") => Ok(Self::Wip),
      Some("overwrite") => Ok(Self::Overwrite),
      _ => Err(SysError::UpdateVar(var.to_owned())),
    }
  }
}

#[cfg(test)]
mod tests {
  use std::ffi::OsString;
  use std::result::Result as StdResult;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;

  use super::*;

  #[test]
  fn parse_accepts_default_and_known_modes() -> StdResult<(), TestFailure> {
    let verify = ensure_ok_source(Update::parse(Some(OsStr::new("verify"))), "verify mode parses")?;
    let wip = ensure_ok_source(Update::parse(Some(OsStr::new("wip"))), "wip mode parses")?;
    let overwrite = ensure_ok_source(Update::parse(Some(OsStr::new("overwrite"))), "overwrite mode parses")?;
    let defaulted = ensure_ok_source(Update::parse(None), "unset mode defaults")?;

    ensure_all(&[
      (verify == Update::Verify, "verify selects verify mode"),
      (wip == Update::Wip, "wip selects wip mode"),
      (overwrite == Update::Overwrite, "overwrite selects overwrite mode"),
      (defaulted == Update::Wip, "unset TRYBUILD defaults to wip mode"),
    ])
  }

  #[test]
  fn parse_rejects_unknown_modes() -> StdResult<(), TestFailure> {
    let error = Update::parse(Some(OsStr::new("later"))).err();
    ensure(
      error.is_some_and(|err| err.to_string().contains("verify")),
      "unrecognized TRYBUILD values report the supported modes",
    )?;
    ensure(
      Update::parse(Some(OsString::from("later").as_os_str())).is_err(),
      "unrecognized OsString values are rejected",
    )
  }
}
