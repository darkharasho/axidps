//! Finding the Guild Wars 2 folder. Ported from AxiOM's `electron/arcdps.ts`, restricted to
//! the 64-bit game: the DLL must sit beside `Gw2-64.exe`, which is where the proxy looks for
//! the game.

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

/// Whether the elevated `--grant-game-folder` may give `user` write access to `path`: `path`
/// must be a folder that holds `Gw2-64.exe` (after following links), and `user` a bare account
/// name, so that a UAC prompt approved for the game folder cannot be turned into write access
/// to another folder, or into other rights through the `icacls` argument. Returns the resolved
/// folder, which is what should be granted.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn check_grant_target(path: &Path, user: &str) -> Result<PathBuf, String> {
    let real = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !real.is_dir() || !has_gw2_exe(&real) {
        return Err(format!("{} is not a Guild Wars 2 folder", path.display()));
    }
    // `:` would end the name in the `icacls` argument; `(`, `)` are allowed in Windows names.
    let bad = |c: char| c.is_control() || matches!(c, ':' | '"' | ',' | '*');
    if user.trim().is_empty() || user.chars().any(bad) {
        return Err(format!("{user:?} is not an account name"));
    }
    Ok(without_verbatim(real))
}

/// `p` without the `\\?\` prefix `canonicalize` adds on Windows, which `icacls` may refuse.
#[cfg_attr(not(windows), allow(dead_code))]
fn without_verbatim(p: PathBuf) -> PathBuf {
    let Some(s) = p.to_str() else { return p };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p
    }
}

/// The game folder for `dir`: `dir` itself when it holds `Gw2-64.exe`, or its parent when
/// `dir` is a `bin64` folder whose parent holds it.
fn game_dir(dir: &Path) -> Option<PathBuf> {
    let is_bin64 = dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("bin64"));
    if is_bin64
        && let Some(parent) = dir.parent()
        && has_gw2_exe(parent)
    {
        return Some(parent.to_path_buf());
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

/// Where AxiOM keeps its config: Electron's userData for the app name `axiom` (the Linux
/// folder may carry the product name instead).
pub fn axiom_config_paths(home: &Path, windows: bool) -> Vec<PathBuf> {
    if windows {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Roaming"));
        vec![base.join("axiom").join("config.json")]
    } else {
        let base = home.join(".config");
        vec![
            base.join("axiom").join("config.json"),
            base.join("AxiOM").join("config.json"),
        ]
    }
}

/// The game folder the user set in AxiOM (`arcdps.gw2PathOverride`), from its config text.
pub fn axiom_gw2_path(axiom_config_json: &str) -> Option<PathBuf> {
    let value: serde_json::Value = serde_json::from_str(axiom_config_json).ok()?;
    let path = value
        .get("arcdps")?
        .get("gw2PathOverride")?
        .as_str()?
        .trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// The folder a registry value names: a folder, or an exe path (possibly quoted, possibly
/// with an icon index `,0` after it), whose folder is meant.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn folder_from_registry_value(value: &str) -> Option<PathBuf> {
    let mut v = value.trim().trim_matches('\0');
    if let Some((path, index)) = v.rsplit_once(',')
        && index.trim().parse::<i32>().is_ok()
    {
        v = path;
    }
    let v = v.trim().trim_matches('"').trim();
    if v.is_empty() {
        return None;
    }
    let p = PathBuf::from(v);
    let is_exe = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if is_exe {
        // `PathBuf` on Linux (tests) does not split on `\`.
        let cut = v.rfind(['\\', '/'])?;
        return Some(PathBuf::from(&v[..cut]));
    }
    Some(p)
}

/// The game folder as Windows records it: the ArenaNet installer's key and the uninstall
/// entries of the standalone and Steam installs. Empty off Windows.
pub fn registry_candidates() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        win::registry_values()
            .iter()
            .filter_map(|v| folder_from_registry_value(v))
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
        RegGetValueW,
    };
    use windows::core::PCWSTR;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read(root: HKEY, key: &str, name: &str) -> Option<String> {
        let (key, name) = (wide(key), wide(name));
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
        let mut size = 0u32;
        // SAFETY: plain registry reads; the buffer is as large as the size passed, and both
        // strings are NUL-terminated and outlive the calls.
        unsafe {
            let probe = RegGetValueW(
                root,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                flags,
                None,
                None,
                Some(&mut size),
            );
            if probe.is_err() || size == 0 {
                return None;
            }
            let mut buf = vec![0u16; (size as usize).div_ceil(2)];
            let got = RegGetValueW(
                root,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                flags,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            );
            if got.is_err() {
                return None;
            }
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..len]))
        }
    }

    pub fn registry_values() -> Vec<String> {
        const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
        const UNINSTALL32: &str =
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";
        let lookups: [(HKEY, String, &str); 10] = [
            (
                HKEY_LOCAL_MACHINE,
                r"SOFTWARE\ArenaNet\Guild Wars 2".into(),
                "Path",
            ),
            (
                HKEY_LOCAL_MACHINE,
                r"SOFTWARE\WOW6432Node\ArenaNet\Guild Wars 2".into(),
                "Path",
            ),
            (
                HKEY_CURRENT_USER,
                r"SOFTWARE\ArenaNet\Guild Wars 2".into(),
                "Path",
            ),
            (
                HKEY_LOCAL_MACHINE,
                format!(r"{UNINSTALL}\Guild Wars 2"),
                "InstallLocation",
            ),
            (
                HKEY_LOCAL_MACHINE,
                format!(r"{UNINSTALL}\Guild Wars 2"),
                "DisplayIcon",
            ),
            (
                HKEY_LOCAL_MACHINE,
                format!(r"{UNINSTALL32}\Guild Wars 2"),
                "InstallLocation",
            ),
            (
                HKEY_LOCAL_MACHINE,
                format!(r"{UNINSTALL32}\Guild Wars 2"),
                "DisplayIcon",
            ),
            (
                HKEY_CURRENT_USER,
                format!(r"{UNINSTALL}\Guild Wars 2"),
                "InstallLocation",
            ),
            (
                HKEY_CURRENT_USER,
                format!(r"{UNINSTALL}\Guild Wars 2"),
                "DisplayIcon",
            ),
            (
                HKEY_LOCAL_MACHINE,
                format!(r"{UNINSTALL}\Steam App 1284210"),
                "InstallLocation",
            ),
        ];
        lookups
            .iter()
            .filter_map(|(root, key, name)| read(*root, key, name))
            .collect()
    }
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
        // Folders people pick for games, on every drive.
        for drive in 'C'..='Z' {
            for sub in [
                r"Guild Wars 2",
                r"Games\Guild Wars 2",
                r"Program Files\Guild Wars 2",
                r"SteamLibrary\steamapps\common\Guild Wars 2",
            ] {
                out.push(PathBuf::from(format!(r"{drive}:\{sub}")));
            }
        }
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
    if let Some(drive_c) = game.ancestors().find(|a| {
        a.file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("drive_c"))
    }) {
        return drive_c.parent().map(Path::to_path_buf);
    }
    let common = game.parent()?;
    let steamapps = common.parent()?;
    let named = |p: &Path, n: &str| p.file_name().is_some_and(|f| f == n);
    (named(common, "common") && named(steamapps, "steamapps"))
        .then(|| steamapps.join("compatdata").join(GW2_APP_ID).join("pfx"))
}

