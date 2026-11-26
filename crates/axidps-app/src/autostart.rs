//! How the launcher gets started. It runs while Guild Wars 2 runs: the game starts it, and it
//! quits after the game closes (see `lifetime`).
//!
//! - Linux: the DLL keeps a marker file in `/dev/shm` while the game runs. A systemd user path
//!   unit ([`PATH_UNIT`]) starts [`SERVICE_UNIT`], which runs the AppImage, when the marker
//!   appears. Without `systemctl --user` the launcher falls back to an XDG autostart entry (start
//!   at login) and stays running.
//! - Windows: the launcher writes its exe path to [`LAUNCHER_PATH_FILE`] in the config folder;
//!   the DLL starts it from there when the game starts.
//!
//! [`LAUNCHER_PATH_FILE`]: axidps_protocol::paths::LAUNCHER_PATH_FILE

use std::io;
use std::path::{Path, PathBuf};

/// Whether this launcher quits after the game closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// The game starts the launcher, so it quits once the game has closed.
    WithGame,
    /// Started at login (or nothing starts it): it stays running.
    Always,
}

/// The systemd path unit that starts [`SERVICE_UNIT`] when the game marker appears.
#[cfg_attr(windows, allow(dead_code))]
pub const PATH_UNIT: &str = "axidps-launch.path";
/// The systemd service that runs the launcher.
pub const SERVICE_UNIT: &str = "axidps-launch.service";
/// The flag [`SERVICE_UNIT`] starts the launcher with.
pub const FROM_GAME_FLAG: &str = "--from-game";

/// Runs `systemctl --user <args>`: `Ok(true)` when it exited 0, `Err` when it cannot run.
#[cfg_attr(windows, allow(dead_code))]
pub type Systemctl<'a> = &'a dyn Fn(&[&str]) -> io::Result<bool>;

/// The text of the autostart entry that starts `exec`.
#[cfg_attr(windows, allow(dead_code))]
pub fn desktop_entry(exec: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=AxiDPS\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        exec.display()
    )
}

/// The text of [`PATH_UNIT`], watching `marker`.
#[cfg_attr(windows, allow(dead_code))]
pub fn path_unit(marker: &Path) -> String {
    format!(
        "[Unit]\nDescription=Start AxiDPS when Guild Wars 2 starts\n\n[Path]\nPathExists={}\nUnit={SERVICE_UNIT}\n\n[Install]\nWantedBy=default.target\n",
        marker.display()
    )
}

/// The text of [`SERVICE_UNIT`], running `exec`. `KillMode=process`: a Linux update starts the
/// new AppImage and exits, and the new one must outlive it. The launcher's own children die
/// with it anyway (they get a parent-death signal).
#[cfg_attr(windows, allow(dead_code))]
pub fn service_unit(exec: &Path) -> String {
    format!(
        "[Unit]\nDescription=AxiDPS while Guild Wars 2 runs\n\n[Service]\nType=simple\nExecStart={} {FROM_GAME_FLAG}\nKillMode=process\n",
        exec_quote(exec)
    )
}

/// `path` as one quoted `ExecStart=` word: `\` and `"` escaped, `%` (specifiers) and `$`
/// (variables) doubled.
#[cfg_attr(windows, allow(dead_code))]
fn exec_quote(path: &Path) -> String {
    let p = path.to_string_lossy();
    let mut out = String::with_capacity(p.len() + 2);
    out.push('"');
    for c in p.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `$XDG_CONFIG_HOME`, else `$HOME/.config`.
#[cfg(unix)]
fn config_home() -> Option<PathBuf> {
    let non_empty = |v: Option<std::ffi::OsString>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    non_empty(std::env::var_os("XDG_CONFIG_HOME"))
        .or_else(|| non_empty(std::env::var_os("HOME")).map(|h| h.join(".config")))
}

#[cfg(unix)]
fn entry_path(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join("axidps.desktop")
}

#[cfg(unix)]
fn units_dir(config_home: &Path) -> PathBuf {
    config_home.join("systemd").join("user")
}

/// Writes `text` to `path`, creating the folder, unless the file already says exactly that.
/// The file appears whole or not at all. Returns whether it was written.
fn write_if_changed(path: &Path, text: &str) -> io::Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|have| have == text) {
        return Ok(false);
    }
    let dir = path.parent().expect("file path has a parent");
    std::fs::create_dir_all(dir)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
        .map(|()| true)
}

