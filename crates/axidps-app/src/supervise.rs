//! Keeps `axidps-core` and the overlay host running, restarting them with a backoff.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FIRST_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(30);
/// A child that stayed up this long starts over at the first delay.
const STABLE_UPTIME: Duration = Duration::from_secs(60);
/// How long a child asked to stop gets to finish (core writes the fight in progress) before it
/// is killed.
const GRACE: Duration = Duration::from_secs(10);

/// Restart delays: 1 s doubling to 30 s, reset by a long-lived run.
pub struct Backoff {
    next: Duration,
}

impl Backoff {
    pub fn new() -> Self {
        Backoff { next: FIRST_DELAY }
    }

    /// The wait before the next start, given how long the last run stayed up.
    pub fn next_delay(&mut self, uptime: Duration) -> Duration {
        if uptime >= STABLE_UPTIME {
            self.next = FIRST_DELAY;
        }
        let delay = self.next;
        self.next = (self.next * 2).min(MAX_DELAY);
        delay
    }
}

/// A program the launcher keeps running.
pub struct Child {
    pub name: &'static str,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Set for a child that stops cleanly when asked: on shutdown it gets SIGTERM (Unix) and
    /// this file is created (its `--stop-file`), then up to [`GRACE`] to exit. Others are killed.
    pub stop_file: Option<PathBuf>,
}

struct Slot {
    spec: Child,
    process: Option<std::process::Child>,
    started: Instant,
    /// Earliest time the next start may happen.
    not_before: Instant,
    backoff: Backoff,
    spawns: u32,
    /// The arguments the running process was started with (they may differ from `spec.args`
    /// until [`Supervisor::set_args`] may restart it).
    running_args: Vec<OsString>,
    /// The running process was already asked to stop for new arguments.
    restart_asked: bool,
}

pub struct Supervisor {
    slots: Vec<Slot>,
    /// Closing it (the launcher dying included) kills every child assigned to it.
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl Supervisor {
    pub fn new(children: Vec<Child>) -> Self {
        let now = Instant::now();
        let slots = children
            .into_iter()
            .map(|spec| Slot {
                spec,
                process: None,
                started: now,
                not_before: now,
                backoff: Backoff::new(),
                spawns: 0,
                running_args: Vec::new(),
                restart_asked: false,
            })
            .collect();
        Supervisor {
            slots,
            #[cfg(windows)]
            job: job::Job::new()
                .map_err(|e| eprintln!("axidps: no job object, children may outlive us: {e}"))
                .ok(),
        }
    }

    /// Notices exited children and (re)starts those whose backoff has elapsed.
    pub fn tick(&mut self, now: Instant) {
        for slot in &mut self.slots {
            if let Some(process) = &mut slot.process {
                match process.try_wait() {
                    Ok(None) => continue,
                    Ok(Some(status)) => {
                        eprintln!("axidps: {} exited ({status})", slot.spec.name);
                    }
                    Err(e) => eprintln!("axidps: cannot poll {}: {e}", slot.spec.name),
                }
                slot.process = None;
                let delay = slot
                    .backoff
                    .next_delay(now.saturating_duration_since(slot.started));
                slot.not_before = now + delay;
            }
            if now < slot.not_before {
                continue;
            }
            slot.started = now;
            slot.spawns += 1;
            match spawn(&slot.spec) {
                Ok(process) => {
                    #[cfg(windows)]
                    if let Some(job) = &self.job
                        && let Err(e) = job.assign(&process)
                    {
                        eprintln!("axidps: cannot tie {} to the launcher: {e}", slot.spec.name);
                    }
                    slot.process = Some(process);
                    slot.running_args = slot.spec.args.clone();
                    slot.restart_asked = false;
                }
                Err(e) => {
                    eprintln!("axidps: cannot start {}: {e}", slot.spec.name);
                    slot.not_before = now + slot.backoff.next_delay(Duration::ZERO);
                }
            }
        }
    }

    /// Sets the arguments `name` starts with; they apply at its next start. With `restart`, a
    /// running child started with other arguments is asked to stop now (the same graceful way
    /// as [`Supervisor::shutdown`], killed if it has no stop file), so that
    /// [`Supervisor::tick`] starts it again with them. That compares against the arguments the
    /// process runs with, not the last ones set: a change set while restarting was not allowed
    /// is still applied by a later call that allows it. Returns whether a restart was asked for.
    pub fn set_args(&mut self, name: &str, args: Vec<OsString>, restart: bool) -> bool {
        let Some(slot) = self.slots.iter_mut().find(|s| s.spec.name == name) else {
            return false;
        };
        slot.spec.args = args;
        let Some(process) = &mut slot.process else {
            return false;
        };
        if !restart || slot.restart_asked || slot.running_args == slot.spec.args {
            return false;
        }
        eprintln!("axidps: restarting {} with new arguments", slot.spec.name);
        slot.restart_asked = true;
        let asked = slot
            .spec
            .stop_file
            .as_deref()
            .is_some_and(|file| request_stop(process, file, slot.spec.name));
        if !asked {
            let _ = process.kill();
        }
        true
    }

    /// Stops every child: asks those with a stop file to stop, waits up to 10 s for them, and
    /// kills the rest (and any that are still running then).
    pub fn shutdown(&mut self) {
        self.shutdown_within(GRACE);
    }

    fn shutdown_within(&mut self, grace: Duration) {
        for slot in &mut self.slots {
            let Some(process) = &mut slot.process else {
                continue;
            };
            let asked = slot
                .spec
                .stop_file
                .as_deref()
                .is_some_and(|file| request_stop(process, file, slot.spec.name));
            if !asked {
                let _ = process.kill();
            }
        }
        let deadline = Instant::now() + grace;
        for slot in &mut self.slots {
            if let Some(mut process) = slot.process.take() {
                loop {
                    match process.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        _ => {
                            eprintln!(
                                "axidps: {} did not stop within {} s; killing it",
                                slot.spec.name,
                                grace.as_secs()
                            );
                            let _ = process.kill();
                            let _ = process.wait();
                            break;
                        }
                    }
                }
            }
            if let Some(file) = &slot.spec.stop_file {
                let _ = std::fs::remove_file(file);
            }
        }
    }

