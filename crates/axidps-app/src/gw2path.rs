//! Finding the Guild Wars 2 folder. Ported from AxiOM's `electron/arcdps.ts`, restricted to
//! the 64-bit game: the DLL must sit beside `Gw2-64.exe`, which is where the proxy looks for
//! arcdps.

use std::path::{Path, PathBuf};

/// Whether `dir` contains `Gw2-64.exe` (any letter case).
fn has_gw2_exe(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        e.file_name()
            .to_str()
            .is_some_and(|n| n.eq_ignore_ascii_case("gw2-64.exe"))
    })
}

/// The game folder for `dir`: `dir` itself when it holds `Gw2-64.exe`, or its parent when
/// `dir` is a `bin64` folder whose parent holds it.
fn game_dir(dir: &Path) -> Option<PathBuf> {
    let is_bin64 = dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("bin64"));
    if is_bin64 {
        if let Some(parent) = dir.parent() {
            if has_gw2_exe(parent) {
                return Some(parent.to_path_buf());
            }
        }
    }
    has_gw2_exe(dir).then(|| dir.to_path_buf())
}

/// The `"path"` values of a Steam `libraryfolders.vdf`, with `\\` unescaped to `\`.
pub fn steam_libraries(vdf: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in vdf.lines() {
        let Some(rest) = line.trim().strip_prefix("\"path\"") else {
            continue;
        };
        let rest = rest.trim();
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        let Some(end) = rest.rfind('"') else {
            continue;
        };
        let value = rest[..end].replace("\\\\", "\\");
        if !value.is_empty() {
            out.push(PathBuf::from(value));
        }
    }
    out
}

/// Where AxiAM keeps its config (it stores `gw2Path` there).
pub fn axiam_config_path(home: &Path, windows: bool) -> PathBuf {
    if windows {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Roaming"));
        base.join("axiam").join("config.json")
    } else {
        home.join(".config").join("axiam").join("config.json")
    }
}

