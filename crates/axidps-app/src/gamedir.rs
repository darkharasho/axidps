//! Keeps `d3d11.dll` in the game folder ours and arcdps (or any other loader) chainloaded.
//!
//! Nothing here deletes a file that axidps did not create: a foreign `d3d11.dll` is only
//! renamed, and every replace goes through a temp file and `fs::rename`. Callers must not
//! call [`ensure`] or [`uninstall`] while `Gw2-64.exe` runs.

use std::io;
use std::path::Path;

use axidps_protocol::paths::{CHAIN_DLL as CHAIN, LEGACY_CHAIN_DLL as LEGACY_CHAIN};

use crate::config::GameState;

const DLL: &str = "d3d11.dll";
const CHAIN_OLD: &str = "axidps_chain_d3d11.dll.axidps-old";
const LEGACY_CHAIN_OLD: &str = "arcdps_d3d11.dll.axidps-old";
const STAGED: &str = "d3d11.dll.axidps-new";
const PROBE: &str = ".axidps-write-test";

/// What a `d3d11.dll` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DllKind {
    Missing,
    /// axidps's DLL, with its embedded version.
    Ours(String),
    Foreign,
}

/// What [`ensure`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Unchanged,
    Installed,
    Updated,
    /// Something overwrote our DLL and it was put back.
    Healed,
    /// The folder is not writable; nothing was changed.
    NeedsElevation,
    /// The installed DLL is ours but newer than this launcher's (another, newer install put it
    /// there). It is left alone: replacing it would downgrade it, and the two installs would
    /// swap `d3d11.dll` back and forth.
    KeptNewer,
}

/// The version given to an axidps DLL from before the version marker: older than any release,
/// so it is always updated, and never mistaken for a foreign loader (moving it aside would make
/// axidps chainload itself).
pub const UNVERSIONED: &str = "0.0.0";

/// Classifies a DLL file by its embedded axidps version marker, or, for builds from before the
/// marker, by axidps's own exports.
pub fn classify(file: &Path) -> io::Result<DllKind> {
    match std::fs::read(file) {
        Ok(bytes) => Ok(match axidps_protocol::ident::find_dll_version(&bytes) {
            Some(v) => DllKind::Ours(v),
            None if axidps_protocol::ident::is_axidps_dll(&bytes) => {
                DllKind::Ours(UNVERSIONED.into())
            }
            None => DllKind::Foreign,
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(DllKind::Missing),
        Err(e) => Err(e),
    }
}

fn is_denied(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
    )
}

/// Whether an installed DLL at `installed` should be replaced by ours at `ours`: only when ours
/// is newer by semver precedence (pre-releases sort before their release; build metadata is
/// ignored). A version that does not parse is replaced when the text differs, as before.
pub fn should_replace(installed: &str, ours: &str) -> bool {
    match (semver::Version::parse(installed), semver::Version::parse(ours)) {
        (Ok(i), Ok(o)) => o.cmp_precedence(&i).is_gt(),
        _ => installed != ours,
    }
}

/// Makes `game/d3d11.dll` ours at `ours_version`. `ours` is the DLL shipped with the launcher.
pub fn ensure(
    game: &Path,
    ours: &Path,
    ours_version: &str,
    state: &mut GameState,
) -> io::Result<Outcome> {
    ensure_with(game, ours, ours_version, state, &|from, to| {
        std::fs::rename(from, to)
    })
}

/// [`ensure`] with the final rename (staged file into place) injectable, so tests can fail it.
fn ensure_with(
    game: &Path,
    ours: &Path,
    ours_version: &str,
    state: &mut GameState,
    final_rename: &dyn Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<Outcome> {
    let dll = game.join(DLL);
    let kind = classify(&dll)?;
    let migrate = needs_chain_rename(game, state);
    let current = kind == DllKind::Ours(ours_version.to_string());
    // Ours but not older than this launcher's: never downgrade it.
    let kept_newer =
        matches!(&kind, DllKind::Ours(v) if !current && !should_replace(v, ours_version));
    let keep = if kept_newer {
        Outcome::KeptNewer
    } else {
        Outcome::Unchanged
    };
    if (current || kept_newer) && !migrate {
        return Ok(keep);
    }

    // Probe writability before touching anything.
    let probe = game.join(PROBE);
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
        }
        Err(e) if is_denied(&e) => return Ok(Outcome::NeedsElevation),
        Err(e) => return Err(e),
    }
    if migrate {
        rename_legacy_chain(game)?;
    }
    if current || kept_newer {
        return Ok(keep);
    }

    // Stage our DLL first, so a failed copy leaves the folder as it was.
    let staged = game.join(STAGED);
    std::fs::copy(ours, &staged)?;
    if classify(&staged)? != DllKind::Ours(ours_version.to_string()) {
        let _ = std::fs::remove_file(&staged);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} does not carry version {ours_version}", ours.display()),
        ));
    }

    let mut moved_aside = false;
    let outcome = match kind {
        DllKind::Ours(_) => Outcome::Updated,
        DllKind::Missing => Outcome::Installed,
        DllKind::Foreign => {
            let healed = state.moved_foreign;
            if let Err(e) = move_foreign_aside(game) {
                let _ = std::fs::remove_file(&staged);
                return Err(e);
            }
            moved_aside = true;
            if healed {
                Outcome::Healed
            } else {
                Outcome::Installed
            }
        }
    };

    if let Err(e) = final_rename(&staged, &dll) {
        // Undo only what this call did: put the moved-aside loader back.
        if moved_aside {
            let chain = game.join(CHAIN);
            if !dll.exists() {
                let _ = std::fs::rename(&chain, &dll);
            }
            if game.join(CHAIN_OLD).exists() && !chain.exists() {
                let _ = std::fs::rename(game.join(CHAIN_OLD), &chain);
            }
        }
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    if moved_aside {
        state.moved_foreign = true;
    }
    Ok(outcome)
}