    #[cfg(test)]
    fn spawn_count(&self, name: &str) -> u32 {
        self.slots
            .iter()
            .find(|s| s.spec.name == name)
            .map_or(0, |s| s.spawns)
    }

    #[cfg(all(test, target_os = "linux"))]
    fn pid(&self, name: &str) -> Option<u32> {
        self.slots
            .iter()
            .find(|s| s.spec.name == name)
            .and_then(|s| s.process.as_ref().map(|p| p.id()))
    }

    #[cfg(test)]
    fn is_running(&self, name: &str) -> bool {
        self.slots
            .iter()
            .any(|s| s.spec.name == name && s.process.is_some())
    }
}

/// Asks `process` to stop: creates `file` and, on Unix, sends SIGTERM. False when neither could
/// be done, so the caller kills it instead of waiting.
fn request_stop(process: &std::process::Child, file: &Path, name: &str) -> bool {
    let wrote = match std::fs::write(file, b"") {
        Ok(()) => true,
        Err(e) => {
            eprintln!(
                "axidps: cannot ask {name} to stop ({}): {e}",
                file.display()
            );
            false
        }
    };
    // SAFETY: plain kill(2) on our own child, which has not been reaped yet.
    #[cfg(unix)]
    let signalled = unsafe { libc::kill(process.id() as libc::pid_t, libc::SIGTERM) } == 0;
    #[cfg(not(unix))]
    let signalled = {
        let _ = process;
        false
    };
    wrote || signalled
}

