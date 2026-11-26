//! The launcher's log, `<config>/axidps.log`. On Windows the launcher has no console, so this
//! is the only place its messages (and core's and the overlay host's, which it copies in) go.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The log is moved to `axidps.log.1` once it reaches this size, so it stays under ~8 MB.
pub const MAX_LEN: u64 = 4 * 1024 * 1024;
const FILE_NAME: &str = "axidps.log";

/// Where stdout and stderr now go.
pub struct Log {
    path: PathBuf,
    /// The open log the standard handles point at.
    current: std::cell::RefCell<Option<Kept>>,
    /// The one before it, closed at the next rotation rather than at once: a write on another
    /// thread may have looked it up just before the switch. So at most one old log stays open.
    retired: std::cell::RefCell<Option<Kept>>,
}

/// `path` with `.1` appended.
fn previous(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// Once `path` holds at least `max` bytes, moves it to `<path>.1` (replacing an older one), or
/// empties it when that fails. True when it did either.
fn rotate(path: &Path, max: u64) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(m) if m.len() >= max => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    if fs::rename(path, previous(path)).is_err() {
        OpenOptions::new().write(true).truncate(true).open(path)?;
    }
    Ok(true)
}

fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// What a redirect leaves to keep open: nothing on Unix (dup2 made copies), the file's handle on
/// Windows (the standard handles point at it).
#[cfg(unix)]
type Kept = ();
#[cfg(windows)]
type Kept = std::os::windows::io::OwnedHandle;

/// Sends this process's stdout and stderr to `file`. Children don't inherit it: the supervisor
/// pipes their output through this process (see `supervise::forward`).
#[cfg(unix)]
fn redirect(file: File) -> io::Result<Kept> {
    use std::os::fd::AsRawFd;
    for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        // SAFETY: dup2 onto the standard descriptors; `file` stays open for the call.
        if unsafe { libc::dup2(file.as_raw_fd(), fd) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Sends this process's stdout and stderr to `file`. Rust's standard streams look the handle up
/// on every write, so this takes effect at once. The returned handle must stay open while the
/// standard handles point at it.
#[cfg(windows)]
fn redirect(file: File) -> io::Result<Kept> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
    let owned = Kept::from(file);
    let handle = HANDLE(owned.as_raw_handle() as _);
    // SAFETY: `handle` is valid and open; `owned` is kept open by the caller.
    unsafe {
        SetStdHandle(STD_OUTPUT_HANDLE, handle).map_err(io::Error::other)?;
        SetStdHandle(STD_ERROR_HANDLE, handle).map_err(io::Error::other)?;
    }
    Ok(owned)
}

/// Whether stderr is a terminal: a launcher started from a shell keeps printing there.
#[cfg(unix)]
fn on_terminal() -> bool {
    use std::io::IsTerminal;
    io::stderr().is_terminal()
}

#[cfg(windows)]
fn on_terminal() -> bool {
    false
}

/// Starts logging to `<dir>/axidps.log`, unless stderr is a terminal. Failures are printed
/// (where they can still be seen) and the launcher goes on without a log file.
pub fn init(dir: &Path, version: &str) -> Option<Log> {
    if on_terminal() {
        return None;
    }
    let path = dir.join(FILE_NAME);
    let started = fs::create_dir_all(dir)
        .and_then(|()| rotate(&path, MAX_LEN))
        .and_then(|_| open(&path))
        .and_then(redirect);
    let kept = match started {
        Ok(kept) => kept,
        Err(e) => {
            eprintln!("axidps: cannot log to {}: {e}", path.display());
            return None;
        }
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = writeln!(io::stderr(), "axidps {version} started (unix time {secs})");
    Some(Log {
        path,
        current: std::cell::RefCell::new(Some(kept)),
        retired: std::cell::RefCell::new(None),
    })
}

/// The standard handles still point at the current log when the launcher returns from its loop
/// and prints its last lines, so that handle is left open (the process is about to end).
impl Drop for Log {
    fn drop(&mut self) {
        std::mem::forget(self.current.take());
    }
}

impl Log {
    /// Rotates the log once it is too big. Children's output follows at once: it reaches the
    /// log through this process.
    pub fn maintain(&self) {
        match rotate(&self.path, MAX_LEN) {
            Ok(false) => {}
            Ok(true) => match open(&self.path).and_then(redirect) {
                Ok(kept) => {
                    let before = self.current.replace(Some(kept));
                    // Drops (closes) the log from two rotations ago.
                    self.retired.replace(before);
                }
                Err(e) => eprintln!("axidps: cannot reopen {}: {e}", self.path.display()),
            },
            Err(e) => eprintln!("axidps: cannot rotate {}: {e}", self.path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_log_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        assert!(!rotate(&path, 10).unwrap(), "missing file");
        fs::write(&path, b"short").unwrap();
        assert!(!rotate(&path, 10).unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"short");
    }

    #[test]
    fn big_log_moves_to_dot_one_replacing_the_old_one() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        fs::write(previous(&path), b"oldest").unwrap();
        fs::write(&path, b"0123456789").unwrap();
        assert!(rotate(&path, 10).unwrap());
        assert!(!path.exists());
        assert_eq!(fs::read(previous(&path)).unwrap(), b"0123456789");
        assert_eq!(previous(&path), tmp.path().join("axidps.log.1"));
    }
}
