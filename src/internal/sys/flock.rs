//! The cross-process build lock that serializes trybuild runs sharing the
//! generated project directory.
//!
//! Two layers cooperate: a process-global mutex coordinates `#[test]` functions
//! within one test binary, and a best-effort lockfile — whose mtime is bumped
//! by a background thread — coordinates separate test binaries.

use std::fs;
use std::fs::File;
use std::fs::FileTimes;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::SystemTime;

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
      match fs::remove_file(path) {
        Ok(()) => continue,
        Err(io_error) => match io_error.kind() {
          io::ErrorKind::NotFound => continue,
          _ => return None,
        },
      }
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
    let times = FileTimes::new().set_modified(SystemTime::from(Utc::now()));
    if done.load(Ordering::Acquire) || lockfile.set_times(times).is_err() {
      return;
    }
  }
}

#[cfg(test)]
mod tests {
  use std::fs::FileTimes;
  use std::result::Result as StdResult;
  use std::sync::atomic::AtomicBool;
  use std::time::SystemTime;

  use chrono::Utc;
  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;
  use strict_test_support::ensure_some;

  use super::*;

  fn set_modified(file: &File, modified: SystemTime) -> StdResult<(), TestFailure> {
    let times = FileTimes::new().set_modified(modified);
    ensure_ok_source(file.set_times(times), "lockfile timestamp can be set")
  }

  #[test]
  fn create_acquires_free_stale_and_future_lockfiles() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("flock-create")?;
    let free = fixture.child("free.lock");
    let stale = fixture.child("stale.lock");
    let future = fixture.child("future.lock");

    let free_lock = ensure_some(create(&free), "free lockfiles can be created")?;
    let stale_lock = ensure_ok_source(File::create(&stale), "stale lockfile can be seeded")?;
    let future_lock = ensure_ok_source(File::create(&future), "future lockfile can be seeded")?;
    let now = SystemTime::from(Utc::now());
    let stale_time = now.checked_sub(Duration::from_secs(4)).unwrap_or(SystemTime::UNIX_EPOCH);
    let future_time = now.checked_add(Duration::from_hours(1)).unwrap_or(SystemTime::UNIX_EPOCH);
    set_modified(&stale_lock, stale_time)?;
    set_modified(&future_lock, future_time)?;

    ensure_all(&[
      (free.exists(), "new lockfiles are created on disk"),
      (create(&stale).is_some(), "stale lockfiles are busted"),
      (create(&future).is_some(), "future-dated lockfiles are busted"),
      (free_lock.metadata().is_ok(), "created lockfiles remain open"),
    ])
  }

  #[test]
  fn create_declines_uncreatable_lock_paths() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("flock-missing")?;
    let missing_parent = fixture.child("missing").join("lock");

    ensure(
      create(&missing_parent).is_none(),
      "file locking is skipped when the lockfile parent does not exist",
    )
  }

  #[test]
  fn create_waits_for_active_lock_release() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("flock-wait")?;
    let path = fixture.child("held.lock");
    let held = ensure_ok_source(File::create(&path), "held lockfile can be seeded")?;
    let release_path = path.clone();
    let release = thread::spawn(move || {
      thread::sleep(Duration::from_millis(200));
      drop(held);
      let _removed = fs::remove_file(release_path);
    });

    let acquired = ensure_some(create(&path), "waiting lock acquisition succeeds after release")?;
    let release_finished = release.join().is_ok();

    ensure_all(&[
      (release_finished, "the holder thread releases the lockfile"),
      (path.exists(), "the waiting acquisition recreates the lockfile"),
      (acquired.metadata().is_ok(), "the waiting acquisition owns an open lockfile"),
    ])
  }

  #[test]
  fn poll_stops_when_signalled() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("flock-poll")?;
    let path = fixture.child("poll.lock");
    let lockfile = ensure_ok_source(File::create(&path), "poll lockfile can be created")?;
    let done = AtomicBool::new(true);

    poll(&lockfile, &done);

    ensure(path.exists(), "poll leaves the lockfile in place when stopping")
  }

  #[test]
  fn poll_refreshes_lockfile_until_signalled() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("flock-poll-refresh")?;
    let path = fixture.child("poll-refresh.lock");
    let lockfile = ensure_ok_source(File::create(&path), "poll-refresh lockfile can be created")?;
    let old_time = SystemTime::from(Utc::now())
      .checked_sub(Duration::from_secs(4))
      .unwrap_or(SystemTime::UNIX_EPOCH);
    set_modified(&lockfile, old_time)?;
    let done = Arc::new(AtomicBool::new(false));
    let thread_done = Arc::clone(&done);
    let refresh = thread::spawn(move || poll(&lockfile, &thread_done));

    thread::sleep(Duration::from_millis(650));
    done.store(true, Ordering::Release);
    let refresh_finished = refresh.join().is_ok();
    let modified = ensure_ok_source(fs::metadata(&path), "refreshed lockfile metadata can be read")?.modified();
    let refreshed = ensure_ok_source(modified, "refreshed lockfile mtime can be read")?;

    ensure_all(&[
      (refresh_finished, "the poll thread exits after it is signalled"),
      (old_time < refreshed, "poll refreshes the lockfile mtime while active"),
      (path.exists(), "poll keeps the lockfile in place while active"),
    ])
  }

  #[test]
  fn lock_acquire_removes_lockfile_on_drop() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("flock-acquire")?;
    let path = fixture.child("held.lock");
    {
      let _lock = ensure_ok_source(Lock::acquire(&path), "lock can be acquired")?;
      ensure(path.exists(), "acquiring a lock creates the lockfile")?;
    };

    ensure(!path.exists(), "dropping the lock removes the lockfile")
  }
}