/// Whether the loader this launcher moved aside still has its pre-0.2.0-rc.4 name
/// ([`LEGACY_CHAIN`]). A file by that name that the launcher did not move there is the user's,
/// and is left alone.
fn needs_chain_rename(game: &Path, state: &GameState) -> bool {
    state.moved_foreign && game.join(LEGACY_CHAIN).exists() && !game.join(CHAIN).exists()
}

/// [`LEGACY_CHAIN`] becomes [`CHAIN`], and its `.axidps-old` backup follows when it has one.
fn rename_legacy_chain(game: &Path) -> io::Result<()> {
    std::fs::rename(game.join(LEGACY_CHAIN), game.join(CHAIN))?;
    if game.join(LEGACY_CHAIN_OLD).exists() && !game.join(CHAIN_OLD).exists() {
        std::fs::rename(game.join(LEGACY_CHAIN_OLD), game.join(CHAIN_OLD))?;
    }
    Ok(())
}

/// `d3d11.dll` becomes [`CHAIN`]; a previous [`CHAIN`] is kept once as `.axidps-old`.
fn move_foreign_aside(game: &Path) -> io::Result<()> {
    let chain = game.join(CHAIN);
    let kept_old = chain.exists();
    if kept_old {
        std::fs::rename(&chain, game.join(CHAIN_OLD))?;
    }
    if let Err(e) = std::fs::rename(game.join(DLL), &chain) {
        if kept_old {
            let _ = std::fs::rename(game.join(CHAIN_OLD), &chain);
        }
        return Err(e);
    }
    Ok(())
}

