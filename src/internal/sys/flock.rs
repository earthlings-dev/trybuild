//! The cross-process build lock that serializes trybuild runs sharing the
//! generated project directory.
//!
//! Two layers cooperate: a process-global mutex coordinates `#[test]` functions
//! within one test binary, and a best-effort lockfile — whose mtime is bumped
//! by a background thread — coordinates separate test binaries.

use std::fs::File;
use std::fs::OpenOptions;
use std::fs::{
  self,
};
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use parking_lot::Mutex;
use parking_lot::MutexGuard;

use crate::internal::error::Result;
use crate::internal::sys::SysError;

/// Process-global mutex coordinating `#[test]` functions within one binary.
static LOCK: Mutex<()> = Mutex::new(());

/// The acquired build lock, releasing both layers when dropped.
pub(in crate::internal) struct Lock {
  /// Holds the process-global mutex.
  intraprocess_guard: Guard,
  /// Holds the cross-process lockfile, if one could be taken.
  lockfile:           FileLock,
}

/// The intraprocess half of the lock, coordinating `#[test]` functions within
/// the *same* test binary.
enum Guard {
  /// Not (or no longer) holding the mutex.
  NotLocked,
  /// Holding the process-global mutex guard.
  Locked(
    #[expect(
      dead_code,
      reason = "the guard is held only to keep the mutex locked until Lock is dropped"
    )]
    MutexGuard<'static, ()>,
  ),
}

/// The cross-process half of the lock: a best-effort lockfile coordinating
/// *different* test binaries.
enum FileLock {
  /// No lockfile taken (file-based locking unavailable); not coordinating.
  NotLocked,
  /// Holding a lockfile, kept fresh by a background poll thread.
  Locked {
    /// Path to the lockfile, removed on drop.
    path: PathBuf,
    /// Signals the poll thread to stop.
    done: Arc<AtomicBool>,
  },
}

impl Lock {
  /// Acquires both lock layers.
  #[allow(
    clippy::single_call_fn,
    reason = "the composite lock constructor coordinating both layers, the type's entry point to the two-layer locking abstraction this \
              module documents"
  )]
  pub(in crate::internal) fn acquire(path: impl AsRef<Path>) -> Result<Self> {
    Ok(Self {
      intraprocess_guard: Guard::acquire(),
      lockfile:           FileLock::acquire(path)?,
    })
  }
}

impl Guard {
  /// Takes the process-global mutex.
  #[allow(
    clippy::single_call_fn,
    reason = "the intraprocess-layer constructor, named to mirror FileLock::acquire across the two cooperating halves of the lock"
  )]
  fn acquire() -> Self {
    Self::Locked(LOCK.lock())
  }
}

impl FileLock {
  /// Takes the lockfile and spawns the background mtime-refresh thread,
  /// falling back to [`FileLock::NotLocked`] if a lockfile cannot be created.
  #[allow(
    clippy::single_call_fn,
    reason = "the cross-process-layer constructor, named to mirror Guard::acquire and own the lockfile creation plus poll-thread spawn"
  )]
  fn acquire(path: impl AsRef<Path>) -> Result<Self> {
    let owned_path = path.as_ref().to_owned();
    let Some(lockfile) = create(&owned_path) else {
      return Ok(Self::NotLocked);
    };
    let done = Arc::new(AtomicBool::new(false));
    let thread = thread::Builder::new().name("trybuild-flock".to_owned());
    // The poll thread is detached: it observes `done` and exits on its own,
    // so the join handle is bound and dropped here rather than joined (joining
    // would add latency to every lock release).
    let _poll_thread = thread
      .spawn({
        let thread_done = Arc::clone(&done);
        move || poll(&lockfile, &thread_done)
      })
      .map_err(SysError::Io)?;
    Ok(Self::Locked {
      path: owned_path,
      done,
    })
  }
}

impl Drop for Lock {
  fn drop(&mut self) {
    // Unlock file lock first.
    self.lockfile = FileLock::NotLocked;
    self.intraprocess_guard = Guard::NotLocked;
  }
}

impl Drop for FileLock {
  fn drop(&mut self) {
    if let &mut Self::Locked {
      ref path,
      ref done,
    } = self
    {
      done.store(true, Ordering::Release);
      // Best-effort cleanup of the lockfile as the lock is released.
      let _removed = fs::remove_file(path);
    }
  }
}

/// Creates the lockfile, busting a stale or future-dated one and otherwise
/// waiting briefly for the current holder to finish.
///
/// Returns `None` if file-based locking is unavailable.
#[allow(
  clippy::single_call_fn,
  reason = "a documented helper encapsulating the stale/future lockfile-busting loop, kept out of FileLock::acquire's happy path"
)]
#[allow(
  clippy::wildcard_enum_match_arm,
  reason = "matching `io::ErrorKind`, which is `#[non_exhaustive]`: a catch-all arm is mandatory and the dozens of named variants carry \
            no distinct handling here"
)]
fn create(path: &Path) -> Option<File> {
  loop {
    match OpenOptions::new().write(true).create_new(true).open(path) {
      // Acquired lock by creating lockfile.
      Ok(lockfile) => return Some(lockfile),
      Err(io_error) => match io_error.kind() {
        // Lock is already held by another test.
        io::ErrorKind::AlreadyExists => {}
        // File based locking isn't going to work for some reason.
        _ => return None,
      },
    }

    // Check whether it's okay to bust the lock.
    let metadata = match fs::metadata(path) {
      Ok(metadata) => metadata,
      Err(io_error) => match io_error.kind() {
        // Other holder of the lock finished. Retry.
        io::ErrorKind::NotFound => continue,
        _ => return None,
      },
    };

    let Ok(system_time) = metadata.modified() else {
      return None;
    };

    let modified = DateTime::<Utc>::from(system_time);
    let now = Utc::now();
    let skew = TimeDelta::milliseconds(1500);
    let stale = now.checked_sub_signed(skew).is_some_and(|cutoff| modified < cutoff);
    let future = now.checked_add_signed(skew).is_some_and(|cutoff| cutoff < modified);
    if stale || future {
      return File::create(path).ok();
    }

    // Try again shortly.
    thread::sleep(Duration::from_millis(500));
  }
}

/// Background thread body: periodically bumps the lockfile's mtime so other
/// processes can tell the lock is still held, until `done` is set.
#[allow(
  clippy::single_call_fn,
  reason = "the background mtime-refresh thread body, named for the spawn site in FileLock::acquire rather than inlined into the closure"
)]
fn poll(lockfile: &File, done: &AtomicBool) {
  loop {
    thread::sleep(Duration::from_millis(500));
    if done.load(Ordering::Acquire) || lockfile.set_len(0).is_err() {
      return;
    }
  }
}
