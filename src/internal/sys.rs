//! Host-system interactions: filesystem directories, the `TRYBUILD`
//! environment variable, and the cross-process build lock.

pub(in crate::internal) mod directory;
pub(in crate::internal) mod env;
pub(in crate::internal) mod flock;

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::result::Result as StdResult;

/// Errors arising from host-system interactions.
#[derive(thiserror::Error, Debug)]
pub enum SysError {
    /// A filesystem or other I/O operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Failed to open a path; carries the path that could not be opened.
    #[error("{}: {}", .0.display(), .1)]
    Open(PathBuf, #[source] io::Error),
    /// The `TRYBUILD` environment variable held an unrecognized value.
    #[error(
        r#"unrecognized value of TRYBUILD: {:?} is not one of "verify", "wip", "overwrite""#,
        .0.to_string_lossy()
    )]
    UpdateVar(OsString),
}

/// Result alias for [`sys`](self) operations.
pub(in crate::internal) type Result<T> = StdResult<T, SysError>;

#[cfg(test)]
mod tests {
    use super::*;
    use strict_test_support::TestFailure;

    #[test]
    fn update_var_message_lists_supported_modes() -> StdResult<(), TestFailure> {
        let error = SysError::UpdateVar(OsString::from("later"));
        let message = error.to_string();

        strict_test_support::ensure(
            message
                == r#"unrecognized value of TRYBUILD: "later" is not one of "verify", "wip", "overwrite""#,
            "invalid TRYBUILD values report the supported modes",
        )?;
        Ok(())
    }
}
