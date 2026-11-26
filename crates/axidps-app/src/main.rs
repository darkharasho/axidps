//! `axidps`: the launcher. It supervises `axidps-core` and the overlay host, keeps the game
//! folder right and updates itself.

// No console window at login (closing it would stop axidps); the log goes to
// `<config>/axidps.log` and the command-line flags attach to the calling terminal.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod applog;
mod autostart;
mod config;
mod firstrun;
mod game;
mod gamedir;
mod gw2path;
mod instance;
#[cfg(target_os = "linux")]
mod kwin;
mod layout;
mod plugin_cmd;
mod preview;
mod run;
#[cfg(windows)]
mod sigcheck;
mod supervise;
#[cfg(windows)]
mod tray;
mod update;

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Parser;

#[derive(Parser)]
#[command(name = "axidps", version = axidps_protocol::ident::build_version())]
struct Cli {
    /// Put the game folder back as it was and exit.
    #[arg(long)]
    uninstall: bool,
    /// Install the game-folder files and exit.
    #[arg(long)]
    install_game: bool,
    /// Grant access to this Guild Wars 2 folder.
    #[arg(long)]
    grant_game_folder: Option<std::path::PathBuf>,
    /// Who the elevated `--grant-game-folder` grants access to (default: the current user).
    #[arg(long)]
    grant_user: Option<String>,
    /// Check for an update and exit.
    #[arg(long)]
    check_update: bool,
    /// Started by a Linux update: wait for the old launcher to exit instead of exiting at once.
    #[arg(long, hide = true)]
    after_update: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Tools for plugin authors.
    Plugin {
        #[command(subcommand)]
        action: plugin_cmd::Action,
    },
}

/// Lets a flag's output (and `--help`) reach the terminal it was run from. Streams the caller
/// already redirected are kept.
#[cfg(windows)]
fn attach_parent_console() {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        SetStdHandle,
    };
    // SAFETY: plain Win32 calls; a handle given to SetStdHandle stays open for the process.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        for id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let unset = GetStdHandle(id).map_or(true, |h| h.is_invalid() || h.0.is_null());
            if unset && let Ok(console) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                let _ = SetStdHandle(id, HANDLE(console.into_raw_handle() as _));
            }
        }
    }
}

fn main() -> ExitCode {
    // Started with arguments: a command-line use, not the login start.
    #[cfg(windows)]
    if std::env::args_os().len() > 1 {
        attach_parent_console();
    }
    let cli = Cli::parse();
    // Runs and exits: no supervisor, tray, game watcher or single-instance lock.
    if let Some(Command::Plugin { action }) = cli.command {
        return plugin_cmd::run(action);
    }
    if cli.check_update {
        return update::check_once();
    }
    let Some(dir) = axidps_protocol::paths::config_dir() else {
        eprintln!("axidps: no config folder (APPDATA, HOME or XDG_CONFIG_HOME is unset)");
        return ExitCode::FAILURE;
    };
    let store = config::Store::new(dir);
    if let Some(path) = &cli.grant_game_folder {
        return run::grant_game_folder(path, cli.grant_user.as_deref(), &store);
    }
    if cli.uninstall {
        return run::uninstall(&store);
    }
    let exe_dir = match std::env::current_exe() {
        Ok(exe) => exe.parent().map(Path::to_path_buf).unwrap_or_default(),
        Err(e) => {
            eprintln!("axidps: cannot find its own folder: {e}");
            return ExitCode::FAILURE;
        }
    };
    let layout = layout::Layout::beside(&exe_dir);
    if cli.install_game {
        return run::install_game(&layout, &store);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let handler_stop = Arc::clone(&stop);
    // Not fatal: without a console (Windows) the tray's Quit still stops the launcher.
    if let Err(e) = ctrlc::set_handler(move || handler_stop.store(true, Ordering::SeqCst)) {
        eprintln!("axidps: cannot watch for Ctrl-C: {e}");
    }
    match run::run(&layout, &store, &stop, cli.after_update) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("axidps: {e}");
            ExitCode::FAILURE
        }
    }
}