/// The EVTC log folder inside the Wine prefix the game in `game` runs from, for core's
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
    fn verbatim_prefix_is_dropped_for_icacls() {
        let p = |s: &str| without_verbatim(PathBuf::from(s));
        assert_eq!(
            p(r"\\?\C:\Games\Guild Wars 2"),
            PathBuf::from(r"C:\Games\Guild Wars 2")
        );
        assert_eq!(
            p(r"\\?\UNC\nas\games\gw2"),
            PathBuf::from(r"\\nas\games\gw2")
        );
        assert_eq!(p("/home/u/gw2"), PathBuf::from("/home/u/gw2"));
    }

    /// The elevated grant runs on whatever command line it is given, so it checks for itself that
    /// the folder is a game folder and that the account name is only a name.
    #[test]
    fn grant_target_must_be_a_game_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let gw2 = game(tmp.path(), "Guild Wars 2");
        let real = fs::canonicalize(&gw2).unwrap();
        assert_eq!(check_grant_target(&gw2, "PC\\player"), Ok(real.clone()));
        // Windows allows parentheses in account names; only `:` splits the icacls argument.
        assert_eq!(
            check_grant_target(&gw2, "PC\\Player (Home)"),
            Ok(real.clone())
        );
        #[cfg(unix)]
        {
            let link = tmp.path().join("link");
            std::os::unix::fs::symlink(&gw2, &link).unwrap();
            assert_eq!(
                check_grant_target(&link, "player"),
                Ok(real),
                "grants the checked folder"
            );
        }
        let other = tmp.path().join("Program Files");
        fs::create_dir_all(&other).unwrap();
        assert!(
            check_grant_target(&other, "player").is_err(),
            "no Gw2-64.exe"
        );
        assert!(
            check_grant_target(&gw2.join("Gw2-64.exe"), "player").is_err(),
            "a file"
        );
        assert!(check_grant_target(&tmp.path().join("missing"), "player").is_err());
        for user in ["", "Everyone:(F)", "a\"b", "x:(OI)", "tab\tname"] {
            assert!(check_grant_target(&gw2, user).is_err(), "{user:?}");
        }
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
        assert_eq!(
            resolve(Some(&root.join("bin64")), None, &[]),
            Some(root.clone())
        );
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
    fn registry_values_name_the_game_folder() {
        let f = |v| folder_from_registry_value(v);
        assert_eq!(
            f(r"C:\Games\Guild Wars 2"),
            Some(PathBuf::from(r"C:\Games\Guild Wars 2"))
        );
        assert_eq!(
            f(r"C:\Games\Guild Wars 2\Gw2-64.exe"),
            Some(PathBuf::from(r"C:\Games\Guild Wars 2"))
        );
        assert_eq!(
            f(r#""D:\GW2\Gw2-64.EXE",0"#),
            Some(PathBuf::from(r"D:\GW2"))
        );
        assert_eq!(f("  "), None);
    }

    #[test]
    fn axiom_override_parsed() {
        let cfg = r#"{"arcdps":{"gw2PathOverride":"C:\\Games\\Guild Wars 2","plugins":{}}}"#;
        assert_eq!(
            axiom_gw2_path(cfg),
            Some(PathBuf::from(r"C:\Games\Guild Wars 2"))
        );
        assert_eq!(axiom_gw2_path(r#"{"arcdps":{"gw2PathOverride":""}}"#), None);
        assert_eq!(axiom_gw2_path(r#"{"arcdps":{}}"#), None);
        assert_eq!(axiom_gw2_path("not json"), None);
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
        assert_eq!(
            c.last().unwrap(),
            &PathBuf::from("/mnt/lib/steamapps/common/Guild Wars 2")
        );
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
        assert_eq!(
            prefix_log_dir(&game, &home, Some("other")),
            Some(logs(&docs))
        );
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
        std::os::unix::fs::symlink(
            home.join("Documents"),
            pfx.join("drive_c/users/me/Documents"),
        )
        .unwrap();
        assert_eq!(prefix_log_dir(&game, &home, Some("me")), None);
    }
}