/// Removes our `d3d11.dll` and, if we moved a foreign one aside, puts it back (from its
/// pre-0.2.0-rc.4 name if it still has that). Leaves any foreign `d3d11.dll` and
/// `.axidps-old` backup alone.
pub fn uninstall(game: &Path, state: &mut GameState) -> io::Result<()> {
    let dll = game.join(DLL);
    if matches!(classify(&dll)?, DllKind::Ours(_)) {
        std::fs::remove_file(&dll)?;
    }
    let _ = std::fs::remove_file(game.join(STAGED));
    if state.moved_foreign {
        let chain = [game.join(CHAIN), game.join(LEGACY_CHAIN)]
            .into_iter()
            .find(|p| p.exists());
        if let Some(chain) = chain.filter(|_| !dll.exists()) {
            std::fs::rename(&chain, &dll)?;
        }
        state.moved_foreign = false;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const OURS: &[u8] = b"MZ ours axidps-dll-version=0.2.0\0 tail";

    struct Env {
        _tmp: tempfile::TempDir,
        game: PathBuf,
        ours: PathBuf,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("gw2");
        fs::create_dir_all(&game).unwrap();
        let ours = tmp.path().join("ours.dll");
        fs::write(&ours, OURS).unwrap();
        Env { _tmp: tmp, game, ours }
    }

    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap()
    }

    fn ensure_v(e: &Env, state: &mut GameState) -> Outcome {
        ensure(&e.game, &e.ours, "0.2.0", state).unwrap()
    }

    #[test]
    fn install_moves_arcdps_and_places_ours() {
        let e = env();
        fs::write(e.game.join("d3d11.dll"), "arcdps").unwrap();
        let mut st = GameState::default();
        assert_eq!(ensure_v(&e, &mut st), Outcome::Installed);
        assert_eq!(read(&e.game.join(CHAIN)), "arcdps");
        assert_eq!(
            classify(&e.game.join("d3d11.dll")).unwrap(),
            DllKind::Ours("0.2.0".into())
        );
        assert!(st.moved_foreign);
        assert!(!e.game.join("d3d11.dll.axidps-new").exists());
        assert!(!e.game.join(".axidps-write-test").exists());
    }

    #[test]
    fn install_without_arcdps() {
        let e = env();
        let mut st = GameState::default();
        assert_eq!(ensure_v(&e, &mut st), Outcome::Installed);
        assert!(!e.game.join(CHAIN).exists());
        assert!(!st.moved_foreign);
    }

    #[test]
    fn ensure_is_idempotent() {
        let e = env();
        fs::write(e.game.join("d3d11.dll"), "arcdps").unwrap();
        let mut st = GameState::default();
        ensure_v(&e, &mut st);
        assert_eq!(ensure_v(&e, &mut st), Outcome::Unchanged);
    }

    #[test]
    fn heal_after_arcdps_replaced_us() {
        let e = env();
        fs::write(e.game.join("d3d11.dll"), "arcdps").unwrap();
        let mut st = GameState::default();
        ensure_v(&e, &mut st);
        fs::write(e.game.join("d3d11.dll"), "arcdps2").unwrap();
        assert_eq!(ensure_v(&e, &mut st), Outcome::Healed);
        assert_eq!(read(&e.game.join(CHAIN)), "arcdps2");
        assert_eq!(read(&e.game.join(CHAIN_OLD)), "arcdps");
        assert_eq!(
            classify(&e.game.join("d3d11.dll")).unwrap(),
            DllKind::Ours("0.2.0".into())
        );
    }

    #[test]
    fn install_keeps_previous_arcdps_once() {
        let e = env();
        fs::write(e.game.join("d3d11.dll"), "a1").unwrap();
        let mut st = GameState::default();
        ensure_v(&e, &mut st);
        for next in ["a2", "a3"] {
            fs::write(e.game.join("d3d11.dll"), next).unwrap();
            ensure_v(&e, &mut st);
        }
        assert_eq!(read(&e.game.join(CHAIN)), "a3");
        // Only the one before the latest is kept; the older backup is replaced.
        assert_eq!(read(&e.game.join(CHAIN_OLD)), "a2");
    }

    #[test]
    fn older_ours_is_updated() {
        let e = env();
        fs::write(
            e.game.join("d3d11.dll"),
            b"MZ axidps-dll-version=0.1.0\0".as_slice(),
        )
        .unwrap();
        fs::write(e.game.join(CHAIN), "arcdps").unwrap();
        let mut st = GameState {
            moved_foreign: true,
            granted: false,
            ..GameState::default()
        };
        assert_eq!(ensure_v(&e, &mut st), Outcome::Updated);
        assert_eq!(
            classify(&e.game.join("d3d11.dll")).unwrap(),
            DllKind::Ours("0.2.0".into())
        );
        assert_eq!(read(&e.game.join(CHAIN)), "arcdps");
        assert!(!e.game.join(CHAIN_OLD).exists());
    }

    #[test]
    fn newer_ours_is_not_downgraded() {
        let e = env();
        let newer = b"MZ axidps-dll-version=0.3.0\0".as_slice();
        fs::write(e.game.join("d3d11.dll"), newer).unwrap();
        fs::write(e.game.join(CHAIN), "arcdps").unwrap();
        let mut st = GameState {
            moved_foreign: true,
            granted: false,
            ..GameState::default()
        };
        assert_eq!(ensure_v(&e, &mut st), Outcome::KeptNewer);
        assert_eq!(fs::read(e.game.join("d3d11.dll")).unwrap(), newer);
        assert_eq!(read(&e.game.join(CHAIN)), "arcdps");
        assert!(!e.game.join("d3d11.dll.axidps-new").exists());
        assert!(!e.game.join(".axidps-write-test").exists());
    }

    #[test]
    fn release_is_newer_than_its_prerelease() {
        let e = env();
        // 0.2.0 installed; a 0.2.0-rc.4 launcher keeps it.
        fs::write(e.game.join("d3d11.dll"), OURS).unwrap();
        let mut st = GameState::default();
        let out = ensure(&e.game, &e.ours, "0.2.0-rc.4", &mut st).unwrap();
        assert_eq!(out, Outcome::KeptNewer);
        // And a 0.2.0 launcher replaces 0.2.0-rc.4.
        fs::write(
            e.game.join("d3d11.dll"),
            b"MZ axidps-dll-version=0.2.0-rc.4\0".as_slice(),
        )
        .unwrap();
        assert_eq!(ensure_v(&e, &mut st), Outcome::Updated);
    }

    #[test]
    fn replace_rule_follows_semver_precedence() {
        assert!(should_replace("0.1.0", "0.2.0"));
        assert!(!should_replace("0.3.0", "0.2.0"));
        assert!(!should_replace("0.2.0", "0.2.0"));
        assert!(should_replace("0.2.0-rc.1", "0.2.0-rc.2"));
        assert!(should_replace("0.2.0-rc.9", "0.2.0-rc.10"));
        assert!(!should_replace("0.2.0-rc.2", "0.2.0-rc.1"));
        assert!(should_replace("0.2.0-rc.4", "0.2.0"));
        assert!(!should_replace("0.2.0", "0.2.0-rc.4"));
        assert!(should_replace("0.2.0-alpha", "0.2.0-beta"));
        // Build metadata does not order builds.
        assert!(!should_replace("0.2.0+a", "0.2.0+b"));
    }

    /// A minimal PE32+ image exporting `names` (see axidps-protocol's tests).
    fn pe_with_exports(names: &[&str]) -> Vec<u8> {
        let mut b = vec![0u8; 0x400];
        b[..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        let (coff, opt) = (0x84, 0x98);
        b[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
        b[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes());
        b[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        b[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
        b[opt + 112..opt + 116].copy_from_slice(&0x1000u32.to_le_bytes());
        let sec = opt + 240;
        for (at, v) in [(8, 0x200u32), (12, 0x1000), (16, 0x200), (20, 0x200)] {
            b[sec + at..sec + at + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[0x200 + 24..0x200 + 28].copy_from_slice(&(names.len() as u32).to_le_bytes());
        b[0x200 + 32..0x200 + 36].copy_from_slice(&0x1040u32.to_le_bytes());
        let mut rva = 0x1080u32;
        for (i, n) in names.iter().enumerate() {
            b[0x240 + i * 4..0x244 + i * 4].copy_from_slice(&rva.to_le_bytes());
            let off = (rva - 0x1000 + 0x200) as usize;
            b[off..off + n.len()].copy_from_slice(n.as_bytes());
            rva += n.len() as u32 + 1;
        }
        b
    }

    #[test]
    fn unmarked_old_axidps_is_ours_and_updated_not_chainloaded() {
        let e = env();
        let old = pe_with_exports(&["D3D11CreateDevice", "pfn_D3D11CreateDevice"]);
        fs::write(e.game.join("d3d11.dll"), &old).unwrap();
        assert_eq!(
            classify(&e.game.join("d3d11.dll")).unwrap(),
            DllKind::Ours(UNVERSIONED.into())
        );
        let mut st = GameState::default();
        assert_eq!(ensure_v(&e, &mut st), Outcome::Updated);
        assert!(!e.game.join(CHAIN).exists());
        assert!(!st.moved_foreign);
        assert_eq!(
            classify(&e.game.join("d3d11.dll")).unwrap(),
            DllKind::Ours("0.2.0".into())
        );
    }

    #[test]
    fn foreign_loader_with_d3d11_exports_stays_foreign() {
        let e = env();
        let arc = pe_with_exports(&["D3D11CreateDevice", "get_init_addr"]);
        fs::write(e.game.join("d3d11.dll"), &arc).unwrap();
        assert_eq!(classify(&e.game.join("d3d11.dll")).unwrap(), DllKind::Foreign);
    }

    #[test]
    fn nexus_loader_is_chainloaded() {
        let e = env();
        fs::create_dir_all(e.game.join("addons")).unwrap();
        fs::write(e.game.join("d3d11.dll"), "nexus").unwrap();
        let mut st = GameState::default();
        assert_eq!(ensure_v(&e, &mut st), Outcome::Installed);
        assert_eq!(read(&e.game.join(CHAIN)), "nexus");
        assert!(st.moved_foreign);
    }

    #[test]
    fn ours_without_matching_version_changes_nothing() {
        let e = env();
        fs::write(e.game.join("d3d11.dll"), "arcdps").unwrap();
        let mut st = GameState::default();
        let err = ensure(&e.game, &e.ours, "9.9.9", &mut st).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(read(&e.game.join("d3d11.dll")), "arcdps");
        assert!(!e.game.join(CHAIN).exists());
        assert!(!e.game.join("d3d11.dll.axidps-new").exists());
        assert!(!st.moved_foreign);
    }

    #[cfg(unix)]
    #[test]
    fn install_readonly_folder_needs_elevation() {
        use std::os::unix::fs::PermissionsExt;
        let e = env();
        fs::write(e.game.join("d3d11.dll"), "arcdps").unwrap();
        fs::set_permissions(&e.game, fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores directory modes.
        let root = fs::write(e.game.join("probe"), b"").is_ok();
        let mut st = GameState::default();
        let out = (!root).then(|| ensure_v(&e, &mut st));
        fs::set_permissions(&e.game, fs::Permissions::from_mode(0o755)).unwrap();
        if let Some(out) = out {
            assert_eq!(out, Outcome::NeedsElevation);
            assert_eq!(read(&e.game.join("d3d11.dll")), "arcdps");
            assert!(!st.moved_foreign);
        }
    }

    fn failing_rename(_: &Path, _: &Path) -> io::Result<()> {
        Err(io::Error::other("injected"))
    }

    #[test]
    fn failed_final_rename_without_foreign_leaves_user_files_alone() {
        let e = env();
        // No d3d11.dll, but the user has their own chain files.
        fs::write(e.game.join(CHAIN), "mine").unwrap();
        fs::write(e.game.join(CHAIN_OLD), "older").unwrap();
        let mut st = GameState::default();
        let r = ensure_with(&e.game, &e.ours, "0.2.0", &mut st, &failing_rename);
        assert!(r.is_err());
        assert!(!e.game.join("d3d11.dll").exists());
        assert_eq!(read(&e.game.join(CHAIN)), "mine");
        assert_eq!(read(&e.game.join(CHAIN_OLD)), "older");
        assert!(!e.game.join("d3d11.dll.axidps-new").exists());
        assert!(!st.moved_foreign);
    }

    #[test]
    fn failed_final_rename_after_move_rolls_back() {
        let e = env();
        fs::write(e.game.join("d3d11.dll"), "arcdps").unwrap();
        fs::write(e.game.join(CHAIN), "prev").unwrap();
        let mut st = GameState::default();
        let r = ensure_with(&e.game, &e.ours, "0.2.0", &mut st, &failing_rename);
        assert!(r.is_err());
        assert_eq!(read(&e.game.join("d3d11.dll")), "arcdps");
        assert_eq!(read(&e.game.join(CHAIN)), "prev");
        assert!(!e.game.join(CHAIN_OLD).exists());
        assert!(!e.game.join("d3d11.dll.axidps-new").exists());
        assert!(!st.moved_foreign);
    }

    /// A folder set up before 0.2.0-rc.4: ours current, the loader we moved under its old name.
    fn legacy_env(state_moved: bool) -> (Env, GameState) {
        let e = env();
        fs::write(e.game.join("d3d11.dll"), OURS).unwrap();
        fs::write(e.game.join(LEGACY_CHAIN), "nexus").unwrap();
        fs::write(e.game.join(LEGACY_CHAIN_OLD), "older").unwrap();
        let st = GameState {
            moved_foreign: state_moved,
            granted: false,
            ..GameState::default()
        };
        (e, st)
    }

    #[test]
    fn legacy_chain_we_moved_is_renamed() {
        let (e, mut st) = legacy_env(true);
        assert_eq!(ensure_v(&e, &mut st), Outcome::Unchanged);
        assert_eq!(read(&e.game.join(CHAIN)), "nexus");
        assert_eq!(read(&e.game.join(CHAIN_OLD)), "older");
        assert!(!e.game.join(LEGACY_CHAIN).exists());
        assert!(!e.game.join(LEGACY_CHAIN_OLD).exists());
        assert!(st.moved_foreign);
        assert_eq!(ensure_v(&e, &mut st), Outcome::Unchanged);
    }

    #[test]
    fn legacy_chain_is_renamed_during_an_update() {
        let (e, mut st) = legacy_env(true);
        fs::write(
            e.game.join("d3d11.dll"),
            b"MZ axidps-dll-version=0.1.0\0".as_slice(),
        )
        .unwrap();
        assert_eq!(ensure_v(&e, &mut st), Outcome::Updated);
        assert_eq!(read(&e.game.join(CHAIN)), "nexus");
        assert!(!e.game.join(LEGACY_CHAIN).exists());
    }

    #[test]
    fn legacy_chain_we_did_not_move_is_left_alone() {
        let (e, mut st) = legacy_env(false);
        assert_eq!(ensure_v(&e, &mut st), Outcome::Unchanged);
        assert_eq!(read(&e.game.join(LEGACY_CHAIN)), "nexus");
        assert!(!e.game.join(CHAIN).exists());
    }

    #[test]
    fn legacy_chain_never_replaces_a_chain_file() {
        let (e, mut st) = legacy_env(true);
        fs::write(e.game.join(CHAIN), "current").unwrap();
        assert_eq!(ensure_v(&e, &mut st), Outcome::Unchanged);
        assert_eq!(read(&e.game.join(CHAIN)), "current");
        assert_eq!(read(&e.game.join(LEGACY_CHAIN)), "nexus");
    }

    #[test]
    fn uninstall_restores_a_legacy_chain() {
        let (e, mut st) = legacy_env(true);
        uninstall(&e.game, &mut st).unwrap();
        assert_eq!(read(&e.game.join("d3d11.dll")), "nexus");
        assert!(!e.game.join(LEGACY_CHAIN).exists());
        assert!(!st.moved_foreign);
    }

    #[test]
    fn uninstall_restores_folder() {
        let e = env();
        fs::write(e.game.join("d3d11.dll"), "arcdps").unwrap();
        let mut st = GameState::default();
        ensure_v(&e, &mut st);
        uninstall(&e.game, &mut st).unwrap();
        assert_eq!(read(&e.game.join("d3d11.dll")), "arcdps");
        assert!(!e.game.join(CHAIN).exists());
        assert!(!st.moved_foreign);
    }

    #[test]
    fn uninstall_without_arcdps_removes_ours_only() {
        let e = env();
        let mut st = GameState::default();
        ensure_v(&e, &mut st);
        uninstall(&e.game, &mut st).unwrap();
        assert!(!e.game.join("d3d11.dll").exists());
    }

    #[test]
    fn uninstall_leaves_foreign_alone() {
        let e = env();
        fs::write(e.game.join("d3d11.dll"), "arcdps").unwrap();
        let mut st = GameState::default();
        uninstall(&e.game, &mut st).unwrap();
        assert_eq!(read(&e.game.join("d3d11.dll")), "arcdps");
    }
}