/// AxiAM's `gw2Path` from its config file text.
pub fn axiam_gw2_path(axiam_config_json: &str) -> Option<PathBuf> {
    let value: serde_json::Value = serde_json::from_str(axiam_config_json).ok()?;
    let path = value.get("gw2Path")?.as_str()?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Places the game is usually installed, then every Steam library's `Guild Wars 2` folder.
pub fn candidates(home: &Path, windows: bool) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let steam_roots: Vec<PathBuf>;
    if windows {
        out.push(PathBuf::from(r"C:\Program Files\Guild Wars 2"));
        out.push(PathBuf::from(r"C:\Program Files (x86)\Guild Wars 2"));
        out.push(PathBuf::from(r"C:\Guild Wars 2"));
        let pf = std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        out.push(pf.join("Guild Wars 2"));
        steam_roots = vec![PathBuf::from(r"C:\Program Files (x86)\Steam")];
    } else {
        let common = |root: &Path| root.join("steamapps").join("common").join("Guild Wars 2");
        steam_roots = vec![
            home.join(".steam").join("steam"),
            home.join(".local").join("share").join("Steam"),
        ];
        out.push(common(&steam_roots[0]));
        out.push(common(&steam_roots[1]));
        out.push(
            home.join("Games")
                .join("guild-wars-2")
                .join("drive_c")
                .join("Program Files")
                .join("Guild Wars 2"),
        );
    }
    for root in &steam_roots {
        let vdf = root.join("steamapps").join("libraryfolders.vdf");
        if let Ok(text) = std::fs::read_to_string(vdf) {
            for lib in steam_libraries(&text) {
                out.push(lib.join("steamapps").join("common").join("Guild Wars 2"));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

/// The first folder that holds `Gw2-64.exe`: the manual path, then AxiAM's, then the
/// candidates. A `bin64` folder resolves to its parent when that holds the exe.
pub fn resolve(
    manual: Option<&Path>,
    axiam: Option<PathBuf>,
    candidates: &[PathBuf],
) -> Option<PathBuf> {
    manual
        .map(Path::to_path_buf)
        .into_iter()
        .chain(axiam)
        .chain(candidates.iter().cloned())
        .find_map(|dir| game_dir(&dir))
}

/// Guild Wars 2's Steam app id (its Proton prefix is `steamapps/compatdata/<id>/pfx`).
#[cfg_attr(windows, allow(dead_code))]
const GW2_APP_ID: &str = "1284210";

/// The Wine prefix the game in `game` runs from: the folder holding the `drive_c` the game sits
/// under (Lutris, Bottles, plain Wine), or, for a Steam install
/// (`<library>/steamapps/common/<game>`), that library's Proton prefix for GW2.
#[cfg_attr(windows, allow(dead_code))]
fn wine_prefix(game: &Path) -> Option<PathBuf> {
    if let Some(drive_c) = game
        .ancestors()
        .find(|a| a.file_name().is_some_and(|n| n.eq_ignore_ascii_case("drive_c")))
    {
        return drive_c.parent().map(Path::to_path_buf);
    }
    let common = game.parent()?;
    let steamapps = common.parent()?;
    let named = |p: &Path, n: &str| p.file_name().is_some_and(|f| f == n);
    (named(common, "common") && named(steamapps, "steamapps")).then(|| {
        steamapps
            .join("compatdata")
            .join(GW2_APP_ID)
            .join("pfx")
    })
}

/// The arcdps log folder inside the Wine prefix the game in `game` runs from, for core's
/// `--log-dir`. The prefix user is `steamuser` (Proton), then `user`, then any other with a
/// Documents folder. `None` when there is no such prefix or folder, or when the prefix's
/// Documents resolves into `home/Documents`: core never writes to the user's own Documents.
#[cfg_attr(windows, allow(dead_code))]
pub fn prefix_log_dir(game: &Path, home: &Path, user: Option<&str>) -> Option<PathBuf> {
    let users = wine_prefix(game)?.join("drive_c").join("users");
    let mut names: Vec<String> = vec!["steamuser".into()];
    names.extend(user.map(str::to_owned));
    if let Ok(entries) = std::fs::read_dir(&users) {
        let mut others: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.eq_ignore_ascii_case("public"))
            .collect();
        others.sort();
        names.extend(others);
    }
    let docs = names
        .iter()
        .map(|n| users.join(n).join("Documents"))
        .find(|d| d.is_dir())?;
    let real = std::fs::canonicalize(&docs).ok()?;
    let home_docs = home.join("Documents");
    let home_docs = std::fs::canonicalize(&home_docs).unwrap_or(home_docs);
    if real.starts_with(&home_docs) {
        return None;
    }
    // Same layout as axidps_evtc::default_log_dir.
    Some(
        docs.join("Guild Wars 2")
            .join("addons")
            .join("arcdps")
            .join("arcdps.cbtlogs"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn game(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Gw2-64.exe"), b"exe").unwrap();
        dir
    }

    #[test]
    fn resolve_prefers_manual_then_axiam_then_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let manual = game(tmp.path(), "manual");
        let axiam = game(tmp.path(), "axiam");
        let cand = game(tmp.path(), "cand");
        let empty = tmp.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        let cands = vec![empty.clone(), cand.clone()];

        assert_eq!(
            resolve(Some(&manual), Some(axiam.clone()), &cands),
            Some(manual.clone())
        );
        // A manual path without the exe does not count.
        assert_eq!(
            resolve(Some(&empty), Some(axiam.clone()), &cands),
            Some(axiam.clone())
        );
        assert_eq!(resolve(None, Some(empty.clone()), &cands), Some(cand));
        assert_eq!(resolve(None, None, &[empty]), None);
        // 32-bit Gw2.exe is not accepted.
        let old = tmp.path().join("old");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("Gw2.exe"), b"x").unwrap();
        assert_eq!(resolve(Some(&old), None, &[]), None);
    }

    #[test]
    fn exe_match_ignores_case_and_bin64_resolves_to_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("gw2");
        fs::create_dir_all(root.join("bin64")).unwrap();
        fs::write(root.join("GW2-64.EXE"), b"exe").unwrap();
        assert_eq!(resolve(Some(&root.join("bin64")), None, &[]), Some(root.clone()));
        assert_eq!(resolve(Some(&root), None, &[]), Some(root));
    }

    #[test]
    fn steam_libraries_parses_vdf() {
        let vdf = r#""libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
	}
	"1"
	{
		"path"		"/mnt/games/SteamLibrary"
		"apps" { "1284210" "1" }
	}
}"#;
        assert_eq!(
            steam_libraries(vdf),
            vec![
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from("/mnt/games/SteamLibrary"),
            ]
        );
    }

    #[test]
    fn axiam_path_parsed() {
        assert_eq!(
            axiam_gw2_path(r#"{"gw2Path":"/g/gw2"}"#),
            Some(PathBuf::from("/g/gw2"))
        );
        assert_eq!(axiam_gw2_path(r#"{"gw2Path":3}"#), None);
        assert_eq!(axiam_gw2_path("nope"), None);
    }

    #[test]
    fn candidates_include_steam_libraries_deduplicated() {
        let home = tempfile::tempdir().unwrap();
        let steamapps = home.path().join(".steam/steam/steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        let default = home.path().join(".steam/steam");
        let vdf = format!(
            "\"path\"\t\t\"{}\"\n\"path\"\t\t\"/mnt/lib\"\n",
            default.display()
        );
        fs::write(steamapps.join("libraryfolders.vdf"), vdf).unwrap();
        let c = candidates(home.path(), false);
        let own = default.join("steamapps/common/Guild Wars 2");
        assert_eq!(c.iter().filter(|p| **p == own).count(), 1);
        assert_eq!(c[0], own);
        assert_eq!(c.last().unwrap(), &PathBuf::from("/mnt/lib/steamapps/common/Guild Wars 2"));
    }

    fn logs(docs: &Path) -> PathBuf {
        docs.join("Guild Wars 2/addons/arcdps/arcdps.cbtlogs")
    }

    #[test]
    fn lutris_prefix_log_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let pfx = home.join("Games/guild-wars-2");
        let game = pfx.join("drive_c/Program Files/Guild Wars 2");
        fs::create_dir_all(&game).unwrap();
        // No Documents yet: nothing to pass.
        assert_eq!(prefix_log_dir(&game, &home, Some("me")), None);
        let docs = pfx.join("drive_c/users/me/Documents");
        fs::create_dir_all(&docs).unwrap();
        fs::create_dir_all(pfx.join("drive_c/users/Public/Documents")).unwrap();
        assert_eq!(prefix_log_dir(&game, &home, Some("me")), Some(logs(&docs)));
        // The user name is only a preference: another user's Documents is found too.
        assert_eq!(prefix_log_dir(&game, &home, Some("other")), Some(logs(&docs)));
        assert_eq!(prefix_log_dir(&game, &home, None), Some(logs(&docs)));
    }

    #[test]
    fn steam_library_prefix_log_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let lib = tmp.path().join("mnt/SteamLibrary");
        let game = lib.join("steamapps/common/Guild Wars 2");
        fs::create_dir_all(&game).unwrap();
        let docs = lib.join("steamapps/compatdata/1284210/pfx/drive_c/users/steamuser/Documents");
        fs::create_dir_all(&docs).unwrap();
        assert_eq!(prefix_log_dir(&game, &home, Some("me")), Some(logs(&docs)));
    }

    #[test]
    fn plain_folder_has_no_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("Guild Wars 2");
        fs::create_dir_all(&game).unwrap();
        assert_eq!(prefix_log_dir(&game, tmp.path(), Some("me")), None);
    }

    #[cfg(unix)]
    #[test]
    fn prefix_documents_linked_to_home_documents_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join("Documents")).unwrap();
        let pfx = home.join("Games/gw2");
        let game = pfx.join("drive_c/Guild Wars 2");
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(pfx.join("drive_c/users/me")).unwrap();
        std::os::unix::fs::symlink(home.join("Documents"), pfx.join("drive_c/users/me/Documents"))
            .unwrap();
        assert_eq!(prefix_log_dir(&game, &home, Some("me")), None);
    }
}
