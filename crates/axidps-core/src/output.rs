//! Where logs go, and writing one fight.

use std::io;
use std::path::{Path, PathBuf};

use axidps_evtc::{Log, default_log_dir, encode, log_file_stem, write_log_atomic};

/// Guild Wars 2's Steam app id.
const GW2_APP_ID: &str = "1284210";

/// `~/.local/share/Steam` and `~/.steam/steam`.
pub fn default_steam_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    steam_roots_in(&home)
}

fn steam_roots_in(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local").join("share").join("Steam"),
        home.join(".steam").join("steam"),
    ]
}

/// Library paths from a `libraryfolders.vdf` (`"path"  "..."` lines).
fn vdf_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('"').skip(1).step_by(2);
            if parts.next()? != "path" {
                return None;
            }
            Some(PathBuf::from(parts.next()?.replace("\\\\", "\\")))
        })
        .collect()
}

/// The Proton prefix's `Documents` folder for GW2, searching each root and the libraries its
/// `libraryfolders.vdf` lists. The first that exists wins.
pub fn find_proton_documents(steam_roots: &[PathBuf]) -> Option<PathBuf> {
    for root in steam_roots {
        let mut libs = vec![root.clone()];
        if let Ok(text) = std::fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf"))
        {
            libs.extend(vdf_paths(&text));
        }
        for lib in libs {
            let docs = lib
                .join("steamapps")
                .join("compatdata")
                .join(GW2_APP_ID)
                .join("pfx")
                .join("drive_c")
                .join("users")
                .join("steamuser")
                .join("Documents");
            if docs.is_dir() {
                return Some(docs);
            }
        }
    }
    None
}

/// The log folder: an explicit one wins; otherwise the platform's Documents folder
/// (Proton prefix on Linux, never `~/Documents`).
pub fn resolve_log_dir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if explicit.is_some() {
        return explicit;
    }
    #[cfg(windows)]
    {
        dirs::document_dir().map(|d| default_log_dir(&d))
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        proton_log_dir(&home)
    }
}

/// The log folder under the Proton prefix found from `home`'s Steam roots. A prefix whose
/// Documents folder resolves (through symlinks) into `home/Documents` counts as not found:
/// logs never go to the user's own Documents folder.
#[cfg_attr(windows, allow(dead_code))]
fn proton_log_dir(home: &Path) -> Option<PathBuf> {
    let docs = find_proton_documents(&steam_roots_in(home))?;
    let real = std::fs::canonicalize(&docs).ok()?;
    let home_docs = home.join("Documents");
    let home_docs = std::fs::canonicalize(&home_docs).unwrap_or(home_docs);
    if real.starts_with(&home_docs) {
        return None;
    }
    Some(default_log_dir(&docs))
}

/// Encode `log` and write it as `<end local time>.zevtc` in `dir`.
pub fn write_fight(
    dir: &Path,
    log: &Log,
    end_unix_s: i64,
    to_local: impl Fn(i64) -> chrono::NaiveDateTime,
) -> io::Result<PathBuf> {
    let stem = log_file_stem(to_local(end_unix_s));
    write_log_atomic(dir, &stem, &encode(log))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tmp(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "axidps-output-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn prefix_found_in_second_library() {
        let root = tmp("root");
        let lib1 = tmp("lib1");
        let lib2 = tmp("lib2");
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        std::fs::create_dir_all(lib1.join("steamapps")).unwrap();
        let docs = lib2.join("steamapps/compatdata/1284210/pfx/drive_c/users/steamuser/Documents");
        std::fs::create_dir_all(&docs).unwrap();
        let vdf = format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
            lib1.display(),
            lib2.display()
        );
        std::fs::write(root.join("steamapps/libraryfolders.vdf"), vdf).unwrap();
        assert_eq!(find_proton_documents(&[root]), Some(docs));
    }

    #[test]
    fn no_prefix_found_is_none() {
        let root = tmp("none");
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        assert_eq!(find_proton_documents(&[root, tmp("absent")]), None);
    }

    fn prefix_docs(home: &Path) -> PathBuf {
        home.join(
            ".local/share/Steam/steamapps/compatdata/1284210/pfx/drive_c/users/steamuser/Documents",
        )
    }

    #[test]
    fn proton_prefix_is_the_log_root() {
        let home = tmp("home-real");
        let docs = prefix_docs(&home);
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        assert_eq!(proton_log_dir(&home), Some(default_log_dir(&docs)));
    }

    #[cfg(unix)]
    #[test]
    fn prefix_symlinked_into_home_documents_is_not_used() {
        let home = tmp("home-link");
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        // The whole prefix's `drive_c/users/steamuser` is a symlink into ~/Documents.
        let docs = prefix_docs(&home);
        let user = docs.parent().unwrap();
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::create_dir_all(home.join("Documents/steamuser/Documents")).unwrap();
        std::os::unix::fs::symlink(home.join("Documents/steamuser"), user).unwrap();
        assert!(docs.is_dir());
        assert_eq!(proton_log_dir(&home), None);
        // So is a Documents folder that is itself a link to ~/Documents.
        let home = tmp("home-link2");
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        let docs = prefix_docs(&home);
        std::fs::create_dir_all(docs.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(home.join("Documents"), &docs).unwrap();
        assert_eq!(proton_log_dir(&home), None);
    }

    #[test]
    fn explicit_dir_wins() {
        let d = PathBuf::from("/some/where");
        assert_eq!(resolve_log_dir(Some(d.clone())), Some(d));
    }

    #[test]
    fn write_fight_names_by_end_local_time() {
        let dir = tmp("write");
        let log = Log {
            header: axidps_evtc::Header::wvw(),
            agents: vec![],
            skills: vec![],
            events: vec![],
        };
        // 2026-11-18 20:06:01 UTC.
        let end = 1_795_032_361;
        let to_local = |s: i64| chrono::DateTime::from_timestamp(s, 0).unwrap().naive_utc();
        let p = write_fight(&dir, &log, end, to_local).unwrap();
        assert_eq!(p.file_name().unwrap(), "20261118-200601.zevtc");
        assert!(p.exists());
    }
}
