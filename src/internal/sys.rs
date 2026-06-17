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
    #[error("unrecognized value of TRYBUILD: {:?}", .0.to_string_lossy())]
    UpdateVar(OsString),
}

/// Result alias for [`sys`](self) operations.
pub(in crate::internal) type Result<T> = StdResult<T, SysError>;
