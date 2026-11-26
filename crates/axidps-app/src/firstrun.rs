//! First run: asking the user where Guild Wars 2 is when nothing finds it.

use std::path::{Path, PathBuf};

use crate::gw2path;

const MESSAGE: &str = "axidps couldn't find Guild Wars 2. Pick the folder that contains Gw2-64.exe.";

/// Whether prompts are off (`AXIDPS_SILENT=1`, which the installer sets under `/S`).
pub fn silent() -> bool {
    std::env::var_os("AXIDPS_SILENT").is_some_and(|v| v == "1")
}

/// The game folder for a picked folder, by the same rule as the automatic search: the folder
/// itself, or the parent of a picked `bin64`.
fn accept(picked: &Path) -> Option<PathBuf> {
    gw2path::resolve(Some(picked), None, &[])
}

/// Asks through `pick` until a pick is accepted: one retry after a wrong folder, none after a
/// cancel.
fn ask_with(pick: &mut dyn FnMut() -> Option<PathBuf>) -> Option<PathBuf> {
    for _ in 0..2 {
        let picked = pick()?;
        if let Some(game) = accept(&picked) {
            return Some(game);
        }
    }
    None
}

/// One round of the real dialogs. A missing portal or display is a cancel, never a panic.
fn pick_with_dialogs() -> Option<PathBuf> {
    std::panic::catch_unwind(|| {
        rfd::MessageDialog::new()
            .set_title("axidps")
            .set_description(MESSAGE)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        rfd::FileDialog::new().pick_folder()
    })
    .ok()
    .flatten()
}

/// Asks the user for the Guild Wars 2 folder. `None` when they cancel, pick a wrong folder
/// twice, or no dialog can be shown.
pub fn ask_gw2_folder() -> Option<PathBuf> {
    ask_with(&mut pick_with_dialogs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn gw2(root: &Path) -> PathBuf {
        fs::create_dir_all(root.join("bin64")).unwrap();
        fs::write(root.join("Gw2-64.exe"), b"exe").unwrap();
        root.to_path_buf()
    }

    #[test]
    fn accepts_a_game_folder_and_normalises_bin64() {
        let tmp = tempfile::tempdir().unwrap();
        let game = gw2(&tmp.path().join("gw2"));
        assert_eq!(accept(&game), Some(game.clone()));
        assert_eq!(accept(&game.join("bin64")), Some(game));
        assert_eq!(accept(tmp.path()), None);
    }

    #[test]
    fn one_retry_then_none() {
        let tmp = tempfile::tempdir().unwrap();
        let game = gw2(&tmp.path().join("gw2"));
        let wrong = tmp.path().to_path_buf();
        let mut picks = vec![Some(game.clone()), Some(wrong.clone())];
        let mut asked = 0;
        // A wrong pick then a right one: accepted on the retry.
        let got = ask_with(&mut || {
            asked += 1;
            picks.pop().unwrap()
        });
        assert_eq!((got, asked), (Some(game), 2));
        // Two wrong picks: gives up.
        let mut asked = 0;
        let got = ask_with(&mut || {
            asked += 1;
            Some(wrong.clone())
        });
        assert_eq!((got, asked), (None, 2));
        // Cancelling ends it at once.
        let mut asked = 0;
        let got = ask_with(&mut || {
            asked += 1;
            None
        });
        assert_eq!((got, asked), (None, 1));
    }
}
