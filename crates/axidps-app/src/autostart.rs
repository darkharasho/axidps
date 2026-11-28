//! How the launcher gets started. With a DLL in the game folder that can tell it the game runs
//! (`CAP_STARTS_LAUNCHER`), the game starts it and it quits after the game closes (see
//! `lifetime`). Otherwise it starts at login and stays running, as before.
//!
//! - Linux: the DLL keeps a marker file in `/dev/shm` while the game runs. A systemd user path
//!   unit ([`PATH_UNIT`]) starts [`SERVICE_UNIT`] when the marker appears; the service runs a
//!   symlink in the config folder ([`APPIMAGE_LINK`]) that the launcher points at the AppImage on
//!   every run. Without `systemctl --user`, or before the game folder has such a DLL, the
//!   launcher keeps an XDG autostart entry instead.
//! - Windows: the launcher writes its exe path to [`LAUNCHER_PATH_FILE`] in the config folder;
//!   the DLL starts it from there when the game starts. Before the game folder has such a DLL,
//!   the launcher keeps its `HKCU\...\Run` value (which the installer writes).
//!
//! [`LAUNCHER_PATH_FILE`]: axidps_protocol::paths::LAUNCHER_PATH_FILE
//! `CAP_STARTS_LAUNCHER`: axidps_protocol::ident::CAP_STARTS_LAUNCHER

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
/// The symlink in the config folder (`<config home>/axidps/`) that [`SERVICE_UNIT`] runs. The
/// launcher points it at the running AppImage, so the unit text never changes with an update.
#[cfg_attr(windows, allow(dead_code))]
pub const APPIMAGE_LINK: &str = "axidps.AppImage";
/// What the launcher brings from a graphical session into the systemd user manager, so the
/// launcher the service starts can open the overlay window. Imported only when the session has
/// a display (`DISPLAY` or `WAYLAND_DISPLAY`).
#[cfg_attr(windows, allow(dead_code))]
/// Not `XDG_RUNTIME_DIR` or `DBUS_SESSION_BUS_ADDRESS`: the user manager has its own, and a
/// launcher started from ssh or a sandbox could replace them for every user service.
pub const SESSION_ENV: [&str; 3] = ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY"];

/// Runs `systemctl --user <args>`: `Ok(true)` when it exited 0, `Err` when it cannot run.
#[cfg_attr(windows, allow(dead_code))]
pub type Systemctl<'a> = &'a dyn Fn(&[&str]) -> io::Result<bool>;

/// The text of the autostart entry that starts `exec`.
#[cfg_attr(windows, allow(dead_code))]
pub fn desktop_entry(exec: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=AxiDPS\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        desktop_exec_escape(exec)
    )
}