/// Deletes `path`; a missing one is fine. Returns whether there was one.
fn remove_file(path: &Path) -> io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Writes the entry for `exec` under `config_home`, unless it is already right.
#[cfg(unix)]
fn install_in(config_home: &Path, exec: &Path) -> io::Result<()> {
    write_if_changed(&entry_path(config_home), &desktop_entry(exec)).map(drop)
}

/// Deletes the entry under `config_home`; a missing one is fine.
#[cfg(unix)]
fn remove_in(config_home: &Path) -> io::Result<()> {
    remove_file(&entry_path(config_home)).map(drop)
}

/// Writes both units under `config_home` for `exec` and `marker`. Returns whether either
/// changed.
#[cfg(unix)]
fn write_units_in(config_home: &Path, exec: &Path, marker: &Path) -> io::Result<bool> {
    let dir = units_dir(config_home);
    let service = write_if_changed(&dir.join(SERVICE_UNIT), &service_unit(exec))?;
    let path = write_if_changed(&dir.join(PATH_UNIT), &path_unit(marker))?;
    Ok(service || path)
}

/// Deletes both units under `config_home`. Returns whether there were any.
#[cfg(unix)]
fn remove_units_in(config_home: &Path) -> io::Result<bool> {
    let dir = units_dir(config_home);
    let service = remove_file(&dir.join(SERVICE_UNIT))?;
    let path = remove_file(&dir.join(PATH_UNIT))?;
    Ok(service || path)
}

/// `systemctl --user <args>` for real, quietly.
#[cfg(unix)]
fn systemctl(args: &[&str]) -> io::Result<bool> {
    use std::process::{Command, Stdio};
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
}

/// Whether `systemctl` ran and succeeded.
#[cfg(unix)]
fn ok(sc: Systemctl, args: &[&str]) -> bool {
    sc(args).unwrap_or(false)
}

/// Keeps the launcher's start right while it runs: called once at start and then every check.
/// The first call decides the [`Lifetime`] for the whole process.
#[derive(Default)]
pub struct Starter {
    decided: Option<Lifetime>,
    /// The path unit was enabled by this process.
    #[cfg_attr(windows, allow(dead_code))]
    enabled: bool,
    logged: bool,
}

impl Starter {
    pub fn new() -> Starter {
        Starter::default()
    }

    /// Logs `msg` once per process (until a call succeeds again).
    fn log_once(&mut self, msg: impl FnOnce() -> String) {
        if !self.logged {
            self.logged = true;
            eprintln!("axidps: {}", msg());
        }
    }

    /// Writes the units (or the Windows launcher path) for the running launcher, and returns
    /// whether it quits with the game.
    pub fn keep(&mut self) -> Lifetime {
        #[cfg(unix)]
        {
            let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
            let marker =
                axidps_protocol::paths::game_marker_path(axidps_protocol::v2::RingHost::Unix);
            self.keep_in(
                config_home().as_deref(),
                appimage.as_deref(),
                &marker,
                &systemctl,
            )
        }
        #[cfg(windows)]
        {
            let dir = axidps_protocol::paths::config_dir();
            let exe = std::env::current_exe().ok();
            self.keep_windows(dir.as_deref(), exe.as_deref())
        }
    }

    /// [`Starter::keep`] on Linux with its inputs passed in. `exec` is the AppImage; without one
    /// (a development build) nothing is written and the launcher stays running.
    #[cfg(unix)]
    fn keep_in(
        &mut self,
        config_home: Option<&Path>,
        exec: Option<&Path>,
        marker: &Path,
        sc: Systemctl,
    ) -> Lifetime {
        let (Some(home), Some(exec)) = (config_home, exec) else {
            if config_home.is_none() {
                self.log_once(|| {
                    "no config folder (XDG_CONFIG_HOME and HOME are unset); nothing starts axidps"
                        .into()
                });
            }
            return *self.decided.insert(Lifetime::Always);
        };
        if self.decided == Some(Lifetime::Always) {
            match install_in(home, exec) {
                Ok(()) => self.logged = false,
                Err(e) => self.log_once(|| format!("cannot write the autostart entry: {e}")),
            }
            return Lifetime::Always;
        }
        let started = match write_units_in(home, exec, marker) {
            Ok(changed) if changed || !self.enabled => {
                ok(sc, &["daemon-reload"]) && ok(sc, &["enable", "--now", PATH_UNIT])
            }
            Ok(_) => true,
            Err(e) => {
                self.log_once(|| format!("cannot write the systemd units: {e}"));
                false
            }
        };
        match (started, self.decided) {
            (true, _) => {
                self.enabled = true;
                self.logged = false;
                if let Err(e) = remove_in(home) {
                    eprintln!("axidps: cannot remove the autostart entry: {e}");
                }
                *self.decided.insert(Lifetime::WithGame)
            }
            // Worked before in this process (an update moved the AppImage): keep trying.
            (false, Some(lifetime)) => {
                self.log_once(|| "cannot update the systemd units; retrying".into());
                self.enabled = false;
                lifetime
            }
            (false, None) => {
                eprintln!(
                    "axidps: systemctl --user is not available; starting at login instead of with the game"
                );
                let _ = remove_units_in(home);
                self.decided = Some(Lifetime::Always);
                self.keep_in(config_home, Some(exec), marker, sc)
            }
        }
    }

