//! Starting the launcher at login. On Linux that is an XDG autostart entry; on Windows the
//! NSIS script owns the Run key, so everything here does nothing.

use std::io;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

/// The text of the autostart entry that starts `exec`.
#[cfg_attr(windows, allow(dead_code))]
pub fn desktop_entry(exec: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=AxiDPS\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        exec.display()
    )
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

/// Writes the entry for `exec` under `config_home`, creating the folder, unless the file
/// already says exactly that. The file appears whole or not at all.
#[cfg(unix)]
fn install_in(config_home: &Path, exec: &Path) -> io::Result<()> {
    let path = entry_path(config_home);
    let wanted = desktop_entry(exec);
    if std::fs::read_to_string(&path).is_ok_and(|have| have == wanted) {
        return Ok(());
    }
    let dir = path.parent().expect("entry path has a parent");
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("axidps.desktop.tmp");
    std::fs::write(&tmp, wanted)?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Deletes the entry under `config_home`; a missing one is fine.
#[cfg(unix)]
fn remove_in(config_home: &Path) -> io::Result<()> {
    match std::fs::remove_file(entry_path(config_home)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Makes the launcher start at login, running `exec`. An entry that is already right is left
/// alone.
pub fn install(exec: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let home = config_home().ok_or_else(no_config_home)?;
        install_in(&home, exec)
    }
    #[cfg(windows)]
    {
        let _ = exec;
        Ok(())
    }
}

/// Stops the launcher starting at login.
pub fn remove() -> io::Result<()> {
    #[cfg(unix)]
    {
        match config_home() {
            Some(home) => remove_in(&home),
            None => Ok(()),
        }
    }
    #[cfg(windows)]
    {
        Ok(())
    }
}

#[cfg(unix)]
fn no_config_home() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "no config folder (XDG_CONFIG_HOME and HOME are unset)")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const EXEC: &str = "/home/u/AppImages/axidps-0.2.0.AppImage";

    #[test]
    fn desktop_entry_text() {
        assert_eq!(
            desktop_entry(Path::new(EXEC)),
            "[Desktop Entry]\nType=Application\nName=AxiDPS\nExec=\"/home/u/AppImages/axidps-0.2.0.AppImage\"\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n"
        );
    }

    #[test]
    fn autostart_install_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        // The autostart folder does not exist yet.
        install_in(tmp.path(), Path::new(EXEC)).unwrap();
        let file = tmp.path().join("autostart/axidps.desktop");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), desktop_entry(Path::new(EXEC)));
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
        assert_eq!(std::fs::read_to_string(&file).unwrap(), desktop_entry(Path::new(EXEC)));
        // Right already: left untouched, even if someone else's mtime matters.
        std::fs::write(&file, desktop_entry(Path::new(EXEC))).unwrap();
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        install_in(tmp.path(), Path::new(EXEC)).unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), before);
    }
}