/// `path` as one quoted Exec argument, without its quotes: per the Desktop Entry spec, `"`,
/// `` ` ``, `$` and `\` take a backslash inside quotes, and the string rule then escapes every
/// backslash again; `%` is doubled, and control characters use the string escapes.
fn desktop_exec_escape(path: &Path) -> String {
    let p = path.to_string_lossy();
    let mut out = String::with_capacity(p.len());
    for c in p.chars() {
        match c {
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '\\' => out.push_str("\\\\\\\\"),
            '%' => out.push_str("%%"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// The text of [`PATH_UNIT`], watching `marker`.
#[cfg_attr(windows, allow(dead_code))]
pub fn path_unit(marker: &Path) -> String {
    format!(
        "[Unit]\nDescription=Start AxiDPS when Guild Wars 2 starts\n\n[Path]\nPathExists={}\nUnit={SERVICE_UNIT}\n\n[Install]\nWantedBy=default.target\n",
        marker.display()
    )
}

/// The text of [`SERVICE_UNIT`], running `link` (the [`APPIMAGE_LINK`]) while `marker` exists.
///
/// Through `sh`: when the AppImage behind `link` is gone (deleted, or moved by hand), the service
/// must not fail. A failing start would be retried at once (the marker still exists) until the
/// start limit fails the path unit for good. So `sh` logs the problem and waits for the game to
/// close; the next start of the launcher from AxiOM points the link at the AppImage again. With
/// the AppImage there, `sh` is replaced by it (`exec`), so the launcher is the main process.
///
/// `KillMode=process`: a Linux update starts the new AppImage and exits, and the new one must
/// outlive it. The launcher's own children die with it anyway (they get a parent-death signal).
#[cfg_attr(windows, allow(dead_code))]
pub fn service_unit(link: &Path, marker: &Path) -> String {
    format!(
        "[Unit]\nDescription=AxiDPS while Guild Wars 2 runs\n\n[Service]\nType=simple\nExecStart=/bin/sh -c '[ -x \"$$0\" ] && exec \"$$0\" {FROM_GAME_FLAG}; echo \"axidps: $$0 is missing; start AxiDPS once from AxiOM\" >&2; while [ -e {} ]; do sleep 5; done' {}\nKillMode=process\n",
        marker.display(),
        exec_quote(link)
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

#[cfg(unix)]
fn link_path(config_home: &Path) -> PathBuf {
    config_home.join("axidps").join(APPIMAGE_LINK)
}

/// Writes `text` to `path`, creating the folder, unless the file already says exactly that.
/// The file appears whole or not at all. Returns whether it was written.
fn write_if_changed(path: &Path, text: &str) -> io::Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|have| have == text) {
        return Ok(false);
    }
    let dir = path.parent().expect("file path has a parent");
    std::fs::create_dir_all(dir)?;
    let tmp = with_suffix(path, ".tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
        .map(|()| true)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(suffix);
    PathBuf::from(p)
}

/// Deletes `path`; a missing one is fine. Returns whether there was one.
fn remove_file(path: &Path) -> io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Points the symlink `link` at `target`, unless it already does. Replaced atomically.
#[cfg(unix)]
fn point_link(link: &Path, target: &Path) -> io::Result<()> {
    if std::fs::read_link(link).is_ok_and(|have| have == target) {
        return Ok(());
    }
    std::fs::create_dir_all(link.parent().expect("link path has a parent"))?;
    let tmp = with_suffix(link, ".tmp");
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)?;
    std::fs::rename(&tmp, link).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
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

/// Points the link at `exec` and writes both units under `config_home`. Returns whether either
/// unit changed.
#[cfg(unix)]
fn write_units_in(config_home: &Path, exec: &Path, marker: &Path) -> io::Result<bool> {
    let link = link_path(config_home);
    point_link(&link, exec)?;
    let dir = units_dir(config_home);
    let service = write_if_changed(&dir.join(SERVICE_UNIT), &service_unit(&link, marker))?;
    let path = write_if_changed(&dir.join(PATH_UNIT), &path_unit(marker))?;
    Ok(service || path)
}

/// Deletes both units and the link under `config_home`. Returns whether there were any units.
#[cfg(unix)]
fn remove_units_in(config_home: &Path) -> io::Result<bool> {
    let dir = units_dir(config_home);
    let service = remove_file(&dir.join(SERVICE_UNIT))?;
    let path = remove_file(&dir.join(PATH_UNIT))?;
    remove_file(&link_path(config_home))?;
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

/// The [`SESSION_ENV`] names set (non-empty) in this process, per `get`; empty when neither
/// display variable is set (no graphical session to bring along).
#[cfg_attr(windows, allow(dead_code))]
pub fn session_env(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<&'static str> {
    let set: Vec<&'static str> = SESSION_ENV
        .into_iter()
        .filter(|n| get(n).is_some_and(|v| !v.is_empty()))
        .collect();
    if set.contains(&"DISPLAY") || set.contains(&"WAYLAND_DISPLAY") {
        set
    } else {
        Vec::new()
    }
}

/// What [`Starter::keep_in`] works from on Linux.
#[cfg(unix)]
struct LinuxIn<'a> {
    config_home: Option<&'a Path>,
    /// The running AppImage.
    exec: Option<&'a Path>,
    marker: &'a Path,
    /// The game folder's DLL tells the launcher the game runs.
    dll_starts_launcher: bool,
    /// [`session_env`] of this process.
    session: &'a [&'a str],
}

/// The launcher's login `Run` value on Windows (`HKCU\...\Run`, value `axidps`).
#[cfg_attr(unix, allow(dead_code))]
pub trait RunValue {
    /// The value's text, `None` when it is not there.
    fn get(&self) -> Option<String>;
    /// Writes `Some(text)` or deletes (`None`) the value.
    fn set(&self, value: Option<&str>) -> io::Result<()>;
}

/// The `Run` value text that starts `exe`, as the installer writes it.
#[cfg_attr(unix, allow(dead_code))]
pub fn run_value_for(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Keeps the launcher's start right while it runs: called once at start and then every check.
/// Once [`Lifetime::WithGame`], a process stays so; [`Lifetime::Always`] becomes `WithGame` as
/// soon as the game folder gets a DLL that starts the launcher.
#[derive(Default)]
pub struct Starter {
    with_game: bool,
    /// `systemctl --user` did not work in this process: start at login for the rest of it.
    #[cfg_attr(windows, allow(dead_code))]
    no_systemd: bool,
    /// The path unit was enabled (and the session imported) by this process.
    #[cfg_attr(windows, allow(dead_code))]
    enabled: bool,
    logged: bool,
    #[cfg_attr(windows, allow(dead_code))]
    logged_old_dll: bool,
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
    /// whether it quits with the game. `dll_starts_launcher`: the DLL in the game folder has
    /// [`axidps_protocol::ident::CAP_STARTS_LAUNCHER`].
    pub fn keep(&mut self, dll_starts_launcher: bool) -> Lifetime {
        #[cfg(unix)]
        {
            let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
            let marker =
                axidps_protocol::paths::game_marker_path(axidps_protocol::v2::RingHost::Unix);
            let home = config_home();
            let session = session_env(|n| std::env::var_os(n));
            let input = LinuxIn {
                config_home: home.as_deref(),
                exec: appimage.as_deref(),
                marker: &marker,
                dll_starts_launcher,
                session: &session,
            };
            self.keep_in(&input, &systemctl)
        }
        #[cfg(windows)]
        {
            let dir = axidps_protocol::paths::config_dir();
            let exe = std::env::current_exe().ok();
            self.keep_windows(
                dir.as_deref(),
                exe.as_deref(),
                dll_starts_launcher,
                &win::Registry,
            )
        }
    }

    /// [`Starter::keep`] on Linux with its inputs passed in. Without an AppImage (a development
    /// build) nothing is written and the launcher stays running.
    #[cfg(unix)]
    fn keep_in(&mut self, i: &LinuxIn, sc: Systemctl) -> Lifetime {
        let (Some(home), Some(exec)) = (i.config_home, i.exec) else {
            if i.config_home.is_none() {
                self.log_once(|| {
                    "no config folder (XDG_CONFIG_HOME and HOME are unset); nothing starts axidps"
                        .into()
                });
            }
            return self.lifetime();
        };
        if self.no_systemd || (!i.dll_starts_launcher && !self.with_game) {
            if !self.no_systemd && !self.logged_old_dll {
                self.logged_old_dll = true;
                eprintln!(
                    "axidps: the game folder has no DLL that starts axidps yet; starting at login until it has"
                );
            }
            match install_in(home, exec) {
                Ok(()) => self.logged = false,
                Err(e) => self.log_once(|| format!("cannot write the autostart entry: {e}")),
            }
            return self.lifetime();
        }
        let started = match write_units_in(home, exec, i.marker) {
            Ok(changed) if changed || !self.enabled => {
                // Before the units are relied on: the launcher the service starts gets its
                // environment from the user manager, and some sessions do not put the display
                // there.
                if !i.session.is_empty() {
                    let mut args = vec!["import-environment"];
                    args.extend_from_slice(i.session);
                    if !ok(sc, &args) {
                        eprintln!(
                            "axidps: cannot import the display environment into systemd --user"
                        );
                    }
                }
                (!changed || ok(sc, &["daemon-reload"])) && ok(sc, &["enable", "--now", PATH_UNIT])
            }
            Ok(_) => true,
            Err(e) => {
                self.log_once(|| format!("cannot write the systemd units: {e}"));
                false
            }
        };
        match (started, self.with_game) {
            (true, _) => {
                self.enabled = true;
                self.logged = false;
                // An older DLL put back in the game folder (this process already runs with the
                // game) cannot start the next launcher: start at login again until it is fixed.
                let entry = if i.dll_starts_launcher {
                    remove_in(home)
                } else {
                    install_in(home, exec)
                };
                if let Err(e) = entry {
                    eprintln!("axidps: cannot update the autostart entry: {e}");
                }
                self.with_game = true;
                Lifetime::WithGame
            }
            // Worked before in this process (an update moved the AppImage): keep trying.
            (false, true) => {
                self.log_once(|| "cannot update the systemd units; retrying".into());
                self.enabled = false;
                Lifetime::WithGame
            }
            (false, false) => {
                eprintln!(
                    "axidps: systemctl --user is not available; starting at login instead of with the game"
                );
                let _ = remove_units_in(home);
                self.no_systemd = true;
                self.keep_in(i, sc)
            }
        }
    }

    fn lifetime(&self) -> Lifetime {
        if self.with_game {
            Lifetime::WithGame
        } else {
            Lifetime::Always
        }
    }

    /// [`Starter::keep`] on Windows with its inputs passed in: writes `exe` to the launcher
    /// path file in `config_dir` (an older DLL ignores it), and keeps the login `Run` value
    /// until the game folder's DLL starts the launcher.
    #[cfg_attr(unix, allow(dead_code))]
    fn keep_windows(
        &mut self,
        config_dir: Option<&Path>,
        exe: Option<&Path>,
        dll_starts_launcher: bool,
        run: &dyn RunValue,
    ) -> Lifetime {
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
        if let Err(e) = &written {
            self.log_once(|| format!("cannot write the launcher path for the game: {e}"));
        }
        let starts = written.is_ok() && dll_starts_launcher;
        let with_game = self.with_game || starts;
        // Kept while the game folder's DLL cannot start the launcher, even in a process that
        // already runs with the game (an older DLL was put back).
        // Written only when it differs, so the registry is not rewritten every check.
        let want = if starts { None } else { exe.map(run_value_for) };
        let key = if run.get() == want {
            Ok(())
        } else {
            run.set(want.as_deref())
        };
        if let Err(e) = key {
            self.log_once(|| format!("cannot update the login start: {e}"));
        } else if written.is_ok() {
            self.logged = false;
        }
        self.with_game = with_game;
        self.lifetime()
    }
}

#[cfg(windows)]
mod win {
    use std::io;

    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    use windows::core::{PCWSTR, w};

    const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    const NAME: PCWSTR = w!("axidps");

    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `axidps` (the installer's).
    pub struct Registry;

    impl super::RunValue for Registry {
        fn get(&self) -> Option<String> {
            let mut size = 0u32;
            // SAFETY: plain registry calls; the buffer is as large as the size passed.
            unsafe {
                let probe = RegGetValueW(
                    HKEY_CURRENT_USER,
                    KEY,
                    NAME,
                    RRF_RT_REG_SZ,
                    None,
                    None,
                    Some(&mut size),
                );
                if probe.is_err() {
                    return None;
                }
                let mut buf = vec![0u16; (size as usize).div_ceil(2)];
                let read = RegGetValueW(
                    HKEY_CURRENT_USER,
                    KEY,
                    NAME,
                    RRF_RT_REG_SZ,
                    None,
                    Some(buf.as_mut_ptr().cast()),
                    Some(&mut size),
                );
                if read.is_err() {
                    return None;
                }
                let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                Some(String::from_utf16_lossy(&buf[..len]))
            }
        }

        fn set(&self, value: Option<&str>) -> io::Result<()> {
            // SAFETY: plain registry calls; every string is NUL-terminated and outlives the call.
            let status = unsafe {
                match value {
                    Some(text) => {
                        let data: Vec<u16> =
                            text.encode_utf16().chain(std::iter::once(0)).collect();
                        RegSetKeyValueW(
                            HKEY_CURRENT_USER,
                            KEY,
                            NAME,
                            REG_SZ.0,
                            Some(data.as_ptr().cast()),
                            (data.len() * 2) as u32,
                        )
                    }
                    None => RegDeleteKeyValueW(HKEY_CURRENT_USER, KEY, NAME),
                }
            };
            if status.is_ok() || (value.is_none() && status == ERROR_FILE_NOT_FOUND) {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(status.0 as i32))
            }
        }
    }
}

/// Stops anything starting the launcher with the game, before the launchers are stopped: else a
/// launcher the path unit starts again could write the units back.
pub fn disable() {
    #[cfg(unix)]
    if let Some(home) = config_home() {
        disable_in(&home, &systemctl);
    }
}

#[cfg(unix)]
fn disable_in(home: &Path, sc: Systemctl) {
    if units_dir(home).join(PATH_UNIT).exists() {
        let _ = sc(&["disable", "--now", PATH_UNIT]);
    }
}

/// Removes what starts the launcher: the units, the link and the autostart entry (Linux), the
/// launcher path file and the `Run` value (Windows).
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
        let path = match axidps_protocol::paths::config_dir() {
            Some(dir) => {
                remove_file(&dir.join(axidps_protocol::paths::LAUNCHER_PATH_FILE)).map(drop)
            }
            None => Ok(()),
        };
        use RunValue as _;
        let key = if win::Registry.get().is_some() {
            win::Registry.set(None)
        } else {
            Ok(())
        };
        path.and(key)
    }
}

/// [`remove`] on Linux. The path unit is disabled and stopped first (again, if [`disable`]
/// already did), so it cannot start the launcher while the files go.
#[cfg(unix)]
fn remove_all_in(home: &Path, sc: Systemctl) -> io::Result<()> {
    disable_in(home, sc);
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

    const ENABLE: [&str; 2] = ["daemon-reload", "enable --now axidps-launch.path"];

    fn keep_with(
        s: &mut Starter,
        home: &Path,
        exec: &str,
        capable: bool,
        session: &[&str],
        fake: &Fake,
    ) -> Lifetime {
        let input = LinuxIn {
            config_home: Some(home),
            exec: Some(Path::new(exec)),
            marker: Path::new(MARKER),
            dll_starts_launcher: capable,
            session,
        };
        s.keep_in(&input, &|a| fake.run(a))
    }

    fn keep(s: &mut Starter, home: &Path, exec: &str, fake: &Fake) -> Lifetime {
        keep_with(s, home, exec, true, &[], fake)
    }

    fn link(home: &Path) -> PathBuf {
        home.join("axidps").join(APPIMAGE_LINK)
    }

    #[test]
    fn desktop_entry_text() {
        assert_eq!(
            desktop_entry(Path::new(EXEC)),
            "[Desktop Entry]\nType=Application\nName=AxiDPS\nExec=\"/home/u/AppImages/axidps-0.2.0.AppImage\"\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n"
        );
    }

    /// The Exec key is escaped as the Desktop Entry spec says: the string escapes first, then
    /// the quoting rule, and `%` doubled; a newline cannot end the key.
    #[test]
    fn desktop_entry_escapes_exec() {
        let e = desktop_entry(Path::new("/h/My $ap`p\" 100%\\x\ny.AppImage"));
        let exec = e.lines().find(|l| l.starts_with("Exec=")).unwrap();
        assert_eq!(exec, r#"Exec="/h/My \\$ap\\`p\\" 100%%\\\\x\ny.AppImage""#);
        assert_eq!(e.lines().count(), 6, "{e}");
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
            service_unit(
                Path::new("/home/u/.config/axidps/axidps.AppImage"),
                Path::new(MARKER)
            ),
            "[Unit]\nDescription=AxiDPS while Guild Wars 2 runs\n\n[Service]\nType=simple\nExecStart=/bin/sh -c '[ -x \"$$0\" ] && exec \"$$0\" --from-game; echo \"axidps: $$0 is missing; start AxiDPS once from AxiOM\" >&2; while [ -e /dev/shm/axidps-game ]; do sleep 5; done' \"/home/u/.config/axidps/axidps.AppImage\"\nKillMode=process\n"
        );
    }

    /// The service's script, as `sh` gets it from systemd (`$$` is a literal `$`): runs the
    /// link when its target is there, else waits for the marker to go and exits 0.
    #[test]
    fn service_script_waits_out_a_missing_appimage() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("marker");
        let link = tmp.path().join("axidps.AppImage");
        let unit = service_unit(&link, &marker);
        let line = unit.lines().find(|l| l.starts_with("ExecStart=")).unwrap();
        let script = line.split('\'').nth(1).unwrap().replace("$$", "$");
        let run = |arg: &Path| {
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(&script)
                .arg(arg)
                .output()
                .unwrap()
        };
        // Missing target, marker gone: logs and exits 0 without waiting.
        std::os::unix::fs::symlink(tmp.path().join("gone"), &link).unwrap();
        let out = run(&link);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("is missing"));
        // A target that is there runs, with the flag.
        let target = tmp.path().join("app");
        std::fs::write(&target, "#!/bin/sh\necho \"ran $1\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = run(&target);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ran --from-game\n");
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
            service_unit(&link(tmp.path()), Path::new(MARKER))
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(PATH_UNIT)).unwrap(),
            path_unit(Path::new(MARKER))
        );
        assert_eq!(
            std::fs::read_link(link(tmp.path())).unwrap(),
            Path::new(EXEC)
        );
        assert_eq!(fake.take(), ENABLE);
        assert!(!tmp.path().join("autostart/axidps.desktop").exists());
        // Nothing changed: systemctl is not run again.
        assert_eq!(keep(&mut s, tmp.path(), EXEC, &fake), Lifetime::WithGame);
        assert!(fake.take().is_empty());
    }

    #[test]
    fn an_update_only_moves_the_link() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        let mut s = Starter::new();
        keep(&mut s, tmp.path(), "/a/axidps-0.1.0.AppImage", &fake);
        fake.take();
        assert_eq!(keep(&mut s, tmp.path(), EXEC, &fake), Lifetime::WithGame);
        assert!(fake.take().is_empty());
        assert_eq!(
            std::fs::read_link(link(tmp.path())).unwrap(),
            Path::new(EXEC)
        );
    }

    #[test]
    fn a_new_process_enables_the_units_again_without_a_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        keep(&mut Starter::new(), tmp.path(), EXEC, &fake);
        fake.take();
        // Units already right, but someone may have disabled them.
        keep(&mut Starter::new(), tmp.path(), EXEC, &fake);
        assert_eq!(fake.take(), ["enable --now axidps-launch.path"]);
    }

    #[test]
    fn a_manual_start_repairs_a_link_to_a_deleted_appimage() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        keep(
            &mut Starter::new(),
            tmp.path(),
            "/gone/axidps-0.1.0.AppImage",
            &fake,
        );
        // The player deleted that AppImage and started a new one from AxiOM.
        keep(&mut Starter::new(), tmp.path(), EXEC, &fake);
        assert_eq!(
            std::fs::read_link(link(tmp.path())).unwrap(),
            Path::new(EXEC)
        );
    }

    #[test]
    fn a_graphical_session_is_imported_before_enabling() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        let session = ["WAYLAND_DISPLAY", "XAUTHORITY"];
        keep_with(&mut Starter::new(), tmp.path(), EXEC, true, &session, &fake);
        assert_eq!(
            fake.take(),
            [
                "import-environment WAYLAND_DISPLAY XAUTHORITY",
                "daemon-reload",
                "enable --now axidps-launch.path"
            ]
        );
    }

    #[test]
    fn session_env_needs_a_display() {
        let env = |set: &'static [&'static str]| {
            move |n: &str| set.contains(&n).then(|| std::ffi::OsString::from("x"))
        };
        assert_eq!(
            session_env(env(&[
                "DISPLAY",
                "XAUTHORITY",
                "XDG_RUNTIME_DIR",
                "DBUS_SESSION_BUS_ADDRESS"
            ])),
            ["DISPLAY", "XAUTHORITY"]
        );
        assert_eq!(session_env(env(&["WAYLAND_DISPLAY"])), ["WAYLAND_DISPLAY"]);
        assert!(session_env(env(&["XAUTHORITY", "XDG_RUNTIME_DIR"])).is_empty());
        assert!(session_env(|_| Some("".into())).is_empty());
    }

    #[test]
    fn an_old_dll_keeps_the_login_start() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        let mut s = Starter::new();
        assert_eq!(
            keep_with(&mut s, tmp.path(), EXEC, false, &[], &fake),
            Lifetime::Always
        );
        assert!(fake.take().is_empty());
        assert!(tmp.path().join("autostart/axidps.desktop").exists());
        assert!(!tmp.path().join("systemd").exists());
        // The launcher installed the new DLL: from now on it runs with the game.
        assert_eq!(keep(&mut s, tmp.path(), EXEC, &fake), Lifetime::WithGame);
        assert_eq!(fake.take(), ENABLE);
        assert!(!tmp.path().join("autostart/axidps.desktop").exists());
        // An older DLL put back: this process still quits with the game, but the login entry
        // returns so the next launcher starts.
        assert_eq!(
            keep_with(&mut s, tmp.path(), EXEC, false, &[], &fake),
            Lifetime::WithGame
        );
        assert!(tmp.path().join("autostart/axidps.desktop").exists());
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
            assert!(std::fs::symlink_metadata(link(tmp.path())).is_err());
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
        // Someone removed the unit, so it is written again and enabling fails.
        std::fs::remove_file(tmp.path().join("systemd/user").join(SERVICE_UNIT)).unwrap();
        let broken = Fake::new(no_bus);
        assert_eq!(keep(&mut s, tmp.path(), EXEC, &broken), Lifetime::WithGame);
        assert!(!tmp.path().join("autostart/axidps.desktop").exists());
        // Retried on the next check, since enabling did not finish.
        broken.take();
        keep(&mut s, tmp.path(), EXEC, &broken);
        assert_eq!(broken.take(), ["enable --now axidps-launch.path"]);
    }

    #[test]
    fn without_an_appimage_nothing_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        let mut s = Starter::new();
        let input = LinuxIn {
            config_home: Some(tmp.path()),
            exec: None,
            marker: Path::new(MARKER),
            dll_starts_launcher: true,
            session: &[],
        };
        assert_eq!(s.keep_in(&input, &|a| fake.run(a)), Lifetime::Always);
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
        assert!(std::fs::symlink_metadata(link(tmp.path())).is_err());
        assert!(!tmp.path().join("autostart/axidps.desktop").exists());
        // Nothing left: systemctl is not run.
        remove_all_in(tmp.path(), &|a| fake.run(a)).unwrap();
        assert!(fake.take().is_empty());
    }

    #[test]
    fn remove_cleans_up_a_half_installed_state() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("systemd/user");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SERVICE_UNIT), "x").unwrap();
        let fake = Fake::new(works);
        remove_all_in(tmp.path(), &|a| fake.run(a)).unwrap();
        // No path unit to disable; the service file still goes and systemd reloads.
        assert_eq!(fake.take(), ["daemon-reload"]);
        assert!(!dir.join(SERVICE_UNIT).exists());
    }

    #[test]
    fn disable_comes_before_the_files_go() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(works);
        keep(&mut Starter::new(), tmp.path(), EXEC, &fake);
        fake.take();
        disable_in(tmp.path(), &|a| fake.run(a));
        assert_eq!(fake.take(), ["disable --now axidps-launch.path"]);
        assert!(tmp.path().join("systemd/user").join(PATH_UNIT).exists());
    }

    /// A fake `Run` value that records writes.
    #[derive(Default)]
    struct FakeRun {
        value: RefCell<Option<String>>,
        writes: RefCell<Vec<Option<String>>>,
    }

    impl RunValue for FakeRun {
        fn get(&self) -> Option<String> {
            self.value.borrow().clone()
        }
        fn set(&self, value: Option<&str>) -> io::Result<()> {
            *self.value.borrow_mut() = value.map(str::to_owned);
            self.writes.borrow_mut().push(value.map(str::to_owned));
            Ok(())
        }
    }

    #[test]
    fn windows_login_start_is_kept_until_the_dll_starts_the_launcher() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = Path::new(r"C:\Users\u\AppData\Local\Programs\axidps\axidps.exe");
        let value = run_value_for(exe);
        assert_eq!(
            value,
            r#""C:\Users\u\AppData\Local\Programs\axidps\axidps.exe""#
        );
        // As the installer left it.
        let run = FakeRun {
            value: RefCell::new(Some(value.clone())),
            ..FakeRun::default()
        };
        let mut s = Starter::new();
        // An older DLL: the path is written for later, the Run value stays (and is not
        // rewritten).
        assert_eq!(
            s.keep_windows(Some(tmp.path()), Some(exe), false, &run),
            Lifetime::Always
        );
        assert!(run.writes.take().is_empty());
        let file = tmp.path().join(axidps_protocol::paths::LAUNCHER_PATH_FILE);
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            axidps_protocol::paths::parse_launcher_path(&text).as_deref(),
            Some(exe)
        );
        // The new DLL is in: the Run value goes, once.
        assert_eq!(
            s.keep_windows(Some(tmp.path()), Some(exe), true, &run),
            Lifetime::WithGame
        );
        assert_eq!(run.writes.take(), [None]);
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        s.keep_windows(Some(tmp.path()), Some(exe), true, &run);
        assert!(run.writes.take().is_empty());
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before
        );
        // An older DLL put back: the Run value returns, the process still quits with the game.
        assert_eq!(
            s.keep_windows(Some(tmp.path()), Some(exe), false, &run),
            Lifetime::WithGame
        );
        assert_eq!(run.writes.take(), [Some(value.clone())]);
        // No config folder: nothing can start it with the game, so the login start stays.
        let fresh = FakeRun::default();
        assert_eq!(
            Starter::new().keep_windows(None, Some(exe), true, &fresh),
            Lifetime::Always
        );
        assert_eq!(fresh.writes.take(), [Some(value)]);
    }
}
