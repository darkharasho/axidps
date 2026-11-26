//! The Windows tray icon: check for updates, open the logs folder, quit.

use std::path::PathBuf;

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

const ICON_PNG: &[u8] = include_bytes!("../../../packaging/icon.png");

/// What the user chose from the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    CheckUpdate,
    OpenLogs,
    Quit,
}

/// The tray icon. Create it on the thread that pumps Windows messages.
pub struct Tray {
    /// Kept alive: dropping it removes the icon. `None` if it could not be created.
    _icon: Option<TrayIcon>,
    check: MenuId,
    logs: MenuId,
    quit: MenuId,
}

fn icon() -> Result<Icon, String> {
    let img = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).map_err(|e| e.to_string())
}

impl Tray {
    pub fn new() -> Tray {
        let check = MenuItem::new("Check for updates", true, None);
        let logs = MenuItem::new("Open logs folder", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let ids = (check.id().clone(), logs.id().clone(), quit.id().clone());
        let built = (|| -> Result<TrayIcon, String> {
            let menu = Menu::new();
            menu.append_items(&[&check, &logs, &quit])
                .map_err(|e| e.to_string())?;
            TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("AxiDPS")
                .with_icon(icon()?)
                .build()
                .map_err(|e| e.to_string())
        })();
        let _icon = match built {
            Ok(i) => Some(i),
            Err(e) => {
                eprintln!("axidps: cannot create the tray icon: {e}");
                None
            }
        };
        Tray {
            _icon,
            check: ids.0,
            logs: ids.1,
            quit: ids.2,
        }
    }

    /// The next menu choice, if any.
    pub fn poll(&self) -> Option<TrayCmd> {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.check {
                return Some(TrayCmd::CheckUpdate);
            } else if event.id == self.logs {
                return Some(TrayCmd::OpenLogs);
            } else if event.id == self.quit {
                return Some(TrayCmd::Quit);
            }
        }
        None
    }
}

/// Where core writes its logs: `<Documents>/Guild Wars 2/addons/arcdps/arcdps.cbtlogs`.
fn logs_dir() -> Option<PathBuf> {
    dirs::document_dir().map(|d| {
        d.join("Guild Wars 2")
            .join("addons")
            .join("arcdps")
            .join("arcdps.cbtlogs")
    })
}

/// Opens the logs folder in Explorer, creating it first if it is missing.
pub fn open_logs() {
    let Some(dir) = logs_dir() else {
        eprintln!("axidps: cannot find the Documents folder");
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("axidps: cannot create {}: {e}", dir.display());
        return;
    }
    if let Err(e) = std::process::Command::new("explorer.exe").arg(&dir).spawn() {
        eprintln!("axidps: cannot open {}: {e}", dir.display());
    }
}