    /// [`Starter::keep`] on Windows with its inputs passed in: writes `exe` to the launcher
    /// path file in `config_dir`, unless it already says that.
    #[cfg_attr(unix, allow(dead_code))]
    fn keep_windows(&mut self, config_dir: Option<&Path>, exe: Option<&Path>) -> Lifetime {
        let written = match (config_dir, exe) {
            (Some(dir), Some(exe)) => write_if_changed(
                &dir.join(axidps_protocol::paths::LAUNCHER_PATH_FILE),
                &format!("{}\n", exe.display()),
            )
            .map(drop),
            _ => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no config folder or exe path",
            )),
        };
        match written {
            Ok(()) => {
                self.logged = false;
                *self.decided.get_or_insert(Lifetime::WithGame)
            }
            Err(e) => {
                self.log_once(|| format!("cannot write the launcher path for the game: {e}"));
                *self.decided.get_or_insert(Lifetime::Always)
            }
        }
    }
}

/// Stops anything starting the launcher: the units and the autostart entry (Linux), the
/// launcher path file (Windows).
pub fn remove() -> io::Result<()> {
    #[cfg(unix)]
    {
        match config_home() {
            Some(home) => remove_all_in(&home, &systemctl),
            None => Ok(()),
        }
    }
    #[cfg(windows)]
    {
        match axidps_protocol::paths::config_dir() {
            Some(dir) => {
                remove_file(&dir.join(axidps_protocol::paths::LAUNCHER_PATH_FILE)).map(drop)
            }
            None => Ok(()),
        }
    }
}

