//! Launcher settings and state, stored as JSON beside each other in the config dir.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// User settings (`app.json`).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// The Guild Wars 2 folder, once the user has picked it.
    pub gw2_path: Option<PathBuf>,
}

/// What the launcher has done to the game folder (`game-state.json`).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameState {
    /// A foreign `d3d11.dll` was moved aside.
    pub moved_foreign: bool,
    /// The user granted access to a game folder. Kept for launchers that only know this flag;
    /// [`GameState::granted_for`] says which folder.
    pub granted: bool,
    /// The folder the grant was for. Files written before it existed have only `granted`.
    pub granted_for: Option<PathBuf>,
}

impl GameState {
    /// Records a grant for `folder`.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn record_grant(&mut self, folder: &Path) {
        self.granted = true;
        self.granted_for = Some(folder.to_path_buf());
    }

    /// Whether access to `folder` was granted. A grant recorded before the folder was kept
    /// (only `granted`) is taken as being for `folder`, and recorded so from now on.
    pub fn granted_to(&mut self, folder: &Path) -> bool {
        match &self.granted_for {
            Some(f) => same_folder(f, folder),
            None if self.granted => {
                self.granted_for = Some(folder.to_path_buf());
                true
            }
            None => false,
        }
    }
}

/// Path equality, ignoring letter case on Windows, where folder names are case-insensitive.
fn same_folder(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Reads and writes the launcher's JSON files in one directory.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The config folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Settings from `app.json`; a missing or invalid file gives the defaults.
    pub fn load_config(&self) -> AppConfig {
        load_json(&self.dir.join(CONFIG_FILE))
    }

    pub fn save_config(&self, config: &AppConfig) -> io::Result<()> {
        save_json(&self.dir, CONFIG_FILE, config)
    }

    /// State from `game-state.json`; a missing or invalid file gives the defaults.
    pub fn load_state(&self) -> GameState {
        load_json(&self.dir.join(STATE_FILE))
    }

    pub fn save_state(&self, state: &GameState) -> io::Result<()> {
        save_json(&self.dir, STATE_FILE, state)
    }
}

const CONFIG_FILE: &str = "app.json";
const STATE_FILE: &str = "game-state.json";
const VERSION_FILE: &str = "axiom-version";

fn load_json<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Writes `value` to `dir/name` through a temp file and a rename, so a crash never leaves a
/// half-written file behind.
fn save_json<T: Serialize>(dir: &Path, name: &str, value: &T) -> io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, dir.join(name))
}

/// Writes `version` (exactly, no trailing newline) to `axiom-version` in `dir`.
pub fn write_axiom_version(dir: &Path, version: &str) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(VERSION_FILE), version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("nested"));
        assert_eq!(store.load_config(), AppConfig::default());
        assert_eq!(store.load_state(), GameState::default());

        let config = AppConfig {
            gw2_path: Some(PathBuf::from("/games/gw2")),
        };
        store.save_config(&config).unwrap();
        assert_eq!(store.load_config(), config);

        let state = GameState {
            moved_foreign: true,
            granted: true,
            granted_for: Some(PathBuf::from("/games/gw2")),
        };
        store.save_state(&state).unwrap();
        assert_eq!(store.load_state(), state);

        std::fs::write(dir.path().join("nested").join("app.json"), "{not json").unwrap();
        assert_eq!(store.load_config(), AppConfig::default());
    }

    #[test]
    fn grant_belongs_to_its_folder() {
        let a = Path::new("/games/a");
        let b = Path::new("/games/b");
        let mut st = GameState::default();
        assert!(!st.granted_to(a));
        st.record_grant(a);
        assert!(st.granted_to(a));
        assert!(!st.granted_to(b));
        // A file from before the folder was recorded: the grant counts for the folder in use,
        // then for that one only.
        let mut old: GameState = serde_json::from_str(r#"{"moved_foreign":false,"granted":true}"#).unwrap();
        assert!(old.granted_to(a));
        assert_eq!(old.granted_for.as_deref(), Some(a));
        assert!(!old.granted_to(b));
    }

    #[test]
    fn axiom_version_written() {
        let dir = tempfile::tempdir().unwrap();
        write_axiom_version(dir.path(), "0.2.0").unwrap();
        let text = std::fs::read_to_string(dir.path().join("axiom-version")).unwrap();
        assert_eq!(text, "0.2.0");
    }
}
