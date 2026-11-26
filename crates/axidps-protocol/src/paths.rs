//! Where axidps keeps its IPC files and its config folder, per host.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::v2::RingHost;

/// The game-folder name of the `d3d11.dll` axidps moved aside (arcdps, Nexus or another
/// loader); the axidps DLL chain-loads it. Not `arcdps_*.dll`: arcdps loads every such file in
/// the game folder as an extension and warns that this one has no init export. Not
/// `d3d11_chainload.dll` either: arcdps chain-loads that name itself.
pub const CHAIN_DLL: &str = "axidps_chain_d3d11.dll";
/// What [`CHAIN_DLL`] was called before 0.2.0-rc.4. The launcher renames it; the DLL still
/// loads it when [`CHAIN_DLL`] is missing.
pub const LEGACY_CHAIN_DLL: &str = "arcdps_d3d11.dll";

/// The overlay frame file (host-to-game shared frames) for `host`.
pub fn overlay_frames_path(host: RingHost) -> PathBuf {
    shared_file(host, "axidps-overlay")
}

/// The live stats feed the overlay meter reads, for `host`.
pub fn live_feed_path(host: RingHost) -> PathBuf {
    shared_file(host, "axidps-live.json")
}

fn shared_file(host: RingHost, name: &str) -> PathBuf {
    match host {
        RingHost::Unix => Path::new("/dev/shm").join(name),
        RingHost::Wine => PathBuf::from(format!("Z:\\dev\\shm\\{name}")),
        RingHost::Windows => std::env::temp_dir().join(name),
    }
}

/// axidps's config folder: `%APPDATA%\axidps` on Windows, else `$XDG_CONFIG_HOME/axidps` or
/// `$HOME/.config/axidps`.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(
        std::env::var_os("APPDATA"),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
        cfg!(windows),
    )
}

/// [`config_dir`] with its inputs passed in.
pub fn config_dir_from(
    appdata: Option<OsString>,
    xdg: Option<OsString>,
    home: Option<OsString>,
    windows: bool,
) -> Option<PathBuf> {
    let base = if windows {
        PathBuf::from(appdata.filter(|v| !v.is_empty())?)
    } else {
        match xdg.filter(|v| !v.is_empty()) {
            Some(x) => PathBuf::from(x),
            None => PathBuf::from(home.filter(|v| !v.is_empty())?).join(".config"),
        }
    };
    Some(base.join("axidps"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_path_per_host() {
        assert_eq!(overlay_frames_path(RingHost::Unix), Path::new("/dev/shm/axidps-overlay"));
        assert_eq!(overlay_frames_path(RingHost::Wine), Path::new(r"Z:\dev\shm\axidps-overlay"));
        assert!(overlay_frames_path(RingHost::Windows).starts_with(std::env::temp_dir()));
    }
    #[test]
    fn live_path_per_host() {
        assert_eq!(live_feed_path(RingHost::Unix), Path::new("/dev/shm/axidps-live.json"));
        assert_eq!(live_feed_path(RingHost::Wine), Path::new(r"Z:\dev\shm\axidps-live.json"));
        assert!(live_feed_path(RingHost::Windows).ends_with("axidps-live.json"));
    }
    #[test]
    fn config_dir_rules() {
        assert_eq!(
            config_dir_from(Some("C:\\U\\AppData\\Roaming".into()), None, None, true),
            Some(PathBuf::from("C:\\U\\AppData\\Roaming").join("axidps"))
        );
        assert_eq!(
            config_dir_from(None, Some("/x".into()), Some("/h".into()), false),
            Some("/x/axidps".into())
        );
        assert_eq!(
            config_dir_from(None, Some("".into()), Some("/h".into()), false),
            Some("/h/.config/axidps".into())
        );
        assert_eq!(config_dir_from(None, None, None, false), None);
    }
}