fn spawn(spec: &Child) -> std::io::Result<std::process::Child> {
    if let Some(file) = &spec.stop_file {
        // A stop request left over from an earlier run would stop the child at once.
        match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);
    // Through the launcher, not inherited: an inherited log handle would keep pointing at the
    // file the log rotation moved to `axidps.log.1` (and, on Windows, keep it open).
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: the hook only calls async-signal-safe functions (prctl, getppid).
        // PDEATHSIG fires when the *thread* that spawned the child exits, so `tick` must be
        // called from the thread that lives as long as the launcher (the run loop's).
        let parent = unsafe { libc::getpid() };
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // The launcher may have died between fork and prctl.
                if libc::getppid() != parent {
                    return Err(std::io::Error::other("launcher already gone"));
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000; // public-check:ok
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    if let Some(out) = child.stdout.take() {
        forward(spec.name, out);
    }
    if let Some(err) = child.stderr.take() {
        forward(spec.name, err);
    }
    Ok(child)
}

/// Copies a child's output, line by line, to the launcher's own stderr, which is its current
/// log (or its terminal). Ends when the child closes the stream (it exited).
fn forward(name: &str, stream: impl Read + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name(format!("{name}-output"))
        .spawn(move || copy_lines(BufReader::new(stream), &mut || io::stderr().lock()));
    if let Err(e) = spawned {
        eprintln!("axidps: cannot forward {name}'s output: {e}");
    }
}

/// Copies whole lines from `from` to the writer `to` returns, fetched per line so a writer that
/// changes (the rotated log) is picked up. Write errors drop the line, never the copying.
fn copy_lines<W: Write>(mut from: impl BufRead, to: &mut dyn FnMut() -> W) {
    let mut line = Vec::new();
    loop {
        line.clear();
        match from.read_until(b'\n', &mut line) {
            Ok(0) => return,
            Ok(_) => {
                let mut w = to();
                let _ = w.write_all(&line);
                let _ = w.flush();
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

#[cfg(windows)]
mod job {
    use std::io;
    use std::os::windows::io::AsRawHandle;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows::core::PCWSTR;

    /// A job object that kills its processes when its last handle closes.
    pub struct Job(HANDLE);

    impl Job {
        pub fn new() -> io::Result<Job> {
            // SAFETY: plain Win32 calls; the info struct outlives the call and its size is passed.
            unsafe {
                let handle = CreateJobObjectW(None, PCWSTR::null()).map_err(io::Error::other)?;
                let job = Job(handle);
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .map_err(io::Error::other)?;
                Ok(job)
            }
        }

        pub fn assign(&self, process: &std::process::Child) -> io::Result<()> {
            // SAFETY: the process handle is valid while `process` lives.
            unsafe {
                AssignProcessToJobObject(self.0, HANDLE(process.as_raw_handle() as _))
                    .map_err(io::Error::other)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and closed once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_caps_and_resets() {
        let mut b = Backoff::new();
        let secs: Vec<u64> = (0..7)
            .map(|_| b.next_delay(Duration::ZERO).as_secs())
            .collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(
            b.next_delay(Duration::from_secs(60)),
            Duration::from_secs(1)
        );
        assert_eq!(b.next_delay(Duration::ZERO), Duration::from_secs(2));
    }

    #[cfg(unix)]
    fn child(name: &'static str, script: &str) -> Child {
        Child {
            name,
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            stop_file: None,
        }
    }

    #[cfg(unix)]
    fn graceful(script: &str, stop: &Path) -> Child {
        Child {
            stop_file: Some(stop.to_path_buf()),
            ..child("graceful", script)
        }
    }

    #[cfg(unix)]
    fn start(sup: &mut Supervisor) {
        sup.tick(Instant::now());
        assert!(sup.is_running("graceful"));
        // Let the shell install its trap.
        std::thread::sleep(Duration::from_millis(200));
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_lets_a_child_finish_on_sigterm() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let script = format!(
            "trap 'echo flushed > {}; exit 0' TERM; while :; do sleep 0.05; done",
            out.display()
        );
        let mut sup = Supervisor::new(vec![graceful(&script, &tmp.path().join("stop"))]);
        start(&mut sup);
        sup.shutdown_within(Duration::from_secs(5));
        assert!(!sup.is_running("graceful"));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "flushed\n");
        assert!(!tmp.path().join("stop").exists(), "stop file left behind");
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_stops_a_child_through_its_stop_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (out, stop) = (tmp.path().join("out"), tmp.path().join("stop"));
        // Ignores SIGTERM, as core on Windows gets only the file.
        let script = format!(
            "trap '' TERM; while [ ! -e {} ]; do sleep 0.05; done; echo flushed > {}",
            stop.display(),
            out.display()
        );
        let mut sup = Supervisor::new(vec![graceful(&script, &stop)]);
        start(&mut sup);
        sup.shutdown_within(Duration::from_secs(5));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "flushed\n");
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_kills_a_child_that_does_not_stop_in_time() {
        let tmp = tempfile::tempdir().unwrap();
        let mut sup = Supervisor::new(vec![graceful(
            "trap '' TERM; exec sleep 30",
            &tmp.path().join("stop"),
        )]);
        start(&mut sup);
        let t0 = Instant::now();
        sup.shutdown_within(Duration::from_millis(300));
        assert!(!sup.is_running("graceful"));
        assert!(t0.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn stale_stop_file_is_removed_before_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        let stop = tmp.path().join("stop");
        std::fs::write(&stop, b"").unwrap();
        let mut sup = Supervisor::new(vec![graceful("sleep 30", &stop)]);
        sup.tick(Instant::now());
        assert!(!stop.exists());
        sup.shutdown_within(Duration::from_millis(300));
    }

    #[test]
    fn copy_lines_fetches_the_writer_per_line() {
        use std::cell::RefCell;
        use std::rc::Rc;
        struct Sink(Rc<RefCell<Vec<Vec<u8>>>>);
        impl Write for Sink {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.0.borrow_mut().last_mut().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        // Each fetch is a new "log file": every line lands whole in the one current then.
        let files = Rc::new(RefCell::new(Vec::new()));
        let input: &[u8] = b"one\ntwo\nthree";
        copy_lines(input, &mut || {
            files.borrow_mut().push(Vec::new());
            Sink(Rc::clone(&files))
        });
        assert_eq!(*files.borrow(), [&b"one\n"[..], b"two\n", b"three"]);
    }

    #[cfg(unix)]
    #[test]
    fn child_output_goes_through_the_launcher() {
        // The child's stdout is a pipe the launcher reads, not an inherited file handle.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("fd1");
        let mut sup = Supervisor::new(vec![child(
            "talk",
            &format!(
                // Read both links before any redirect: dash applies `> file` to the shell's
                // own fd 1 while running a command, so `readlink … > file` would see the file.
                "a=$(readlink /proc/$$/fd/1); b=$(readlink /proc/$$/fd/2); \
                 printf '%s\\n%s\\n' \"$a\" \"$b\" > {0}; sleep 5",
                out.display()
            ),
        )]);
        sup.tick(Instant::now());
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(&out).map_or(true, |t| t.lines().count() < 2) {
            assert!(Instant::now() < deadline, "child never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
        let fds = std::fs::read_to_string(&out).unwrap();
        assert!(fds.lines().all(|l| l.starts_with("pipe:")), "{fds}");
        sup.shutdown_within(Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn set_args_restarts_with_the_new_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("args");
        let stop = dir.path().join("stop");
        // Writes its argument, then runs until the stop file appears.
        let script = format!(
            "echo \"$1\" > {}; while [ ! -e {} ]; do sleep 0.05; done",
            out.display(),
            stop.display()
        );
        let args = |v: &str| -> Vec<OsString> {
            vec!["-c".into(), script.clone().into(), "sh".into(), v.into()]
        };
        let mut sup = Supervisor::new(vec![Child {
            name: "c",
            program: "sh".into(),
            args: args("one"),
            stop_file: Some(stop.clone()),
        }]);
        let wait_for = |sup: &mut Supervisor, want: &str| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                sup.tick(Instant::now());
                if std::fs::read_to_string(&out).is_ok_and(|t| t.trim() == want) {
                    return;
                }
                assert!(Instant::now() < deadline, "never saw {want}");
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        wait_for(&mut sup, "one");
        assert!(!sup.set_args("c", args("one"), true));
        assert!(sup.set_args("c", args("two"), true));
        wait_for(&mut sup, "two");
        assert_eq!(sup.spawn_count("c"), 2);
        // Set while a restart is not allowed (the game runs): kept, not lost, and applied by
        // the next call that allows it even though the arguments did not change again.
        assert!(!sup.set_args("c", args("three"), false));
        assert!(!sup.set_args("c", args("three"), false));
        assert_eq!(sup.spawn_count("c"), 2);
        assert!(sup.set_args("c", args("three"), true));
        // Asked once; a repeat while it is still stopping does not ask again.
        assert!(!sup.set_args("c", args("three"), true));
        wait_for(&mut sup, "three");
        assert_eq!(sup.spawn_count("c"), 3);
        sup.shutdown_within(Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn supervisor_restarts_exited_child() {
        let mut sup = Supervisor::new(vec![child("quick", "exit 0")]);
        let t0 = Instant::now();
        sup.tick(t0);
        assert_eq!(sup.spawn_count("quick"), 1);
        // Wait for the exit to be noticed (the tick schedules the restart).
        let deadline = Instant::now() + Duration::from_secs(5);
        while sup.is_running("quick") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            sup.tick(t0);
        }
        assert!(!sup.is_running("quick"), "child never exited");
        assert_eq!(sup.spawn_count("quick"), 1, "backoff not elapsed yet");
        sup.tick(t0 + Duration::from_secs(10));
        assert_eq!(sup.spawn_count("quick"), 2);
        sup.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_kills_children() {
        let mut sup = Supervisor::new(vec![child("long", "sleep 30")]);
        sup.tick(Instant::now());
        assert!(sup.is_running("long"));
        sup.shutdown();
        assert!(!sup.is_running("long"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn child_dies_with_the_spawning_thread() {
        let pid = std::thread::spawn(|| {
            let mut sup = Supervisor::new(vec![child("orphan", "sleep 30")]);
            sup.tick(Instant::now());
            sup.pid("orphan").unwrap()
            // The thread ends without shutdown(), as when the launcher is killed.
        })
        .join()
        .unwrap();
        let gone = || {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
                stat.rsplit(") ").next().is_some_and(|r| r.starts_with('Z'))
            })
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gone() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(gone(), "child {pid} outlived its spawner");
    }
}