/// [`remove`] on Linux: the path unit is disabled and stopped first, so it cannot start the
/// launcher again while the files go.
#[cfg(unix)]
fn remove_all_in(home: &Path, sc: Systemctl) -> io::Result<()> {
    if units_dir(home).join(PATH_UNIT).exists() {
        let _ = sc(&["disable", "--now", PATH_UNIT]);
    }
    let removed = remove_units_in(home);
    if removed.as_ref().is_ok_and(|r| *r) {
        let _ = sc(&["daemon-reload"]);
    }
    let entry = remove_in(home);
    removed.and(entry)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const EXEC: &str = "/home/u/AppImages/axidps-0.2.0.AppImage";
    const MARKER: &str = "/dev/shm/axidps-game";

    /// A fake `systemctl` that records its calls and answers with `answer`.
    struct Fake {
        calls: RefCell<Vec<String>>,
        answer: fn(&[&str]) -> io::Result<bool>,
    }

    impl Fake {
        fn new(answer: fn(&[&str]) -> io::Result<bool>) -> Fake {
            Fake {
                calls: RefCell::new(Vec::new()),
                answer,
            }
        }
        fn run(&self, args: &[&str]) -> io::Result<bool> {
            self.calls.borrow_mut().push(args.join(" "));
            (self.answer)(args)
        }
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.calls.borrow_mut())
        }
    }

    fn works(_: &[&str]) -> io::Result<bool> {
        Ok(true)
    }
    fn missing(_: &[&str]) -> io::Result<bool> {
        Err(io::Error::new(io::ErrorKind::NotFound, "no systemctl"))
    }
    fn no_bus(_: &[&str]) -> io::Result<bool> {
        Ok(false)
    }

    fn keep(s: &mut Starter, home: &Path, exec: &str, fake: &Fake) -> Lifetime {
        s.keep_in(Some(home), Some(Path::new(exec)), Path::new(MARKER), &|a| {
            fake.run(a)
        })
    }

    #[test]
    fn desktop_entry_text() {
        assert_eq!(
            desktop_entry(Path::new(EXEC)),
            "[Desktop Entry]\nType=Application\nName=AxiDPS\nExec=\"/home/u/AppImages/axidps-0.2.0.AppImage\"\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n"
        );
    }

    #[test]
    fn path_unit_text() {
        assert_eq!(
            path_unit(Path::new(MARKER)),
            "[Unit]\nDescription=Start AxiDPS when Guild Wars 2 starts\n\n[Path]\nPathExists=/dev/shm/axidps-game\nUnit=axidps-launch.service\n\n[Install]\nWantedBy=default.target\n"
        );
    }

    #[test]
    fn service_unit_text() {
        assert_eq!(
            service_unit(Path::new(EXEC)),
            "[Unit]\nDescription=AxiDPS while Guild Wars 2 runs\n\n[Service]\nType=simple\nExecStart=\"/home/u/AppImages/axidps-0.2.0.AppImage\" --from-game\nKillMode=process\n"
        );
    }

    #[test]
    fn unit_names_do_not_clash_with_dev_units() {
        for name in [PATH_UNIT, SERVICE_UNIT] {
            assert!(name.starts_with("axidps-launch."));
        }
    }

    #[test]
    fn exec_path_is_quoted_for_systemd() {
        assert_eq!(
            exec_quote(Path::new(r#"/home/u/My Apps/a"b\c%d$e.AppImage"#)),
            r#""/home/u/My Apps/a\"b\\c%%d$$e.AppImage""#
        );
    }

    #[test]
    fn autostart_install_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        // The autostart folder does not exist yet.
        install_in(tmp.path(), Path::new(EXEC)).unwrap();
        let file = tmp.path().join("autostart/axidps.desktop");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            desktop_entry(Path::new(EXEC))
        );
        assert!(!tmp.path().join("autostart/axidps.desktop.tmp").exists());
        remove_in(tmp.path()).unwrap();
        assert!(!file.exists());
        // Removing again is fine.
        remove_in(tmp.path()).unwrap();
    }

    #[test]
    fn install_rewrites_only_a_wrong_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("autostart/axidps.desktop");
        install_in(tmp.path(), Path::new("/old/axidps.AppImage")).unwrap();
        install_in(tmp.path(), Path::new(EXEC)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            desktop_entry(Path::new(EXEC))
        );
        // Right already: left untouched, even if someone else's mtime matters.
        std::fs::write(&file, desktop_entry(Path::new(EXEC))).unwrap();
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        install_in(tmp.path(), Path::new(EXEC)).unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn units_are_written_enabled_and_replace_the_autostart_entry() {
        let tmp = tempfile::tempdir().unwrap();
        // An older launcher's login entry.
        install_in(tmp.path(), Path::new(EXEC)).unwrap();
        let fake = Fake::new(works);
        let mut s = Starter::new();
        assert_eq!(keep(&mut s, tmp.path(), EXEC, &fake), Lifetime::WithGame);
        let dir = tmp.path().join("systemd/user");
        assert_eq!(
            std::fs::read_to_string(dir.join(SERVICE_UNIT)).unwrap(),
            service_unit(Path::new(EXEC))
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(PATH_UNIT)).unwrap(),
            path_unit(Path::new(MARKER))
        );
        assert_eq!(
            fake.take(),
            ["daemon-reload", "enable --now axidps-launch.path"]
        );
        assert!(!tmp.path().join("autostart/axidps.desktop").exists());
        // Nothing changed: systemctl is not run again.
        assert_eq!(keep(&mut s, tmp.path(), EXEC, &fake), Lifetime::WithGame);
        assert!(fake.take().is_empty());
    }

    #[test]
    fn units_follow_the_appimage_after_an_update() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        let mut s = Starter::new();
        keep(&mut s, tmp.path(), "/a/axidps-0.1.0.AppImage", &fake);
        fake.take();
        assert_eq!(keep(&mut s, tmp.path(), EXEC, &fake), Lifetime::WithGame);
        assert_eq!(
            fake.take(),
            ["daemon-reload", "enable --now axidps-launch.path"]
        );
        let text = std::fs::read_to_string(tmp.path().join("systemd/user").join(SERVICE_UNIT));
        assert_eq!(text.unwrap(), service_unit(Path::new(EXEC)));
    }

    #[test]
    fn a_new_process_enables_the_units_again() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        keep(&mut Starter::new(), tmp.path(), EXEC, &fake);
        fake.take();
        // Units already right, but someone may have disabled them.
        keep(&mut Starter::new(), tmp.path(), EXEC, &fake);
        assert_eq!(
            fake.take(),
            ["daemon-reload", "enable --now axidps-launch.path"]
        );
    }

    #[test]
    fn without_systemctl_it_starts_at_login_and_stays_running() {
        for answer in [missing as fn(&[&str]) -> io::Result<bool>, no_bus] {
            let tmp = tempfile::tempdir().unwrap();
            let fake = Fake::new(answer);
            let mut s = Starter::new();
            assert_eq!(keep(&mut s, tmp.path(), EXEC, &fake), Lifetime::Always);
            assert_eq!(
                std::fs::read_to_string(tmp.path().join("autostart/axidps.desktop")).unwrap(),
                desktop_entry(Path::new(EXEC))
            );
            assert!(!tmp.path().join("systemd/user").join(SERVICE_UNIT).exists());
            assert!(!tmp.path().join("systemd/user").join(PATH_UNIT).exists());
            // Decided for this process: no more systemctl, the entry follows the AppImage.
            fake.take();
            assert_eq!(
                keep(&mut s, tmp.path(), "/b/axidps.AppImage", &fake),
                Lifetime::Always
            );
            assert!(fake.take().is_empty());
            assert_eq!(
                std::fs::read_to_string(tmp.path().join("autostart/axidps.desktop")).unwrap(),
                desktop_entry(Path::new("/b/axidps.AppImage"))
            );
        }
    }

    #[test]
    fn a_failure_after_success_keeps_quitting_with_the_game() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Starter::new();
        keep(&mut s, tmp.path(), EXEC, &Fake::new(works));
        let broken = Fake::new(no_bus);
        assert_eq!(
            keep(&mut s, tmp.path(), "/b/axidps.AppImage", &broken),
            Lifetime::WithGame
        );
        assert!(!tmp.path().join("autostart/axidps.desktop").exists());
        // Retried on the next check, since enabling did not finish.
        broken.take();
        keep(&mut s, tmp.path(), "/b/axidps.AppImage", &broken);
        assert_eq!(broken.take(), ["daemon-reload"]);
    }

    #[test]
    fn without_an_appimage_nothing_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        let mut s = Starter::new();
        let got = s.keep_in(Some(tmp.path()), None, Path::new(MARKER), &|a| fake.run(a));
        assert_eq!(got, Lifetime::Always);
        assert!(fake.take().is_empty());
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn remove_disables_then_deletes_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        keep(&mut Starter::new(), tmp.path(), EXEC, &fake);
        install_in(tmp.path(), Path::new(EXEC)).unwrap();
        fake.take();
        remove_all_in(tmp.path(), &|a| fake.run(a)).unwrap();
        assert_eq!(
            fake.take(),
            ["disable --now axidps-launch.path", "daemon-reload"]
        );
        assert!(!tmp.path().join("systemd/user").join(PATH_UNIT).exists());
        assert!(!tmp.path().join("systemd/user").join(SERVICE_UNIT).exists());
        assert!(!tmp.path().join("autostart/axidps.desktop").exists());
        // Nothing left: systemctl is not run.
        remove_all_in(tmp.path(), &|a| fake.run(a)).unwrap();
        assert!(fake.take().is_empty());
    }

    #[test]
    fn windows_launcher_path_is_written_once() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = Path::new(r"C:\Users\u\AppData\Local\Programs\axidps\axidps.exe");
        let mut s = Starter::new();
        assert_eq!(
            s.keep_windows(Some(tmp.path()), Some(exe)),
            Lifetime::WithGame
        );
        let file = tmp.path().join(axidps_protocol::paths::LAUNCHER_PATH_FILE);
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            axidps_protocol::paths::parse_launcher_path(&text).as_deref(),
            Some(exe)
        );
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        s.keep_windows(Some(tmp.path()), Some(exe));
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before
        );
        // No config folder: nothing can start it with the game.
        assert_eq!(
            Starter::new().keep_windows(None, Some(exe)),
            Lifetime::Always
        );
    }
}
