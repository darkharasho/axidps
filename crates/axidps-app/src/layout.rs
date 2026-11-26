//! Where the installed pieces live, relative to the `axidps` executable.

use std::path::{Path, PathBuf};

/// Paths of the installed components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub core: PathBuf,
    pub overlay: PathBuf,
    pub page: PathBuf,
    pub dll: PathBuf,
}

/// `name` with the `.exe` suffix on Windows.
fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

impl Layout {
    /// The layout for an install whose `axidps` executable sits in `exe_dir`.
    pub fn beside(exe_dir: &Path) -> Layout {
        Layout {
            core: exe_dir.join(exe_name("axidps-core")),
            overlay: exe_dir.join("overlay").join(exe_name("axidps-overlay")),
            page: exe_dir.join("overlay").join("page").join("index.html"),
            dll: exe_dir.join("game").join("d3d11.dll"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_beside() {
        let l = Layout::beside(Path::new("/opt/x"));
        let exe = |name: &str| {
            if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.to_string()
            }
        };
        assert_eq!(l.core, Path::new("/opt/x").join(exe("axidps-core")));
        assert_eq!(
            l.overlay,
            Path::new("/opt/x/overlay").join(exe("axidps-overlay"))
        );
        assert_eq!(l.page, Path::new("/opt/x/overlay/page/index.html"));
        assert_eq!(l.dll, Path::new("/opt/x/game/d3d11.dll"));
    }
}
