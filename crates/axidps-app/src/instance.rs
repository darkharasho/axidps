//! One launcher per user: a second one (AxiOM starting it while the autostart copy runs, or a
//! double-click) would run a second core and overlay host on the same ring and frame file.

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

/// [`acquire`], retried for up to `wait` while another launcher holds the lock: the one a Linux
/// update starts runs while the old launcher is still exiting.
pub fn acquire_waiting(dir: &std::path::Path, wait: std::time::Duration) -> io::Result<Option<Instance>> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        match acquire(dir)? {
            Some(i) => return Ok(Some(i)),
            None if std::time::Instant::now() >= deadline => return Ok(None),
            None => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    }
}

/// Creates the named mutex `Local\axidps-launcher` (per login session). `None` when it already
/// exists.
#[cfg(windows)]
pub fn acquire(_dir: &std::path::Path) -> io::Result<Option<Instance>> {
    Ok(mutex::Mutex::create()?.map(|m| Instance { _mutex: m }))
}

#[cfg(windows)]
mod mutex {
    use std::io;

    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    pub struct Mutex(HANDLE);

    impl Mutex {
        pub fn create() -> io::Result<Option<Mutex>> {
            // SAFETY: plain Win32 calls; the name is a static NUL-terminated string.
            unsafe {
                let handle = CreateMutexW(None, false, w!("Local\\axidps-launcher"))
                    .map_err(io::Error::other)?;
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

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
        let again = acquire_waiting(&dir, std::time::Duration::from_secs(5)).unwrap();
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
        let got = acquire_waiting(&dir, std::time::Duration::from_secs(5)).unwrap();
        assert!(got.is_some());
        t.join().unwrap();
        let none = acquire_waiting(&dir, std::time::Duration::from_millis(100)).unwrap();
        assert!(none.is_none());
    }
}
