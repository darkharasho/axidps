//! One launcher per user: a second one (AxiOM starting it while the game-started copy runs, or
//! a double-click) would run a second core and overlay host on the same ring and frame file.

use std::io;
#[cfg(unix)]
use std::path::Path;

/// Held while this launcher runs; dropping it (or the process ending) frees it.
pub struct Instance {
    #[cfg(unix)]
    _file: std::fs::File,
    #[cfg(windows)]
    _mutex: mutex::Mutex,
}

/// Takes the lock on `<dir>/launcher.lock` (`flock`). `None` when another launcher holds it.
#[cfg(unix)]
pub fn acquire(dir: &Path) -> io::Result<Option<Instance>> {
    use std::os::fd::AsRawFd;
    std::fs::create_dir_all(dir)?;
    // Opened close-on-exec, so no child (the AppImage a Linux update starts) inherits it.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("launcher.lock"))?;
    // SAFETY: flock on a descriptor we own.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(Instance { _file: file }));
    }
    let e = io::Error::last_os_error();
    if e.kind() == io::ErrorKind::WouldBlock {
        Ok(None)
    } else {
        Err(e)
    }
}

/// [`acquire`], retried every 200 ms while another launcher holds the lock and `keep_waiting`
/// says to: the one a Linux update starts runs while the old launcher is still exiting, and the
/// one the game starts waits while a launcher started by hand runs.
pub fn acquire_while(
    dir: &std::path::Path,
    keep_waiting: impl FnMut() -> bool,
) -> io::Result<Option<Instance>> {
    wait_for_lock(
        || acquire(dir),
        keep_waiting,
        || std::thread::sleep(std::time::Duration::from_millis(200)),
    )
}

/// Tries `try_lock` until it succeeds or `keep_waiting` says to give up, calling `pause`
/// between tries.
fn wait_for_lock<T>(
    mut try_lock: impl FnMut() -> io::Result<Option<T>>,
    mut keep_waiting: impl FnMut() -> bool,
    mut pause: impl FnMut(),
) -> io::Result<Option<T>> {
    loop {
        if let Some(lock) = try_lock()? {
            return Ok(Some(lock));
        }
        if !keep_waiting() {
            return Ok(None);
        }
        pause();
    }
}

/// Creates the named mutex [`LAUNCHER_MUTEX`] (per login session); the DLL looks for it to
/// tell whether to start the launcher. `None` when it already exists.
///
/// [`LAUNCHER_MUTEX`]: axidps_protocol::paths::LAUNCHER_MUTEX
#[cfg(windows)]
pub fn acquire(_dir: &std::path::Path) -> io::Result<Option<Instance>> {
    Ok(mutex::Mutex::create()?.map(|m| Instance { _mutex: m }))
}

#[cfg(windows)]
mod mutex {
    use std::io;

    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::HSTRING;

    pub struct Mutex(HANDLE);

    impl Mutex {
        pub fn create() -> io::Result<Option<Mutex>> {
            // SAFETY: plain Win32 calls; the name is a static NUL-terminated string.
            unsafe {
                let name = HSTRING::from(axidps_protocol::paths::LAUNCHER_MUTEX);
                let handle = CreateMutexW(None, false, &name).map_err(io::Error::other)?;
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    let _ = CloseHandle(handle);
                    return Ok(None);
                }
                Ok(Some(Mutex(handle)))
            }
        }
    }

    impl Drop for Mutex {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and closed once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod wait_tests {
    use super::*;

    #[test]
    fn lock_waits_while_asked_and_gives_up_when_not() {
        let mut tries = 0;
        let got = wait_for_lock(
            || {
                tries += 1;
                Ok((tries == 3).then_some("lock"))
            },
            || true,
            || {},
        );
        assert_eq!(got.unwrap(), Some("lock"));
        let mut asked = 0;
        let none: io::Result<Option<()>> = wait_for_lock(
            || Ok(None),
            || {
                asked += 1;
                asked < 4
            },
            || {},
        );
        assert!(none.unwrap().is_none());
        assert_eq!(asked, 4);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn within(wait: std::time::Duration) -> impl FnMut() -> bool {
        let deadline = std::time::Instant::now() + wait;
        move || std::time::Instant::now() < deadline
    }

    #[test]
    fn second_acquire_fails_until_the_first_is_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cfg");
        let first = acquire(&dir).unwrap();
        assert!(first.is_some());
        assert!(acquire(&dir).unwrap().is_none());
        drop(first);
        // Waiting: a process another test forks in parallel holds a copy of the descriptor
        // until it execs (close-on-exec), which briefly keeps the lock taken.
        let again = acquire_while(&dir, within(std::time::Duration::from_secs(5))).unwrap();
        assert!(again.is_some());
    }

    #[test]
    fn waiting_acquire_gets_the_lock_once_it_is_freed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cfg");
        let first = acquire(&dir).unwrap().unwrap();
        let t = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(first);
        });
        let got = acquire_while(&dir, within(std::time::Duration::from_secs(5))).unwrap();
        assert!(got.is_some());
        t.join().unwrap();
        let none = acquire_while(&dir, within(std::time::Duration::from_millis(100))).unwrap();
        assert!(none.is_none());
    }
}
